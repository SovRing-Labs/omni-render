// SPDX-License-Identifier: MIT OR Apache-2.0
//! jsdom — page scripts against the page (BROWSER-BODY-0 step 8, JS-0): a live, mutable document tree for jsi, built from
//! the frozen packed DOM, plus the Web platform surface light pages use (DOM Standard: Node / Element / Document / events;
//! HTML Standard: window, location, timers, storage; Selectors 4 for querySelector). Clean room, from those specs.
//! Rendering stays the engine's: when a script changed the tree, `serialize()` writes it back as HTML (every element
//! carries `data-omr="<live id>"`) and the page process re-runs load → layout → raster, mapping hits back by that id.
//! Privacy (net.rs rules hold): document.cookie reads / writes the broker's jar for this page's URL (cookie.rs: first-party
//! only, HttpOnly hidden) through `cookie_io`; storage lives in memory for the page's life only.
use crate::jsi::{st, Ctl, Interp, Kind, Val};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub const NONE: u32 = u32::MAX;
/// Host object roles (Kind::Host(id, role)).
pub const R_NODE: u8 = 0;
pub const R_STYLE: u8 = 1;
pub const R_CLASSES: u8 = 2;
pub const R_DATASET: u8 = 3;
pub const R_WINDOW: u8 = 4;
pub const R_LOCATION: u8 = 5;
pub const R_STORAGE: u8 = 6;
/// getComputedStyle(el): the engine's computed values for the element (through `style_io`)
pub const R_CSTYLE: u8 = 7;

#[derive(Clone, Debug)]
pub struct LNode { pub kind: u8, pub tag: String, pub text: String, pub attrs: Vec<(String, String)>, pub kids: Vec<u32>, pub parent: u32 }
// kind: 1 element, 3 text, 8 comment, 9 document, 10 doctype, 11 fragment, 0 = an engine-generated ::before/::after box (not in the DOM)

pub struct Live {
    pub n: Vec<LNode>,
    pub doc: u32,
    pub url: String,
    pub dirty: bool,
    pub navigate: Option<String>,
    pub wrappers: HashMap<(u32, u8), Val>,
    pub listeners: HashMap<(u32, String), Vec<Val>>,
    pub storage: Vec<(String, String)>,
    pub boxes: HashMap<u32, [i32; 4]>,
    pub viewport: (i32, i32),
    pub ready: &'static str,
    pub log: Vec<String>,
    /// document.cookie: None = read, Some(line) = write; answered by the broker's jar (pageproc sets it)
    pub cookie_io: Option<Rc<dyn Fn(Option<&str>) -> String>>,
    /// script requests (XHR / fetch from the prelude): (method, url, content type, body) → (status, final url, headers, body)
    pub req_io: Option<Rc<dyn Fn(&str, &str, &str, Option<&str>) -> Option<(u16, String, Vec<(String, String)>, Vec<u8>)>>>,
    /// the engine's computed style for a live node: (node, css property) → value
    pub style_io: Option<Rc<dyn Fn(u32, &str) -> Option<String>>>,
    /// DOM §4.3 mutation records while an observer exists: (1 childList | 2 attributes | 3 characterData, target, attribute, added, removed)
    pub mutations: Vec<(u8, u32, String, u32, u32)>,
    pub observing: bool,
    /// template element → its content fragment (HTML §4.12.3); shadow host → its shadow root (DOM §4.2.2)
    pub templates: HashMap<u32, u32>,
    pub shadows: HashMap<u32, u32>,
    /// history entries pushed by script (HTML §7.4.3 pushState) and the current state object
    pub history_len: usize,
    /// scripts that must never run: inserted through innerHTML / insertAdjacentHTML (HTML §8.1.3: parser-inserted into a
    /// fragment, "already started"), or already run
    pub started: std::collections::HashSet<u32>,
    pub current_script: u32,
    /// window.open(url) requests for the broker (HTML §7.2.2.1: a new browsing context = a new tab here)
    pub opened: Vec<String>,
    /// WebSocket operations through the broker (pageproc broker_ws)
    pub ws_io: Option<Rc<dyn Fn(&serde_json::Value) -> serde_json::Value>>,
}

pub type LiveRef = Rc<RefCell<Live>>;

const VOID: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr", "param"];

impl Live {
    /// The live tree from the frozen DOM (same node order; `<style>` text comes back from `dom.sheets`).
    pub fn from_dom(d: &crate::dom::Dom, url: &str) -> Live {
        use crate::dom::Kind as K;
        let mut n: Vec<LNode> = Vec::with_capacity(d.nodes.len());
        let sheet_of: HashMap<u32, &String> = d.sheets.iter().chain(d.scripts.iter()).map(|(e, s)| (*e, s)).collect();
        let mut extra: Vec<(u32, String)> = vec![];
        for (i, x) in d.nodes.iter().enumerate() {
            let i = i as u32;
            let (kind, tag, text) = match x.kind {
                K::Document => (9, String::new(), String::new()),
                K::Element => (1, d.tag(i).to_string(), String::new()),
                K::Text => (3, String::new(), d.text(i).to_string()),
                K::Comment => (8, String::new(), d.text(i).to_string()),
                K::Doctype => (10, String::new(), String::new()),
            };
            let attrs = d.attr_list(i).iter().map(|a| (d.names[a.name as usize].local.to_string(), d.span_str(a.val).to_string())).collect();
            let parent = if x.parent == crate::dom::NONE { NONE } else { x.parent };
            // generated boxes (with_pseudo) keep their index so packed ids stay = live ids, but are not part of the tree
            let kind = if tag.starts_with("::") || (parent != NONE && n.get(parent as usize).map(|p: &LNode| p.kind == 0).unwrap_or(false)) { 0 } else { kind };
            n.push(LNode { kind, tag, text, attrs, kids: vec![], parent });
            if let Some(css) = sheet_of.get(&i) { extra.push((i, (*css).clone())); }
        }
        for i in 0..n.len() { let p = n[i].parent; if n[i].kind != 0 && p != NONE && (p as usize) < n.len() { n[p as usize].kids.push(i as u32); } }
        // template contents (packed after the document by dom.rs) move into each template's content fragment
        let mut templates = HashMap::new();
        for t in 0..n.len() {
            if n[t].kind == 1 && n[t].tag == "template" && !n[t].kids.is_empty() {
                let f = n.len() as u32;
                let ks = std::mem::take(&mut n[t].kids);
                for &k in &ks { n[k as usize].parent = f; }
                n.push(LNode { kind: 11, tag: String::new(), text: String::new(), attrs: vec![], kids: ks, parent: NONE });
                templates.insert(t as u32, f);
            }
        }
        let len0 = n.len();
        for (e, css) in extra.into_iter().filter(|(e, _)| (*e as usize) < len0) { let id = n.len() as u32; n.push(LNode { kind: 3, tag: String::new(), text: css, attrs: vec![], kids: vec![], parent: e }); n[e as usize].kids.push(id); }
        Live { n, doc: 0, url: url.to_string(), dirty: false, navigate: None, wrappers: HashMap::new(), listeners: HashMap::new(), storage: vec![],
               boxes: HashMap::new(), viewport: (1280, 800), ready: "loading", log: vec![], cookie_io: None, req_io: None,
               style_io: None, mutations: vec![], observing: false, templates, shadows: HashMap::new(), history_len: 1, started: Default::default(), current_script: NONE, opened: vec![], ws_io: None }
    }

    pub fn new_node(&mut self, kind: u8, tag: &str, text: &str) -> u32 {
        self.n.push(LNode { kind, tag: tag.to_ascii_lowercase(), text: text.to_string(), attrs: vec![], kids: vec![], parent: NONE });
        (self.n.len() - 1) as u32
    }
    pub fn attr(&self, i: u32, k: &str) -> Option<&str> { self.n.get(i as usize)?.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str()) }
    pub fn set_attr(&mut self, i: u32, k: &str, v: &str) {
        let k = k.to_ascii_lowercase();
        let a = &mut self.n[i as usize].attrs;
        match a.iter_mut().find(|(x, _)| *x == k) { Some(p) => p.1 = v.to_string(), None => a.push((k.clone(), v.to_string())) }
        self.dirty = true;
        if self.observing { self.mutations.push((2, i, k, NONE, NONE)); }
    }
    pub fn del_attr(&mut self, i: u32, k: &str) { self.n[i as usize].attrs.retain(|(a, _)| a != k); self.dirty = true; if self.observing { self.mutations.push((2, i, k.to_string(), NONE, NONE)); } }
    pub fn elem(&self, i: u32) -> bool { self.n.get(i as usize).map(|x| x.kind == 1).unwrap_or(false) }
    pub fn find_tag(&self, t: &str) -> u32 { (0..self.n.len() as u32).find(|&i| self.n[i as usize].kind == 1 && self.n[i as usize].tag == t && self.connected(i)).unwrap_or(NONE) }
    pub fn connected(&self, mut i: u32) -> bool { while i != NONE { if i == self.doc { return true; } i = self.n[i as usize].parent; } false }

    pub fn detach(&mut self, c: u32) {
        let p = self.n[c as usize].parent;
        if p != NONE { self.n[p as usize].kids.retain(|&k| k != c); if self.observing { self.mutations.push((1, p, String::new(), NONE, c)); } }
        self.n[c as usize].parent = NONE;
        self.dirty = true;
    }
    /// Insert `c` into `p` before `before` (NONE = append). A fragment inserts its children (DOM §4.2.3).
    pub fn insert(&mut self, p: u32, c: u32, before: u32) {
        if c == p { return; }
        let moving: Vec<u32> = if self.n[c as usize].kind == 11 { std::mem::take(&mut self.n[c as usize].kids) } else { self.detach(c); vec![c] };
        let mut at = if before == NONE { self.n[p as usize].kids.len() } else { self.n[p as usize].kids.iter().position(|&k| k == before).unwrap_or(self.n[p as usize].kids.len()) };
        for m in moving { self.n[m as usize].parent = p; self.n[p as usize].kids.insert(at, m); at += 1; if self.observing { self.mutations.push((1, p, String::new(), m, NONE)); } }
        self.dirty = true;
    }
    pub fn clone_node(&mut self, i: u32, deep: bool) -> u32 {
        let mut x = self.n[i as usize].clone();
        x.parent = NONE;
        let kids = std::mem::take(&mut x.kids);
        self.n.push(x);
        let id = (self.n.len() - 1) as u32;
        if deep { for k in kids { let c = self.clone_node(k, true); self.n[c as usize].parent = id; self.n[id as usize].kids.push(c); } }
        id
    }

    pub fn text_content(&self, i: u32) -> String {
        let x = &self.n[i as usize];
        match x.kind { 3 | 8 => x.text.clone(), _ => { let mut s = String::new(); for &k in &x.kids { if self.n[k as usize].kind != 8 { s.push_str(&self.text_content(k)); } } s } }
    }
    pub fn set_text(&mut self, i: u32, t: &str) {
        if matches!(self.n[i as usize].kind, 3 | 8) { self.n[i as usize].text = t.to_string(); self.dirty = true; if self.observing { self.mutations.push((3, i, String::new(), NONE, NONE)); } return; }
        for k in std::mem::take(&mut self.n[i as usize].kids) { self.n[k as usize].parent = NONE; }
        if !t.is_empty() { let tn = self.new_node(3, "", t); self.insert(i, tn, NONE); }
        self.dirty = true;
    }

    // ---- HTML serialization (HTML §13.3 fragment serialization; data-omr ids for the hit map) ----
    pub fn serialize(&self, quirks: bool) -> String {
        let mut o = String::with_capacity(self.n.len() * 32);
        if !quirks { o.push_str("<!DOCTYPE html>"); }
        for &k in &self.n[self.doc as usize].kids { self.ser(k, &mut o, true, false); }
        o
    }
    pub fn inner_html(&self, i: u32) -> String { let mut o = String::new(); let raw = matches!(self.n[i as usize].tag.as_str(), "script" | "style"); for &k in &self.n[i as usize].kids { self.ser(k, &mut o, false, raw); } o }
    pub fn outer_html(&self, i: u32) -> String { let mut o = String::new(); self.ser(i, &mut o, false, false); o }
    fn ser(&self, i: u32, o: &mut String, ids: bool, raw: bool) {
        let x = &self.n[i as usize];
        match x.kind {
            1 => {
                o.push('<'); o.push_str(&x.tag);
                for (k, v) in &x.attrs { if k == "data-omr" { continue; } o.push(' '); o.push_str(k); o.push_str("=\""); o.push_str(&v.replace('&', "&amp;").replace('"', "&quot;")); o.push('"'); }
                if ids { o.push_str(&format!(" data-omr=\"{i}\"")); }
                o.push('>');
                if VOID.contains(&x.tag.as_str()) { return; }
                // a script-attached shadow root goes out as declarative shadow DOM (the engine renders it in place)
                if x.tag == "template" { if let Some(&f) = self.templates.get(&i) { for &k in &self.n[f as usize].kids { self.ser(k, o, ids, false); } } }
                if let Some(&sr) = self.shadows.get(&i) { o.push_str("<template shadowrootmode=\"open\">"); for &k in &self.n[sr as usize].kids { self.ser(k, o, ids, false); } o.push_str("</template>"); }
                let r = matches!(x.tag.as_str(), "script" | "style" | "textarea" | "title" | "xmp" | "noscript" | "noembed" | "noframes");
                for &k in &x.kids { self.ser(k, o, ids, r); }
                o.push_str("</"); o.push_str(&x.tag); o.push('>');
            }
            3 => { if raw { o.push_str(&x.text) } else { o.push_str(&x.text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")) } }
            8 => { o.push_str("<!--"); o.push_str(&x.text); o.push_str("-->"); }
            11 | 9 => for &k in &x.kids { self.ser(k, o, ids, raw); },
            _ => {}
        }
    }

    /// Parse an HTML fragment (innerHTML / insertAdjacentHTML / document.write) into new detached nodes.
    pub fn parse_fragment(&mut self, html: &str) -> Vec<u32> {
        let d = crate::dom::Dom::parse(format!("<!DOCTYPE html><html><head></head><body>{html}</body></html>").as_bytes());
        let body = (0..d.nodes.len() as u32).find(|&i| d.tag(i) == "body").unwrap_or(0);
        let tmp = Live::from_dom(&d, &self.url);
        let mut out = vec![];
        for &k in &tmp.n[body as usize].kids { out.push(self.import(&tmp, k)); }
        // scripts that arrive through markup never run (HTML §8.4 innerHTML note; §8.1.3 "already started")
        let mut st = out.clone();
        while let Some(i) = st.pop() { if self.n[i as usize].tag == "script" { self.started.insert(i); } st.extend(self.n[i as usize].kids.iter().copied()); }
        out
    }
    fn import(&mut self, from: &Live, i: u32) -> u32 {
        let mut x = from.n[i as usize].clone();
        let kids = std::mem::take(&mut x.kids);
        x.parent = NONE;
        self.n.push(x);
        let id = (self.n.len() - 1) as u32;
        for k in kids { let c = self.import(from, k); self.n[c as usize].parent = id; self.n[id as usize].kids.push(c); }
        id
    }

    // ---- Selectors 4 subset: type / * / #id / .class / [attr] [attr=v] [~= ^= $= *= |=] / :first-child :last-child
    //      :not(simple) :checked; combinators descendant, >, +, ~; lists with ',' ----
    pub fn select(&self, root: u32, sel: &str, first: bool) -> Result<Vec<u32>, String> {
        let list = parse_selectors(sel)?;
        let mut out = vec![];
        let mut stack: Vec<u32> = self.n[root as usize].kids.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            if self.n[i as usize].kind == 1 && list.iter().any(|c| self.matches_complex(i, c)) { out.push(i); if first { return Ok(out); } }
            stack.extend(self.n[i as usize].kids.iter().rev());
        }
        Ok(out)
    }
    pub fn matches(&self, i: u32, sel: &str) -> bool { parse_selectors(sel).map(|l| l.iter().any(|c| self.matches_complex(i, c))).unwrap_or(false) }
    fn prev_elem(&self, i: u32) -> u32 {
        let p = self.n[i as usize].parent; if p == NONE { return NONE; }
        let ks = &self.n[p as usize].kids; let pos = ks.iter().position(|&k| k == i).unwrap_or(0);
        ks[..pos].iter().rev().copied().find(|&k| self.n[k as usize].kind == 1).unwrap_or(NONE)
    }
    fn matches_complex(&self, i: u32, c: &[(char, Vec<Simple>)]) -> bool {
        // right to left with backtracking over descendant / sibling combinators
        fn go(l: &Live, i: u32, c: &[(char, Vec<Simple>)], k: usize) -> bool {
            if !l.matches_compound(i, &c[k].1) { return false; }
            if k == 0 { return true; }
            match c[k].0 {
                '>' => { let p = l.n[i as usize].parent; p != NONE && l.elem(p) && go(l, p, c, k - 1) }
                ' ' => { let mut p = l.n[i as usize].parent; while p != NONE && l.elem(p) { if go(l, p, c, k - 1) { return true; } p = l.n[p as usize].parent; } false }
                '+' => { let s = l.prev_elem(i); s != NONE && go(l, s, c, k - 1) }
                '~' => { let mut s = l.prev_elem(i); while s != NONE { if go(l, s, c, k - 1) { return true; } s = l.prev_elem(s); } false }
                _ => false,
            }
        }
        !c.is_empty() && go(self, i, c, c.len() - 1)
    }
    fn matches_compound(&self, i: u32, cs: &[Simple]) -> bool {
        let x = &self.n[i as usize];
        cs.iter().all(|s| match s {
            Simple::Tag(t) => t == "*" || x.tag.eq_ignore_ascii_case(t),
            Simple::Id(v) => self.attr(i, "id") == Some(v.as_str()),
            Simple::Class(v) => self.attr(i, "class").map(|c| c.split_ascii_whitespace().any(|w| w == v)).unwrap_or(false),
            Simple::Attr(k, op, v) => match self.attr(i, k) {
                None => false,
                Some(a) => match op.as_str() { "" => true, "=" => a == v, "~=" => a.split_ascii_whitespace().any(|w| w == v), "^=" => !v.is_empty() && a.starts_with(v.as_str()),
                    "$=" => !v.is_empty() && a.ends_with(v.as_str()), "*=" => !v.is_empty() && a.contains(v.as_str()), "|=" => a == v || a.starts_with(&format!("{v}-")), _ => false },
            },
            Simple::FirstChild => self.prev_elem(i) == NONE,
            Simple::LastChild => { let p = x.parent; p != NONE && self.n[p as usize].kids.iter().rev().copied().find(|&k| self.n[k as usize].kind == 1) == Some(i) }
            Simple::Checked => self.attr(i, "checked").is_some() || self.attr(i, "selected").is_some(),
            Simple::Not(inner) => !self.matches_compound(i, inner),
            Simple::Never => false,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Simple { Tag(String), Id(String), Class(String), Attr(String, String, String), FirstChild, LastChild, Checked, Not(Vec<Simple>), Never }

fn parse_selectors(s: &str) -> Result<Vec<Vec<(char, Vec<Simple>)>>, String> {
    let mut out = vec![];
    for part in split_commas(s) {
        let b: Vec<char> = part.trim().chars().collect();
        if b.is_empty() { return Err("SyntaxError: empty selector".into()); }
        let mut i = 0; let mut complex: Vec<(char, Vec<Simple>)> = vec![]; let mut comb = ' ';
        while i < b.len() {
            let (cs, ni) = compound(&b, i)?;
            if cs.is_empty() { return Err(format!("SyntaxError: bad selector {part}")); }
            complex.push((comb, cs)); i = ni;
            let mut saw_ws = false;
            while i < b.len() && b[i].is_whitespace() { i += 1; saw_ws = true; }
            if i < b.len() && matches!(b[i], '>' | '+' | '~') { comb = b[i]; i += 1; while i < b.len() && b[i].is_whitespace() { i += 1; } }
            else if saw_ws { comb = ' '; }
        }
        out.push(complex);
    }
    Ok(out)
}
fn split_commas(s: &str) -> Vec<String> {
    let (mut v, mut cur, mut depth) = (vec![], String::new(), 0);
    for c in s.chars() { match c { '(' | '[' => { depth += 1; cur.push(c) } ')' | ']' => { depth -= 1; cur.push(c) } ',' if depth == 0 => v.push(std::mem::take(&mut cur)), _ => cur.push(c) } }
    v.push(cur);
    v
}
fn ident(b: &[char], mut i: usize) -> (String, usize) {
    let mut s = String::new();
    while i < b.len() && (b[i].is_alphanumeric() || b[i] == '-' || b[i] == '_' || b[i] == '\\' || b[i] as u32 > 127) {
        if b[i] == '\\' && i + 1 < b.len() { s.push(b[i + 1]); i += 2; continue; }
        s.push(b[i]); i += 1;
    }
    (s, i)
}
fn compound(b: &[char], mut i: usize) -> Result<(Vec<Simple>, usize), String> {
    let mut v = vec![];
    if i < b.len() && b[i] == '*' { v.push(Simple::Tag("*".into())); i += 1; }
    else if i < b.len() && (b[i].is_alphabetic() || b[i] == '_') { let (t, j) = ident(b, i); v.push(Simple::Tag(t)); i = j; }
    while i < b.len() {
        match b[i] {
            '#' => { let (t, j) = ident(b, i + 1); v.push(Simple::Id(t)); i = j; }
            '.' => { let (t, j) = ident(b, i + 1); v.push(Simple::Class(t)); i = j; }
            '[' => {
                let end = b[i..].iter().position(|&c| c == ']').ok_or("SyntaxError: unclosed [")? + i;
                let inner: String = b[i + 1..end].iter().collect();
                let (k, op, val) = match ["~=", "^=", "$=", "*=", "|=", "="].iter().find_map(|op| inner.find(op).map(|p| (p, *op))) {
                    Some((p, op)) => { let val = inner[p + op.len()..].trim().trim_end_matches(|c| c == 'i' || c == 's').trim(); (inner[..p].trim().to_string(), op.to_string(), val.trim_matches(['"', '\'']).to_string()) }
                    None => (inner.trim().to_string(), String::new(), String::new()),
                };
                v.push(Simple::Attr(k.to_ascii_lowercase(), op, val)); i = end + 1;
            }
            ':' => {
                let dbl = b.get(i + 1) == Some(&':');
                let (t, mut j) = ident(b, i + if dbl { 2 } else { 1 });
                if j < b.len() && b[j] == '(' {
                    let mut d = 1; let st = j + 1; j += 1;
                    while j < b.len() && d > 0 { if b[j] == '(' { d += 1 } else if b[j] == ')' { d -= 1 } j += 1; }
                    let arg: Vec<char> = b[st..j - 1].iter().copied().collect();
                    if t == "not" { let (inner, _) = compound(&arg, 0)?; v.push(Simple::Not(inner)); } else { v.push(Simple::Never); }
                } else {
                    v.push(match (dbl, t.as_str()) { (false, "first-child") => Simple::FirstChild, (false, "last-child") => Simple::LastChild, (false, "checked") => Simple::Checked,
                        (false, "link" | "any-link") => Simple::Attr("href".into(), String::new(), String::new()), _ => Simple::Never });
                }
                i = j;
            }
            _ => break,
        }
    }
    Ok((v, i))
}

// ======================= bindings =======================

fn live(it: &Interp) -> LiveRef { it.dom.clone().expect("page DOM") }
fn arg(a: &[Val], i: usize) -> Val { a.get(i).cloned().unwrap_or(Val::Undef) }
fn sv(it: &mut Interp, v: &Val) -> Result<String, Ctl> { Ok(it.to_str(v)?.to_string()) }

/// The one JS object for (node, role): identity is stable (`a === b`, expandos survive).
pub fn wrap(it: &Interp, i: u32, role: u8) -> Val {
    if i == NONE { return Val::Null; }
    let l = live(it);
    if let Some(v) = l.borrow().wrappers.get(&(i, role)) { return v.clone(); }
    let v = Val::obj(Kind::Host(i, role));
    l.borrow_mut().wrappers.insert((i, role), v.clone());
    v
}
pub fn node_of(v: &Val) -> Option<u32> { if let Val::Obj(o) = v { if let Kind::Host(i, R_NODE) = o.borrow().kind { return Some(i); } } None }

macro_rules! m { ($name:expr, |$it:ident, $this:ident, $a:ident| $body:expr) => { Val::native($name, move |$it: &mut Interp, $this: Val, $a: Vec<Val>| -> Result<Val, Ctl> { $body }) }; }

fn camel_to_kebab(k: &str) -> String { let mut s = String::new(); for c in k.chars() { if c.is_ascii_uppercase() { s.push('-'); s.push(c.to_ascii_lowercase()); } else { s.push(c); } } s }
fn style_decls(s: &str) -> Vec<(String, String)> { s.split(';').filter_map(|d| d.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).filter(|(k, _)| !k.is_empty()).collect() }

fn resolve(l: &Live, u: &str) -> String { resolve_frag(&l.url, u) }
/// page::resolve keeps no fragment (it serves fetches); URLs scripts see keep theirs (WHATWG URL "fragment state").
fn resolve_frag(base: &str, u: &str) -> String {
    let u = u.trim();
    if let Some(f) = u.strip_prefix('#') { return format!("{}#{f}", base.split('#').next().unwrap_or("")); }
    match u.split_once('#') { Some((a, f)) => format!("{}#{f}", crate::page::resolve(base, a)), None => crate::page::resolve(base, u) }
}

fn arr_of(v: Vec<u32>, it: &Interp) -> Val { Val::array(v.into_iter().map(|i| wrap(it, i, R_NODE)).collect()) }

/// Nodes from append()/prepend()/before()/after()/replaceWith() arguments (strings become text nodes).
fn nodes_from(it: &mut Interp, a: &[Val]) -> Result<Vec<u32>, Ctl> {
    let mut v = vec![];
    for x in a { match node_of(x) { Some(i) => v.push(i), None => { let t = sv(it, x)?; let l = live(it); let id = l.borrow_mut().new_node(3, "", &t); v.push(id); } } }
    Ok(v)
}

/// Element / node / document properties (DOM §4.4–4.9, HTML §3.2 + reflected attributes).
pub fn get(it: &mut Interp, this: &Val, i: u32, role: u8, key: &str) -> Result<Val, Ctl> {
    match role {
        R_WINDOW => return window_get(it, key),
        R_LOCATION => return location_get(it, key),
        R_STORAGE => return storage_get(it, key),
        R_STYLE => {
            let l = live(it); let l = l.borrow();
            let s = l.attr(i, "style").unwrap_or("").to_string();
            return Ok(match key {
                "cssText" => st(&s),
                "getPropertyValue" => { drop(l); m!("getPropertyValue", |it, this, a| { let k = sv(it, &arg(&a, 0))?; it.get(&this, &k) }) }
                "setProperty" => { drop(l); m!("setProperty", |it, this, a| { let k = sv(it, &arg(&a, 0))?; let v = arg(&a, 1); it.set(&this, &k, v)?; Ok(Val::Undef) }) }
                "removeProperty" => { drop(l); m!("removeProperty", |it, this, a| { let k = sv(it, &arg(&a, 0))?; it.set(&this, &k, st(""))?; Ok(Val::Undef) }) }
                _ => { let k = if key.contains('-') { key.to_string() } else { camel_to_kebab(key) }; st(&style_decls(&s).into_iter().rev().find(|(x, _)| *x == k).map(|(_, v)| v).unwrap_or_default()) }
            });
        }
        R_CLASSES => {
            let l = live(it);
            let cls: Vec<String> = l.borrow().attr(i, "class").unwrap_or("").split_ascii_whitespace().map(|s| s.to_string()).collect();
            let upd = move |it: &mut Interp, f: &dyn Fn(&mut Vec<String>)| { let l = live(it); let mut lb = l.borrow_mut(); let mut c: Vec<String> = lb.attr(i, "class").unwrap_or("").split_ascii_whitespace().map(|s| s.to_string()).collect(); f(&mut c); let j = c.join(" "); lb.set_attr(i, "class", &j); };
            return Ok(match key {
                "length" => Val::Num(cls.len() as f64),
                "value" => st(&cls.join(" ")),
                "contains" => m!("contains", |it, _t, a| { let c = sv(it, &arg(&a, 0))?; let l = live(it); let r = l.borrow().attr(i, "class").unwrap_or("").split_ascii_whitespace().any(|w| w == c); Ok(Val::Bool(r)) }),
                "add" => m!("add", |it, _t, a| { let names: Vec<String> = a.iter().map(|x| sv(it, x)).collect::<Result<_, _>>()?; upd(it, &|c| for n in &names { if !c.contains(n) { c.push(n.clone()); } }); Ok(Val::Undef) }),
                "remove" => m!("remove", |it, _t, a| { let names: Vec<String> = a.iter().map(|x| sv(it, x)).collect::<Result<_, _>>()?; upd(it, &|c| c.retain(|w| !names.contains(w))); Ok(Val::Undef) }),
                "toggle" => m!("toggle", |it, _t, a| {
                    let n = sv(it, &arg(&a, 0))?; let force = match arg(&a, 1) { Val::Undef => None, v => Some(v.truthy()) };
                    let l = live(it); let has = l.borrow().attr(i, "class").unwrap_or("").split_ascii_whitespace().any(|w| w == n);
                    let want = force.unwrap_or(!has);
                    if want != has { let n2 = n.clone(); upd(it, &move |c| if want { c.push(n2.clone()) } else { c.retain(|w| *w != n2) }); }
                    Ok(Val::Bool(want))
                }),
                "replace" => m!("replace", |it, _t, a| { let (o, n) = (sv(it, &arg(&a, 0))?, sv(it, &arg(&a, 1))?); upd(it, &|c| for w in c.iter_mut() { if *w == o { *w = n.clone(); } }); Ok(Val::Bool(true)) }),
                "item" => m!("item", |it, _t, a| { let k = it.to_num(&arg(&a, 0))? as usize; let l = live(it); let r = l.borrow().attr(i, "class").unwrap_or("").split_ascii_whitespace().nth(k).map(st).unwrap_or(Val::Null); Ok(r) }),
                "forEach" => m!("forEach", |it, this, a| { let f = arg(&a, 0); let l = live(it); let c: Vec<String> = l.borrow().attr(i, "class").unwrap_or("").split_ascii_whitespace().map(|s| s.to_string()).collect(); for (k, w) in c.iter().enumerate() { it.call(&f, Val::Undef, vec![st(w), Val::Num(k as f64), this.clone()])?; } Ok(Val::Undef) }),
                _ => key.parse::<usize>().ok().and_then(|k| cls.get(k)).map(|w| st(w)).unwrap_or(Val::Undef),
            });
        }
        R_CSTYLE => {
            // CSSOM §9 getComputedStyle: resolved values from the engine's cascade (style_io); width / height from layout
            let prop = |key: &str| if key.contains('-') { key.to_string() } else { camel_to_kebab(key) };
            if key == "getPropertyValue" { return Ok(m!("getPropertyValue", |it, this, a| { let k = sv(it, &arg(&a, 0))?; it.get(&this, &k) })); }
            if key == "length" { return Ok(Val::Num(0.0)); }
            let k = prop(key);
            let l = live(it);
            let b = l.borrow();
            let v = match k.as_str() {
                "width" | "height" => b.boxes.get(&i).map(|x| format!("{}px", if k == "width" { x[2] } else { x[3] })),
                _ => b.style_io.as_ref().and_then(|f| f(i, &k)),
            };
            let v = v.or_else(|| style_decls(b.attr(i, "style").unwrap_or("")).into_iter().rev().find(|(x, _)| *x == k).map(|(_, v)| v)).unwrap_or_default();
            return Ok(st(&v));
        }
        R_DATASET => {
            let k = format!("data-{}", camel_to_kebab(key));
            let l = live(it);
            let r = l.borrow().attr(i, &k).map(st).unwrap_or(Val::Undef);
            return Ok(r);
        }
        _ => {}
    }
    let l = live(it);
    let (kind, tag, parent) = { let b = l.borrow(); let x = &b.n[i as usize]; (x.kind, x.tag.clone(), x.parent) };
    let attr = |k: &str| -> Option<String> { l.borrow().attr(i, k).map(|s| s.to_string()) };
    let kids = || -> Vec<u32> { l.borrow().n[i as usize].kids.clone() };
    let sib = |d: i32, elem: bool| -> u32 {
        let b = l.borrow(); if parent == NONE { return NONE; }
        let ks = &b.n[parent as usize].kids; let pos = ks.iter().position(|&k| k == i).unwrap_or(0) as i32;
        let mut p = pos + d;
        while p >= 0 && (p as usize) < ks.len() { let k = ks[p as usize]; if !elem || b.n[k as usize].kind == 1 { return k; } p += d; }
        NONE
    };
    let is_doc = kind == 9;
    Ok(match key {
        "nodeType" => Val::Num(kind as f64),
        "nodeName" | "tagName" => if kind == 1 { st(&tag.to_ascii_uppercase()) } else if kind == 3 { st("#text") } else if is_doc { st("#document") } else { st("#comment") },
        "localName" => st(&tag),
        "ownerDocument" => wrap(it, l.borrow().doc, R_NODE),
        "parentNode" => wrap(it, parent, R_NODE),
        "parentElement" => if parent != NONE && l.borrow().elem(parent) { wrap(it, parent, R_NODE) } else { Val::Null },
        "childNodes" => arr_of(kids(), it),
        "children" => { let k: Vec<u32> = kids().into_iter().filter(|&k| l.borrow().elem(k)).collect(); arr_of(k, it) }
        "childElementCount" => Val::Num(kids().into_iter().filter(|&k| l.borrow().elem(k)).count() as f64),
        "firstChild" => wrap(it, kids().first().copied().unwrap_or(NONE), R_NODE),
        "lastChild" => wrap(it, kids().last().copied().unwrap_or(NONE), R_NODE),
        "firstElementChild" => { let k = kids().into_iter().find(|&k| l.borrow().elem(k)).unwrap_or(NONE); wrap(it, k, R_NODE) }
        "lastElementChild" => { let k = kids().into_iter().rev().find(|&k| l.borrow().elem(k)).unwrap_or(NONE); wrap(it, k, R_NODE) }
        "nextSibling" => wrap(it, sib(1, false), R_NODE),
        "previousSibling" => wrap(it, sib(-1, false), R_NODE),
        "nextElementSibling" => wrap(it, sib(1, true), R_NODE),
        "previousElementSibling" => wrap(it, sib(-1, true), R_NODE),
        "isConnected" => Val::Bool(l.borrow().connected(i)),
        "textContent" | "innerText" | "nodeValue" | "data" | "text" if !(key == "text" && tag != "script" && tag != "a" && tag != "option") => if is_doc && key == "textContent" { Val::Null } else { st(&l.borrow().text_content(i)) },
        "innerHTML" => st(&l.borrow().inner_html(i)),
        "outerHTML" => st(&l.borrow().outer_html(i)),
        "id" | "className" | "title" | "lang" | "dir" | "name" | "type" | "placeholder" | "rel" | "target" | "alt" | "role" | "htmlFor" | "action" | "method" if !is_doc =>
            st(&attr(match key { "className" => "class", "htmlFor" => "for", k => k }).unwrap_or_default()),
        "href" | "src" => match attr(key) { Some(u) => st(&resolve(&l.borrow(), &u)), None => st("") },
        "value" => if tag == "textarea" { st(&l.borrow().text_content(i)) } else if tag == "select" {
            let b = l.borrow(); let opts = b.select(i, "option", false).unwrap_or_default();
            let o = opts.iter().copied().find(|&o| b.attr(o, "selected").is_some()).or(opts.first().copied());
            st(&o.map(|o| b.attr(o, "value").map(|s| s.to_string()).unwrap_or_else(|| b.text_content(o))).unwrap_or_default())
        } else { st(&attr("value").unwrap_or_default()) },
        "checked" | "disabled" | "hidden" | "selected" | "required" | "readOnly" | "multiple" | "open" => Val::Bool(attr(&key.to_ascii_lowercase()).is_some()),
        "style" => wrap(it, i, R_STYLE),
        // DOM §4.9.1 NamedNodeMap (a snapshot): indexed Attr objects {name, value, nodeName, nodeValue, localName, specified},
        // also reachable by name; jQuery 1.12's support tests read div.attributes["onsubmit"]
        "attributes" if kind == 1 => {
            let list: Vec<(String, String)> = l.borrow().n[i as usize].attrs.iter().filter(|(k, _)| k != "data-omr").cloned().collect();
            let attr = |k: &str, v: &str| Val::plain(vec![("name", st(k)), ("localName", st(k)), ("nodeName", st(k)), ("value", st(v)), ("nodeValue", st(v)),
                ("specified", Val::Bool(true)), ("nodeType", Val::Num(2.0)), ("expando", Val::Bool(false))]);
            let arr = Val::array(list.iter().map(|(k, v)| attr(k, v)).collect());
            for (k, v) in &list { it.set(&arr, k, attr(k, v))?; }
            let n = list.len();
            it.set(&arr, "getNamedItem", Val::native("getNamedItem", move |it, this, a| { let k = it.to_str(&arg(&a, 0))?.to_ascii_lowercase(); let v = it.get(&this, &k)?; Ok(if matches!(v, Val::Undef) { Val::Null } else { v }) }))?;
            let _ = n;
            arr
        }
        // HTML §4.12.3: the template's contents (from_dom moved the parsed ones into a fragment; a script-made template starts empty)
        "content" if tag == "template" => {
            let f = { let mut b = l.borrow_mut(); match b.templates.get(&i) { Some(&f) => f, None => {
                let f = b.new_node(11, "", "");
                b.templates.insert(i, f); f } } };
            wrap(it, f, R_NODE)
        }
        "shadowRoot" => { let r = l.borrow().shadows.get(&i).copied().unwrap_or(NONE); wrap(it, r, R_NODE) }
        "host" if kind == 11 => { let h = l.borrow().shadows.iter().find(|&(_, &r)| r == i).map(|(&h, _)| h).unwrap_or(NONE); wrap(it, h, R_NODE) }
        "classList" => wrap(it, i, R_CLASSES),
        "dataset" => wrap(it, i, R_DATASET),
        "offsetWidth" | "clientWidth" | "scrollWidth" => Val::Num(l.borrow().boxes.get(&i).map(|b| b[2]).unwrap_or(0) as f64),
        "offsetHeight" | "clientHeight" | "scrollHeight" => Val::Num(l.borrow().boxes.get(&i).map(|b| b[3]).unwrap_or(0) as f64),
        "offsetTop" => Val::Num(l.borrow().boxes.get(&i).map(|b| b[1]).unwrap_or(0) as f64),
        "offsetLeft" => Val::Num(l.borrow().boxes.get(&i).map(|b| b[0]).unwrap_or(0) as f64),
        "scrollTop" | "scrollLeft" => Val::Num(0.0),
        "isContentEditable" => Val::Bool(attr("contenteditable").map(|v| v != "false").unwrap_or(false)),
        "tabIndex" => Val::Num(attr("tabindex").and_then(|v| v.parse().ok()).unwrap_or(if matches!(tag.as_str(), "a" | "button" | "input" | "select" | "textarea") { 0.0 } else { -1.0 })),
        // document
        "documentElement" if is_doc => { let r = l.borrow().find_tag("html"); wrap(it, r, R_NODE) }
        "body" if is_doc => { let r = l.borrow().find_tag("body"); wrap(it, r, R_NODE) }
        "head" if is_doc => { let r = l.borrow().find_tag("head"); wrap(it, r, R_NODE) }
        "title" if is_doc => { let b = l.borrow(); let t = b.find_tag("title"); st(&if t == NONE { String::new() } else { b.text_content(t).split_whitespace().collect::<Vec<_>>().join(" ") }) }
        "readyState" if is_doc => st(l.borrow().ready),
        "cookie" if is_doc => { let f = l.borrow().cookie_io.clone(); st(&f.map(|f| f(None)).unwrap_or_default()) }
        "referrer" if is_doc => st(""),
        "URL" | "documentURI" if is_doc => st(&l.borrow().url),
        "location" if is_doc => wrap(it, 0, R_LOCATION),
        "defaultView" if is_doc => wrap(it, 0, R_WINDOW),
        "visibilityState" if is_doc => st("visible"),
        "hidden" if is_doc => Val::Bool(false),
        "currentScript" if is_doc => { let c = l.borrow().current_script; wrap(it, c, R_NODE) }
        "activeElement" if is_doc => { let b = l.borrow().find_tag("body"); wrap(it, b, R_NODE) }
        "characterSet" | "charset" if is_doc => st("UTF-8"),
        "compatMode" if is_doc => st("CSS1Compat"),
        "forms" if is_doc => { let v = l.borrow().select(i, "form", false).unwrap_or_default(); arr_of(v, it) }
        "images" if is_doc => { let v = l.borrow().select(i, "img", false).unwrap_or_default(); arr_of(v, it) }
        "links" if is_doc => { let v = l.borrow().select(i, "a[href]", false).unwrap_or_default(); arr_of(v, it) }
        _ => return Ok(match method(it, this, i, key) { Some(m) => m, None => proto_lookup(it, kind, &tag, key)? }),
    })
}

/// A key the platform does not define: the global interface objects' prototypes (scripts patch Element.prototype /
/// Node.prototype / HTMLElement.prototype — polyfills, frameworks), most derived first (WebIDL §3.7.3 interface prototype objects).
fn proto_lookup(it: &mut Interp, kind: u8, tag: &str, key: &str) -> Result<Val, Ctl> {
    let chain: &[&str] = match kind { 1 => &["HTMLElement", "Element", "Node", "EventTarget"], 9 => &["HTMLDocument", "Document", "Node", "EventTarget"],
        3 => &["Text", "CharacterData", "Node", "EventTarget"], 11 => &["DocumentFragment", "Node", "EventTarget"], _ => &["Node", "EventTarget"] };
    let g = it.global.clone();
    let specific = if kind == 1 { element_iface(tag) } else { None };
    for name in specific.into_iter().chain(chain.iter().copied()) {
        let Some(c) = it.lookup_opt(&g, name) else { continue };
        if !matches!(c, Val::Obj(_)) { continue; }
        let p = it.get(&c, "prototype")?;
        if let Val::Obj(o) = &p { if let Some((_, v)) = o.borrow().props.iter().find(|(k, _)| &**k == key) { return Ok(v.clone()); } }
    }
    Ok(Val::Undef)
}

/// The HTML interface name of an element (HTML §3.2.2 element interfaces), for prototype lookups.
pub fn element_iface(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "a" => "HTMLAnchorElement", "link" => "HTMLLinkElement", "script" => "HTMLScriptElement", "input" => "HTMLInputElement", "form" => "HTMLFormElement",
        "img" => "HTMLImageElement", "iframe" => "HTMLIFrameElement", "div" => "HTMLDivElement", "span" => "HTMLSpanElement", "button" => "HTMLButtonElement",
        "select" => "HTMLSelectElement", "textarea" => "HTMLTextAreaElement", "template" => "HTMLTemplateElement", "style" => "HTMLStyleElement",
        "meta" => "HTMLMetaElement", "body" => "HTMLBodyElement", "head" => "HTMLHeadElement", "html" => "HTMLHtmlElement", "video" => "HTMLVideoElement",
        "audio" => "HTMLAudioElement", "canvas" => "HTMLCanvasElement", "ul" => "HTMLUListElement", "ol" => "HTMLOListElement", "li" => "HTMLLIElement",
        "p" => "HTMLParagraphElement", "table" => "HTMLTableElement", "label" => "HTMLLabelElement", "option" => "HTMLOptionElement",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement", "pre" => "HTMLPreElement", "source" => "HTMLSourceElement", "dialog" => "HTMLDialogElement",
        "details" => "HTMLDetailsElement", "slot" => "HTMLSlotElement", "picture" => "HTMLPictureElement", "svg" => "SVGSVGElement", _ => return None,
    })
}

fn method(it: &mut Interp, _this: &Val, i: u32, key: &str) -> Option<Val> {
    Some(match key {
        "getAttribute" => m!("getAttribute", |it, _t, a| { let k = sv(it, &arg(&a, 0))?.to_ascii_lowercase(); let l = live(it); let r = l.borrow().attr(i, &k).map(st).unwrap_or(Val::Null); Ok(r) }),
        "hasAttribute" => m!("hasAttribute", |it, _t, a| { let k = sv(it, &arg(&a, 0))?.to_ascii_lowercase(); let l = live(it); let r = l.borrow().attr(i, &k).is_some(); Ok(Val::Bool(r)) }),
        "getAttributeNames" => m!("getAttributeNames", |it, _t, _a| { let l = live(it); let v: Vec<Val> = l.borrow().n[i as usize].attrs.iter().filter(|(k, _)| k != "data-omr").map(|(k, _)| st(k)).collect(); Ok(Val::array(v)) }),
        "hasAttributes" => m!("hasAttributes", |it, _t, _a| { let l = live(it); let r = !l.borrow().n[i as usize].attrs.is_empty(); Ok(Val::Bool(r)) }),
        "setAttribute" => m!("setAttribute", |it, _t, a| { let (k, v) = (sv(it, &arg(&a, 0))?, sv(it, &arg(&a, 1))?); live(it).borrow_mut().set_attr(i, &k, &v); Ok(Val::Undef) }),
        "removeAttribute" => m!("removeAttribute", |it, _t, a| { let k = sv(it, &arg(&a, 0))?.to_ascii_lowercase(); live(it).borrow_mut().del_attr(i, &k); Ok(Val::Undef) }),
        "toggleAttribute" => m!("toggleAttribute", |it, _t, a| { let k = sv(it, &arg(&a, 0))?.to_ascii_lowercase(); let l = live(it); let has = l.borrow().attr(i, &k).is_some(); let want = match arg(&a, 1) { Val::Undef => !has, v => v.truthy() }; if want && !has { l.borrow_mut().set_attr(i, &k, ""); } else if !want && has { l.borrow_mut().del_attr(i, &k); } Ok(Val::Bool(want)) }),
        "appendChild" => m!("appendChild", |it, _t, a| { let c = arg(&a, 0); if let Some(ci) = node_of(&c) { live(it).borrow_mut().insert(i, ci, NONE); } Ok(c) }),
        "insertBefore" => m!("insertBefore", |it, _t, a| { let c = arg(&a, 0); let before = node_of(&arg(&a, 1)).unwrap_or(NONE); if let Some(ci) = node_of(&c) { live(it).borrow_mut().insert(i, ci, before); } Ok(c) }),
        "removeChild" => m!("removeChild", |it, _t, a| { let c = arg(&a, 0); if let Some(ci) = node_of(&c) { live(it).borrow_mut().detach(ci); } Ok(c) }),
        "replaceChild" => m!("replaceChild", |it, _t, a| { let (n, o) = (arg(&a, 0), arg(&a, 1)); if let (Some(ni), Some(oi)) = (node_of(&n), node_of(&o)) { let l = live(it); let mut lb = l.borrow_mut(); lb.insert(i, ni, oi); lb.detach(oi); } Ok(o) }),
        "append" | "prepend" => { let pre = key == "prepend"; m!("append", |it, _t, a| { let ns = nodes_from(it, &a)?; let l = live(it); let mut lb = l.borrow_mut(); let first = if pre { lb.n[i as usize].kids.first().copied().unwrap_or(NONE) } else { NONE }; for c in ns { lb.insert(i, c, first); } Ok(Val::Undef) }) }
        "replaceChildren" => m!("replaceChildren", |it, _t, a| { let ns = nodes_from(it, &a)?; let l = live(it); let mut lb = l.borrow_mut(); for k in std::mem::take(&mut lb.n[i as usize].kids) { lb.n[k as usize].parent = NONE; } for c in ns { lb.insert(i, c, NONE); } lb.dirty = true; Ok(Val::Undef) }),
        "remove" => m!("remove", |it, _t, _a| { live(it).borrow_mut().detach(i); Ok(Val::Undef) }),
        "before" | "after" | "replaceWith" => { let k = key.to_string(); m!("before", |it, _t, a| {
            let ns = nodes_from(it, &a)?; let l = live(it); let mut lb = l.borrow_mut(); let p = lb.n[i as usize].parent; if p == NONE { return Ok(Val::Undef); }
            let anchor = if k == "after" { let ks = &lb.n[p as usize].kids; let pos = ks.iter().position(|&x| x == i).unwrap_or(0); ks.get(pos + 1).copied().unwrap_or(NONE) } else { i };
            for c in ns { lb.insert(p, c, anchor); }
            if k == "replaceWith" { lb.detach(i); }
            Ok(Val::Undef) }) }
        "cloneNode" => m!("cloneNode", |it, _t, a| { let c = live(it).borrow_mut().clone_node(i, arg(&a, 0).truthy()); Ok(wrap(it, c, R_NODE)) }),
        "contains" => m!("contains", |it, _t, a| { let Some(mut c) = node_of(&arg(&a, 0)) else { return Ok(Val::Bool(false)) }; let l = live(it); let b = l.borrow(); while c != NONE { if c == i { return Ok(Val::Bool(true)); } c = b.n[c as usize].parent; } Ok(Val::Bool(false)) }),
        "hasChildNodes" => m!("hasChildNodes", |it, _t, _a| { let l = live(it); let r = !l.borrow().n[i as usize].kids.is_empty(); Ok(Val::Bool(r)) }),
        "matches" => m!("matches", |it, _t, a| { let s = sv(it, &arg(&a, 0))?; let l = live(it); let r = l.borrow().matches(i, &s); Ok(Val::Bool(r)) }),
        "closest" => m!("closest", |it, _t, a| { let s = sv(it, &arg(&a, 0))?; let l = live(it); let mut c = i; let mut found = NONE; { let b = l.borrow(); while c != NONE && b.elem(c) { if b.matches(c, &s) { found = c; break; } c = b.n[c as usize].parent; } } Ok(wrap(it, found, R_NODE)) }),
        "querySelector" | "querySelectorAll" => { let first = key == "querySelector"; m!("querySelector", |it, _t, a| {
            let s = sv(it, &arg(&a, 0))?; let l = live(it); let r = l.borrow().select(i, &s, first);
            match r { Ok(v) => Ok(if first { wrap(it, v.first().copied().unwrap_or(NONE), R_NODE) } else { arr_of(v, it) }), Err(e) => Err(Ctl::Throw(it.error("SyntaxError", &e))) } }) }
        "getElementById" => m!("getElementById", |it, _t, a| { let id = sv(it, &arg(&a, 0))?; let l = live(it); let r = { let b = l.borrow(); (0..b.n.len() as u32).find(|&k| b.n[k as usize].kind == 1 && b.attr(k, "id") == Some(id.as_str()) && b.connected(k)).unwrap_or(NONE) }; Ok(wrap(it, r, R_NODE)) }),
        "getElementsByTagName" => m!("getElementsByTagName", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let l = live(it); let v = l.borrow().select(i, if t == "*" { "*" } else { &t }, false).unwrap_or_default(); Ok(arr_of(v, it)) }),
        "getElementsByClassName" => m!("getElementsByClassName", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let s: String = t.split_whitespace().map(|c| format!(".{c}")).collect(); let l = live(it); let v = l.borrow().select(i, &s, false).unwrap_or_default(); Ok(arr_of(v, it)) }),
        "getElementsByName" => m!("getElementsByName", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let l = live(it); let v = l.borrow().select(i, &format!("[name=\"{t}\"]"), false).unwrap_or_default(); Ok(arr_of(v, it)) }),
        "createElement" | "createElementNS" => { let ns = key == "createElementNS"; m!("createElement", |it, _t, a| { let t = sv(it, &arg(&a, if ns { 1 } else { 0 }))?; let id = live(it).borrow_mut().new_node(1, &t, ""); Ok(wrap(it, id, R_NODE)) }) }
        "createTextNode" => m!("createTextNode", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let id = live(it).borrow_mut().new_node(3, "", &t); Ok(wrap(it, id, R_NODE)) }),
        "createComment" => m!("createComment", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let id = live(it).borrow_mut().new_node(8, "", &t); Ok(wrap(it, id, R_NODE)) }),
        "createDocumentFragment" => m!("createDocumentFragment", |it, _t, _a| { let id = live(it).borrow_mut().new_node(11, "", ""); Ok(wrap(it, id, R_NODE)) }),
        "createEvent" => m!("createEvent", |_it, _t, _a| Ok(event_obj("", false))),
        "write" | "writeln" => m!("write", |it, _t, a| { let mut h = String::new(); for x in &a { h += &sv(it, x)?; } let l = live(it); let mut lb = l.borrow_mut(); let body = lb.find_tag("body"); if body != NONE { for c in lb.parse_fragment(&h) { lb.insert(body, c, NONE); } } Ok(Val::Undef) }),
        "insertAdjacentHTML" | "insertAdjacentText" | "insertAdjacentElement" => { let k = key.to_string(); m!("insertAdjacentHTML", |it, _t, a| {
            let pos = sv(it, &arg(&a, 0))?.to_ascii_lowercase();
            let l = live(it);
            let ns = if k == "insertAdjacentElement" { node_of(&arg(&a, 1)).into_iter().collect() } else { let h = sv(it, &arg(&a, 1))?; if k == "insertAdjacentHTML" { l.borrow_mut().parse_fragment(&h) } else { vec![l.borrow_mut().new_node(3, "", &h)] } };
            let mut lb = l.borrow_mut(); let p = lb.n[i as usize].parent;
            match pos.as_str() {
                "beforebegin" if p != NONE => for c in ns { lb.insert(p, c, i); },
                "afterend" if p != NONE => { let ks = &lb.n[p as usize].kids; let nx = ks.get(ks.iter().position(|&x| x == i).unwrap_or(0) + 1).copied().unwrap_or(NONE); for c in ns { lb.insert(p, c, nx); } }
                "afterbegin" => { let f = lb.n[i as usize].kids.first().copied().unwrap_or(NONE); for c in ns { lb.insert(i, c, f); } }
                _ => for c in ns { lb.insert(i, c, NONE); },
            }
            Ok(Val::Undef) }) }
        "addEventListener" => m!("addEventListener", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let f = arg(&a, 1); if f.is_callable() || matches!(f, Val::Obj(_)) { live(it).borrow_mut().listeners.entry((i, t)).or_default().push(f); } Ok(Val::Undef) }),
        "removeEventListener" => m!("removeEventListener", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let f = arg(&a, 1); if let Some(v) = live(it).borrow_mut().listeners.get_mut(&(i, t)) { v.retain(|x| !Interp::strict_eq(x, &f)); } Ok(Val::Undef) }),
        "dispatchEvent" => m!("dispatchEvent", |it, _t, a| { let e = arg(&a, 0); let ok = dispatch(it, i, e)?; Ok(Val::Bool(ok)) }),
        "click" => m!("click", |it, _t, _a| { let e = event_obj("click", true); dispatch(it, i, e)?; Ok(Val::Undef) }),
        "focus" | "blur" | "scrollIntoView" | "scrollTo" | "scrollBy" | "normalize" | "setPointerCapture" | "releasePointerCapture" => m!("noop", |_it, _t, _a| Ok(Val::Undef)),
        "getBoundingClientRect" => m!("getBoundingClientRect", |it, _t, _a| { let l = live(it); let b = l.borrow().boxes.get(&i).copied().unwrap_or([0; 4]); Ok(rect(b)) }),
        "getClientRects" => m!("getClientRects", |it, _t, _a| { let l = live(it); let b = l.borrow().boxes.get(&i).copied(); Ok(Val::array(b.into_iter().map(rect).collect())) }),
        "attachShadow" => m!("attachShadow", |it, _t, _a| { let l = live(it); let id = { let mut b = l.borrow_mut(); let id = b.new_node(11, "", ""); b.shadows.insert(i, id); b.dirty = true; id }; Ok(wrap(it, id, R_NODE)) }),
        "toString" => m!("toString", |it, _t, _a| { let l = live(it); let t = l.borrow().n[i as usize].tag.clone(); Ok(st(&format!("[object HTML{}Element]", t))) }),
        // HTML §4.12.5: no 2D / WebGL context in this engine — getContext answers null (the spec's "unsupported" answer), so
        // pages take their fallback path instead of throwing
        "getContext" => m!("getContext", |_it, _t, _a| Ok(Val::Null)),
        "toDataURL" => m!("toDataURL", |_it, _t, _a| Ok(st("data:,"))),
        _ => return None,
    })
}

fn rect(b: [i32; 4]) -> Val {
    let f = |x: i32| Val::Num(x as f64);
    Val::plain(vec![("x", f(b[0])), ("y", f(b[1])), ("left", f(b[0])), ("top", f(b[1])), ("width", f(b[2])), ("height", f(b[3])), ("right", f(b[0] + b[2])), ("bottom", f(b[1] + b[3]))])
}

/// Property writes (true = handled by the platform; false = an expando on the JS object).
pub fn set(it: &mut Interp, i: u32, role: u8, key: &str, v: &Val) -> Result<bool, Ctl> {
    match role {
        R_WINDOW => { if key == "location" { let u = sv(it, v)?; let l = live(it); let u = resolve(&l.borrow(), &u); l.borrow_mut().navigate = Some(u); return Ok(true); } let g = it.global.clone(); Interp::declare(&g, key, v.clone()); return Ok(true); }
        R_LOCATION => { if matches!(key, "href" | "pathname" | "search" | "hash") { let u = sv(it, v)?; let l = live(it); let base = l.borrow().url.clone(); let nu = match key { "href" => crate::page::resolve(&base, &u), "hash" => format!("{}#{}", base.split('#').next().unwrap_or(""), u.trim_start_matches('#')), "search" => format!("{}?{}", base.split(['?', '#']).next().unwrap_or(""), u.trim_start_matches('?')), _ => crate::page::resolve(&base, &u) }; if key != "hash" { l.borrow_mut().navigate = Some(nu); } else { l.borrow_mut().url = nu; } return Ok(true); } return Ok(false); }
        R_STORAGE => { let s = sv(it, v)?; storage_set(it, key, &s); return Ok(true); }
        R_STYLE => {
            let val = if matches!(v, Val::Undef | Val::Null) { String::new() } else { sv(it, v)? };
            let l = live(it); let mut lb = l.borrow_mut();
            if key == "cssText" { lb.set_attr(i, "style", &val); return Ok(true); }
            let k = if key.contains('-') { key.to_string() } else { camel_to_kebab(key) };
            let mut d = style_decls(lb.attr(i, "style").unwrap_or(""));
            d.retain(|(x, _)| *x != k);
            if !val.is_empty() { d.push((k, val)); }
            let s = d.iter().map(|(a, b)| format!("{a}: {b}")).collect::<Vec<_>>().join("; ");
            lb.set_attr(i, "style", &s);
            return Ok(true);
        }
        R_DATASET => { let s = sv(it, v)?; live(it).borrow_mut().set_attr(i, &format!("data-{}", camel_to_kebab(key)), &s); return Ok(true); }
        R_CLASSES => return Ok(true),
        _ => {}
    }
    let l = live(it);
    let tag = l.borrow().n[i as usize].tag.clone();
    let is_doc = l.borrow().n[i as usize].kind == 9;
    match key {
        "textContent" | "innerText" | "nodeValue" | "data" => { let t = if matches!(v, Val::Null | Val::Undef) { String::new() } else { sv(it, v)? }; l.borrow_mut().set_text(i, &t); }
        "innerHTML" => { let h = sv(it, v)?; let mut lb = l.borrow_mut(); for k in std::mem::take(&mut lb.n[i as usize].kids) { lb.n[k as usize].parent = NONE; } for c in lb.parse_fragment(&h) { lb.insert(i, c, NONE); } lb.dirty = true; }
        "outerHTML" => { let h = sv(it, v)?; let mut lb = l.borrow_mut(); let p = lb.n[i as usize].parent; if p != NONE { for c in lb.parse_fragment(&h) { lb.insert(p, c, i); } lb.detach(i); } }
        "id" | "className" | "title" | "lang" | "dir" | "name" | "type" | "placeholder" | "href" | "src" | "rel" | "target" | "alt" | "role" | "htmlFor" | "action" | "method" if !is_doc => {
            let s = sv(it, v)?; l.borrow_mut().set_attr(i, match key { "className" => "class", "htmlFor" => "for", k => k }, &s);
        }
        "value" => { let s = sv(it, v)?; if tag == "textarea" { l.borrow_mut().set_text(i, &s); } else { l.borrow_mut().set_attr(i, "value", &s); } }
        "checked" | "disabled" | "hidden" | "selected" | "required" | "open" => { let k = key; if v.truthy() { l.borrow_mut().set_attr(i, k, ""); } else { l.borrow_mut().del_attr(i, k); } }
        "title" if is_doc => { let s = sv(it, v)?; let mut lb = l.borrow_mut(); let t = lb.find_tag("title"); if t != NONE { lb.set_text(t, &s); } }
        "cookie" if is_doc => { let s = sv(it, v)?; let f = l.borrow().cookie_io.clone(); if let Some(f) = f { f(Some(&s)); } }
        _ => return Ok(false),
    }
    Ok(true)
}

// ---- events (DOM §2) ----
pub fn event_obj(ty: &str, bubbles: bool) -> Val {
    Val::plain(vec![("type", st(ty)), ("bubbles", Val::Bool(bubbles)), ("cancelable", Val::Bool(true)), ("defaultPrevented", Val::Bool(false)),
        ("target", Val::Null), ("currentTarget", Val::Null), ("button", Val::Num(0.0)), ("key", st("")), ("timeStamp", Val::Num(0.0)), ("isTrusted", Val::Bool(true)),
        ("preventDefault", Val::native("preventDefault", |it, this, _a| { it.set(&this, "defaultPrevented", Val::Bool(true))?; Ok(Val::Undef) })),
        ("stopPropagation", Val::native("stopPropagation", |it, this, _a| { it.set(&this, "__stop", Val::Bool(true))?; Ok(Val::Undef) })),
        ("stopImmediatePropagation", Val::native("stopImmediatePropagation", |it, this, _a| { it.set(&this, "__stop", Val::Bool(true))?; it.set(&this, "__stopnow", Val::Bool(true))?; Ok(Val::Undef) })),
        ("composedPath", Val::native("composedPath", |_it, _t, _a| Ok(Val::array(vec![])))),
        // DOM §2.2 legacy initialisers (document.createEvent + initEvent / initCustomEvent)
        ("initEvent", Val::native("initEvent", |it, this, a| { let t = arg(&a, 0); it.set(&this, "type", t)?; it.set(&this, "bubbles", Val::Bool(arg(&a, 1).truthy()))?; it.set(&this, "cancelable", Val::Bool(arg(&a, 2).truthy()))?; Ok(Val::Undef) })),
        ("initCustomEvent", Val::native("initCustomEvent", |it, this, a| { let t = arg(&a, 0); it.set(&this, "type", t)?; it.set(&this, "bubbles", Val::Bool(arg(&a, 1).truthy()))?; it.set(&this, "cancelable", Val::Bool(arg(&a, 2).truthy()))?; it.set(&this, "detail", arg(&a, 3))?; Ok(Val::Undef) }))])
}

/// Fire `ev` at node `target`: at-target then bubbling through ancestors, then document and window (DOM §2.9 dispatch, without
/// a capture phase). Listeners: addEventListener entries, then the `on<type>` property / content attribute. Returns
/// !defaultPrevented. Handler exceptions are reported to the page log and do not stop dispatch (HTML "report the exception").
pub fn dispatch(it: &mut Interp, target: u32, ev: Val) -> Result<bool, Ctl> {
    let ty = it.get(&ev, "type").and_then(|t| it.to_str(&t)).map(|t| t.to_string()).unwrap_or_default();
    let bubbles = it.get(&ev, "bubbles").map(|b| b.truthy()).unwrap_or(false);
    let l = live(it);
    let mut path = vec![(target, R_NODE)];
    if bubbles || target == l.borrow().doc {
        let mut p = l.borrow().n.get(target as usize).map(|x| x.parent).unwrap_or(NONE);
        while p != NONE { path.push((p, R_NODE)); p = l.borrow().n[p as usize].parent; }
        if bubbles || target == l.borrow().doc { path.push((0, R_WINDOW)); }
    }
    let tw = wrap(it, target, R_NODE);
    it.set(&ev, "target", tw.clone())?; it.set(&ev, "srcElement", tw)?;
    for (n, role) in path {
        let cur = wrap(it, n, role);
        it.set(&ev, "currentTarget", cur.clone())?;
        let key = (if role == R_WINDOW { NONE - 1 } else { n }, ty.clone());
        let ls = l.borrow().listeners.get(&key).cloned().unwrap_or_default();
        for f in ls {
            let r = if f.is_callable() { it.call(&f, cur.clone(), vec![ev.clone()]) } else { let h = it.get(&f, "handleEvent")?; it.call(&h, f.clone(), vec![ev.clone()]) };
            if let Err(Ctl::Throw(e)) = r { let m = it.to_str(&e).map(|x| x.to_string()).unwrap_or_default(); l.borrow_mut().log.push(format!("{ty} listener: {m}")); }
            if it.get(&ev, "__stopnow").map(|x| x.truthy()).unwrap_or(false) { break; }
        }
        // on<type>: a property set from script, or the content attribute compiled as a handler body
        let prop = { if let Val::Obj(o) = &cur { o.borrow().props.iter().find(|(k, _)| **k == *format!("on{ty}")).map(|(_, v)| v.clone()) } else { None } };
        let handler = match prop { Some(f) if f.is_callable() => Some(f), _ => {
            let src = if role == R_NODE { l.borrow().attr(n, &format!("on{ty}")).map(|s| s.to_string()) } else { None };
            match src { Some(s) => compile_handler(it, &s), None => None }
        } };
        if let Some(h) = handler {
            match it.call(&h, cur.clone(), vec![ev.clone()]) {
                Ok(Val::Bool(false)) => { it.set(&ev, "defaultPrevented", Val::Bool(true))?; }
                Err(Ctl::Throw(e)) => { let m = it.to_str(&e).map(|x| x.to_string()).unwrap_or_default(); l.borrow_mut().log.push(format!("on{ty}: {m}")); }
                _ => {}
            }
        }
        if it.get(&ev, "__stop").map(|x| x.truthy()).unwrap_or(false) { break; }
    }
    Ok(!it.get(&ev, "defaultPrevented").map(|x| x.truthy()).unwrap_or(false))
}

/// An event-handler content attribute → a function (HTML §8.1.8.1: the body of `function (event) { … }`).
fn compile_handler(it: &mut Interp, body: &str) -> Option<Val> {
    let prog = crate::jsparse::parse(&format!("(function (event) {{\n{body}\n}})"), false).ok()?;
    let e = &prog["body"][0]["expression"];
    let g = it.global.clone();
    it.eval(e, &g).ok()
}

// ---- window / location / storage ----
fn window_get(it: &mut Interp, key: &str) -> Result<Val, Ctl> {
    let g = it.global.clone();
    if let Some(v) = it.lookup_opt(&g, key) { return Ok(v); }
    let l = live(it);
    Ok(match key {
        "innerWidth" | "outerWidth" => Val::Num(l.borrow().viewport.0 as f64),
        "innerHeight" | "outerHeight" => Val::Num(l.borrow().viewport.1 as f64),
        "devicePixelRatio" => Val::Num(1.0),
        "scrollX" | "scrollY" | "pageXOffset" | "pageYOffset" => Val::Num(0.0),
        "addEventListener" => m!("addEventListener", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let f = arg(&a, 1); live(it).borrow_mut().listeners.entry((NONE - 1, t)).or_default().push(f); Ok(Val::Undef) }),
        "removeEventListener" => m!("removeEventListener", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let f = arg(&a, 1); if let Some(v) = live(it).borrow_mut().listeners.get_mut(&(NONE - 1, t)) { v.retain(|x| !Interp::strict_eq(x, &f)); } Ok(Val::Undef) }),
        "dispatchEvent" => m!("dispatchEvent", |it, _t, a| { let d = live(it).borrow().doc; dispatch(it, d, arg(&a, 0))?; Ok(Val::Bool(true)) }),
        _ => Val::Undef,
    })
}

fn url_parts(u: &str) -> (String, String, String, String, String) {
    // (protocol, host, pathname, search, hash) — WHATWG URL fields for an absolute http(s) URL
    let (scheme, rest) = u.split_once("://").unwrap_or(("https", u));
    let (rest, hash) = match rest.split_once('#') { Some((a, b)) => (a, format!("#{b}")), None => (rest, String::new()) };
    let (rest, search) = match rest.split_once('?') { Some((a, b)) => (a, format!("?{b}")), None => (rest, String::new()) };
    let (host, path) = match rest.find('/') { Some(p) => (&rest[..p], &rest[p..]), None => (rest, "/") };
    (format!("{scheme}:"), host.to_string(), path.to_string(), if search == "?" { String::new() } else { search }, if hash == "#" { String::new() } else { hash })
}
fn location_get(it: &mut Interp, key: &str) -> Result<Val, Ctl> {
    let l = live(it);
    let u = l.borrow().url.clone();
    let (proto, host, path, search, hash) = url_parts(&u);
    Ok(match key {
        "href" => st(&u), "protocol" => st(&proto), "host" => st(&host), "hostname" => st(host.split(':').next().unwrap_or("")),
        "port" => st(host.split_once(':').map(|x| x.1).unwrap_or("")), "pathname" => st(&path), "search" => st(&search), "hash" => st(&hash),
        "origin" => st(&format!("{proto}//{host}")), "toString" => m!("toString", |it, _t, _a| { let l = live(it); let u = l.borrow().url.clone(); Ok(st(&u)) }),
        "assign" | "replace" => m!("assign", |it, _t, a| { let u = sv(it, &arg(&a, 0))?; let l = live(it); let r = resolve(&l.borrow(), &u); l.borrow_mut().navigate = Some(r); Ok(Val::Undef) }),
        "reload" => m!("reload", |it, _t, _a| { let l = live(it); let u = l.borrow().url.clone(); l.borrow_mut().navigate = Some(u); Ok(Val::Undef) }),
        _ => Val::Undef,
    })
}
fn storage_get(it: &mut Interp, key: &str) -> Result<Val, Ctl> {
    let l = live(it);
    Ok(match key {
        "length" => Val::Num(l.borrow().storage.len() as f64),
        "getItem" => m!("getItem", |it, _t, a| { let k = sv(it, &arg(&a, 0))?; let l = live(it); let r = l.borrow().storage.iter().find(|(x, _)| *x == k).map(|(_, v)| st(v)).unwrap_or(Val::Null); Ok(r) }),
        "setItem" => m!("setItem", |it, _t, a| { let (k, v) = (sv(it, &arg(&a, 0))?, sv(it, &arg(&a, 1))?); storage_set(it, &k, &v); Ok(Val::Undef) }),
        "removeItem" => m!("removeItem", |it, _t, a| { let k = sv(it, &arg(&a, 0))?; live(it).borrow_mut().storage.retain(|(x, _)| *x != k); Ok(Val::Undef) }),
        "clear" => m!("clear", |it, _t, _a| { live(it).borrow_mut().storage.clear(); Ok(Val::Undef) }),
        "key" => m!("key", |it, _t, a| { let n = it.to_num(&arg(&a, 0))? as usize; let l = live(it); let r = l.borrow().storage.get(n).map(|(k, _)| st(k)).unwrap_or(Val::Null); Ok(r) }),
        _ => l.borrow().storage.iter().find(|(x, _)| x == key).map(|(_, v)| st(v)).unwrap_or(Val::Undef),
    })
}
fn storage_set(it: &mut Interp, k: &str, v: &str) {
    let l = live(it); let mut lb = l.borrow_mut();
    match lb.storage.iter_mut().find(|(x, _)| x == k) { Some(p) => p.1 = v.to_string(), None => lb.storage.push((k.to_string(), v.to_string())) }
}

fn pct_dec(s: &str) -> String {
    let b = s.replace('+', " ").into_bytes(); let mut o = Vec::with_capacity(b.len()); let mut i = 0;
    while i < b.len() { if b[i] == b'%' && i + 2 < b.len() + 0 + 1 && i + 2 < b.len() { if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("x"), 16) { o.push(v); i += 3; continue; } } o.push(b[i]); i += 1; }
    String::from_utf8_lossy(&o).into_owned()
}
fn search_params(q: &str) -> Val {
    let pairs: Vec<(String, String)> = q.split('&').filter(|x| !x.is_empty()).map(|kv| { let (k, v) = kv.split_once('=').unwrap_or((kv, "")); (pct_dec(k), pct_dec(v)) }).collect();
    let p2 = pairs.clone(); let p3 = pairs.clone(); let p4 = pairs.clone();
    Val::plain(vec![
        ("get", Val::native("get", move |it, _t, a| { let k = sv(it, &arg(&a, 0))?; Ok(p2.iter().find(|(x, _)| *x == k).map(|(_, v)| st(v)).unwrap_or(Val::Null)) })),
        ("has", Val::native("has", move |it, _t, a| { let k = sv(it, &arg(&a, 0))?; Ok(Val::Bool(p3.iter().any(|(x, _)| *x == k))) })),
        ("getAll", Val::native("getAll", move |it, _t, a| { let k = sv(it, &arg(&a, 0))?; Ok(Val::array(p4.iter().filter(|(x, _)| *x == k).map(|(_, v)| st(v)).collect())) })),
        ("toString", { let q = q.to_string(); Val::native("toString", move |_it, _t, _a| Ok(st(&q))) }), ("__omrParams", Val::Bool(true)),
        ("forEach", { let p = pairs.clone(); Val::native("forEach", move |it, _t, a| { let f = arg(&a, 0); for (k, v) in &p { it.call(&f, Val::Undef, vec![st(v), st(k)])?; } Ok(Val::Undef) }) }),
    ])
}
fn url_obj(u: &str) -> Val {
    let (proto, host, path, search, hash) = url_parts(u);
    let href = u.to_string();
    Val::plain(vec![("href", st(u)), ("protocol", st(&proto)), ("host", st(&host)), ("hostname", st(host.split(':').next().unwrap_or(""))),
        ("port", st(host.split_once(':').map(|x| x.1).unwrap_or(""))), ("pathname", st(&path)), ("search", st(&search)), ("hash", st(&hash)),
        ("origin", st(&format!("{proto}//{host}"))), ("searchParams", search_params(search.trim_start_matches('?'))),
        ("toString", Val::native("toString", move |_it, _t, _a| Ok(st(&href))))])
}

/// Install the page globals on `it` (window = the global scope seen as an object).
pub fn install(it: &mut Interp, l: LiveRef, fetch: Val) {
    it.dom = Some(l.clone());
    let doc = l.borrow().doc;
    let g = it.global.clone();
    let win = wrap(it, 0, R_WINDOW);
    let put = |k: &str, v: Val| Interp::declare(&g, k, v);
    put("this", win.clone()); // a classic script's top-level `this` is the window (UMD bundles test `this.define`, set `this.x`)
    put("window", win.clone()); put("self", win.clone()); put("globalThis", win.clone()); put("top", win.clone()); put("parent", win.clone()); put("frames", win);
    put("document", wrap(it, doc, R_NODE));
    put("location", wrap(it, 0, R_LOCATION));
    put("localStorage", wrap(it, 0, R_STORAGE)); put("sessionStorage", wrap(it, 0, R_STORAGE));
    put("navigator", Val::plain(vec![("userAgent", st(&crate::net::user_agent())), ("language", st("en-US")),
        ("languages", Val::array(vec![st("en-US")])), ("cookieEnabled", Val::Bool(true)), ("onLine", Val::Bool(true)), ("platform", st("Linux x86_64")),
        ("doNotTrack", st("1")), ("hardwareConcurrency", Val::Num(2.0)), ("maxTouchPoints", Val::Num(0.0)), ("webdriver", Val::Bool(false))]));
    // HTML §7.4.3 session history: pushState / replaceState change the document URL (same origin) without a navigation
    let hist_set = |push: bool| move |it: &mut Interp, this: Val, a: Vec<Val>| -> Result<Val, Ctl> {
        let st_v = arg(&a, 0);
        let l = live(it);
        let u = match arg(&a, 2) { Val::Undef | Val::Null => None, v => Some(sv(it, &v)?) };
        if let Some(u) = u {
            let nu = resolve(&l.borrow(), &u);
            let same = crate::cookie::parts(&nu).map(|p| p.1) == crate::cookie::parts(&l.borrow().url).map(|p| p.1);
            if !same { return Err(Ctl::Throw(it.error("SecurityError", "pushState to another origin"))); }
            l.borrow_mut().url = nu;
        }
        if push { l.borrow_mut().history_len += 1; }
        it.set(&this, "state", st_v)?;
        let n = l.borrow().history_len as f64;
        it.set(&this, "length", Val::Num(n))?;
        Ok(Val::Undef)
    };
    put("history", Val::plain(vec![("length", Val::Num(1.0)), ("state", Val::Null), ("scrollRestoration", st("auto")),
        ("pushState", Val::native("pushState", hist_set(true))), ("replaceState", Val::native("replaceState", hist_set(false))),
        ("back", m!("back", |_i, _t, _a| Ok(Val::Undef))), ("forward", m!("forward", |_i, _t, _a| Ok(Val::Undef))), ("go", m!("go", |_i, _t, _a| Ok(Val::Undef)))]));
    put("screen", Val::plain(vec![("width", Val::Num(1280.0)), ("height", Val::Num(800.0)), ("availWidth", Val::Num(1280.0)), ("availHeight", Val::Num(800.0)), ("colorDepth", Val::Num(24.0))]));
    put("fetch", fetch);
    put("requestAnimationFrame", m!("requestAnimationFrame", |it, _t, a| { it.timers.push((arg(&a, 0), 16.0, false, vec![Val::Num(16.0)])); Ok(Val::Num(it.timers.len() as f64)) }));
    put("cancelAnimationFrame", m!("cancelAnimationFrame", |it, _t, a| { if let Val::Num(n) = arg(&a, 0) { let k = n as usize; if k >= 1 && k <= it.timers.len() { it.timers[k - 1].0 = Val::Undef; } } Ok(Val::Undef) }));
    put("requestIdleCallback", m!("requestIdleCallback", |it, _t, a| { it.timers.push((arg(&a, 0), 50.0, false, vec![Val::plain(vec![("didTimeout", Val::Bool(false)), ("timeRemaining", m!("timeRemaining", |_i, _t, _a| Ok(Val::Num(10.0))))])])); Ok(Val::Num(it.timers.len() as f64)) }));
    put("matchMedia", m!("matchMedia", |it, _t, a| { let q = sv(it, &arg(&a, 0))?; let w = live(it).borrow().viewport.0;
        // the width features the author's light pages branch on; everything else (color scheme, motion, hover) answers false
        let num = |s: &str| s.trim().trim_end_matches(')').trim().trim_end_matches("px").trim().parse::<f64>().ok();
        let m = if let Some(p) = q.find("min-width:") { num(&q[p + 10..]).map(|v| w as f64 >= v).unwrap_or(false) } else if let Some(p) = q.find("max-width:") { num(&q[p + 10..]).map(|v| (w as f64) <= v).unwrap_or(false) } else { false };
        Ok(Val::plain(vec![("matches", Val::Bool(m)), ("media", st(&q)), ("addListener", m!("addListener", |_i, _t, _a| Ok(Val::Undef))), ("removeListener", m!("removeListener", |_i, _t, _a| Ok(Val::Undef))),
            ("addEventListener", m!("addEventListener", |_i, _t, _a| Ok(Val::Undef))), ("removeEventListener", m!("removeEventListener", |_i, _t, _a| Ok(Val::Undef)))])) }));
    put("getComputedStyle", m!("getComputedStyle", |it, _t, a| { match node_of(&arg(&a, 0)) { Some(i) => Ok(wrap(it, i, R_CSTYLE)), None => Ok(Val::plain(vec![])) } }));
    put("scrollTo", m!("scrollTo", |_i, _t, _a| Ok(Val::Undef))); put("scrollBy", m!("scrollBy", |_i, _t, _a| Ok(Val::Undef))); put("scroll", m!("scroll", |_i, _t, _a| Ok(Val::Undef)));
    put("alert", m!("alert", |it, _t, a| { let s = sv(it, &arg(&a, 0))?; live(it).borrow_mut().log.push(format!("alert: {s}")); Ok(Val::Undef) }));
    put("confirm", m!("confirm", |_i, _t, _a| Ok(Val::Bool(false)))); put("prompt", m!("prompt", |_i, _t, _a| Ok(Val::Null)));
    put("Event", m!("Event", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let b = it.get(&arg(&a, 1), "bubbles").map(|x| x.truthy()).unwrap_or(false); Ok(event_obj(&t, b)) }));
    put("CustomEvent", m!("CustomEvent", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; let o = arg(&a, 1); let b = if matches!(o, Val::Obj(_)) { it.get(&o, "bubbles")?.truthy() } else { false }; let e = event_obj(&t, b); let d = if matches!(o, Val::Obj(_)) { it.get(&o, "detail")? } else { Val::Null }; it.set(&e, "detail", d)?; Ok(e) }));
    for name in ["KeyboardEvent", "MouseEvent", "FocusEvent", "InputEvent", "PointerEvent", "UIEvent"] { put(name, m!("Event", |it, _t, a| { let t = sv(it, &arg(&a, 0))?; Ok(event_obj(&t, true)) })); }
    // observers: accepted, never fire (no layout feedback loop in v1)
    for name in ["MutationObserver", "IntersectionObserver", "ResizeObserver", "PerformanceObserver"] {
        put(name, m!("Observer", |_it, _t, _a| Ok(Val::plain(vec![("observe", m!("observe", |_i, _t, _a| Ok(Val::Undef))), ("unobserve", m!("unobserve", |_i, _t, _a| Ok(Val::Undef))),
            ("disconnect", m!("disconnect", |_i, _t, _a| Ok(Val::Undef))), ("takeRecords", m!("takeRecords", |_i, _t, _a| Ok(Val::array(vec![]))))]))));
    }
    // custom elements (HTML §4.13): define() upgrades the elements already in the tree — the class's prototype methods
    // join the element object and connectedCallback runs with the element as `this` (no constructor call in v1)
    put("customElements", Val::plain(vec![
        ("define", m!("define", |it, _t, a| {
            let name = sv(it, &arg(&a, 0))?.to_ascii_lowercase();
            let cls = arg(&a, 1);
            let proto = it.get(&cls, "prototype")?;
            let methods = if let Val::Obj(o) = &proto { o.borrow().props.clone() } else { vec![] };
            let l = live(it);
            let els: Vec<u32> = { let b = l.borrow(); (0..b.n.len() as u32).filter(|&i| b.n[i as usize].kind == 1 && b.n[i as usize].tag == name && b.connected(i)).collect() };
            for e in els {
                let w = wrap(it, e, R_NODE);
                if let Val::Obj(o) = &w { o.borrow_mut().props.extend(methods.iter().cloned()); }
                let cb = it.get(&w, "connectedCallback")?;
                if cb.is_callable() { if let Err(Ctl::Throw(x)) = it.call(&cb, w.clone(), vec![]) { let m = it.to_str(&x).map(|s| s.to_string()).unwrap_or_default(); l.borrow_mut().log.push(format!("{name} connectedCallback: {m}")); } }
            }
            Ok(Val::Undef)
        })),
        ("get", m!("get", |_i, _t, _a| Ok(Val::Undef))),
        ("whenDefined", m!("whenDefined", |_i, _t, _a| Ok(crate::jsi_lib::resolved(Val::Undef)))),
    ]));
    // Object reflection, Symbol, Promise combinators, URL / URLSearchParams, queueMicrotask: the language library's (jsi_lib)
    for name in ["Node", "Element", "HTMLElement", "HTMLDocument", "Document", "Text", "HTMLInputElement", "HTMLAnchorElement", "SVGElement", "DocumentFragment", "EventTarget", "XMLHttpRequest", "Image", "WebSocket", "Worker", "ShadowRoot", "HTMLIFrameElement", "HTMLFormElement", "HTMLImageElement", "HTMLScriptElement", "CSSStyleSheet", "DOMParser", "FormData", "Blob", "File", "FileReader", "URLSearchParams", "TextEncoder", "TextDecoder", "AbortSignal"] {
        if it.lookup_opt(&g, name).is_some() { continue; }
        put(name, Val::plain(vec![("ELEMENT_NODE", Val::Num(1.0)), ("TEXT_NODE", Val::Num(3.0)), ("COMMENT_NODE", Val::Num(8.0)), ("DOCUMENT_NODE", Val::Num(9.0)), ("DOCUMENT_FRAGMENT_NODE", Val::Num(11.0)), ("prototype", Val::plain(vec![]))]));
    }
    // window members scripts use as bare names (the global object IS the window: HTML §7.2)
    let (vw, vh) = l.borrow().viewport;
    for (k, v) in [("innerWidth", vw), ("outerWidth", vw), ("innerHeight", vh), ("outerHeight", vh)] { put(k, Val::Num(v as f64)); }
    for k in ["scrollX", "scrollY", "pageXOffset", "pageYOffset"] { put(k, Val::Num(0.0)); }
    put("devicePixelRatio", Val::Num(1.0));
    for k in ["addEventListener", "removeEventListener", "dispatchEvent"] { if let Ok(f) = window_get(it, k) { put(k, f); } }
    // natives the platform prelude builds on (names start with __omr: not a web API)
    put("__omr_req", m!("__omr_req", |it, _t, a| {
        let (m, u, ct) = (sv(it, &arg(&a, 0))?, sv(it, &arg(&a, 1))?, sv(it, &arg(&a, 2))?);
        let body = match arg(&a, 3) { Val::Undef | Val::Null => None, v => Some(sv(it, &v)?) };
        let io = live(it).borrow().req_io.clone();
        let Some(io) = io else { return Ok(Val::Null) };
        Ok(match io(&m, &u, &ct, body.as_deref()) {
            Some((status, fu, hs, b)) => {
                let ctype = hs.iter().find(|(k, _)| k == "content-type").map(|(_, v)| v.clone());
                let text = String::from_utf8_lossy(&crate::net::to_utf8(b, ctype.as_deref())).into_owned();
                Val::array(vec![Val::Num(status as f64), st(&fu), Val::array(hs.iter().map(|(k, v)| Val::array(vec![st(k), st(v)])).collect()), st(&text)])
            }
            None => Val::Null,
        })
    }));
    put("open", m!("open", |it, _t, a| { let u = match arg(&a, 0) { Val::Undef | Val::Null => String::new(), v => sv(it, &v)? }; if !u.is_empty() { let l = live(it); let r = resolve(&l.borrow(), &u); l.borrow_mut().opened.push(r); } Ok(Val::Null) }));
    put("__omr_ws", m!("__omr_ws", |it, _t, a| {
        // (op, id, url / data, code, reason) → the broker's answer as a JS value
        let op = sv(it, &arg(&a, 0))?; let id = it.to_num(&arg(&a, 1))?;
        let data = match arg(&a, 2) { Val::Undef | Val::Null => String::new(), v => sv(it, &v)? };
        let code = match arg(&a, 3) { Val::Undef => 1000.0, v => it.to_num(&v)? }; let reason = match arg(&a, 4) { Val::Undef => String::new(), v => sv(it, &v)? };
        let mut url = String::new();
        if op == "open" { let l = live(it); let r = resolve(&l.borrow(), &data); url = if let Some(x) = r.strip_prefix("https://") { format!("wss://{x}") } else if let Some(x) = r.strip_prefix("http://") { format!("ws://{x}") } else { r }; }
        let io = live(it).borrow().ws_io.clone();
        let Some(io) = io else { return Ok(Val::Null) };
        let r = io(&serde_json::json!({"op": op, "id": id.max(0.0) as u64, "url": url, "data": data, "code": code.clamp(0.0, 65535.0) as u64, "reason": reason}));
        Ok(crate::jsi_lib::json_parse(&r.to_string()).unwrap_or(Val::Null))
    }));
    put("__omr_resolve", m!("__omr_resolve", |it, _t, a| { let u = sv(it, &arg(&a, 0))?; let l = live(it); let r = resolve(&l.borrow(), &u); Ok(st(&r)) }));
    put("__omr_observe", m!("__omr_observe", |it, _t, a| { let on = arg(&a, 0).truthy(); let l = live(it); let mut b = l.borrow_mut(); b.observing = on; if !on { b.mutations.clear(); } Ok(Val::Undef) }));
    put("__omr_mutations", m!("__omr_mutations", |it, _t, _a| {
        let ms = std::mem::take(&mut live(it).borrow_mut().mutations);
        let v: Vec<Val> = ms.into_iter().map(|(t, n, k, add, rem)| Val::array(vec![Val::Num(t as f64), wrap(it, n, R_NODE), if k.is_empty() { Val::Null } else { st(&k) }, wrap(it, add, R_NODE), wrap(it, rem, R_NODE)])).collect();
        Ok(Val::array(v))
    }));
    put("__omr_isnode", m!("__omr_isnode", |it, _t, a| {
        // (value, node kind or 0 = any, tag or "") → is it a DOM node of that kind / tag
        let Some(n) = node_of(&arg(&a, 0)) else { return Ok(Val::Bool(false)) };
        let k = it.to_num(&arg(&a, 1))? as u8; let t = match arg(&a, 2) { Val::Undef | Val::Null => String::new(), v => sv(it, &v)? };
        let l = live(it); let b = l.borrow(); let x = &b.n[n as usize];
        Ok(Val::Bool((k == 0 || x.kind == k || (k == 11 && x.kind == 11)) && (t.is_empty() || t.split(',').any(|w| w == x.tag))))
    }));
    put("__omr_formdata", m!("__omr_formdata", |it, _t, a| {
        // XHR §4.3 "constructing the entry list": named, enabled controls of the form in tree order
        let Some(f) = node_of(&arg(&a, 0)) else { return Ok(Val::array(vec![])) };
        let l = live(it); let b = l.borrow();
        let mut out = vec![];
        for c in b.select(f, "input,textarea,select", false).unwrap_or_default() {
            let Some(name) = b.attr(c, "name").filter(|n| !n.is_empty()) else { continue };
            if b.attr(c, "disabled").is_some() { continue; }
            let ty = b.attr(c, "type").unwrap_or("text").to_ascii_lowercase();
            let tag = b.n[c as usize].tag.as_str();
            let v = match (tag, ty.as_str()) {
                ("input", "submit" | "button" | "image" | "reset" | "file") => continue,
                ("input", "checkbox" | "radio") => { if b.attr(c, "checked").is_none() { continue; } b.attr(c, "value").unwrap_or("on").to_string() }
                ("textarea", _) => b.text_content(c),
                ("select", _) => { let opts = b.select(c, "option", false).unwrap_or_default(); match opts.iter().copied().find(|&o| b.attr(o, "selected").is_some()).or(opts.first().copied()) { Some(o) => b.attr(o, "value").map(|s| s.to_string()).unwrap_or_else(|| b.text_content(o)), None => continue } }
                _ => b.attr(c, "value").unwrap_or("").to_string(),
            };
            out.push(Val::array(vec![st(name), st(&v)]));
        }
        Ok(Val::array(out))
    }));
    match crate::jsparse::parse(PRELUDE, false) {
        Ok(p) => if let Some(e) = it.run_program(&p) { l.borrow_mut().log.push(format!("platform prelude: {e}")); },
        Err(e) => l.borrow_mut().log.push(format!("platform prelude parse: {e}")),
    }
}

/// The platform parts written in JS on top of the natives above (XHR Standard, DOM §4.3 MutationObserver, HTML §4.13 custom
/// elements, WebIDL interface objects). ES5 only: it must run on every engine revision. Clean room, from those specs.
/// Loop variables are declared before their loops (`var i; for (i = …)`): engine revisions before the for-init hoisting
/// fix (law js.scope.var_in_for_is_function_local) shared a for-init `var` with the caller's.
const PRELUDE: &str = r#"(function (g) {
  function fireOn(x, t, ev) { if (typeof x['on' + t] === 'function') { try { x['on' + t].call(x, ev); } catch (e) {} } var a = (x._ls && x._ls[t]) ? x._ls[t].slice() : []; var i; for (i = 0; i < a.length; i++) { try { a[i].call(x, ev); } catch (e) {} } }
  function evt(t, x) { return { type: t, target: x, currentTarget: x, loaded: 0, total: 0, lengthComputable: false, preventDefault: function () {}, stopPropagation: function () {} }; }
  function listeners(x) { x._ls = {}; x.addEventListener = function (t, f) { (x._ls[t] = x._ls[t] || []).push(f); }; x.removeEventListener = function (t, f) { var a = x._ls[t] || []; var i; for (i = a.length - 1; i >= 0; i--) { if (a[i] === f) a.splice(i, 1); } }; }
  function enc(s) { return encodeURIComponent(s).replace(/%20/g, '+'); }

  function FormData(form) {
    var p = form ? __omr_formdata(form) : []; var self = this;
    this.__omrFormData = true;
    this.append = function (k, v) { p.push([String(k), String(v)]); };
    this.set = function (k, v) { k = String(k); var q = []; var put = false; var i; for (i = 0; i < p.length; i++) { if (p[i][0] !== k) q.push(p[i]); else if (!put) { q.push([k, String(v)]); put = true; } } if (!put) q.push([k, String(v)]); p = q; };
    this.get = function (k) { var i; for (i = 0; i < p.length; i++) { if (p[i][0] === String(k)) return p[i][1]; } return null; };
    this.getAll = function (k) { var o = []; var i; for (i = 0; i < p.length; i++) { if (p[i][0] === String(k)) o.push(p[i][1]); } return o; };
    this.has = function (k) { return this.get(k) !== null; };
    this['delete'] = function (k) { var q = []; var i; for (i = 0; i < p.length; i++) { if (p[i][0] !== String(k)) q.push(p[i]); } p = q; };
    this.entries = function () { return p.map(function (e) { return [e[0], e[1]]; }); };
    this.keys = function () { return p.map(function (e) { return e[0]; }); };
    this.values = function () { return p.map(function (e) { return e[1]; }); };
    this.forEach = function (f, t) { var i; for (i = 0; i < p.length; i++) f.call(t, p[i][1], p[i][0], self); };
    this._enc = function () { return p.map(function (e) { return enc(e[0]) + '=' + enc(e[1]); }).join('&'); };
    try { this[Symbol.iterator] = function () { return self.entries()[Symbol.iterator](); }; } catch (e) {}
  }
  g.FormData = FormData;

  // XMLHttpRequest Standard: async requests answer in a later task (setTimeout 0), sync ones before send() returns
  function XMLHttpRequest() {
    var x = this; listeners(x);
    x.readyState = 0; x.status = 0; x.statusText = ''; x.responseText = ''; x.response = ''; x.responseType = ''; x.responseURL = ''; x.responseXML = null;
    x.withCredentials = false; x.timeout = 0; x.upload = { addEventListener: function () {}, removeEventListener: function () {} };
    var h = {}; var rh = []; var m = 'GET'; var u = ''; var async = true; var aborted = false;
    function state(n) { x.readyState = n; fireOn(x, 'readystatechange', evt('readystatechange', x)); }
    x.open = function (method, url, a) { m = String(method).toUpperCase(); u = String(url); async = a !== false; aborted = false; h = {}; rh = []; x.status = 0; x.responseText = ''; x.response = ''; state(1); };
    x.setRequestHeader = function (k, v) { h[String(k).toLowerCase()] = String(v); };
    x.getResponseHeader = function (k) { k = String(k).toLowerCase(); var o = []; var i; for (i = 0; i < rh.length; i++) { if (rh[i][0] === k) o.push(rh[i][1]); } return o.length ? o.join(', ') : null; };
    x.getAllResponseHeaders = function () { var s = ''; var i; for (i = 0; i < rh.length; i++) { if (rh[i][0] !== 'set-cookie') s += rh[i][0] + ': ' + rh[i][1] + '\r\n'; } return s; };
    x.overrideMimeType = function () {};
    x.abort = function () { aborted = true; if (x.readyState > 0 && x.readyState < 4) { state(4); fireOn(x, 'abort', evt('abort', x)); fireOn(x, 'loadend', evt('loadend', x)); } x.readyState = 0; };
    x.send = function (body) {
      var ct = h['content-type'] || ''; var b = null;
      if (body !== undefined && body !== null && m !== 'GET' && m !== 'HEAD') {
        if (body.__omrFormData) { b = body._enc(); if (!ct) ct = 'application/x-www-form-urlencoded;charset=UTF-8'; }
        else { b = String(body); if (!ct) ct = (typeof body === 'object' && body.__omrParams) ? 'application/x-www-form-urlencoded;charset=UTF-8' : 'text/plain;charset=UTF-8'; }
      }
      fireOn(x, 'loadstart', evt('loadstart', x));
      function done() {
        if (aborted) return;
        var r = __omr_req(m, __omr_resolve(u), ct, b);
        if (!r) { state(4); fireOn(x, 'error', evt('error', x)); fireOn(x, 'loadend', evt('loadend', x)); return; }
        x.status = r[0]; x.responseURL = r[1]; rh = r[2]; x.responseText = r[3]; x.statusText = r[0] === 200 ? 'OK' : String(r[0]);
        if (x.responseType === 'json') { try { x.response = JSON.parse(r[3]); } catch (e) { x.response = null; } } else { x.response = r[3]; }
        state(2); state(3); state(4);
        fireOn(x, 'load', evt('load', x)); fireOn(x, 'loadend', evt('loadend', x));
      }
      if (async) setTimeout(done, 0); else done();
    };
  }
  XMLHttpRequest.UNSENT = 0; XMLHttpRequest.OPENED = 1; XMLHttpRequest.HEADERS_RECEIVED = 2; XMLHttpRequest.LOADING = 3; XMLHttpRequest.DONE = 4;
  g.XMLHttpRequest = XMLHttpRequest;

  // DOM §4.3 MutationObserver: records collected by the engine, delivered after each script and on a 16 ms tick
  var observers = []; var polling = false;
  var TYPES = ['', 'childList', 'attributes', 'characterData'];
  function deliver() {
    if (!observers.length) return;
    var ms = __omr_mutations(); if (!ms.length) return;
    var o; for (o = 0; o < observers.length; o++) {
      var ob = observers[o]; var recs = [];
      var i; for (i = 0; i < ms.length; i++) {
        var r = ms[i]; var t = TYPES[r[0]];
        var j; for (j = 0; j < ob._t.length; j++) {
          var s = ob._t[j];
          var hit = r[1] === s.node || (s.subtree && s.node && s.node.contains && s.node.contains(r[1]));
          if (!hit) continue;
          if ((t === 'childList' && !s.childList) || (t === 'attributes' && !s.attributes) || (t === 'characterData' && !s.characterData)) continue;
          if (t === 'attributes' && s.filter && s.filter.indexOf(r[2]) < 0) continue;
          recs.push({ type: t, target: r[1], attributeName: r[2], attributeNamespace: null, oldValue: null, addedNodes: r[3] ? [r[3]] : [], removedNodes: r[4] ? [r[4]] : [], previousSibling: null, nextSibling: null });
          break;
        }
      }
      if (recs.length) { try { ob._cb.call(ob, recs, ob); } catch (e) {} }
    }
  }
  g.__omr_deliver = deliver;
  function MutationObserver(cb) {
    var self = this; this._cb = cb; this._t = [];
    this.observe = function (node, o) {
      o = o || {};
      self._t.push({ node: node, subtree: !!o.subtree, childList: !!o.childList, attributes: !!(o.attributes || o.attributeFilter || o.attributeOldValue), characterData: !!(o.characterData || o.characterDataOldValue), filter: o.attributeFilter || null });
      if (observers.indexOf(self) < 0) observers.push(self);
      __omr_observe(true);
      if (!polling) { polling = true; setInterval(deliver, 16); }
    };
    this.disconnect = function () { self._t = []; var i = observers.indexOf(self); if (i >= 0) observers.splice(i, 1); if (!observers.length) __omr_observe(false); };
    this.takeRecords = function () { return []; };
  }
  g.MutationObserver = MutationObserver; g.WebKitMutationObserver = MutationObserver;

  // ResizeObserver / IntersectionObserver: layout is not live in v1 — observe() reports one entry with the current box
  function boxObserver(kind) {
    return function (cb) {
      var self = this;
      this.observe = function (el) { setTimeout(function () { var r = el.getBoundingClientRect(); var e = { target: el, contentRect: r, boundingClientRect: r, intersectionRect: r, isIntersecting: true, intersectionRatio: 1, rootBounds: null, time: 0, borderBoxSize: [{ inlineSize: r.width, blockSize: r.height }], contentBoxSize: [{ inlineSize: r.width, blockSize: r.height }] }; try { cb.call(self, [e], self); } catch (x) {} }, 0); };
      this.unobserve = function () {}; this.disconnect = function () {}; this.takeRecords = function () { return []; };
    };
  }
  g.ResizeObserver = boxObserver('resize'); g.IntersectionObserver = boxObserver('intersection');

  // WebIDL interface objects: not constructible, a prototype object scripts may patch, instanceof through Symbol.hasInstance
  var ifaces = [['EventTarget', 0, ''], ['Node', 0, ''], ['CharacterData', 3, ''], ['Text', 3, ''], ['Comment', 8, ''], ['CDATASection', 4, ''], ['ProcessingInstruction', 7, ''], ['Element', 1, ''], ['HTMLElement', 1, ''],
    ['Document', 9, ''], ['HTMLDocument', 9, ''], ['DocumentFragment', 11, ''], ['ShadowRoot', 11, ''], ['SVGElement', 1, 'svg,path,g,circle,rect,line,polyline,polygon,use,symbol,defs,text,tspan'],
    ['HTMLAnchorElement', 1, 'a'], ['HTMLLinkElement', 1, 'link'], ['HTMLScriptElement', 1, 'script'], ['HTMLInputElement', 1, 'input'], ['HTMLFormElement', 1, 'form'],
    ['HTMLImageElement', 1, 'img'], ['HTMLIFrameElement', 1, 'iframe'], ['HTMLDivElement', 1, 'div'], ['HTMLSpanElement', 1, 'span'], ['HTMLButtonElement', 1, 'button'],
    ['HTMLSelectElement', 1, 'select'], ['HTMLTextAreaElement', 1, 'textarea'], ['HTMLTemplateElement', 1, 'template'], ['HTMLStyleElement', 1, 'style'],
    ['HTMLMetaElement', 1, 'meta'], ['HTMLBodyElement', 1, 'body'], ['HTMLHeadElement', 1, 'head'], ['HTMLHtmlElement', 1, 'html'], ['HTMLVideoElement', 1, 'video'],
    ['HTMLAudioElement', 1, 'audio'], ['HTMLMediaElement', 1, 'video,audio'], ['HTMLCanvasElement', 1, 'canvas'], ['HTMLUListElement', 1, 'ul'], ['HTMLOListElement', 1, 'ol'],
    ['HTMLLIElement', 1, 'li'], ['HTMLParagraphElement', 1, 'p'], ['HTMLTableElement', 1, 'table'], ['HTMLLabelElement', 1, 'label'], ['HTMLOptionElement', 1, 'option'],
    ['HTMLHeadingElement', 1, 'h1,h2,h3,h4,h5,h6'], ['HTMLPreElement', 1, 'pre'], ['HTMLSourceElement', 1, 'source'], ['HTMLDialogElement', 1, 'dialog'],
    ['HTMLDetailsElement', 1, 'details'], ['HTMLSlotElement', 1, 'slot'], ['HTMLPictureElement', 1, 'picture'], ['SVGSVGElement', 1, 'svg']];
  var i; for (i = 0; i < ifaces.length; i++) {
    (function (name, kind, tags) {
      // HTML §4.13.5 "upgrade an element": while upgrade() runs a custom class's constructor, the platform base constructor
      // hands back the element being upgraded (the construction stack's top) instead of throwing
      var C = function () { var pe = g.__omr_pending; if (pe) return pe; throw new TypeError('Illegal constructor'); };
      C.prototype = {};
      var test = function (x) { return __omr_isnode(x, kind, tags); };
      try { Object.defineProperty(C, Symbol.hasInstance, { value: test }); } catch (e) {}
      C.ELEMENT_NODE = 1; C.TEXT_NODE = 3; C.COMMENT_NODE = 8; C.DOCUMENT_NODE = 9; C.DOCUMENT_FRAGMENT_NODE = 11; C.DOCUMENT_POSITION_CONTAINED_BY = 16; C.DOCUMENT_POSITION_FOLLOWING = 4;
      g[name] = C;
    })(ifaces[i][0], ifaces[i][1], ifaces[i][2]);
  }

  // HTML §4.13 custom elements (v1 subset): define() upgrades existing and later-created elements by copying the class's
  // prototype methods onto them and calling connectedCallback; the class constructor itself is not run on the element
  var registry = {}; var waiting = {};
  function protoKeys(p) { var ks = []; try { ks = Object.getOwnPropertyNames(p); } catch (e) { var k; for (k in p) ks.push(k); } return ks; }
  function upgrade(el, C) {
    if (el.__omrCE) return; el.__omrCE = true;
    // the real thing first (§4.13.5): the element takes the class's prototype, then the class constructor runs ON it (Polymer's
    // constructor sets up its property system; attributeChangedCallback / connectedCallback follow); any failure falls back to
    // copying the prototype's methods onto the element, the older subset below
    try {
      var proto0 = Object.getPrototypeOf(el);
      Object.setPrototypeOf(el, C.prototype);
      g.__omr_pending = el;
      var ok = false;
      try { var r = Reflect.construct(C, [], C); ok = (r === el); } finally { g.__omr_pending = null; }
      if (ok) {
        var oa = C.observedAttributes; var ai;
        if (typeof el.attributeChangedCallback === 'function' && oa && oa.length) { for (ai = 0; ai < oa.length; ai++) { var av = el.getAttribute(oa[ai]); if (av !== null) { try { el.attributeChangedCallback(oa[ai], null, av); } catch (e) {} } } }
        if (typeof el.connectedCallback === 'function' && el.isConnected !== false) { try { el.connectedCallback(); } catch (e) { g.__omr_ce_error = String(e && e.message || e); (g.__omr_ce_errors = g.__omr_ce_errors || []).push((el.localName || el.tagName) + ': ' + String(e && e.message || e)); } }
        return;
      }
      Object.setPrototypeOf(el, proto0);
    } catch (e) { g.__omr_ce_error = String(e && e.message || e); g.__omr_pending = null; }
    var chain = []; var p = C.prototype; var n = 0;
    while (p && p !== Object.prototype && n < 8) { chain.unshift(p); try { p = Object.getPrototypeOf(p); } catch (e) { p = null; } n++; }
    var c; for (c = 0; c < chain.length; c++) { var ks = protoKeys(chain[c]); var j; for (j = 0; j < ks.length; j++) { if (ks[j] !== 'constructor') { try { el[ks[j]] = chain[c][ks[j]]; } catch (e) {} } } }
    if (typeof el.connectedCallback === 'function') { try { el.connectedCallback(); } catch (e) {} }
  }
  g.customElements = {
    define: function (name, C) { name = String(name).toLowerCase(); if (registry[name]) throw new Error('NotSupportedError: ' + name + ' already defined'); registry[name] = C;
      // a streaming parser has not created the elements AFTER the running script yet: only those before it exist at define()
      // time (HTML §13.2.6.1 — a parser-created element of a defined name is constructed, so its constructor runs with the page's
      // later scripts already loaded). The rest are upgraded by the end-of-parse sweep (__omr_upgrade_all).
      var els = document.getElementsByTagName(name); var i; var cs = document.currentScript;
      for (i = 0; i < els.length; i++) {
        if (cs && els[i].compareDocumentPosition && (els[i].compareDocumentPosition(cs) & 4) && !els[i].contains(cs)) continue; // element follows the script
        upgrade(els[i], C);
      }
      if (waiting[name]) { var w; for (w = 0; w < waiting[name].length; w++) waiting[name][w](C); delete waiting[name]; } },
    get: function (name) { return registry[String(name).toLowerCase()]; },
    getName: function (C) { var k; for (k in registry) { if (registry[k] === C) return k; } return null; },
    whenDefined: function (name) { name = String(name).toLowerCase(); if (registry[name]) return Promise.resolve(registry[name]); return new Promise(function (res) { (waiting[name] = waiting[name] || []).push(res); }); },
    upgrade: function (root) { var k; for (k in registry) { var els = root.getElementsByTagName ? root.getElementsByTagName(k) : []; var i; for (i = 0; i < els.length; i++) upgrade(els[i], registry[k]); } }
  };
  g.__omr_upgrade_all = function () { g.customElements.upgrade(document); };

  // DOM §4.5 importNode / adoptNode: one document here, so import = clone (template stamping — Polymer, lit — goes through it)
  if (typeof document.importNode !== 'function') document.importNode = function (n, deep) { if (!n || typeof n.cloneNode !== 'function') throw new TypeError("Failed to execute 'importNode' on 'Document': parameter 1 is not of type 'Node'."); return n.cloneNode(!!deep); };
  if (typeof document.adoptNode !== 'function') document.adoptNode = function (n) { if (n && n.parentNode) n.parentNode.removeChild(n); return n; };

  // classes extend the platform's event constructors (class MyEvent extends Event): they need prototype objects (§15.7.14)
  var ev = ['Event', 'CustomEvent']; var e0; for (e0 = 0; e0 < ev.length; e0++) { try { if (g[ev[e0]] && typeof g[ev[e0]].prototype !== 'object') g[ev[e0]].prototype = {}; } catch (x) {} }
  // DOM §4.5.1 createHTMLDocument: a detached html / head / body tree sharing this document's node factory (jQuery.parseHTML)
  var impl = { hasFeature: function () { return true; }, createHTMLDocument: function (title) {
    var h = document.createElement('html'), hd = document.createElement('head'), b = document.createElement('body');
    h.appendChild(hd); h.appendChild(b);
    if (title !== undefined) { var t = document.createElement('title'); t.textContent = String(title); hd.appendChild(t); }
    return { nodeType: 9, documentElement: h, head: hd, body: b, title: title || '', implementation: impl, location: null,
      createElement: function (n) { return document.createElement(n); }, createElementNS: function (ns, n) { return document.createElement(n); },
      createTextNode: function (x) { return document.createTextNode(x); }, createComment: function (x) { return document.createComment(x); },
      createDocumentFragment: function () { return document.createDocumentFragment(); },
      querySelector: function (s) { return h.querySelector(s); }, querySelectorAll: function (s) { return h.querySelectorAll(s); },
      getElementById: function (id) { return h.querySelector('#' + id); }, getElementsByTagName: function (n) { return h.getElementsByTagName(n); } };
  } };
  try { document.implementation = impl; } catch (e) {}
  // node lists are arrays here: NodeList / HTMLCollection answer instanceof for them
  var listTest = function (x) { return Array.isArray(x); };
  var names = ['NodeList', 'HTMLCollection'];
  var n0; for (n0 = 0; n0 < names.length; n0++) { var L = function () { throw new TypeError('Illegal constructor'); }; L.prototype = {}; try { Object.defineProperty(L, Symbol.hasInstance, { value: listTest }); } catch (e) {} g[names[n0]] = L; }
  // new Image(w, h) = an <img> (HTML §4.8.4.1 legacy factory)
  g.Image = function (w, h) { var i = document.createElement('img'); if (w !== undefined) i.setAttribute('width', String(w)); if (h !== undefined) i.setAttribute('height', String(h)); return i; };
  // HTML §8.1.8.3: core-js tests for PromiseRejectionEvent before trusting the native Promise
  g.PromiseRejectionEvent = function (type, init) { var e = new Event(type, init); e.promise = init && init.promise; e.reason = init && init.reason; return e; };

  // DOM §6 Traversal: NodeFilter, TreeWalker (§6.2 algorithms), NodeIterator (document order; no live-removal adjustment)
  var NF = g.NodeFilter = { FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 4294967295, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2, SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256, SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024 };
  function tflt(w, node) {
    var t = node.nodeType; var sh = w.whatToShow >>> 0;
    if (!(sh & (1 << (t - 1)))) return 3;
    var f = w.filter; if (!f) return 1;
    var r = typeof f === 'function' ? f.call(undefined, node) : f.acceptNode(node);
    return r === true ? 1 : (r | 0);
  }
  function tkids(w, first) {
    var node = first ? w.currentNode.firstChild : w.currentNode.lastChild;
    while (node) {
      var r = tflt(w, node);
      if (r === 1) { w.currentNode = node; return node; }
      if (r === 3) { var c = first ? node.firstChild : node.lastChild; if (c) { node = c; continue; } }
      while (node) {
        var sib = first ? node.nextSibling : node.previousSibling;
        if (sib) { node = sib; break; }
        var p = node.parentNode;
        if (!p || p === w.root || p === w.currentNode) return null;
        node = p;
      }
    }
    return null;
  }
  function tsib(w, next) {
    var node = w.currentNode; if (node === w.root) return null;
    for (;;) {
      var sib = next ? node.nextSibling : node.previousSibling;
      while (sib) {
        node = sib; var r = tflt(w, node);
        if (r === 1) { w.currentNode = node; return node; }
        sib = next ? node.firstChild : node.lastChild;
        if (r === 2 || !sib) sib = next ? node.nextSibling : node.previousSibling;
      }
      node = node.parentNode;
      if (!node || node === w.root) return null;
      if (tflt(w, node) === 1) return null;
    }
  }
  function TreeWalker(root, what, filter) { this.root = root; this.whatToShow = what === undefined ? 4294967295 : what >>> 0; this.filter = filter || null; this.currentNode = root; }
  TreeWalker.prototype.parentNode = function () {
    var node = this.currentNode;
    while (node && node !== this.root) { node = node.parentNode; if (node && tflt(this, node) === 1) { this.currentNode = node; return node; } }
    return null;
  };
  TreeWalker.prototype.firstChild = function () { return tkids(this, true); };
  TreeWalker.prototype.lastChild = function () { return tkids(this, false); };
  TreeWalker.prototype.nextSibling = function () { return tsib(this, true); };
  TreeWalker.prototype.previousSibling = function () { return tsib(this, false); };
  TreeWalker.prototype.nextNode = function () {
    var node = this.currentNode; var r = 1;
    for (;;) {
      while (r !== 2 && node.firstChild) { node = node.firstChild; r = tflt(this, node); if (r === 1) { this.currentNode = node; return node; } }
      var t = node; var sib = null;
      while (t) { if (t === this.root) return null; sib = t.nextSibling; if (sib) break; t = t.parentNode; }
      if (!sib) return null;
      node = sib; r = tflt(this, node);
      if (r === 1) { this.currentNode = node; return node; }
    }
  };
  TreeWalker.prototype.previousNode = function () {
    var node = this.currentNode;
    while (node !== this.root) {
      var sib = node.previousSibling;
      while (sib) {
        node = sib; var r = tflt(this, node);
        while (r !== 2 && node.lastChild) { node = node.lastChild; r = tflt(this, node); }
        if (r === 1) { this.currentNode = node; return node; }
        sib = node.previousSibling;
      }
      if (node === this.root || !node.parentNode) return null;
      node = node.parentNode;
      if (tflt(this, node) === 1) { this.currentNode = node; return node; }
    }
    return null;
  };
  function tfollow(n, root) { if (n.firstChild) return n.firstChild; while (n) { if (n === root) return null; if (n.nextSibling) return n.nextSibling; n = n.parentNode; } return null; }
  function tprecede(n, root) { if (n === root) return null; var s = n.previousSibling; if (!s) return n.parentNode; while (s.lastChild) s = s.lastChild; return s; }
  function NodeIterator(root, what, filter) { this.root = root; this.whatToShow = what === undefined ? 4294967295 : what >>> 0; this.filter = filter || null; this.referenceNode = root; this.pointerBeforeReferenceNode = true; }
  NodeIterator.prototype.nextNode = function () {
    var n = this.referenceNode; var before = this.pointerBeforeReferenceNode;
    for (;;) {
      if (!before) { n = tfollow(n, this.root); if (!n) return null; } else before = false;
      if (tflt(this, n) === 1) { this.referenceNode = n; this.pointerBeforeReferenceNode = false; return n; }
    }
  };
  NodeIterator.prototype.previousNode = function () {
    var n = this.referenceNode; var before = this.pointerBeforeReferenceNode;
    for (;;) {
      if (before) { n = tprecede(n, this.root); if (!n) return null; } else before = true;
      if (tflt(this, n) === 1) { this.referenceNode = n; this.pointerBeforeReferenceNode = true; return n; }
    }
  };
  NodeIterator.prototype.detach = function () {};
  g.TreeWalker = TreeWalker; g.NodeIterator = NodeIterator;
  document.createTreeWalker = function (root, what, filter) { return new TreeWalker(root, what, filter); };
  document.createNodeIterator = function (root, what, filter) { return new NodeIterator(root, what, filter); };

  // HTML §9.4 channel messaging: two entangled ports, a message is delivered to the other port on a later task
  function MessagePort() { this.onmessage = null; this._l = []; this._o = null; }
  MessagePort.prototype.postMessage = function (data) {
    var o = this._o; if (!o) return;
    setTimeout(function () {
      var e = new Event('message'); e.data = data; e.origin = ''; e.ports = []; e.source = null;
      if (typeof o.onmessage === 'function') { try { o.onmessage(e); } catch (x) {} }
      var i; var ls = o._l.slice(); for (i = 0; i < ls.length; i++) { try { ls[i].call(o, e); } catch (x) {} }
    }, 0);
  };
  MessagePort.prototype.addEventListener = function (t, f) { if (t === 'message' && this._l.indexOf(f) < 0) this._l.push(f); };
  MessagePort.prototype.removeEventListener = function (t, f) { var i = this._l.indexOf(f); if (t === 'message' && i >= 0) this._l.splice(i, 1); };
  MessagePort.prototype.start = function () {}; MessagePort.prototype.close = function () { this._o = null; };
  g.MessagePort = MessagePort;
  g.MessageChannel = function () { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._o = this.port2; this.port2._o = this.port1; };
  // HTML §8.1.8.2 ErrorEvent; §3.2.2 HTMLUnknownElement; §4.13.4 CustomElementRegistry (the object `customElements` is one)
  g.ErrorEvent = function (type, init) { var e = new Event(type, init); init = init || {}; e.message = init.message || ''; e.filename = init.filename || ''; e.lineno = init.lineno | 0; e.colno = init.colno | 0; e.error = init.error; return e; };
  g.HTMLUnknownElement = function () { throw new TypeError('Illegal constructor'); };
  g.HTMLUnknownElement.prototype = Object.create(g.HTMLElement.prototype, { constructor: { value: g.HTMLUnknownElement, writable: true, configurable: true } });
  g.CustomElementRegistry = function () { throw new TypeError('Illegal constructor'); };
  g.CustomElementRegistry.prototype = g.customElements;
  // UI event constructors are native functions without a prototype object: give them one (polyfills extend MouseEvent.prototype)
  var uiev = ['KeyboardEvent', 'MouseEvent', 'FocusEvent', 'InputEvent', 'PointerEvent', 'UIEvent']; var ue;
  for (ue = 0; ue < uiev.length; ue++) { var UC = g[uiev[ue]]; if (UC && !UC.prototype) { try { UC.prototype = Object.create(Event.prototype, { constructor: { value: UC, writable: true, configurable: true } }); } catch (e) {} } }
  // UI Events §5 WheelEvent / DragEvent / TouchEvent / CompositionEvent: the interfaces exist and carry their init fields
  // (music.youtube.com's polymer bundle references WheelEvent at load — YTMUSIC-0 Y3)
  function mkev(name, base, fields) {
    var B = g[base] || Event;
    var C = function (type, init) {
      var e = new B(type, init); init = init || {}; var k;
      for (k in fields) e[k] = init[k] !== undefined ? init[k] : fields[k];
      try { Object.setPrototypeOf(e, C.prototype); } catch (x) {}
      return e;
    };
    C.prototype = Object.create(B.prototype, { constructor: { value: C, writable: true, configurable: true } });
    g[name] = C;
  }
  if (!g.WheelEvent) mkev('WheelEvent', 'MouseEvent', { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 });
  if (!g.DragEvent) mkev('DragEvent', 'MouseEvent', { dataTransfer: null });
  if (!g.TouchEvent) mkev('TouchEvent', 'UIEvent', { touches: [], targetTouches: [], changedTouches: [], altKey: false, metaKey: false, ctrlKey: false, shiftKey: false });
  if (!g.CompositionEvent) mkev('CompositionEvent', 'UIEvent', { data: '' });
  // ECMA-402 §9.2.9 supportedLocalesOf on every service constructor (en-US only here): the requested tags we can honour
  if (typeof Intl !== 'undefined') {
    var isv = ['NumberFormat', 'DateTimeFormat', 'Collator', 'PluralRules', 'RelativeTimeFormat', 'ListFormat', 'Segmenter', 'DisplayNames'], iv;
    for (iv = 0; iv < isv.length; iv++) {
      var IC = Intl[isv[iv]];
      if (IC && typeof IC.supportedLocalesOf !== 'function') {
        try { IC.supportedLocalesOf = function (l) { var a = l === undefined ? [] : (typeof l === 'string' ? [l] : Array.prototype.slice.call(l)); return a.filter(function (t) { return /^en(-|$)/i.test(String(t)); }); }; } catch (e) {}
      }
    }
  }
  g.WheelEvent.DOM_DELTA_PIXEL = 0; g.WheelEvent.DOM_DELTA_LINE = 1; g.WheelEvent.DOM_DELTA_PAGE = 2;
  g.Window = function () { throw new TypeError('Illegal constructor'); };
  g.Window.prototype = Object.create(g.EventTarget.prototype, { constructor: { value: g.Window, writable: true, configurable: true } });
  // the window's own event methods move onto Window.prototype (the shape polyfills such as webcomponents-sd patch), and the
  // window inherits from it; own properties still win, so nothing the page already reads changes
  ['addEventListener', 'removeEventListener', 'dispatchEvent'].forEach(function (n) { if (typeof g[n] === 'function') { try { Object.defineProperty(g.Window.prototype, n, { value: g[n], writable: true, configurable: true }); } catch (e) {} } });
  try { Object.setPrototypeOf(g, g.Window.prototype); } catch (e) {}
  // DOM §5 Range and the Selection API: boundary points are kept, there is no text layout behind them (toString is empty,
  // rects are zero) — frameworks only need the interfaces to exist and to take the calls
  function Range() { this.startContainer = document; this.startOffset = 0; this.endContainer = document; this.endOffset = 0; }
  Range.prototype = { constructor: Range,
    get collapsed() { return this.startContainer === this.endContainer && this.startOffset === this.endOffset; },
    get commonAncestorContainer() { var a = this.startContainer; while (a) { if (a === this.endContainer || (a.contains && a.contains(this.endContainer))) return a; a = a.parentNode; } return document; },
    setStart: function (n, o) { this.startContainer = n; this.startOffset = o | 0; if (this.endContainer === document && this.endOffset === 0) { this.endContainer = n; this.endOffset = o | 0; } },
    setEnd: function (n, o) { this.endContainer = n; this.endOffset = o | 0; },
    setStartBefore: function (n) { this.setStart(n.parentNode, Array.prototype.indexOf.call(n.parentNode.childNodes, n)); },
    setStartAfter: function (n) { this.setStart(n.parentNode, Array.prototype.indexOf.call(n.parentNode.childNodes, n) + 1); },
    setEndBefore: function (n) { this.setEnd(n.parentNode, Array.prototype.indexOf.call(n.parentNode.childNodes, n)); },
    setEndAfter: function (n) { this.setEnd(n.parentNode, Array.prototype.indexOf.call(n.parentNode.childNodes, n) + 1); },
    selectNode: function (n) { this.setStartBefore(n); this.setEndAfter(n); },
    selectNodeContents: function (n) { this.startContainer = n; this.startOffset = 0; this.endContainer = n; this.endOffset = n.nodeType === 1 || n.nodeType === 11 || n.nodeType === 9 ? n.childNodes.length : (n.data || '').length; },
    collapse: function (toStart) { if (toStart) { this.endContainer = this.startContainer; this.endOffset = this.startOffset; } else { this.startContainer = this.endContainer; this.startOffset = this.endOffset; } },
    cloneRange: function () { var r = new Range(); r.startContainer = this.startContainer; r.startOffset = this.startOffset; r.endContainer = this.endContainer; r.endOffset = this.endOffset; return r; },
    detach: function () {}, toString: function () { return ''; }, deleteContents: function () {}, compareBoundaryPoints: function () { return 0; },
    comparePoint: function () { return 0; }, isPointInRange: function () { return false; }, intersectsNode: function () { return false; },
    insertNode: function (n) { var c = this.startContainer; if (c.nodeType === 1 || c.nodeType === 11) { c.insertBefore(n, c.childNodes[this.startOffset] || null); } },
    getBoundingClientRect: function () { return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 }; }, getClientRects: function () { return []; },
    createContextualFragment: function (html) { var d = document.createElement('div'); d.innerHTML = html; var f = document.createDocumentFragment(); while (d.firstChild) f.appendChild(d.firstChild); return f; } };
  g.Range = Range; g.StaticRange = Range; g.AbstractRange = Range;
  document.createRange = function () { return new Range(); };
  function Selection() { this.ranges = []; }
  Selection.prototype = { constructor: Selection, get rangeCount() { return this.ranges.length; }, get anchorNode() { return this.ranges.length ? this.ranges[0].startContainer : null; },
    get focusNode() { return this.ranges.length ? this.ranges[0].endContainer : null; }, get anchorOffset() { return this.ranges.length ? this.ranges[0].startOffset : 0; },
    get focusOffset() { return this.ranges.length ? this.ranges[0].endOffset : 0; }, get isCollapsed() { return !this.ranges.length || this.ranges[0].collapsed; }, type: 'None',
    getRangeAt: function (i) { if (!this.ranges[i]) throw new Error('IndexSizeError'); return this.ranges[i]; }, addRange: function (r) { this.ranges = [r]; }, removeAllRanges: function () { this.ranges = []; },
    removeRange: function () { this.ranges = []; }, empty: function () { this.ranges = []; }, collapse: function () {}, collapseToStart: function () {}, collapseToEnd: function () {}, extend: function () {},
    selectAllChildren: function (n) { var r = new Range(); r.selectNodeContents(n); this.ranges = [r]; }, containsNode: function () { return false; }, toString: function () { return ''; } };
  g.Selection = Selection; var theSelection = new Selection();
  g.getSelection = function () { return theSelection; }; document.getSelection = g.getSelection;
  // Interfaces that code names (instanceof, extends, decorator metadata, feature tests) though nothing here draws on them.
  // Event kinds: constructed through Event, init fields copied on (UI Events §3.4 wheel/drag; Touch Events)
  function evKind(name, fields) {
    var C = function (type, init) { var e = new Event(type, init); var i; for (i = 0; i < fields.length; i++) { var f = fields[i]; e[f[0]] = init && init[f[0]] !== undefined ? init[f[0]] : f[1]; } return e; };
    C.prototype = Object.create(Event.prototype, { constructor: { value: C, writable: true, configurable: true } }); g[name] = C; return C;
  }
  evKind('WheelEvent', [['deltaX', 0], ['deltaY', 0], ['deltaZ', 0], ['deltaMode', 0], ['clientX', 0], ['clientY', 0], ['ctrlKey', false], ['shiftKey', false], ['altKey', false], ['metaKey', false]]).DOM_DELTA_PIXEL = 0;
  evKind('DragEvent', [['dataTransfer', null], ['clientX', 0], ['clientY', 0]]);
  evKind('TouchEvent', [['touches', []], ['targetTouches', []], ['changedTouches', []]]);
  evKind('MessageEvent', [['data', null], ['origin', ''], ['lastEventId', ''], ['source', null], ['ports', []]]);
  evKind('ProgressEvent', [['lengthComputable', false], ['loaded', 0], ['total', 0]]);
  g.Touch = function (init) { init = init || {}; this.identifier = init.identifier | 0; this.target = init.target || null; this.clientX = init.clientX || 0; this.clientY = init.clientY || 0; this.pageX = init.pageX || 0; this.pageY = init.pageY || 0; };
  // WebIDL §3.14.1 DOMException with the legacy error codes
  var DE_CODES = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21, QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 };
  if (typeof g.DOMException === 'undefined') {
    var DE = function (message, name) { var e = new Error(message === undefined ? '' : String(message)); e.name = name === undefined ? 'Error' : String(name); e.code = DE_CODES[e.name] || 0; try { Object.setPrototypeOf(e, DE.prototype); } catch (x) {} return e; };
    DE.prototype = Object.create(Error.prototype, { constructor: { value: DE, writable: true, configurable: true } });
    var dn; for (dn in DE_CODES) { DE[dn.replace(/([a-z])([A-Z])/g, '$1_$2').toUpperCase().replace(/_ERROR$/, '_ERR')] = DE_CODES[dn]; }
    g.DOMException = DE; g.DOMError = DE;
  }
  // Geometry Interfaces §3: DOMRectReadOnly / DOMRect
  function DOMRectReadOnly(x, y, w, h) { this.x = +x || 0; this.y = +y || 0; this.width = +w || 0; this.height = +h || 0; }
  ['top', 'left', 'right', 'bottom'].forEach(function (n) { Object.defineProperty(DOMRectReadOnly.prototype, n, { get: function () { var x = this.x, y = this.y, w = this.width, h = this.height; return n === 'top' ? Math.min(y, y + h) : n === 'left' ? Math.min(x, x + w) : n === 'right' ? Math.max(x, x + w) : Math.max(y, y + h); }, configurable: true }); });
  DOMRectReadOnly.prototype.toJSON = function () { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, right: this.right, bottom: this.bottom, left: this.left }; };
  DOMRectReadOnly.fromRect = function (r) { r = r || {}; return new DOMRectReadOnly(r.x, r.y, r.width, r.height); };
  function DOMRect(x, y, w, h) { DOMRectReadOnly.call(this, x, y, w, h); }
  DOMRect.prototype = Object.create(DOMRectReadOnly.prototype, { constructor: { value: DOMRect, writable: true, configurable: true } });
  DOMRect.fromRect = function (r) { r = r || {}; return new DOMRect(r.x, r.y, r.width, r.height); };
  g.DOMRectReadOnly = DOMRectReadOnly; g.DOMRect = DOMRect;
  // CSSOM §2.1.2 CSS.escape; CSS.supports answers from the engine's property list when it has one, else false (honest)
  g.CSS = { escape: function (v) {
      var s = String(v), out = '', i, c;
      for (i = 0; i < s.length; i++) { c = s.charCodeAt(i);
        if (c === 0) out += '�';
        else if ((c >= 1 && c <= 31) || c === 127 || (i === 0 && c >= 48 && c <= 57) || (i === 1 && c >= 48 && c <= 57 && s.charCodeAt(0) === 45)) out += '\\' + c.toString(16) + ' ';
        else if (i === 0 && c === 45 && s.length === 1) out += '\\' + s.charAt(i);
        else if (c >= 128 || c === 45 || c === 95 || (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122)) out += s.charAt(i);
        else out += '\\' + s.charAt(i); }
      return out; },
    supports: function () { return false; } };
  // Interfaces with no behaviour here: they exist so `instanceof`, `extends` and type metadata resolve
  ['CSSRule', 'CSSStyleRule', 'CSSGroupingRule', 'CSSConditionRule', 'CSSMediaRule', 'CSSSupportsRule', 'CSSStyleDeclaration', 'CSSStyleSheet', 'StyleSheet', 'DOMTokenList', 'Animation',
   'SVGGraphicsElement', 'PerformanceNavigationTiming', 'PerformanceEntry', 'Storage', 'BroadcastChannel'].forEach(function (n) {
    if (typeof g[n] === 'undefined') { var C = function () { throw new TypeError('Illegal constructor'); }; C.prototype = {}; Object.defineProperty(C.prototype, 'constructor', { value: C, writable: true, configurable: true }); g[n] = C; } });
  g.BroadcastChannel = function (name) { this.name = String(name); this.onmessage = null; }; g.BroadcastChannel.prototype = { postMessage: function () {}, close: function () {}, addEventListener: function () {}, removeEventListener: function () {} };

  // WebSockets (WHATWG WebSockets §3): the broker holds the socket (RFC 6455); events arrive on a 50 ms poll while any is open
  var sockets = {}; var wsPoll = null;
  function wsEvents() {
    var r = __omr_ws('poll', 0); var ev = (r && r.events) || [];
    var i; for (i = 0; i < ev.length; i++) {
      var e = ev[i]; var ws = sockets[e[0]]; if (!ws) continue;
      if (e[1] === 'open') { ws.readyState = 1; fireOn(ws, 'open', evt('open', ws)); }
      else if (e[1] === 'message' || e[1] === 'binary') { var m = evt('message', ws); m.data = e[1] === 'binary' ? new Uint8Array(e[2]).buffer : e[2]; m.origin = ws.url; fireOn(ws, 'message', m); }
      else if (e[1] === 'error') { fireOn(ws, 'error', evt('error', ws)); }
      else if (e[1] === 'close') { ws.readyState = 3; var c = evt('close', ws); c.code = e[2]; c.reason = e[3] || ''; c.wasClean = e[2] !== 1006; delete sockets[e[0]]; fireOn(ws, 'close', c); }
    }
    var open = false; var k; for (k in sockets) { open = true; break; }
    if (!open && wsPoll !== null) { clearInterval(wsPoll); wsPoll = null; }
  }
  function WebSocket(url, protocols) {
    var ws = this; listeners(ws);
    ws.url = String(url); ws.readyState = 0; ws.protocol = ''; ws.extensions = ''; ws.bufferedAmount = 0; ws.binaryType = 'blob';
    var r = __omr_ws('open', 0, ws.url);
    ws.send = function (d) { if (ws.readyState !== 1) throw new Error('InvalidStateError: the socket is not open'); __omr_ws('send', ws._id, String(d)); };
    ws.close = function (code, reason) { if (ws.readyState >= 2) return; ws.readyState = 2; __omr_ws('close', ws._id, '', code || 1000, reason || ''); };
    if (!r || r.error) { setTimeout(function () { ws.readyState = 3; fireOn(ws, 'error', evt('error', ws)); var c = evt('close', ws); c.code = 1006; c.reason = ''; c.wasClean = false; fireOn(ws, 'close', c); }, 0); return; }
    ws._id = r.id; sockets[r.id] = ws;
    if (wsPoll === null) wsPoll = setInterval(wsEvents, 50);
  }
  WebSocket.CONNECTING = 0; WebSocket.OPEN = 1; WebSocket.CLOSING = 2; WebSocket.CLOSED = 3;
  g.WebSocket = WebSocket;
})(window);
"#;

/// Run the page's scripts (HTML §8.1.3 "prepare the script element" / §8.1.4.2 the end of parsing): parser-blocking classic
/// scripts in tree order, then async ones, then deferred classic + module scripts in tree order; DOMContentLoaded; custom
/// elements upgraded; load (listeners, then window.onload); timers up to `timer_ms` of virtual time. Scripts a script inserts
/// (createElement + append) run when connected and fire load / error at their element. Every script gets its own step slice
/// (`budget / 2`, the page `budget × 2` in all): one runaway script cannot stop the rest. `fetch_src(url)` brings an
/// external script's bytes through the broker. Returns a per-page report.
pub fn run_page(it: &mut Interp, l: &LiveRef, fetch_src: &dyn Fn(&str) -> Option<Vec<u8>>, timer_ms: f64, budget: u64) -> serde_json::Value {
    let t0 = std::time::Instant::now();
    let start = it.steps;
    let cap = start.saturating_add(budget.saturating_mul(2));
    let slice = (budget / 2).max(1);
    let mut rep = Report::default();
    let list = |l: &LiveRef| -> Vec<(u32, Option<String>, bool, u8)> {
        let b = l.borrow();
        (0..b.n.len() as u32).filter(|&i| b.n[i as usize].kind == 1 && b.n[i as usize].tag == "script" && !b.started.contains(&i) && b.connected(i)).filter_map(|i| {
            let ty = b.attr(i, "type").unwrap_or("").trim().to_ascii_lowercase();
            let module = ty == "module";
            let classic = ty.is_empty() || matches!(ty.as_str(), "text/javascript" | "application/javascript" | "text/ecmascript" | "application/ecmascript" | "javascript" | "application/x-javascript");
            if !(classic || module) || (classic && b.attr(i, "nomodule").is_some()) { return None; }
            let src = b.attr(i, "src").map(|s| s.to_string());
            // phase: 0 parser-blocking, 1 async, 2 deferred (classic defer with src, or module)
            let phase = if b.attr(i, "async").is_some() && (src.is_some() || module) { 1 } else if module || (b.attr(i, "defer").is_some() && src.is_some()) { 2 } else { 0 };
            Some((i, src, module, phase))
        }).collect()
    };
    let first = list(l);
    // the parser's scripts are claimed up front: only scripts inserted later count as "inserted"
    { let mut b = l.borrow_mut(); for x in &first { b.started.insert(x.0); } }
    for ph in 0..3u8 {
        for (i, src, module, phase) in first.iter().cloned() { if phase == ph { run_one(it, l, fetch_src, i, src, module, slice, cap, &mut rep); run_inserted(it, l, fetch_src, slice, cap, &mut rep); } }
    }
    l.borrow_mut().ready = "interactive";
    let doc = l.borrow().doc;
    let g = it.global.clone();
    it.limit = it.steps.saturating_add(slice).min(cap);
    let _ = dispatch(it, doc, event_obj("DOMContentLoaded", true));
    if let Some(f) = it.lookup_opt(&g, "__omr_upgrade_all") { let _ = it.call(&f, Val::Undef, vec![]); }
    l.borrow_mut().ready = "complete";
    let _ = dispatch(it, doc, event_obj("readystatechange", false));
    let ev = event_obj("load", false);
    let lw = l.borrow().listeners.get(&(NONE - 1, "load".to_string())).cloned().unwrap_or_default();
    for f in lw { let _ = it.call(&f, Val::Undef, vec![ev.clone()]); }
    // HTML §8.1.8.1: window.onload (a global function property) after the listeners
    if let Some(f) = it.lookup_opt(&g, "onload") { if f.is_callable() { let _ = it.call(&f, Val::Undef, vec![ev.clone()]); } }
    deliver(it);
    run_inserted(it, l, fetch_src, slice, cap, &mut rep);
    // timers, and the scripts they insert, in rounds of virtual time
    let mut timers = 0;
    for _ in 0..3 {
        if it.steps >= cap { break; }
        it.limit = it.steps.saturating_add(slice).min(cap);
        timers += it.run_timers(timer_ms / 3.0);
        deliver(it);
        if !run_inserted(it, l, fetch_src, slice, cap, &mut rep) && it.timers.iter().all(|x| !x.0.is_callable()) { break; }
    }
    it.fnstat_report(); crate::jsi::prof_report(); it.timers_report();
    let mut log = std::mem::take(&mut l.borrow_mut().log);
    log.extend(std::mem::take(&mut it.rejection_log));
    rep.errors.append(&mut log);
    serde_json::json!({"scripts_ran": rep.ran, "scripts_failed": rep.failed, "parse_errors": rep.parse_err, "script_kb": rep.bytes / 1024, "timers_ran": timers,
        "steps": it.steps - start, "budget_spent": it.steps >= cap, "pending": it.timers.iter().filter(|x| x.0.is_callable()).count(), "slices_spent": rep.sliced, "ms": (t0.elapsed().as_secs_f64() * 1e4).round() / 10.0,
        "errors": rep.errors.iter().take(12).collect::<Vec<_>>()})
}

#[derive(Default)]
struct Report { ran: usize, failed: usize, parse_err: usize, bytes: usize, sliced: usize, errors: Vec<String> }

/// After a task (a click, a form event): mutation records to their observers, scripts the task inserted, timers for `ms`.
pub fn settle(it: &mut Interp, l: &LiveRef, fetch_src: &dyn Fn(&str) -> Option<Vec<u8>>, ms: f64) {
    let slice = 20_000_000u64;
    let cap = it.steps.saturating_add(slice * 2);
    let mut rep = Report::default();
    deliver(it);
    run_inserted(it, l, fetch_src, slice, cap, &mut rep);
    it.limit = it.steps.saturating_add(slice).min(cap);
    it.run_timers(ms);
    deliver(it);
    run_inserted(it, l, fetch_src, slice, cap, &mut rep);
    l.borrow_mut().log.extend(rep.errors);
    l.borrow_mut().log.extend(std::mem::take(&mut it.rejection_log));
}

/// The person's values into a form's controls (HTML §4.10.18 control values): text / textarea / select by name, checkbox
/// "on" / "off", radio = the chosen value. Returns the controls that changed (they get input + change events).
pub fn fill_form(l: &mut Live, form: u32, vals: &[(String, String)]) -> Vec<u32> {
    let mut changed = vec![];
    let ctrls = l.select(form, "input,textarea,select", false).unwrap_or_default();
    for (name, v) in vals {
        let named: Vec<u32> = ctrls.iter().copied().filter(|&c| l.attr(c, "name") == Some(name.as_str())).collect();
        for c in named {
            let tag = l.n[c as usize].tag.clone();
            let ty = l.attr(c, "type").unwrap_or("text").to_ascii_lowercase();
            match (tag.as_str(), ty.as_str()) {
                ("input", "checkbox") => { let on = v == "on" || v == "true"; if on != l.attr(c, "checked").is_some() { if on { l.set_attr(c, "checked", ""); } else { l.del_attr(c, "checked"); } changed.push(c); } }
                ("input", "radio") => { let on = l.attr(c, "value").unwrap_or("on") == v; if on != l.attr(c, "checked").is_some() { if on { l.set_attr(c, "checked", ""); } else { l.del_attr(c, "checked"); } changed.push(c); } }
                ("textarea", _) => { if l.text_content(c) != *v { l.set_text(c, v); changed.push(c); } }
                ("select", _) => {
                    let opts = l.select(c, "option", false).unwrap_or_default();
                    for o in opts { let ov = l.attr(o, "value").map(|s| s.to_string()).unwrap_or_else(|| l.text_content(o)); if ov == *v { l.set_attr(o, "selected", ""); } else if l.attr(o, "selected").is_some() { l.del_attr(o, "selected"); } }
                    changed.push(c);
                }
                ("input", _) => { if l.attr(c, "value") != Some(v.as_str()) { l.set_attr(c, "value", v); changed.push(c); } }
                _ => {}
            }
        }
    }
    changed
}

/// MutationObserver records gathered so far go to their observers (the prelude's deliver).
fn deliver(it: &mut Interp) {
    let g = it.global.clone();
    if let Some(f) = it.lookup_opt(&g, "__omr_deliver") { let _ = it.call(&f, Val::Undef, vec![]); }
}

/// Scripts that scripts connected since the last look: run them in tree order. True when any ran.
fn run_inserted(it: &mut Interp, l: &LiveRef, fetch_src: &dyn Fn(&str) -> Option<Vec<u8>>, slice: u64, cap: u64, rep: &mut Report) -> bool {
    let mut any = false;
    for _ in 0..8 {
        let new: Vec<(u32, Option<String>, bool)> = {
            let b = l.borrow();
            (0..b.n.len() as u32).filter(|&i| b.n[i as usize].kind == 1 && b.n[i as usize].tag == "script" && !b.started.contains(&i) && b.connected(i)).filter_map(|i| {
                let ty = b.attr(i, "type").unwrap_or("").trim().to_ascii_lowercase();
                let module = ty == "module";
                if !(module || ty.is_empty() || ty.contains("javascript") || ty.contains("ecmascript")) { return None; }
                Some((i, b.attr(i, "src").map(|s| s.to_string()), module))
            }).collect()
        };
        if new.is_empty() { break; }
        for (i, src, module) in new { run_one(it, l, fetch_src, i, src, module, slice, cap, rep); any = true; }
    }
    any
}

fn run_one(it: &mut Interp, l: &LiveRef, fetch_src: &dyn Fn(&str) -> Option<Vec<u8>>, i: u32, src: Option<String>, module: bool, slice: u64, cap: u64, rep: &mut Report) {
    l.borrow_mut().started.insert(i);
    if it.steps >= cap { rep.errors.push("page step budget spent".into()); return; }
    // PARSE-DOCK P3: the broker may hold this script already parsed (its cache, or a background parse it started) — then the
    // renderer never fetches the text or builds the tree
    let fed = match &src { Some(u) => { let url = resolve(&l.borrow(), u); crate::jscomp::feed(&url, module) } None => None };
    let prog = if let Some(p) = fed { p } else {
    let text = match &src {
        Some(u) => { let url = resolve(&l.borrow(), u); match fetch_src(&url) { Some(b) => String::from_utf8_lossy(&b).into_owned(), None => {
            rep.failed += 1; rep.errors.push(format!("not loaded: {url}")); it.limit = it.steps.saturating_add(slice).min(cap); let _ = dispatch(it, i, event_obj("error", false)); return; } } }
        None => l.borrow().text_content(i),
    };
    rep.bytes += text.len();
    match crate::jsparse::parse(&text, module) { Ok(p) => p, Err(e) => { rep.parse_err += 1; rep.errors.push(format!("parse {}: {e}", src.as_deref().unwrap_or("inline"))); return; } }
    };
    it.limit = it.steps.saturating_add(slice).min(cap);
    l.borrow_mut().current_script = if module { NONE } else { i };
    // module scripts run as modules (HTML §8.1.3.1 module graph): imports load through the interpreter's module_loader (pageproc
    // plugs the broker in); the key is the script's URL, or the page URL + #inline-<node> for an inline module
    let r = if module {
        let key = match &src { Some(u) => resolve(&l.borrow(), u), None => format!("{}#inline-{i}", l.borrow().url.split('#').next().unwrap_or("")) };
        it.run_module(&key, &prog)
    } else { it.run_program(&prog) };
    match r {
        None => rep.ran += 1,
        Some(e) => { rep.failed += 1; if e.contains("step limit") { rep.sliced += 1; } rep.errors.push(format!("{}: {}", src.as_deref().unwrap_or("inline"), e.chars().take(160).collect::<String>())); }
    }
    l.borrow_mut().current_script = NONE;
    deliver(it);
    // HTML §8.1.4.2 "execute the script element": an external script's element gets load after it ran
    if src.is_some() { it.limit = it.steps.saturating_add(slice).min(cap); let _ = dispatch(it, i, event_obj("load", false)); }
}
