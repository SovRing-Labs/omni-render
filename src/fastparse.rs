// SPDX-License-Identifier: MIT OR Apache-2.0
//! INCR-R3 A — a direct parser for well-formed, machine-written HTML (the app runtimes re-emit the whole document on every
//! event). It builds the same frozen Dom that html5ever + dom::Builder::freeze produce — preorder nodes, adjacent text merged,
//! <style> text moved to `sheets`, script / comment bodies emptied, lowercase names — and returns None (caller falls back to
//! html5ever) on anything where HTML tree construction would repair or reinterpret the markup: a missing html/head/body
//! skeleton tag, implied end tags (p closers, li / dd / dt, headings, nested a / button / form), tables, select, foreign
//! content (svg / math), RCDATA (title / textarea), templates, frames, end tags that do not match the open element, unknown or
//! unterminated character references, NUL, CDATA. Exactness gate: Dom::dump + sheets + quirks identical to Dom::parse.
use crate::dom::{Attr, Dom, Kind, Node, Span, NONE};
use html5ever::QualName;
use std::collections::HashMap;

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr", "keygen", "param", "basefont", "bgsound", "frame"];
/// start tags that close an open <p> (HTML §13.2.6.4.7) — we bail instead of emulating
const P_CLOSERS: &[&str] = &["address", "article", "aside", "blockquote", "center", "details", "dialog", "dir", "div", "dl", "fieldset",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hgroup", "hr", "main", "menu", "nav", "ol", "p",
    "pre", "section", "summary", "table", "ul", "li", "dd", "dt", "listing", "plaintext", "xmp", "search"];
/// tags whose start would trigger a different insertion mode or reconstruction — bail
const BAIL: &[&str] = &["table", "caption", "colgroup", "col", "tbody", "thead", "tfoot", "tr", "td", "th",
    "math", "title", "template", "frameset", "frame", "iframe", "noembed", "noframes", "plaintext", "xmp", "image",
    "isindex", "nobr", "marquee", "object", "applet", "html", "head", "body", "listing", "base", "basefont", "bgsound", "keygen"];
/// "special" elements (HTML §13.2.4.2) used by the li / dd / dt walk
const SPECIAL: &[&str] = &["address", "applet", "area", "article", "aside", "base", "basefont", "bgsound", "blockquote", "body", "br",
    "button", "caption", "center", "col", "colgroup", "dd", "details", "dir", "div", "dl", "dt", "embed", "fieldset", "figcaption", "figure",
    "footer", "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hgroup", "hr", "html", "iframe", "img",
    "input", "keygen", "li", "link", "listing", "main", "marquee", "menu", "meta", "nav", "noembed", "noframes", "noscript", "object", "ol",
    "p", "param", "plaintext", "pre", "script", "search", "section", "select", "source", "style", "summary", "table", "tbody", "td",
    "template", "textarea", "tfoot", "th", "thead", "title", "tr", "track", "ul", "wbr", "xmp"];
/// SVG tag / attribute case adjustment (HTML §13.2.6.5) — names are lowercased by the tokenizer, then mapped back
const SVG_TAGS: &[(&str, &str)] = &[("altglyph", "altGlyph"), ("altglyphdef", "altGlyphDef"), ("altglyphitem", "altGlyphItem"),
    ("animatecolor", "animateColor"), ("animatemotion", "animateMotion"), ("animatetransform", "animateTransform"), ("clippath", "clipPath"),
    ("feblend", "feBlend"), ("fecolormatrix", "feColorMatrix"), ("fecomponenttransfer", "feComponentTransfer"), ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"), ("fediffuselighting", "feDiffuseLighting"), ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"), ("fedropshadow", "feDropShadow"), ("feflood", "feFlood"), ("fefunca", "feFuncA"), ("fefuncb", "feFuncB"),
    ("fefuncg", "feFuncG"), ("fefuncr", "feFuncR"), ("fegaussianblur", "feGaussianBlur"), ("feimage", "feImage"), ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"), ("femorphology", "feMorphology"), ("feoffset", "feOffset"), ("fepointlight", "fePointLight"),
    ("fespecularlighting", "feSpecularLighting"), ("fespotlight", "feSpotLight"), ("fetile", "feTile"), ("feturbulence", "feTurbulence"),
    ("foreignobject", "foreignObject"), ("glyphref", "glyphRef"), ("lineargradient", "linearGradient"), ("radialgradient", "radialGradient"),
    ("textpath", "textPath")];
const SVG_ATTRS: &[(&str, &str)] = &[("attributename", "attributeName"), ("attributetype", "attributeType"), ("basefrequency", "baseFrequency"),
    ("baseprofile", "baseProfile"), ("calcmode", "calcMode"), ("clippathunits", "clipPathUnits"), ("diffuseconstant", "diffuseConstant"),
    ("edgemode", "edgeMode"), ("filterunits", "filterUnits"), ("glyphref", "glyphRef"), ("gradienttransform", "gradientTransform"),
    ("gradientunits", "gradientUnits"), ("kernelmatrix", "kernelMatrix"), ("kernelunitlength", "kernelUnitLength"), ("keypoints", "keyPoints"),
    ("keysplines", "keySplines"), ("keytimes", "keyTimes"), ("lengthadjust", "lengthAdjust"), ("limitingconeangle", "limitingConeAngle"),
    ("markerheight", "markerHeight"), ("markerunits", "markerUnits"), ("markerwidth", "markerWidth"), ("maskcontentunits", "maskContentUnits"),
    ("maskunits", "maskUnits"), ("numoctaves", "numOctaves"), ("pathlength", "pathLength"), ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"), ("patternunits", "patternUnits"), ("pointsatx", "pointsAtX"), ("pointsaty", "pointsAtY"),
    ("pointsatz", "pointsAtZ"), ("preservealpha", "preserveAlpha"), ("preserveaspectratio", "preserveAspectRatio"), ("primitiveunits", "primitiveUnits"),
    ("refx", "refX"), ("refy", "refY"), ("repeatcount", "repeatCount"), ("repeatdur", "repeatDur"), ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"), ("specularconstant", "specularConstant"), ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"), ("startoffset", "startOffset"), ("stddeviation", "stdDeviation"), ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"), ("systemlanguage", "systemLanguage"), ("tablevalues", "tableValues"), ("targetx", "targetX"),
    ("targety", "targetY"), ("textlength", "textLength"), ("viewbox", "viewBox"), ("viewtarget", "viewTarget"),
    ("xchannelselector", "xChannelSelector"), ("ychannelselector", "yChannelSelector"), ("zoomandpan", "zoomAndPan")];
/// foreign attributes that become (prefix, namespace, local): only the local part is kept in our names
const FOREIGN_ATTRS: &[(&str, &str)] = &[("xlink:actuate", "actuate"), ("xlink:arcrole", "arcrole"), ("xlink:href", "href"), ("xlink:role", "role"),
    ("xlink:show", "show"), ("xlink:title", "title"), ("xlink:type", "type"), ("xml:lang", "lang"), ("xml:space", "space"), ("xmlns:xlink", "xlink")];
/// HTML start tags that break out of foreign content (font only with color / face / size)
const BREAKOUT: &[&str] = &["b", "big", "blockquote", "body", "br", "center", "code", "dd", "div", "dl", "dt", "em", "embed", "h1", "h2", "h3",
    "h4", "h5", "h6", "head", "hr", "i", "img", "li", "listing", "menu", "meta", "nobr", "ol", "p", "pre", "ruby", "s", "small", "span",
    "strong", "strike", "sub", "sup", "table", "tt", "u", "ul", "var"];
const ENT: &[(&str, &str)] = &[("amp", "&"), ("lt", "<"), ("gt", ">"), ("quot", "\""), ("apos", "'"), ("nbsp", "\u{a0}"), ("copy", "©"),
    ("reg", "®"), ("trade", "™"), ("mdash", "—"), ("ndash", "–"), ("hellip", "…"), ("middot", "·"), ("times", "×"), ("divide", "÷"),
    ("laquo", "«"), ("raquo", "»"), ("lsquo", "‘"), ("rsquo", "’"), ("ldquo", "“"), ("rdquo", "”"), ("bull", "•"), ("rarr", "→"),
    ("larr", "←"), ("uarr", "↑"), ("darr", "↓"), ("harr", "↔"), ("deg", "°"), ("plusmn", "±"), ("para", "¶"), ("sect", "§"),
    ("euro", "€"), ("pound", "£"), ("yen", "¥"), ("cent", "¢"), ("hearts", "♥"), ("check", "✓"), ("star", "☆"), ("thinsp", "\u{2009}"),
    ("ensp", "\u{2002}"), ("emsp", "\u{2003}"), ("zwj", "\u{200d}"), ("zwnj", "\u{200c}"), ("shy", "\u{ad}"), ("le", "≤"), ("ge", "≥"),
    ("ne", "≠"), ("infin", "∞"), ("minus", "−"), ("lowast", "∗"), ("prime", "′"), ("Prime", "″")];

struct P<'a> {
    s: &'a str, i: usize,
    nodes: Vec<Node>, names: Vec<QualName>, name_ix: HashMap<(bool, String), u16>,
    texts: Vec<Span>, attrs: Vec<Attr>, bytes: Vec<u8>, sheets: Vec<(u32, String)>, scripts: Vec<(u32, String)>,
    stack: Vec<u32>, text: String,
}

impl<'a> P<'a> {
    fn name(&mut self, elem: bool, local: &str) -> u16 {
        if let Some(&i) = self.name_ix.get(&(elem, local.to_string())) { return i; }
        let q = if elem { QualName::new(None, html5ever::ns!(html), html5ever::LocalName::from(local)) }
                else { QualName::new(None, html5ever::ns!(), html5ever::LocalName::from(local)) };
        self.names.push(q); let i = (self.names.len() - 1) as u16; self.name_ix.insert((elem, local.to_string()), i); i
    }
    fn span(&mut self, s: &str) -> Span { let off = self.bytes.len() as u32; self.bytes.extend_from_slice(s.as_bytes()); Span { off, len: s.len() as u32 } }
    fn top(&self) -> u32 { *self.stack.last().unwrap() }
    fn tag_of(&self, n: u32) -> &str { &self.names[self.nodes[n as usize].name as usize].local }
    fn leaf(&mut self, kind: Kind, text: &str) -> u32 {
        let sp = self.span(text); self.texts.push(sp);
        let i = self.nodes.len() as u32;
        self.nodes.push(Node { parent: self.top(), end: i + 1, data: (self.texts.len() - 1) as u32, name: 0, kind, nattr: 0 });
        i
    }
    fn flush_text(&mut self) {
        if self.text.is_empty() { return; }
        let t = std::mem::take(&mut self.text);
        // the parent decides what freeze keeps: <style> → sheets, <script> → empty
        let ptag = self.tag_of(self.top()).to_string();
        if ptag == "style" { let p = self.top(); self.sheets.push((p, t)); self.leaf(Kind::Text, ""); }
        else if ptag == "script" { let p = self.top(); self.scripts.push((p, t)); self.leaf(Kind::Text, ""); }
        else { self.leaf(Kind::Text, &t); }
    }
    fn open(&mut self, tag: &str, attrs: Vec<(String, String)>) -> u32 {
        self.flush_text();
        let nid = self.name(true, tag);
        let a0 = self.attrs.len() as u32;
        let na = attrs.len().min(255);
        for (k, v) in attrs.into_iter().take(255) { let an = self.name(false, &k); let sp = self.span(&v); self.attrs.push(Attr { name: an, val: sp }); }
        let i = self.nodes.len() as u32;
        let parent = self.stack.last().copied().unwrap_or(NONE);
        self.nodes.push(Node { parent, end: 0, data: if na > 0 { a0 } else { a0 }, name: nid, kind: Kind::Element, nattr: na as u8 });
        i
    }
    fn close_top(&mut self) { self.flush_text(); let n = self.stack.pop().unwrap(); self.nodes[n as usize].end = self.nodes.len() as u32; }

    /// character reference at self.i (just after '&'): decoded text, or None = bail
    fn charref(&mut self) -> Option<String> {
        let r = &self.s[self.i..];
        if let Some(rest) = r.strip_prefix('#') {
            let (hex, body) = match rest.strip_prefix(['x', 'X']) { Some(b) => (true, b), None => (false, rest) };
            let digits: String = body.chars().take_while(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }).collect();
            if digits.is_empty() || digits.len() > 7 || !body[digits.len()..].starts_with(';') { return None; }
            let v = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).ok()?;
            if v == 0 || (0x80..=0x9f).contains(&v) || (0xd800..=0xdfff).contains(&v) || v > 0x10ffff || v < 0x20 && v != 9 && v != 10 && v != 12 { return None; }
            self.i += 1 + (hex as usize) + digits.len() + 1;
            return char::from_u32(v).map(|c| c.to_string());
        }
        let nm: String = r.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if nm.is_empty() { return Some("&".into()); } // a bare '&' is literal text
        if !r[nm.len()..].starts_with(';') { return None; }
        let v = ENT.iter().find(|(k, _)| *k == nm)?.1;
        self.i += nm.len() + 1;
        Some(v.to_string())
    }
    /// text up to the next '<' with references decoded (None = bail)
    fn text_run(&mut self) -> Option<()> {
        while self.i < self.s.len() {
            let c = self.s.as_bytes()[self.i];
            if c == b'<' { break; }
            if c == b'&' { self.i += 1; let t = self.charref()?; self.text.push_str(&t); continue; }
            let j = self.s[self.i..].find(['<', '&']).map_or(self.s.len(), |k| self.i + k);
            self.text.push_str(&self.s[self.i..j]); self.i = j;
        }
        Some(())
    }
    /// a tag at self.i (on '<'): (end?, name, attrs). None = bail; Some(None) = '<' is literal text
    fn tag(&mut self) -> Option<Option<(bool, String, Vec<(String, String)>)>> { self.tag_sc().map(|o| o.map(|(e, n, a, _)| (e, n, a))) }
    /// as `tag`, plus whether the tag was written self-closing (`/>`)
    fn tag_sc(&mut self) -> Option<Option<(bool, String, Vec<(String, String)>, bool)>> {
        let b = self.s.as_bytes();
        let mut j = self.i + 1;
        let end = b.get(j) == Some(&b'/');
        if end { j += 1; }
        match b.get(j) { Some(c) if c.is_ascii_alphabetic() => {}, _ => return if end { None } else { Some(None) } }
        let n0 = j;
        while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'\n' | b'\x0c' | b'/' | b'>') { if !b[j].is_ascii() { return None; } j += 1; }
        let name = self.s[n0..j].to_ascii_lowercase();
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut sc = false;
        loop {
            while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\x0c') { j += 1; }
            if j >= b.len() { return None; }
            if b[j] == b'>' { j += 1; break; }
            if b[j] == b'/' { if b.get(j + 1) == Some(&b'>') { j += 2; sc = true; break; } return None; }
            if end { return None; } // attributes on an end tag
            let a0 = j;
            while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'\n' | b'\x0c' | b'/' | b'>' | b'=') { if matches!(b[j], b'"' | b'\'' | b'<') { return None; } j += 1; }
            let an = self.s[a0..j].to_ascii_lowercase();
            while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\x0c') { j += 1; }
            let mut val = String::new();
            if b.get(j) == Some(&b'=') {
                j += 1;
                while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\x0c') { j += 1; }
                let (v0, v1, nj) = match b.get(j) {
                    Some(&q) if q == b'"' || q == b'\'' => { let k = self.s[j + 1..].find(q as char)? + j + 1; (j + 1, k, k + 1) }
                    Some(_) => { let mut k = j; while k < b.len() && !matches!(b[k], b' ' | b'\t' | b'\n' | b'\x0c' | b'>') { if matches!(b[k], b'"' | b'\'' | b'<' | b'=' | b'`') { return None; } k += 1; } (j, k, k) }
                    None => return None,
                };
                let raw = &self.s[v0..v1];
                if raw.contains('&') {
                    let (save_s, save_i) = (self.s, self.i);
                    self.s = raw; self.i = 0;
                    let mut out = String::new();
                    while self.i < raw.len() {
                        if raw.as_bytes()[self.i] == b'&' { self.i += 1; match self.charref() { Some(t) => out.push_str(&t), None => { self.s = save_s; self.i = save_i; return None; } } }
                        else { let k = raw[self.i..].find('&').map_or(raw.len(), |k| self.i + k); out.push_str(&raw[self.i..k]); self.i = k; }
                    }
                    self.s = save_s; self.i = save_i;
                    val = out;
                } else { val = raw.to_string(); }
                j = nj;
            }
            if !attrs.iter().any(|(k, _)| *k == an) { attrs.push((an, val)); }
        }
        self.i = j;
        Some(Some((end, name, attrs, sc)))
    }
    /// raw text (style / script) up to the matching end tag
    fn raw(&mut self, tag: &str) -> Option<()> {
        let k = self.find_end(tag)?;
        let body = &self.s[self.i..self.i + k];
        if tag == "script" && body.contains("<!--") { return None; }
        self.text.push_str(body);
        self.i += k;
        Some(())
    }
    /// an <svg> subtree (start tag already read): foreign elements with case-adjusted names, self-closing honoured
    fn svg(&mut self, attrs: Vec<(String, String)>, sc: bool) -> Option<()> {
        let fix_attrs = |a: Vec<(String, String)>| -> Vec<(String, String)> {
            let mut out: Vec<(String, String)> = Vec::new();
            for (k, v) in a {
                let k = SVG_ATTRS.iter().find(|(l, _)| *l == k).map(|x| x.1.to_string())
                    .or_else(|| FOREIGN_ATTRS.iter().find(|(l, _)| *l == k).map(|x| x.1.to_string())).unwrap_or(k);
                if !out.iter().any(|(x, _)| *x == k) { out.push((k, v)); }
            }
            out
        };
        let root = self.open_ns("svg", fix_attrs(attrs), true);
        if sc { self.nodes[root as usize].end = root + 1; return Some(()); }
        self.stack.push(root);
        let depth = self.stack.len();
        loop {
            self.text_run()?;
            if self.i >= self.s.len() { return None; }
            if self.s[self.i..].starts_with("<!--") { let k = self.s[self.i + 4..].find("-->")?; self.flush_text(); self.leaf(Kind::Comment, ""); self.i += 4 + k + 3; continue; }
            if self.s[self.i..].starts_with("<!") { return None; }
            let Some((end, name, a, sc)) = self.tag_sc()? else { self.text.push('<'); self.i += 1; continue };
            if end {
                if self.tag_of(self.top()).to_ascii_lowercase() != name { return None; }
                self.close_top();
                if self.stack.len() < depth { return Some(()); }
                continue;
            }
            if BREAKOUT.contains(&name.as_str()) || (name == "font" && a.iter().any(|(k, _)| matches!(k.as_str(), "color" | "face" | "size")))
                || matches!(name.as_str(), "foreignobject" | "desc" | "title" | "script" | "style") { return None; }
            let tag = SVG_TAGS.iter().find(|(l, _)| *l == name).map(|x| x.1.to_string()).unwrap_or(name);
            let e = self.open_ns(&tag, fix_attrs(a), true);
            if sc { self.nodes[e as usize].end = e + 1; } else { self.stack.push(e); }
        }
    }
    fn open_ns(&mut self, tag: &str, attrs: Vec<(String, String)>, svg: bool) -> u32 {
        let e = self.open(tag, attrs);
        if svg {
            // element names in the svg namespace (nothing reads the namespace today; kept faithful)
            let q = QualName::new(None, html5ever::ns!(svg), html5ever::LocalName::from(tag));
            if let Some(i) = self.names.iter().position(|x| *x == q) { self.nodes[e as usize].name = i as u16; }
            else { self.names.push(q); self.nodes[e as usize].name = (self.names.len() - 1) as u16; }
        }
        e
    }
    /// <textarea> RCDATA: text up to </textarea with references decoded; a leading newline is dropped
    fn rcdata(&mut self, tag: &str) -> Option<()> {
        let k = self.find_end(tag)?;
        let stop = self.i + k;
        if self.s[self.i..].starts_with('\n') { self.i += 1; }
        while self.i < stop {
            if self.s.as_bytes()[self.i] == b'&' { self.i += 1; let t = self.charref()?; if self.i > stop { return None; } self.text.push_str(&t); }
            else { let j = self.s[self.i..stop].find('&').map_or(stop, |x| self.i + x); self.text.push_str(&self.s[self.i..j]); self.i = j; }
        }
        Some(())
    }
    /// offset (from self.i) of the first `</tag` (ASCII case-insensitive) followed by a tag-ending char; no copying
    fn find_end(&self, tag: &str) -> Option<usize> {
        let b = self.s.as_bytes(); let tb = tag.as_bytes();
        let mut k = self.i;
        while let Some(off) = self.s[k..].find("</") {
            let at = k + off; let nm = at + 2;
            if b.len() >= nm + tb.len() && b[nm..nm + tb.len()].eq_ignore_ascii_case(tb) {
                return match b.get(nm + tb.len()) { Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'/') => Some(at - self.i), _ => None };
            }
            k = at + 2;
        }
        None
    }
    fn in_stack(&self, t: &str) -> bool { self.stack.iter().any(|&n| self.tag_of(n) == t) }
}

pub fn parse_fast(html: &[u8]) -> Option<Dom> {
    let src = std::str::from_utf8(html).ok()?;
    if src.contains('\0') || src.contains("<![CDATA[") || src.contains("<?") { return None; }
    let norm; let s: &str = if src.contains('\r') { norm = src.replace("\r\n", "\n").replace('\r', "\n"); &norm } else { src };
    let mut p = P { s, i: 0, nodes: Vec::with_capacity(s.len() / 40), names: Vec::new(), name_ix: HashMap::new(), texts: Vec::new(),
                    attrs: Vec::new(), bytes: Vec::with_capacity(s.len() / 2), sheets: Vec::new(), scripts: Vec::new(), stack: Vec::new(), text: String::new() };
    p.names.push(QualName::new(None, html5ever::ns!(), html5ever::local_name!("")));
    p.nodes.push(Node { parent: NONE, end: 0, data: 0, name: 0, kind: Kind::Document, nattr: 0 });
    p.stack.push(0);
    // skeleton: <!doctype html> <html …> <head> … </head> <body …> … [</body>] [</html>]
    let low = |p: &P, k: usize| p.s[p.i..].get(..k).map(|x| x.to_ascii_lowercase());
    if low(&p, 15).as_deref() != Some("<!doctype html>") { return None; }
    p.i += 15;
    p.nodes.push(Node { parent: 0, end: 2, data: 0, name: 0, kind: Kind::Doctype, nattr: 0 });
    let expect = |p: &mut P, want: &str| -> Option<Vec<(String, String)>> {
        match p.tag()? { Some((false, n, a)) if n == want => Some(a), _ => None }
    };
    let a = expect(&mut p, "html")?; let h = p.open("html", a); p.stack.push(h);
    let a = expect(&mut p, "head")?; let h = p.open("head", a); p.stack.push(h);
    // in head: meta / link (void), style / script (raw), whitespace text; </head>
    loop {
        let ws = p.s[p.i..].len() - p.s[p.i..].trim_start_matches([' ', '\t', '\n', '\x0c']).len();
        if ws > 0 { let t = p.s[p.i..p.i + ws].to_string(); p.text.push_str(&t); p.i += ws; }
        if p.s.as_bytes().get(p.i) != Some(&b'<') { return None; }
        if p.s[p.i..].starts_with("<!--") { let k = p.s[p.i + 4..].find("-->")?; p.flush_text(); p.leaf(Kind::Comment, ""); p.i += 4 + k + 3; continue; }
        let (end, name, a) = p.tag()??;
        match (end, name.as_str()) {
            (true, "head") => { p.close_top(); break; }
            (false, "meta") | (false, "link") => { let e = p.open(&name, a); p.nodes[e as usize].end = e + 1; }
            (false, "style") | (false, "script") => { let e = p.open(&name, a); p.stack.push(e); p.raw(&name)?; let (_, n2, _) = p.tag()??; if n2 != name { return None; } p.close_top(); }
            _ => return None,
        }
    }
    // after head: whitespace (into html), then <body>
    let ws = p.s[p.i..].len() - p.s[p.i..].trim_start_matches([' ', '\t', '\n', '\x0c']).len();
    if ws > 0 { let t = p.s[p.i..p.i + ws].to_string(); p.text.push_str(&t); p.i += ws; }
    let a = expect(&mut p, "body")?; let b = p.open("body", a); p.stack.push(b);
    let body_depth = p.stack.len();
    // in body
    loop {
        p.text_run()?;
        if p.i >= p.s.len() { break; }
        if p.s[p.i..].starts_with("<!--") {
            let k = p.s[p.i + 4..].find("-->")?;
            if p.s[p.i + 4..].starts_with('>') || p.s[p.i + 4..].starts_with("->") { return None; }
            p.flush_text(); p.leaf(Kind::Comment, ""); p.i += 4 + k + 3; continue;
        }
        if p.s[p.i..].starts_with("<!") { return None; }
        let Some((end, name, a, sc)) = p.tag_sc()? else { p.text.push('<'); p.i += 1; continue };
        if end {
            if name == "body" || name == "html" {
                // </body> [</html>] then only whitespace (whitespace after body goes into body)
                if p.stack.len() != body_depth { return None; }
                if name == "body" {
                    let ws = p.s[p.i..].len() - p.s[p.i..].trim_start_matches([' ', '\t', '\n', '\x0c']).len();
                    if ws > 0 { let t = p.s[p.i..p.i + ws].to_string(); p.text.push_str(&t); p.i += ws; }
                    if p.i < p.s.len() { if p.tag()? != Some((true, "html".into(), vec![])) { return None; } }
                }
                let ws = p.s[p.i..].len() - p.s[p.i..].trim_start_matches([' ', '\t', '\n', '\x0c']).len();
                if ws > 0 { let t = p.s[p.i..p.i + ws].to_string(); p.text.push_str(&t); p.i += ws; }
                if p.i < p.s.len() { return None; }
                break;
            }
            if VOID.contains(&name.as_str()) || p.stack.len() <= body_depth || p.tag_of(p.top()) != name { return None; }
            p.close_top();
            continue;
        }
        let t = name.as_str();
        if BAIL.contains(&t) { return None; }
        // inside <select>: only well-formed option / optgroup (any implied close or ignored token → bail)
        if p.in_stack("select") {
            match t {
                "option" => if p.tag_of(p.top()) == "option" { return None; },
                "optgroup" => if matches!(p.tag_of(p.top()), "option" | "optgroup") { return None; },
                _ => return None,
            }
        } else if t == "select" && p.in_stack("p") { /* select does not close p */ }
        if matches!(t, "option" | "optgroup") && p.tag_of(p.top()) == "option" { return None; }
        if t == "svg" { p.flush_text(); p.svg(a, sc)?; continue; }
        if P_CLOSERS.contains(&t) && p.in_stack("p") { return None; }
        if matches!(t, "li" | "dd" | "dt") {
            for &n in p.stack.iter().rev() {
                let tn = p.tag_of(n);
                if tn == t || (t != "li" && matches!(tn, "dd" | "dt")) { return None; }
                if SPECIAL.contains(&tn) && !matches!(tn, "address" | "div" | "p") { break; }
            }
        }
        if matches!(t, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") && matches!(p.tag_of(p.top()), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") { return None; }
        if matches!(t, "a" | "button" | "form") && p.in_stack(t) { return None; }
        if matches!(t, "noscript" | "frame") { return None; }
        let e = p.open(&name, a);
        if VOID.contains(&t) { p.nodes[e as usize].end = e + 1; continue; }
        p.stack.push(e);
        if t == "style" || t == "script" { p.raw(t)?; let (_, n2, _) = p.tag()??; if n2 != t { return None; } p.close_top(); continue; }
        if t == "textarea" { p.rcdata(t)?; let (_, n2, _) = p.tag()??; if n2 != t { return None; } p.close_top(); continue; }
        if t == "pre" && p.s[p.i..].starts_with('\n') { p.i += 1; } // a newline right after <pre> is dropped
    }
    // EOF: close everything still open (html5ever does the same at end of input)
    while p.stack.len() > 1 { p.close_top(); }
    p.flush_text();
    p.nodes[0].end = p.nodes.len() as u32;
    Some(Dom { nodes: p.nodes, names: p.names, texts: p.texts, attrs: p.attrs, bytes: p.bytes, quirks: false, sheets: p.sheets, scripts: p.scripts })
}

/// Fast path with exact fallback.
pub fn parse(html: &[u8]) -> (Dom, bool) {
    match parse_fast(html) { Some(d) => (d, true), None => (Dom::parse(html), false) }
}

/// `omni-render fastparse <dir>…`: every page.html under the dirs — fast vs html5ever: dump + sheets + quirks identical
/// wherever the fast path accepts; timing.
pub fn cli(dirs: &[String]) {
    let (mut acc, mut fall, mut bad, mut tf, mut th) = (0, 0, 0, 0f64, 0f64);
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for d in dirs { for e in walk(std::path::Path::new(d)) { if e.file_name().map_or(false, |f| f == "page.html") { files.push(e); } } }
    files.sort();
    for f in &files {
        let html = std::fs::read(f).unwrap_or_default();
        let t = std::time::Instant::now(); let r = Dom::parse(&html); let h = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now(); let fast = parse_fast(&html); let fm = t.elapsed().as_secs_f64() * 1e3;
        match fast {
            None => fall += 1,
            Some(d) => {
                acc += 1; tf += fm; th += h;
                let same = d.dump() == r.dump() && d.sheets == r.sheets && d.quirks == r.quirks;
                if !same {
                    bad += 1;
                    let (a, b) = (d.dump(), r.dump());
                    let k = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
                    eprintln!("MISMATCH {}: line {k}\n  fast: {:?}\n  h5e : {:?}", f.display(), a.lines().nth(k), b.lines().nth(k));
                }
            }
        }
    }
    println!("fastparse: {} files · accepted {acc} · fell back {fall} · mismatches {bad} · accepted parse {:.1} ms (fast) vs {:.1} ms (html5ever) = {:.1}x",
        files.len(), tf, th, th / tf.max(1e-9));
}
fn walk(p: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { let q = e.path(); if q.is_dir() { v.extend(walk(&q)); } else { v.push(q); } } }
    v
}
