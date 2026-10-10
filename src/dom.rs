// SPDX-License-Identifier: MIT OR Apache-2.0
//! Packed DOM — the document as flat columns, not an object graph.
//!
//! Build phase: html5ever's tree builder drives `Builder` (a TreeSink) that keeps 5 u32 links per node
//! so the HTML5 algorithm can insert / move / reparent anywhere. `Builder::finish` then *freezes* the
//! tree into preorder: children follow their parent, so `first_child` is implicit (i+1) and each node
//! stores only `parent`, `end` (one past its subtree) and a packed name/kind + data word = 16 bytes/node.
//! Text lives in one byte arena; attributes in one span table. No per-node heap allocation anywhere.
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{parse_document, Attribute, ParseOpts, QualName};
use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;

pub const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Document = 0,
    Element = 1,
    Text = 2,
    Comment = 3,
    Doctype = 4,
}

/// Frozen node: 16 bytes. `name` is the interned tag id for elements (0 otherwise).
/// `data` = text span index (Text/Comment) or first attribute index (Element, count in `nattr`).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Node {
    pub parent: u32,
    pub end: u32,
    pub data: u32,
    pub name: u16,
    pub kind: Kind,
    pub nattr: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub off: u32,
    pub len: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Attr {
    pub name: u16,
    pub val: Span,
}

/// The frozen document.
pub struct Dom {
    pub nodes: Vec<Node>,
    pub names: Vec<QualName>,   // interned element + attribute names
    pub texts: Vec<Span>,       // Text/Comment spans into `bytes`
    pub attrs: Vec<Attr>,
    pub bytes: Vec<u8>,         // all text + attribute values, deduplicated
    pub quirks: bool,
    /// `<style>` element text in document order (moved out of the DOM; the CSS engine consumes it).
    pub sheets: Vec<(u32, String)>,
    /// inline `<script>` text in document order (moved out of the DOM like sheets; page scripts read it — BROWSER-BODY-0 JS-0)
    pub scripts: Vec<(u32, String)>,
}

impl Dom {
    pub fn parse(html: &[u8]) -> Dom {
        let b = Builder::new();
        let mut o = ParseOpts::default();
        o.tree_builder.scripting_enabled = false; // document engine: <noscript> content renders
        parse_document(b, o).from_utf8().one(html)
    }
    /// Inside the document (template contents are packed after it: dom.rs freeze)
    #[inline]
    pub fn in_doc(&self, i: u32) -> bool { self.nodes.first().map(|d| i < d.end).unwrap_or(false) }
    #[inline]
    pub fn first_child(&self, i: u32) -> u32 {
        if self.nodes[i as usize].end > i + 1 { i + 1 } else { NONE }
    }
    #[inline]
    pub fn next_sibling(&self, i: u32) -> u32 {
        let n = self.nodes[i as usize];
        if n.parent == NONE { return NONE; }
        if n.end < self.nodes[n.parent as usize].end { n.end } else { NONE }
    }
    pub fn children(&self, i: u32) -> Children<'_> {
        Children { dom: self, cur: self.first_child(i) }
    }
    pub fn tag(&self, i: u32) -> &str {
        let n = self.nodes[i as usize];
        if n.kind == Kind::Element { &self.names[n.name as usize].local } else { "" }
    }
    pub fn text(&self, i: u32) -> &str {
        let s = self.texts[self.nodes[i as usize].data as usize];
        std::str::from_utf8(&self.bytes[s.off as usize..(s.off + s.len) as usize]).unwrap_or("")
    }
    pub fn attr_list(&self, i: u32) -> &[Attr] {
        let n = self.nodes[i as usize];
        if n.kind != Kind::Element || n.nattr == 0 { return &[]; }
        &self.attrs[n.data as usize..n.data as usize + n.nattr as usize]
    }
    pub fn attr(&self, i: u32, name: &str) -> Option<&str> {
        for a in self.attr_list(i) {
            if &*self.names[a.name as usize].local == name {
                return std::str::from_utf8(&self.bytes[a.val.off as usize..(a.val.off + a.val.len) as usize]).ok();
            }
        }
        None
    }
    pub fn span_str(&self, s: Span) -> &str {
        std::str::from_utf8(&self.bytes[s.off as usize..(s.off + s.len) as usize]).unwrap_or("")
    }
    /// Heap bytes held by the frozen DOM (capacity, i.e. what the allocator actually gave us).
    pub fn heap_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<Node>()
            + self.texts.capacity() * 8
            + self.attrs.capacity() * std::mem::size_of::<Attr>()
            + self.bytes.capacity()
            + self.names.capacity() * std::mem::size_of::<QualName>()
            + self.sheets.iter().map(|x| x.1.capacity()).sum::<usize>()
            + self.scripts.iter().map(|x| x.1.capacity()).sum::<usize>()
    }
    /// New document with generated ::before/::after boxes materialized as element nodes (local name "::before"/
    /// "::after", one text child). Returns the document and old-index → new-index map, plus each box's new index.
    pub fn with_pseudo(&self, boxes: &[(u32, u8, String)]) -> (Dom, Vec<u32>, Vec<u32>) {
        let mut before: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut after: HashMap<u32, Vec<usize>> = HashMap::new();
        for (k, (e, w, _)) in boxes.iter().enumerate() { if *w == 1 { before.entry(*e).or_default().push(k) } else { after.entry(*e).or_default().push(k) } }
        let mut names = self.names.clone();
        let mut name_of = |local: &str, names: &mut Vec<QualName>| -> u16 {
            if let Some(i) = names.iter().position(|q| &*q.local == local) { return i as u16; }
            names.push(QualName::new(None, html5ever::ns!(html), html5ever::LocalName::from(local)));
            (names.len() - 1) as u16
        };
        let (nb, na) = (name_of("::before", &mut names), name_of("::after", &mut names));
        let mut bytes = self.bytes.clone();
        let mut texts = self.texts.clone();
        let mut nodes: Vec<Node> = Vec::with_capacity(self.nodes.len() + boxes.len() * 2);
        let mut map = vec![NONE; self.nodes.len()];
        let mut box_ix = vec![NONE; boxes.len()];
        enum Ev { Enter(u32, u32), Exit(usize), Gen(usize, u16, u32) }
        let mut stack = vec![Ev::Enter(0, NONE)];
        while let Some(ev) = stack.pop() {
            match ev {
                Ev::Exit(fi) => { let l = nodes.len() as u32; nodes[fi].end = l; }
                Ev::Gen(k, name, parent) => {
                    let fi = nodes.len();
                    nodes.push(Node { parent, end: 0, data: 0, name, kind: Kind::Element, nattr: 0 });
                    box_ix[k] = fi as u32;
                    let t = &boxes[k].2;
                    if !t.is_empty() {
                        let sp = Span { off: bytes.len() as u32, len: t.len() as u32 };
                        bytes.extend_from_slice(t.as_bytes());
                        texts.push(sp);
                        nodes.push(Node { parent: fi as u32, end: nodes.len() as u32 + 1, data: (texts.len() - 1) as u32, name: 0, kind: Kind::Text, nattr: 0 });
                    }
                    let l = nodes.len() as u32;
                    nodes[fi].end = l;
                }
                Ev::Enter(o, np) => {
                    let n = self.nodes[o as usize];
                    let fi = nodes.len();
                    nodes.push(Node { parent: np, end: 0, ..n });
                    map[o as usize] = fi as u32;
                    stack.push(Ev::Exit(fi));
                    if let Some(v) = after.get(&o) { for &k in v.iter().rev() { stack.push(Ev::Gen(k, na, fi as u32)); } }
                    let kids: Vec<u32> = self.children(o).collect();
                    for &c in kids.iter().rev() { stack.push(Ev::Enter(c, fi as u32)); }
                    if let Some(v) = before.get(&o) { for &k in v.iter().rev() { stack.push(Ev::Gen(k, nb, fi as u32)); } }
                }
            }
        }
        (Dom { nodes, names, texts, attrs: self.attrs.clone(), bytes, quirks: self.quirks, sheets: self.sheets.iter().map(|(n, s)| (map.get(*n as usize).copied().unwrap_or(NONE), s.clone())).collect(), scripts: self.scripts.iter().map(|(n, s)| (map.get(*n as usize).copied().unwrap_or(NONE), s.clone())).collect() }, map, box_ix)
    }

    /// Markup of an element subtree (used to hand inline <svg> to the SVG renderer).
    pub fn outer_xml(&self, n: u32) -> String {
        let mut o = String::new();
        self.xml_rec(n, &mut o);
        o
    }
    fn xml_rec(&self, n: u32, o: &mut String) {
        match self.nodes[n as usize].kind {
            Kind::Element => {
                let tag = self.tag(n);
                o.push('<'); o.push_str(tag);
                for a in self.attr_list(n) {
                    o.push(' '); o.push_str(&self.names[a.name as usize].local);
                    o.push_str("=\""); o.push_str(&self.span_str(a.val).replace('"', "&quot;")); o.push('"');
                }
                o.push('>');
                for c in self.children(n) { self.xml_rec(c, o); }
                o.push_str("</"); o.push_str(tag); o.push('>');
            }
            Kind::Text => o.push_str(&self.text(n).replace('<', "&lt;")),
            _ => {}
        }
    }

    /// Canonical serialization (for round-trip tests against other tree builders).
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.dump_rec(0, 0, &mut out);
        out
    }
    fn dump_rec(&self, i: u32, depth: usize, out: &mut String) {
        let n = self.nodes[i as usize];
        match n.kind {
            Kind::Document => out.push_str("#document\n"),
            Kind::Element => {
                out.push_str(&"  ".repeat(depth));
                out.push('<');
                out.push_str(self.tag(i));
                let mut at: Vec<(String, String)> = self.attr_list(i).iter()
                    .map(|a| (self.names[a.name as usize].local.to_string(), self.span_str(a.val).to_string())).collect();
                at.sort();
                for (k, v) in at { out.push_str(&format!(" {k}=\"{v}\"")); }
                out.push_str(">\n");
            }
            Kind::Text => { out.push_str(&"  ".repeat(depth)); out.push_str(&format!("{:?}\n", self.text(i))); }
            Kind::Comment => { out.push_str(&"  ".repeat(depth)); out.push_str(&format!("<!-- {} -->\n", self.text(i))); }
            Kind::Doctype => out.push_str("<!DOCTYPE>\n"),
        }
        for c in self.children(i) { self.dump_rec(c, depth + (n.kind != Kind::Document) as usize, out); }
    }
}

pub struct Children<'a> { dom: &'a Dom, cur: u32 }
impl Iterator for Children<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.cur == NONE { return None; }
        let c = self.cur;
        self.cur = self.dom.next_sibling(c);
        Some(c)
    }
}

// ───────────────────────── build phase ─────────────────────────

#[derive(Clone, Copy)]
struct BNode {
    parent: u32, first: u32, last: u32, prev: u32, next: u32,
    kind: Kind, name: u16, data: u32, nattr: u8,
}

pub struct Builder {
    n: RefCell<Vec<BNode>>,
    names: RefCell<Vec<QualName>>,
    name_ix: RefCell<HashMap<QualName, u16>>,
    texts: RefCell<Vec<Span>>,
    attrs: RefCell<Vec<Attr>>,
    bytes: RefCell<Vec<u8>>,
    quirks: Cell<bool>,
    templates: RefCell<HashMap<u32, u32>>,
}

impl Builder {
    fn new() -> Self {
        let b = Builder {
            n: RefCell::new(Vec::with_capacity(4096)),
            names: RefCell::new(Vec::new()),
            name_ix: RefCell::new(HashMap::new()),
            texts: RefCell::new(Vec::new()),
            attrs: RefCell::new(Vec::new()),
            bytes: RefCell::new(Vec::with_capacity(1 << 16)),
            quirks: Cell::new(false),
            templates: RefCell::new(HashMap::new()),
        };
        b.intern(QualName::new(None, html5ever::ns!(), html5ever::local_name!("")));
        b.alloc(Kind::Document, 0, 0, 0);
        b
    }
    fn intern(&self, q: QualName) -> u16 {
        if let Some(&i) = self.name_ix.borrow().get(&q) { return i; }
        let mut v = self.names.borrow_mut();
        let i = v.len() as u16;
        v.push(q.clone());
        self.name_ix.borrow_mut().insert(q, i);
        i
    }
    fn alloc(&self, kind: Kind, name: u16, data: u32, nattr: u8) -> u32 {
        let mut v = self.n.borrow_mut();
        v.push(BNode { parent: NONE, first: NONE, last: NONE, prev: NONE, next: NONE, kind, name, data, nattr });
        (v.len() - 1) as u32
    }
    fn push_bytes(&self, s: &str) -> Span {
        let mut b = self.bytes.borrow_mut();
        let off = b.len() as u32;
        b.extend_from_slice(s.as_bytes());
        Span { off, len: s.len() as u32 }
    }
    fn new_text(&self, s: &str) -> u32 {
        let sp = self.push_bytes(s);
        let mut t = self.texts.borrow_mut();
        t.push(sp);
        let ti = (t.len() - 1) as u32;
        drop(t);
        self.alloc(Kind::Text, 0, ti, 0)
    }
    /// Append text to an existing text node (tree builder merges adjacent text).
    fn extend_text(&self, node: u32, s: &str) {
        let ti = self.n.borrow()[node as usize].data as usize;
        let mut texts = self.texts.borrow_mut();
        let sp = texts[ti];
        let mut b = self.bytes.borrow_mut();
        if (sp.off + sp.len) as usize == b.len() {
            b.extend_from_slice(s.as_bytes());
            texts[ti].len += s.len() as u32;
        } else {
            let old = b[sp.off as usize..(sp.off + sp.len) as usize].to_vec();
            let off = b.len() as u32;
            b.extend_from_slice(&old);
            b.extend_from_slice(s.as_bytes());
            texts[ti] = Span { off, len: sp.len + s.len() as u32 };
        }
    }
    fn detach(&self, c: u32) {
        let mut v = self.n.borrow_mut();
        let BNode { parent, prev, next, .. } = v[c as usize];
        if parent == NONE { return; }
        if prev != NONE { v[prev as usize].next = next } else { v[parent as usize].first = next }
        if next != NONE { v[next as usize].prev = prev } else { v[parent as usize].last = prev }
        let cn = &mut v[c as usize];
        cn.parent = NONE; cn.prev = NONE; cn.next = NONE;
    }
    fn append_node(&self, p: u32, c: u32) {
        self.detach(c);
        let mut v = self.n.borrow_mut();
        let last = v[p as usize].last;
        v[c as usize].parent = p;
        v[c as usize].prev = last;
        if last != NONE { v[last as usize].next = c } else { v[p as usize].first = c }
        v[p as usize].last = c;
    }
    fn insert_before(&self, sib: u32, c: u32) {
        self.detach(c);
        let mut v = self.n.borrow_mut();
        let p = v[sib as usize].parent;
        let prev = v[sib as usize].prev;
        v[c as usize].parent = p;
        v[c as usize].prev = prev;
        v[c as usize].next = sib;
        v[sib as usize].prev = c;
        if prev != NONE { v[prev as usize].next = c } else if p != NONE { v[p as usize].first = c }
    }
    fn text_into(&self, parent: u32, before: Option<u32>, t: &str) {
        // merge with the adjacent text node, as the HTML5 algorithm requires
        let adj = {
            let v = self.n.borrow();
            match before {
                None => v[parent as usize].last,
                Some(s) => v[s as usize].prev,
            }
        };
        if adj != NONE && self.n.borrow()[adj as usize].kind == Kind::Text {
            self.extend_text(adj, t);
            return;
        }
        let c = self.new_text(t);
        match before { None => self.append_node(parent, c), Some(s) => self.insert_before(s, c) }
    }

    fn freeze(self) -> Dom {
        let v = self.n.into_inner();
        let mut nodes: Vec<Node> = Vec::with_capacity(v.len());
        // iterative preorder with explicit exit events; nodes not attached under the root are dropped
        enum Ev { Enter(u32, u32), Exit(usize) }
        let mut stack = vec![Ev::Enter(0, NONE)];
        let mut kids: Vec<u32> = Vec::new();
        let templates = self.templates.into_inner();
        let mut tpl_at: Vec<(u32, u32)> = vec![]; // (old template id, packed index)
        let mut walk = |stack: &mut Vec<Ev>, nodes: &mut Vec<Node>, tpl_at: &mut Vec<(u32, u32)>| while let Some(ev) = stack.pop() {
            match ev {
                Ev::Exit(fi) => { let l = nodes.len() as u32; nodes[fi].end = l; }
                Ev::Enter(b, fparent) => {
                    let bn = v[b as usize];
                    let fi = nodes.len();
                    nodes.push(Node { parent: fparent, end: 0, data: bn.data, name: bn.name, kind: bn.kind, nattr: bn.nattr });
                    if templates.contains_key(&b) { tpl_at.push((b, fi as u32)); }
                    stack.push(Ev::Exit(fi));
                    kids.clear();
                    let mut c = bn.first;
                    while c != NONE { kids.push(c); c = v[c as usize].next; }
                    for &k in kids.iter().rev() { stack.push(Ev::Enter(k, fi as u32)); }
                }
            }
        };
        walk(&mut stack, &mut nodes, &mut tpl_at);
        // HTML §4.12.3 template contents live in their own fragment, outside the document: packed AFTER the document's range
        // (nodes[0].end), parented to their template for the script DOM, never inside any element's range (layout, the
        // corpus ids and the gate see exactly the document they saw before)
        let main_end = nodes.len();
        let mut k = 0;
        while k < tpl_at.len() {
            let (old, fi) = tpl_at[k];
            if let Some(&frag) = templates.get(&old) {
                let mut c = v[frag as usize].first; let mut ks = vec![];
                while c != NONE { ks.push(c); c = v[c as usize].next; }
                for &x in ks.iter().rev() { stack.push(Ev::Enter(x, fi)); }
                walk(&mut stack, &mut nodes, &mut tpl_at);
            }
            k += 1;
        }
        nodes.shrink_to_fit();
        // compaction pass: rebuild the byte arena with every string hash-consed (streams repeat: class
        // names, hrefs, whitespace runs), drop comment and script bodies, move <style> text to `sheets`.
        let names = self.names.into_inner();
        let old_texts = self.texts.into_inner();
        let old_attrs = self.attrs.into_inner();
        let old_bytes = self.bytes.into_inner();
        let mut bytes: Vec<u8> = Vec::with_capacity(old_bytes.len() / 2);
        let mut seen: HashMap<&[u8], Span> = HashMap::new();
        let mut texts: Vec<Span> = Vec::new();
        let mut attrs: Vec<Attr> = Vec::new();
        let mut sheets = Vec::new();
        let mut scripts = Vec::new();
        let mut put = |b: &[u8], bytes: &mut Vec<u8>, seen: &mut HashMap<&[u8], Span>| -> Span {
            if let Some(&sp) = seen.get(b) { return sp; }
            let sp = Span { off: bytes.len() as u32, len: b.len() as u32 };
            bytes.extend_from_slice(b);
            sp
        };
        let get = |sp: Span| &old_bytes[sp.off as usize..(sp.off + sp.len) as usize];
        let mut text_ix: HashMap<u32, u32> = HashMap::new();
        for i in 0..nodes.len() {
            let n = nodes[i];
            match n.kind {
                Kind::Text | Kind::Comment => {
                    let ptag: &str = if n.parent != NONE && nodes[n.parent as usize].kind == Kind::Element {
                        &names[nodes[n.parent as usize].name as usize].local } else { "" };
                    let raw = get(old_texts[n.data as usize]);
                    let outside = i >= main_end; // template contents: their <style>/<script> text stays put (inert)
                    let keep: &[u8] = if n.kind == Kind::Comment { b"" } else if outside { raw }
                        else if ptag == "script" { if !raw.is_empty() { scripts.push((n.parent, String::from_utf8_lossy(raw).into_owned())); } b"" }
                        else if ptag == "style" { sheets.push((n.parent, String::from_utf8_lossy(raw).into_owned())); b"" }
                        else { raw };
                    let sp = put(keep, &mut bytes, &mut seen);
                    if !keep.is_empty() && keep.len() <= 256 { seen.insert(keep, sp); }
                    let ti = *text_ix.entry(n.data).or_insert_with(|| { texts.push(sp); (texts.len() - 1) as u32 });
                    nodes[i].data = ti;
                }
                Kind::Element if n.nattr > 0 => {
                    let a0 = attrs.len() as u32;
                    for a in &old_attrs[n.data as usize..n.data as usize + n.nattr as usize] {
                        let raw = get(a.val);
                        let sp = put(raw, &mut bytes, &mut seen);
                        if raw.len() <= 256 { seen.insert(raw, sp); }
                        attrs.push(Attr { name: a.name, val: sp });
                    }
                    nodes[i].data = a0;
                }
                _ => {}
            }
        }
        drop(seen);
        texts.shrink_to_fit(); attrs.shrink_to_fit(); bytes.shrink_to_fit();
        let mut names = names; names.shrink_to_fit();
        Dom { nodes, names, texts, attrs, bytes, quirks: self.quirks.get(), sheets, scripts }
    }
}


impl TreeSink for Builder {
    type Handle = u32;
    type Output = Dom;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Dom { self.freeze() }
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn get_document(&self) -> u32 { 0 }
    fn elem_name<'a>(&'a self, t: &'a u32) -> Ref<'a, QualName> {
        let id = self.n.borrow()[*t as usize].name as usize;
        Ref::map(self.names.borrow(), |v| &v[id])
    }
    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> u32 {
        let nid = self.intern(name);
        let a0 = self.attrs.borrow().len() as u32;
        let na = attrs.len().min(255);
        for a in attrs.into_iter().take(255) {
            let an = self.intern(a.name);
            let sp = self.push_bytes(&a.value);
            self.attrs.borrow_mut().push(Attr { name: an, val: sp });
        }
        let e = self.alloc(Kind::Element, nid, a0, na as u8);
        if flags.template {
            let frag = self.alloc(Kind::Document, 0, 0, 0);
            self.templates.borrow_mut().insert(e, frag);
        }
        e
    }
    fn create_comment(&self, text: StrTendril) -> u32 {
        let sp = self.push_bytes(&text);
        let mut t = self.texts.borrow_mut();
        t.push(sp);
        let ti = (t.len() - 1) as u32;
        drop(t);
        self.alloc(Kind::Comment, 0, ti, 0)
    }
    fn create_pi(&self, _t: StrTendril, data: StrTendril) -> u32 { self.create_comment(data) }
    fn append(&self, parent: &u32, child: NodeOrText<u32>) {
        match child {
            NodeOrText::AppendNode(c) => self.append_node(*parent, c),
            NodeOrText::AppendText(t) => self.text_into(*parent, None, &t),
        }
    }
    fn append_based_on_parent_node(&self, element: &u32, prev: &u32, child: NodeOrText<u32>) {
        if self.n.borrow()[*element as usize].parent != NONE {
            self.append_before_sibling(element, child)
        } else {
            self.append(prev, child)
        }
    }
    fn append_doctype_to_document(&self, _n: StrTendril, _p: StrTendril, _s: StrTendril) {
        let d = self.alloc(Kind::Doctype, 0, 0, 0);
        self.append_node(0, d);
    }
    /// Declarative shadow DOM (`<template shadowrootmode>`): the shadow root becomes a `::shadow` child of the host with
    /// display: contents, so its tree renders in place (slot assignment and style scoping are not modelled yet).
    fn attach_declarative_shadow(&self, host: &u32, template: &u32, _attrs: &[Attribute]) -> bool {
        let nid = self.intern(QualName::new(None, html5ever::ns!(html), html5ever::LocalName::from("::shadow")));
        let sr = self.alloc(Kind::Element, nid, self.attrs.borrow().len() as u32, 0);
        self.append_node(*host, sr);
        self.templates.borrow_mut().insert(*template, sr);
        true
    }
    fn get_template_contents(&self, t: &u32) -> u32 {
        *self.templates.borrow().get(t).unwrap_or(t)
    }
    fn same_node(&self, x: &u32, y: &u32) -> bool { x == y }
    fn set_quirks_mode(&self, mode: QuirksMode) { self.quirks.set(mode == QuirksMode::Quirks) }
    fn append_before_sibling(&self, sib: &u32, new: NodeOrText<u32>) {
        match new {
            NodeOrText::AppendNode(c) => self.insert_before(*sib, c),
            NodeOrText::AppendText(t) => {
                let p = self.n.borrow()[*sib as usize].parent;
                if p != NONE { self.text_into(p, Some(*sib), &t) }
            }
        }
    }
    fn add_attrs_if_missing(&self, target: &u32, add: Vec<Attribute>) {
        // rare (duplicate <html>/<body>); re-home the attr run at the end of the table
        let (a0, na) = { let b = self.n.borrow()[*target as usize]; (b.data as usize, b.nattr as usize) };
        let mut cur: Vec<Attr> = self.attrs.borrow()[a0..a0 + na].to_vec();
        for a in add {
            let an = self.intern(a.name);
            if cur.iter().any(|x| x.name == an) { continue; }
            let sp = self.push_bytes(&a.value);
            cur.push(Attr { name: an, val: sp });
        }
        let mut at = self.attrs.borrow_mut();
        let start = at.len() as u32;
        let n = cur.len().min(255);
        at.extend(cur.into_iter().take(255));
        drop(at);
        let mut v = self.n.borrow_mut();
        v[*target as usize].data = start;
        v[*target as usize].nattr = n as u8;
    }
    fn remove_from_parent(&self, t: &u32) { self.detach(*t) }
    fn reparent_children(&self, node: &u32, newp: &u32) {
        loop {
            let c = self.n.borrow()[*node as usize].first;
            if c == NONE { break; }
            self.append_node(*newp, c);
        }
    }
}
