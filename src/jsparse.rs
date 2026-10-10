// SPDX-License-Identifier: MIT OR Apache-2.0
//! jsparse — the run-time JavaScript front-end (BROWSER-BODY-0 step 8, JS-0): source text → the estree IR (JSON, positions
//! stripped) that jsi_ast lowers, so page scripts are parsed inside the locked-down renderer (js2ir.mjs runs acorn under
//! node at build time; node cannot run in the sandbox). Clean room: written from ECMA-262 (lexical grammar §12, expressions
//! §13, statements §14, functions and classes §15, scripts and modules §16) and the ESTree spec for the output shape.
//! Parity is measured, not assumed: `omni-render js-parse --parity` compares our IR with js2ir's on a corpus.
use serde_json::{json, Map, Value as J};

/// An estree node from (key, value) pairs, MOVING the values in (serde_json's json! serialises each interpolated value —
/// a deep copy of every child subtree per node, quadratic on nested code).
fn obj(kv: Vec<(&str, J)>) -> J { let mut m = Map::with_capacity(kv.len()); for (k, v) in kv { m.insert(k.to_string(), v); } J::Object(m) }

#[derive(Debug, Clone, PartialEq)]
enum T {
    Name { s: String, esc: bool },
    Priv(String),
    Num(f64, Option<String>),
    Str { v: String, raw: String },
    Punct(&'static str),
    Template, // the '`' that opens a template: the parser scans its chunks itself
    Regex(String, String),
    Eof,
}

#[derive(Debug, Clone)]
struct Tok { t: T, start: usize, end: usize, nl: bool }

const PUNCTS: [&str; 54] = [">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=",
    "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "**",
    "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%", "&", "|", "^", "!", "~", "?"];
const PUNCTS2: [&str; 3] = [":", "=", "."];

fn is_lt(c: char) -> bool { matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') }
fn is_ws(c: char) -> bool { matches!(c, '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{a0}' | '\u{feff}') || (c as u32 > 127 && c.is_whitespace() && !is_lt(c)) }
/// IdentifierStartChar / IdentifierPartChar (ECMA-262 §12.7). Without Unicode property tables: in source that acorn
/// accepts, a non-ASCII character outside strings, templates, regexes and comments that is not white space or a line
/// terminator can only be part of an identifier (letters, ℘, combining marks such as U+103A, connector punctuation …),
/// so any such character qualifies.
fn id_start(c: char) -> bool { c == '$' || c == '_' || c.is_ascii_alphabetic() || (c as u32 > 127 && !c.is_whitespace() && !matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}' | '\u{200b}')) }
fn id_part(c: char) -> bool { id_start(c) || c.is_ascii_digit() || c == '\u{200c}' || c == '\u{200d}' }

const RESERVED: [&str; 36] = ["break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends",
    "finally", "for", "function", "if", "import", "in", "instanceof", "new", "return", "super", "switch", "this", "throw", "try", "typeof", "var", "void",
    "while", "with", "null", "true", "false", "enum"];

pub struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    cur: Tok,
    prev_end: usize,
    module: bool,
    in_fn: bool,
    in_async: bool,
    in_gen: bool,
    top_await: bool,
    pub steps: usize,
}

type R<X> = Result<X, String>;

fn num_json(v: f64) -> J {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 && !(v == 0.0 && v.is_sign_negative()) {
        if v >= 0.0 { json!(v as u64) } else { json!(v as i64) }
    } else if v.is_finite() { json!(v) } else { J::Null }
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str, module: bool) -> Parser<'a> {
        Parser { src, b: src.as_bytes(), pos: 0, cur: Tok { t: T::Eof, start: 0, end: 0, nl: false }, prev_end: 0, module, in_fn: false,
                 in_async: false, in_gen: false, top_await: module, steps: 0 }
    }

    fn err<X>(&self, m: &str) -> R<X> {
        let line = self.src[..self.cur.start.min(self.src.len())].matches('\n').count() + 1;
        Err(format!("SyntaxError: {m} (line {line}, token {:?})", self.cur.t))
    }

    fn ch(&self, i: usize) -> Option<char> { self.src.get(i..).and_then(|s| s.chars().next()) }

    // ---- lexer (§12) ----
    /// Skip white space and comments; true when a line terminator was crossed.
    fn skip(&mut self) -> R<bool> {
        let mut nl = false;
        loop {
            let Some(c) = self.ch(self.pos) else { return Ok(nl) };
            if is_lt(c) { nl = true; self.pos += c.len_utf8(); }
            else if is_ws(c) { self.pos += c.len_utf8(); }
            else if c == '/' && self.b.get(self.pos + 1) == Some(&b'/') { self.line_comment(); }
            else if c == '/' && self.b.get(self.pos + 1) == Some(&b'*') {
                let end = self.src[self.pos + 2..].find("*/").ok_or("SyntaxError: unterminated comment")?;
                if self.src[self.pos + 2..self.pos + 2 + end].contains(is_lt) { nl = true; }
                self.pos += end + 4;
            }
            // Annex B.1.1 HTML-like comments (scripts only)
            else if !self.module && c == '<' && self.src[self.pos..].starts_with("<!--") { self.line_comment(); }
            else if !self.module && c == '-' && (nl || self.pos == 0) && self.src[self.pos..].starts_with("-->") { self.line_comment(); }
            else if self.pos == 0 && self.src.starts_with("#!") { self.line_comment(); }
            else { return Ok(nl); }
        }
    }
    fn line_comment(&mut self) {
        while let Some(c) = self.ch(self.pos) { if is_lt(c) { break; } self.pos += c.len_utf8(); }
    }

    fn next(&mut self) -> R<()> {
        self.steps += 1;
        self.prev_end = self.cur.end;
        let nl = self.skip()?;
        let start = self.pos;
        let t = self.scan()?;
        self.cur = Tok { t, start, end: self.pos, nl };
        Ok(())
    }

    fn scan(&mut self) -> R<T> {
        let Some(c) = self.ch(self.pos) else { return Ok(T::Eof) };
        if id_start(c) || c == '\\' { let (s, esc) = self.ident()?; return Ok(T::Name { s, esc }); }
        if c == '#' { self.pos += 1; let (s, _) = self.ident()?; return Ok(T::Priv(s)); }
        if c.is_ascii_digit() || (c == '.' && self.b.get(self.pos + 1).map_or(false, |d| d.is_ascii_digit())) { return self.number(); }
        if c == '"' || c == '\'' { return self.string(c); }
        if c == '`' { self.pos += 1; return Ok(T::Template); }
        let rest = &self.src[self.pos..];
        // `?.` is optional chaining only when not followed by a digit (§12.8 OptionalChainingPunctuator)
        if rest.starts_with("?.") && !rest[2..].starts_with(|d: char| d.is_ascii_digit()) { self.pos += 2; return Ok(T::Punct("?.")); }
        for p in PUNCTS.iter().chain(PUNCTS2.iter()) {
            if *p == "?." { continue; }
            if rest.starts_with(p) { self.pos += p.len(); return Ok(T::Punct(p)); }
        }
        Err(format!("SyntaxError: unexpected character {c:?}"))
    }

    fn hex(&mut self, n: usize) -> R<u32> {
        let s = self.src.get(self.pos..self.pos + n).ok_or("SyntaxError: bad escape")?;
        let v = u32::from_str_radix(s, 16).map_err(|_| "SyntaxError: bad escape")?;
        self.pos += n;
        Ok(v)
    }
    fn unicode_escape(&mut self) -> R<u32> {
        // after "\u"
        if self.b.get(self.pos) == Some(&b'{') {
            let end = self.src[self.pos..].find('}').ok_or("SyntaxError: bad escape")?;
            let v = u32::from_str_radix(&self.src[self.pos + 1..self.pos + end], 16).map_err(|_| "SyntaxError: bad escape")?;
            self.pos += end + 1;
            if v > 0x10ffff { return Err("SyntaxError: code point out of range".into()); }
            Ok(v)
        } else { self.hex(4) }
    }

    fn ident(&mut self) -> R<(String, bool)> {
        let mut s = String::new();
        let mut esc = false;
        while let Some(c) = self.ch(self.pos) {
            if c == '\\' && self.b.get(self.pos + 1) == Some(&b'u') {
                self.pos += 2;
                let v = self.unicode_escape()?;
                s.push(char::from_u32(v).ok_or("SyntaxError: bad identifier escape")?);
                esc = true;
            } else if if s.is_empty() { id_start(c) } else { id_part(c) } { s.push(c); self.pos += c.len_utf8(); } else { break; }
        }
        if s.is_empty() { return Err("SyntaxError: bad identifier".into()); }
        Ok((s, esc))
    }

    fn number(&mut self) -> R<T> {
        let st = self.pos;
        let b = self.b;
        let radix = |p: &mut Self, r: u32| -> R<T> {
            p.pos += 2;
            let s0 = p.pos;
            while p.pos < b.len() && ((b[p.pos] as char).is_digit(r) || b[p.pos] == b'_') { p.pos += 1; }
            let digits: String = p.src[s0..p.pos].chars().filter(|c| *c != '_').collect();
            if b.get(p.pos) == Some(&b'n') { p.pos += 1; let v = u128::from_str_radix(&digits, r).unwrap_or(0); return Ok(T::Num(v as f64, Some(v.to_string()))); }
            let mut v = 0f64;
            for c in digits.chars() { v = v * r as f64 + c.to_digit(r).unwrap_or(0) as f64; }
            Ok(T::Num(v, None))
        };
        if b[st] == b'0' && matches!(b.get(st + 1), Some(b'x' | b'X')) { return radix(self, 16); }
        if b[st] == b'0' && matches!(b.get(st + 1), Some(b'o' | b'O')) { return radix(self, 8); }
        if b[st] == b'0' && matches!(b.get(st + 1), Some(b'b' | b'B')) { return radix(self, 2); }
        // Annex B legacy octal 017 (and 089 decimal)
        if b[st] == b'0' && b.get(st + 1).map_or(false, |d| d.is_ascii_digit()) {
            let mut p = st + 1;
            while p < b.len() && b[p].is_ascii_digit() { p += 1; }
            let s = &self.src[st + 1..p];
            if s.bytes().all(|d| d < b'8') && !matches!(b.get(p), Some(b'.' | b'e' | b'E')) {
                self.pos = p;
                let mut v = 0f64; for d in s.bytes() { v = v * 8.0 + (d - b'0') as f64; }
                return Ok(T::Num(v, None));
            }
        }
        let mut p = st;
        while p < b.len() && (b[p].is_ascii_digit() || b[p] == b'_') { p += 1; }
        if b.get(p) == Some(&b'n') {
            let digits: String = self.src[st..p].chars().filter(|c| *c != '_').collect();
            self.pos = p + 1;
            let v: f64 = digits.parse().unwrap_or(0.0);
            return Ok(T::Num(v, Some(digits.trim_start_matches('0').to_string()).map(|s| if s.is_empty() { "0".into() } else { s })));
        }
        if b.get(p) == Some(&b'.') { p += 1; while p < b.len() && (b[p].is_ascii_digit() || b[p] == b'_') { p += 1; } }
        if matches!(b.get(p), Some(b'e' | b'E')) {
            let mut q = p + 1;
            if matches!(b.get(q), Some(b'+' | b'-')) { q += 1; }
            if b.get(q).map_or(false, |d| d.is_ascii_digit()) { p = q; while p < b.len() && (b[p].is_ascii_digit() || b[p] == b'_') { p += 1; } }
        }
        let txt: String = self.src[st..p].chars().filter(|c| *c != '_').collect();
        self.pos = p;
        if self.ch(p).map_or(false, |c| id_start(c) || c.is_ascii_digit()) { return Err("SyntaxError: identifier directly after number".into()); }
        Ok(T::Num(txt.parse::<f64>().map_err(|_| "SyntaxError: bad number")?, None))
    }

    /// One escape sequence after '\' (§12.9.4); None = an escape a template may leave uncooked.
    fn escape(&mut self, out: &mut String, template: bool) -> R<bool> {
        let Some(c) = self.ch(self.pos) else { return Err("SyntaxError: unterminated string".into()) };
        self.pos += c.len_utf8();
        match c {
            'n' => out.push('\n'), 't' => out.push('\t'), 'r' => out.push('\r'), 'b' => out.push('\u{8}'), 'f' => out.push('\u{c}'), 'v' => out.push('\u{b}'),
            '\r' => { if self.b.get(self.pos) == Some(&b'\n') { self.pos += 1; } }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            'x' => match self.hex(2) { Ok(v) => out.push(char::from_u32(v).unwrap_or('\u{fffd}')), Err(e) => { if template { return Ok(false); } return Err(e); } },
            'u' => {
                let v = match self.unicode_escape() { Ok(v) => v, Err(e) => { if template { return Ok(false); } return Err(e); } };
                if (0xd800..0xdc00).contains(&v) && self.src[self.pos..].starts_with("\\u") {
                    let save = self.pos; self.pos += 2;
                    match self.unicode_escape() { Ok(lo) if (0xdc00..0xe000).contains(&lo) => { out.push(char::from_u32(0x10000 + ((v - 0xd800) << 10) + (lo - 0xdc00)).unwrap()); return Ok(true); } _ => self.pos = save }
                }
                // a lone surrogate travels as jsi's private-use carrier (U+F0000 + unit), as ir_json maps acorn's `\udXXX`
                out.push(char::from_u32(v).or_else(|| char::from_u32(0xF0000 + v)).unwrap_or('\u{fffd}'));
            }
            '0'..='7' => {
                if c == '0' && !self.b.get(self.pos).map_or(false, |d| d.is_ascii_digit()) { out.push('\0'); }
                else {
                    if template { return Ok(false); }
                    // Annex B legacy octal escape: up to 3 digits, value ≤ 0o377
                    let mut v = c.to_digit(8).unwrap();
                    for _ in 0..2 { match self.b.get(self.pos) { Some(d @ b'0'..=b'7') if v * 8 + ((*d - b'0') as u32) <= 0o377 => { v = v * 8 + (*d - b'0') as u32; self.pos += 1; } _ => break } }
                    out.push(char::from_u32(v).unwrap());
                }
            }
            '8' | '9' if template => return Ok(false),
            _ => out.push(c),
        }
        Ok(true)
    }

    fn string(&mut self, q: char) -> R<T> {
        let st = self.pos;
        self.pos += 1;
        let mut v = String::new();
        loop {
            let Some(c) = self.ch(self.pos) else { return Err("SyntaxError: unterminated string".into()) };
            if c == q { self.pos += 1; break; }
            if c == '\\' { self.pos += 1; self.escape(&mut v, false)?; continue; }
            if c == '\n' || c == '\r' { return Err("SyntaxError: unterminated string".into()); }
            v.push(c); self.pos += c.len_utf8();
        }
        Ok(T::Str { v, raw: self.src[st + 1..self.pos - 1].to_string() })
    }

    /// Template chunk from self.pos (just after '`' or '}'): (cooked or None, tail).
    fn template_chunk(&mut self) -> R<(Option<String>, bool)> {
        let mut v = String::new();
        let mut ok = true;
        loop {
            let Some(c) = self.ch(self.pos) else { return Err("SyntaxError: unterminated template".into()) };
            if c == '`' { self.pos += 1; return Ok((if ok { Some(v) } else { None }, true)); }
            if c == '$' && self.b.get(self.pos + 1) == Some(&b'{') { self.pos += 2; return Ok((if ok { Some(v) } else { None }, false)); }
            if c == '\\' { self.pos += 1; if !self.escape(&mut v, true)? { ok = false; } continue; }
            if c == '\r' { self.pos += 1; if self.b.get(self.pos) == Some(&b'\n') { self.pos += 1; } v.push('\n'); continue; }
            v.push(c); self.pos += c.len_utf8();
        }
    }

    /// The current '/' or '/=' token is a regular expression literal: rescan it (§12.9.5).
    fn rescan_regex(&mut self) -> R<()> {
        let st = self.cur.start;
        let mut p = st + 1;
        let mut class = false;
        loop {
            let c = self.ch(p).ok_or("SyntaxError: unterminated regex")?;
            if is_lt(c) { return Err("SyntaxError: unterminated regex".into()); }
            if c == '\\' { p += 1; let d = self.ch(p).ok_or("SyntaxError: unterminated regex")?; p += d.len_utf8(); continue; }
            if c == '[' { class = true; } else if c == ']' { class = false; } else if c == '/' && !class { break; }
            p += c.len_utf8();
        }
        let pattern = self.src[st + 1..p].to_string();
        p += 1;
        let fs = p;
        while let Some(c) = self.ch(p) { if id_part(c) { p += c.len_utf8(); } else { break; } }
        let flags = self.src[fs..p].to_string();
        self.pos = p;
        self.cur = Tok { t: T::Regex(pattern, flags), start: st, end: p, nl: self.cur.nl };
        Ok(())
    }

    // ---- token helpers ----
    fn is(&self, p: &str) -> bool { matches!(&self.cur.t, T::Punct(x) if *x == p) }
    fn is_name(&self, n: &str) -> bool { matches!(&self.cur.t, T::Name { s, esc: false } if s == n) }
    fn eat(&mut self, p: &str) -> R<bool> { if self.is(p) { self.next()?; Ok(true) } else { Ok(false) } }
    fn eat_name(&mut self, n: &str) -> R<bool> { if self.is_name(n) { self.next()?; Ok(true) } else { Ok(false) } }
    fn expect(&mut self, p: &str) -> R<()> { if self.eat(p)? { Ok(()) } else { self.err(&format!("expected {p}")) } }
    fn semi(&mut self) -> R<()> {
        if self.eat(";")? { return Ok(()); }
        if self.is("}") || self.cur.t == T::Eof || self.cur.nl { return Ok(()); }
        self.err("missing ;")
    }
    /// Peek the token after the current one without consuming (lexer state restored).
    fn peek(&mut self) -> R<Tok> {
        let (pos, cur, pe) = (self.pos, self.cur.clone(), self.prev_end);
        self.next()?;
        let t = self.cur.clone();
        self.pos = pos; self.cur = cur; self.prev_end = pe;
        Ok(t)
    }

    fn ident_name(&mut self) -> R<String> {
        // IdentifierName (any reserved word allowed: property names, `a.class`)
        match self.cur.t.clone() { T::Name { s, .. } => { self.next()?; Ok(s) } _ => self.err("expected a name") }
    }
    fn binding_ident(&mut self) -> R<J> {
        match self.cur.t.clone() {
            T::Name { s, esc } if esc || !RESERVED.contains(&s.as_str()) => { self.next()?; Ok(obj(vec![("type", J::from("Identifier")), ("name", J::from(s))])) }
            _ => self.err("expected an identifier"),
        }
    }

    // ---- program (§16) ----
    pub fn program(&mut self) -> R<J> {
        self.next()?;
        let body = self.stmt_list(true, true)?;
        if self.cur.t != T::Eof { return self.err("unexpected token"); }
        Ok(obj(vec![("type", J::from("Program")), ("body", J::from(body)), ("sourceType", J::from(if self.module { "module" } else { "script" }))]))
    }

    /// Statements until '}' / EOF; `directives` marks the directive prologue (§11.2.1).
    fn stmt_list(&mut self, top: bool, directives: bool) -> R<Vec<J>> {
        let mut v = Vec::new();
        let mut prologue = directives;
        while !self.is("}") && self.cur.t != T::Eof {
            if prologue {
                if let T::Str { raw, .. } = self.cur.t.clone() {
                    let mut s = self.stmt(top)?;
                    if s["type"] == "ExpressionStatement" && s["expression"]["type"] == "Literal" && s["expression"]["value"].is_string()
                        && self.src[..self.prev_end].trim_end().trim_end_matches(';').trim_end().ends_with(|c| c == '"' || c == '\'') {
                        s["directive"] = J::String(raw);
                    } else { prologue = false; }
                    v.push(s);
                    continue;
                }
                prologue = false;
            }
            v.push(self.stmt(top)?);
        }
        Ok(v)
    }

    // ---- statements (§14) ----
    fn stmt(&mut self, top: bool) -> R<J> {
        if let T::Name { s, esc: false } = self.cur.t.clone() {
            match s.as_str() {
                "var" | "const" => { let d = self.var_decl(false)?; self.semi()?; return Ok(d); }
                "let" => {
                    let p = self.peek()?;
                    let decl = match &p.t { T::Name { s, .. } => !(p.nl && (s == "in" || s == "instanceof")) && s != "in" && s != "instanceof", T::Punct("[") | T::Punct("{") => true, _ => false };
                    if decl { let d = self.var_decl(false)?; self.semi()?; return Ok(d); }
                }
                "function" => return self.function(true, false, false),
                "async" => { let p = self.peek()?; if !p.nl && matches!(&p.t, T::Name { s, .. } if s == "function") { self.next()?; return self.function(true, true, false); } }
                "class" => return self.class(true),
                "if" => {
                    self.next()?; self.expect("(")?; let test = self.expr(false)?; self.expect(")")?;
                    let cons = self.stmt(false)?;
                    let alt = if self.eat_name("else")? { self.stmt(false)? } else { J::Null };
                    return Ok(obj(vec![("type", J::from("IfStatement")), ("test", J::from(test)), ("consequent", J::from(cons)), ("alternate", J::from(alt))]));
                }
                "for" => return self.for_stmt(),
                "while" => { self.next()?; self.expect("(")?; let test = self.expr(false)?; self.expect(")")?; let body = self.stmt(false)?; return Ok(obj(vec![("type", J::from("WhileStatement")), ("test", J::from(test)), ("body", J::from(body))])); }
                "do" => {
                    self.next()?; let body = self.stmt(false)?;
                    if !self.eat_name("while")? { return self.err("expected while"); }
                    self.expect("(")?; let test = self.expr(false)?; self.expect(")")?;
                    self.eat(";")?; // §12.10.1: a ; after do-while is always optional
                    return Ok(obj(vec![("type", J::from("DoWhileStatement")), ("body", J::from(body)), ("test", J::from(test))]));
                }
                "return" => {
                    self.next()?;
                    let arg = if self.is(";") || self.is("}") || self.cur.t == T::Eof || self.cur.nl { J::Null } else { self.expr(false)? };
                    self.semi()?;
                    return Ok(obj(vec![("type", J::from("ReturnStatement")), ("argument", J::from(arg))]));
                }
                "break" | "continue" => {
                    self.next()?;
                    let label = if !self.cur.nl && matches!(self.cur.t, T::Name { .. }) { self.binding_ident()? } else { J::Null };
                    self.semi()?;
                    return Ok(obj(vec![("type", J::from(if s == "break" { "BreakStatement" } else { "ContinueStatement" })), ("label", J::from(label))]));
                }
                "throw" => {
                    self.next()?;
                    if self.cur.nl { return self.err("newline after throw"); }
                    let arg = self.expr(false)?; self.semi()?;
                    return Ok(obj(vec![("type", J::from("ThrowStatement")), ("argument", J::from(arg))]));
                }
                "try" => {
                    self.next()?;
                    let block = self.block()?;
                    let mut handler = J::Null; let mut fin = J::Null;
                    if self.eat_name("catch")? {
                        let param = if self.eat("(")? { let p = self.binding_target()?; self.expect(")")?; p } else { J::Null };
                        let body = self.block()?;
                        handler = obj(vec![("type", J::from("CatchClause")), ("param", J::from(param)), ("body", J::from(body))]);
                    }
                    if self.eat_name("finally")? { fin = self.block()?; }
                    if handler.is_null() && fin.is_null() { return self.err("try without catch or finally"); }
                    return Ok(obj(vec![("type", J::from("TryStatement")), ("block", J::from(block)), ("handler", J::from(handler)), ("finalizer", J::from(fin))]));
                }
                "switch" => {
                    self.next()?; self.expect("(")?; let d = self.expr(false)?; self.expect(")")?; self.expect("{")?;
                    let mut cases = Vec::new();
                    while !self.eat("}")? {
                        let test = if self.eat_name("case")? { self.expr(false)? } else if self.eat_name("default")? { J::Null } else { return self.err("expected case"); };
                        self.expect(":")?;
                        let mut cons = Vec::new();
                        while !self.is("}") && !self.is_name("case") && !self.is_name("default") && self.cur.t != T::Eof { cons.push(self.stmt(false)?); }
                        cases.push(obj(vec![("type", J::from("SwitchCase")), ("consequent", J::from(cons)), ("test", J::from(test))]));
                    }
                    return Ok(obj(vec![("type", J::from("SwitchStatement")), ("discriminant", J::from(d)), ("cases", J::from(cases))]));
                }
                "debugger" => { self.next()?; self.semi()?; return Ok(obj(vec![("type", J::from("DebuggerStatement"))])); }
                "with" if !self.module => { self.next()?; self.expect("(")?; let o = self.expr(false)?; self.expect(")")?; let body = self.stmt(false)?; return Ok(obj(vec![("type", J::from("WithStatement")), ("object", J::from(o)), ("body", J::from(body))])); }
                "import" if self.module && top => { let p = self.peek()?; if !matches!(p.t, T::Punct("(") | T::Punct(".")) { return self.import_decl(); } }
                "export" if self.module && top => return self.export_decl(),
                _ => {}
            }
            // LabelledStatement
            if !RESERVED.contains(&s.as_str()) {
                let p = self.peek()?;
                if p.t == T::Punct(":") {
                    let label = self.binding_ident()?; self.next()?;
                    let body = self.stmt(false)?;
                    return Ok(obj(vec![("type", J::from("LabeledStatement")), ("body", J::from(body)), ("label", J::from(label))]));
                }
            }
        }
        if self.is("{") { return self.block(); }
        if self.eat(";")? { return Ok(obj(vec![("type", J::from("EmptyStatement"))])); }
        let e = self.expr(false)?;
        self.semi()?;
        Ok(obj(vec![("type", J::from("ExpressionStatement")), ("expression", J::from(e))]))
    }

    fn block(&mut self) -> R<J> {
        self.expect("{")?;
        let body = self.stmt_list(false, false)?;
        self.expect("}")?;
        Ok(obj(vec![("type", J::from("BlockStatement")), ("body", J::from(body))]))
    }

    fn var_decl(&mut self, no_in: bool) -> R<J> {
        let kind = self.ident_name()?;
        let mut decls = Vec::new();
        loop {
            let id = self.binding_target()?;
            let init = if self.eat("=")? { self.assign(no_in)? } else { J::Null };
            decls.push(obj(vec![("type", J::from("VariableDeclarator")), ("id", J::from(id)), ("init", J::from(init))]));
            if !self.eat(",")? { break; }
        }
        Ok(obj(vec![("type", J::from("VariableDeclaration")), ("declarations", J::from(decls)), ("kind", J::from(kind))]))
    }

    fn for_stmt(&mut self) -> R<J> {
        self.next()?;
        let is_await = if self.is_name("await") && (self.in_async || self.top_await) { self.next()?; true } else { false };
        self.expect("(")?;
        let mut init = J::Null;
        if self.is(";") {
        } else {
            let decl = self.is_name("var") || self.is_name("const") || (self.is_name("let") && { let p = self.peek()?; matches!(p.t, T::Name { .. } | T::Punct("[") | T::Punct("{")) });
            if decl {
                init = self.var_decl(true)?;
            } else {
                let start_let = self.is_name("let");
                let e = self.expr(true)?;
                let _ = start_let;
                if self.is_name("of") || self.is_name("in") { init = self.to_pattern(e, false)?; } else { init = e; }
            }
            if self.is_name("of") || self.is_name("in") {
                let of = self.is_name("of");
                self.next()?;
                let right = if of { self.assign(false)? } else { self.expr(false)? };
                self.expect(")")?;
                let body = self.stmt(false)?;
                return Ok(if of { obj(vec![("type", J::from("ForOfStatement")), ("await", J::from(is_await)), ("left", J::from(init)), ("right", J::from(right)), ("body", J::from(body))]) }
                          else { obj(vec![("type", J::from("ForInStatement")), ("left", J::from(init)), ("right", J::from(right)), ("body", J::from(body))]) });
            }
        }
        self.expect(";")?;
        let test = if self.is(";") { J::Null } else { self.expr(false)? };
        self.expect(";")?;
        let update = if self.is(")") { J::Null } else { self.expr(false)? };
        self.expect(")")?;
        let body = self.stmt(false)?;
        Ok(obj(vec![("type", J::from("ForStatement")), ("init", J::from(init)), ("test", J::from(test)), ("update", J::from(update)), ("body", J::from(body))]))
    }

    fn import_decl(&mut self) -> R<J> {
        self.next()?;
        let mut specs = Vec::new();
        if let T::Str { v, .. } = self.cur.t.clone() {
            self.next()?; let attrs = self.import_attrs()?; self.semi()?;
            return Ok(obj(vec![("type", J::from("ImportDeclaration")), ("specifiers", J::from(specs)), ("source", obj(vec![("type", J::from("Literal")), ("value", J::from(v))])), ("attributes", J::from(attrs))]));
        }
        if matches!(self.cur.t, T::Name { .. }) && !self.is("{") {
            let local = self.binding_ident()?;
            specs.push(obj(vec![("type", J::from("ImportDefaultSpecifier")), ("local", J::from(local))]));
            self.eat(",")?;
        }
        if self.eat("*")? {
            if !self.eat_name("as")? { return self.err("expected as"); }
            let local = self.binding_ident()?;
            specs.push(obj(vec![("type", J::from("ImportNamespaceSpecifier")), ("local", J::from(local))]));
        } else if self.eat("{")? {
            while !self.eat("}")? {
                let imported = self.module_export_name()?;
                let local = if self.eat_name("as")? { self.binding_ident()? } else { imported.clone() };
                specs.push(obj(vec![("type", J::from("ImportSpecifier")), ("imported", J::from(imported)), ("local", J::from(local))]));
                if !self.eat(",")? { self.expect("}")?; break; }
            }
        }
        if !self.eat_name("from")? { return self.err("expected from"); }
        let src = match self.cur.t.clone() { T::Str { v, .. } => { self.next()?; v } _ => return self.err("expected a module specifier") };
        let attrs = self.import_attrs()?;
        self.semi()?;
        Ok(obj(vec![("type", J::from("ImportDeclaration")), ("specifiers", J::from(specs)), ("source", obj(vec![("type", J::from("Literal")), ("value", J::from(src))])), ("attributes", J::from(attrs))]))
    }
    fn import_attrs(&mut self) -> R<Vec<J>> {
        let mut v = Vec::new();
        if self.is_name("with") && !self.cur.nl {
            self.next()?; self.expect("{")?;
            while !self.eat("}")? {
                let key = match self.cur.t.clone() { T::Str { v, .. } => { self.next()?; obj(vec![("type", J::from("Literal")), ("value", J::from(v))]) } _ => obj(vec![("type", J::from("Identifier")), ("name", J::from(self.ident_name()?))]) };
                self.expect(":")?;
                let val = match self.cur.t.clone() { T::Str { v, .. } => { self.next()?; obj(vec![("type", J::from("Literal")), ("value", J::from(v))]) } _ => return self.err("expected a string") };
                v.push(obj(vec![("type", J::from("ImportAttribute")), ("key", J::from(key)), ("value", J::from(val))]));
                if !self.eat(",")? { self.expect("}")?; break; }
            }
        }
        Ok(v)
    }
    fn module_export_name(&mut self) -> R<J> {
        match self.cur.t.clone() { T::Str { v, .. } => { self.next()?; Ok(obj(vec![("type", J::from("Literal")), ("value", J::from(v))])) } _ => Ok(obj(vec![("type", J::from("Identifier")), ("name", J::from(self.ident_name()?))])) }
    }
    fn export_decl(&mut self) -> R<J> {
        self.next()?;
        if self.eat("*")? {
            let exported = if self.eat_name("as")? { self.module_export_name()? } else { J::Null };
            if !self.eat_name("from")? { return self.err("expected from"); }
            let src = match self.cur.t.clone() { T::Str { v, .. } => { self.next()?; v } _ => return self.err("expected a module specifier") };
            let attrs = self.import_attrs()?; self.semi()?;
            return Ok(obj(vec![("type", J::from("ExportAllDeclaration")), ("exported", J::from(exported)), ("source", obj(vec![("type", J::from("Literal")), ("value", J::from(src))])), ("attributes", J::from(attrs))]));
        }
        if self.eat_name("default")? {
            let d = if self.is_name("function") { self.function(true, false, true)? }
                else if self.is_name("async") && { let p = self.peek()?; !p.nl && matches!(&p.t, T::Name { s, .. } if s == "function") } { self.next()?; self.function(true, true, true)? }
                else if self.is_name("class") { self.class_inner(true, true)? }
                else { let e = self.assign(false)?; self.semi()?; e };
            return Ok(obj(vec![("type", J::from("ExportDefaultDeclaration")), ("declaration", J::from(d))]));
        }
        if self.eat("{")? {
            let mut specs = Vec::new();
            while !self.eat("}")? {
                let local = self.module_export_name()?;
                let exported = if self.eat_name("as")? { self.module_export_name()? } else { local.clone() };
                specs.push(obj(vec![("type", J::from("ExportSpecifier")), ("local", J::from(local)), ("exported", J::from(exported))]));
                if !self.eat(",")? { self.expect("}")?; break; }
            }
            let (src, attrs) = if self.eat_name("from")? {
                let s = match self.cur.t.clone() { T::Str { v, .. } => { self.next()?; v } _ => return self.err("expected a module specifier") };
                (obj(vec![("type", J::from("Literal")), ("value", J::from(s))]), self.import_attrs()?)
            } else { (J::Null, vec![]) };
            self.semi()?;
            return Ok(obj(vec![("type", J::from("ExportNamedDeclaration")), ("declaration", J::Null), ("specifiers", J::from(specs)), ("source", J::from(src)), ("attributes", J::from(attrs))]));
        }
        let d = self.stmt(false)?;
        Ok(obj(vec![("type", J::from("ExportNamedDeclaration")), ("declaration", J::from(d)), ("specifiers", J::Array(vec![])), ("source", J::Null), ("attributes", J::Array(vec![]))]))
    }

    // ---- functions and classes (§15) ----
    fn function(&mut self, decl: bool, is_async: bool, default_export: bool) -> R<J> {
        self.next()?; // `function`
        let is_gen = self.eat("*")?;
        let id = if matches!(self.cur.t, T::Name { .. }) && !self.is("(") { self.binding_ident()? } else if decl && !default_export { return self.err("function name required"); } else { J::Null };
        let (params, body) = self.fn_rest(is_async, is_gen)?;
        Ok(crate::jscomp::lazy_fn(obj(vec![("type", J::from(if decl { "FunctionDeclaration" } else { "FunctionExpression" })), ("id", J::from(id)), ("expression", J::from(false)), ("generator", J::from(is_gen)), ("async", J::from(is_async)), ("params", J::from(params)), ("body", J::from(body))])))
    }

    fn fn_rest(&mut self, is_async: bool, is_gen: bool) -> R<(Vec<J>, J)> {
        let saved = (self.in_fn, self.in_async, self.in_gen, self.top_await);
        self.in_fn = true; self.in_async = is_async; self.in_gen = is_gen; self.top_await = false;
        let r = (|| -> R<(Vec<J>, J)> {
            self.expect("(")?;
            let params = self.params()?;
            let body = self.fn_body()?;
            Ok((params, body))
        })();
        (self.in_fn, self.in_async, self.in_gen, self.top_await) = saved;
        r
    }
    fn params(&mut self) -> R<Vec<J>> {
        let mut v = Vec::new();
        while !self.eat(")")? {
            if self.eat("...")? { let a = self.binding_target()?; v.push(obj(vec![("type", J::from("RestElement")), ("argument", J::from(a))])); self.eat(",")?; self.expect(")")?; break; }
            let mut p = self.binding_target()?;
            if self.eat("=")? { let r = self.assign(false)?; p = obj(vec![("type", J::from("AssignmentPattern")), ("left", J::from(p)), ("right", J::from(r))]); }
            v.push(p);
            if !self.eat(",")? { self.expect(")")?; break; }
        }
        Ok(v)
    }
    fn fn_body(&mut self) -> R<J> {
        self.expect("{")?;
        let body = self.stmt_list(false, true)?;
        self.expect("}")?;
        Ok(obj(vec![("type", J::from("BlockStatement")), ("body", J::from(body))]))
    }

    /// BindingIdentifier | BindingPattern (§14.3.3)
    fn binding_target(&mut self) -> R<J> {
        if self.is("[") || self.is("{") {
            let e = self.primary()?;
            return self.to_pattern(e, true);
        }
        self.binding_ident()
    }

    fn class(&mut self, decl: bool) -> R<J> { self.class_inner(decl, false) }
    fn class_inner(&mut self, decl: bool, default_export: bool) -> R<J> {
        self.next()?; // `class`
        let id = if matches!(self.cur.t, T::Name { .. }) && !self.is_name("extends") { self.binding_ident()? } else if decl && !default_export { return self.err("class name required"); } else { J::Null };
        let sup = if self.eat_name("extends")? { self.lhs()? } else { J::Null };
        self.expect("{")?;
        let mut body = Vec::new();
        while !self.eat("}")? {
            if self.eat(";")? { continue; }
            body.push(self.class_member()?);
        }
        Ok(obj(vec![("type", J::from(if decl { "ClassDeclaration" } else { "ClassExpression" })), ("id", J::from(id)), ("superClass", J::from(sup)), ("body", obj(vec![("type", J::from("ClassBody")), ("body", J::from(body))]))]))
    }

    /// A name word `w` used as a key when what follows does not continue a modifier (`static() {}`, `get = 1`, `async;`).
    fn modifier_is_key(&mut self) -> R<bool> {
        let p = self.peek()?;
        Ok(matches!(p.t, T::Punct("(") | T::Punct("=") | T::Punct(";") | T::Punct("}")) || p.t == T::Eof)
    }

    fn class_member(&mut self) -> R<J> {
        let mut is_static = false;
        if self.is_name("static") && !self.modifier_is_key()? {
            self.next()?;
            if self.is("{") {
                let saved = (self.in_fn, self.in_async, self.in_gen);
                self.in_fn = true; self.in_async = false; self.in_gen = false;
                self.next()?; let b = self.stmt_list(false, false); (self.in_fn, self.in_async, self.in_gen) = saved; let b = b?; self.expect("}")?;
                return Ok(obj(vec![("type", J::from("StaticBlock")), ("body", J::from(b))]));
            }
            is_static = true;
        }
        let (kind, is_async, is_gen, key, computed) = self.method_head()?;
        if self.is("(") {
            let is_ctor = !is_static && !computed && kind == "method" && (key["name"] == "constructor" || key["value"] == "constructor") && key["type"] != "PrivateIdentifier";
            let (params, body) = self.fn_rest(is_async, is_gen)?;
            let value = crate::jscomp::lazy_fn(obj(vec![("type", J::from("FunctionExpression")), ("id", J::Null), ("expression", J::from(false)), ("generator", J::from(is_gen)), ("async", J::from(is_async)), ("params", J::from(params)), ("body", J::from(body))]));
            return Ok(obj(vec![("type", J::from("MethodDefinition")), ("static", J::from(is_static)), ("computed", J::from(computed)), ("key", J::from(key)), ("kind", J::from(if is_ctor { "constructor" } else { kind })), ("value", J::from(value))]));
        }
        // field (PropertyDefinition)
        let value = if self.eat("=")? {
            let saved = (self.in_fn, self.in_async, self.in_gen);
            self.in_fn = true; self.in_async = false; self.in_gen = false;
            let v = self.assign(false); (self.in_fn, self.in_async, self.in_gen) = saved; v?
        } else { J::Null };
        self.semi()?;
        Ok(obj(vec![("type", J::from("PropertyDefinition")), ("static", J::from(is_static)), ("computed", J::from(computed)), ("key", J::from(key)), ("value", J::from(value))]))
    }

    /// Method / accessor / property head shared by classes and object literals: (kind, async, generator, key, computed).
    fn method_head(&mut self) -> R<(&'static str, bool, bool, J, bool)> {
        let mut kind = "method"; let mut is_async = false; let mut is_gen = false;
        if self.is_name("async") && !self.modifier_is_key()? && !self.is(":") && !{ let p = self.peek()?; p.nl || matches!(p.t, T::Punct(",") | T::Punct(":")) } {
            self.next()?; is_async = true;
        }
        if self.eat("*")? { is_gen = true; }
        if !is_async && !is_gen && (self.is_name("get") || self.is_name("set")) && !self.modifier_is_key()? && !{ let p = self.peek()?; matches!(p.t, T::Punct(",") | T::Punct(":")) } {
            kind = if self.is_name("get") { "get" } else { "set" };
            self.next()?;
        }
        let (key, computed) = self.prop_key()?;
        Ok((kind, is_async, is_gen, key, computed))
    }

    fn prop_key(&mut self) -> R<(J, bool)> {
        match self.cur.t.clone() {
            T::Punct("[") => { self.next()?; let e = self.assign(false)?; self.expect("]")?; Ok((e, true)) }
            T::Str { v, .. } => { self.next()?; Ok((obj(vec![("type", J::from("Literal")), ("value", J::from(v))]), false)) }
            T::Num(n, big) => { self.next()?; let mut l = obj(vec![("type", J::from("Literal")), ("value", J::from(num_json(n)))]); if let Some(b) = big { l["bigint"] = J::String(b); } Ok((l, false)) }
            T::Priv(s) => { self.next()?; Ok((obj(vec![("type", J::from("PrivateIdentifier")), ("name", J::from(s))]), false)) }
            T::Name { s, .. } => { self.next()?; Ok((obj(vec![("type", J::from("Identifier")), ("name", J::from(s))]), false)) }
            _ => self.err("expected a property name"),
        }
    }

    // ---- expressions (§13) ----
    pub fn expr(&mut self, no_in: bool) -> R<J> {
        let e = self.assign(no_in)?;
        if self.is(",") {
            let mut v = vec![e];
            while self.eat(",")? { v.push(self.assign(no_in)?); }
            return Ok(obj(vec![("type", J::from("SequenceExpression")), ("expressions", J::from(v))]));
        }
        Ok(e)
    }

    fn arrow_params_ok(e: &J) -> bool { matches!(e["type"].as_str(), Some("Identifier")) }

    fn assign(&mut self, no_in: bool) -> R<J> {
        // yield (§15.5)
        if self.in_gen && self.is_name("yield") {
            self.next()?;
            if self.cur.nl || matches!(self.cur.t, T::Punct(")") | T::Punct("]") | T::Punct("}") | T::Punct(",") | T::Punct(";") | T::Punct(":")) || self.cur.t == T::Eof {
                return Ok(obj(vec![("type", J::from("YieldExpression")), ("delegate", J::from(false)), ("argument", J::Null)]));
            }
            let delegate = self.eat("*")?;
            let a = self.assign(no_in)?;
            return Ok(obj(vec![("type", J::from("YieldExpression")), ("delegate", J::from(delegate)), ("argument", J::from(a))]));
        }
        // async arrow: `async x =>`, `async (a) =>`
        if self.is_name("async") {
            let p = self.peek()?;
            if !p.nl {
                if let T::Name { s, .. } = &p.t {
                    if s != "function" {
                        let (pos, cur, pe) = (self.pos, self.cur.clone(), self.prev_end);
                        self.next()?;
                        let id = self.binding_ident()?;
                        if self.is("=>") && !self.cur.nl { return self.arrow(vec![id], true); }
                        self.pos = pos; self.cur = cur; self.prev_end = pe;
                    }
                }
            }
        }
        let start_paren = self.is("(");
        let start_async_call = self.is_name("async") && { let p = self.peek()?; !p.nl && p.t == T::Punct("(") };
        let left = self.cond(no_in)?;
        if self.is("=>") && !self.cur.nl {
            if left.get("__parens").is_some() || start_paren {
                let mut left = left;
                let items = match left.as_object_mut().and_then(|m| m.remove("__parens")) { Some(J::Array(v)) => v, _ => vec![left] };
                let params = items.into_iter().map(|x| self.to_pattern(x, true)).collect::<R<Vec<_>>>()?;
                return self.arrow(params, false);
            }
            if start_async_call && left["type"] == "CallExpression" {
                let mut left = left;
                let params = match left["arguments"].take() { J::Array(v) => v, _ => vec![] }.into_iter().map(|x| self.to_pattern(x, true)).collect::<R<Vec<_>>>()?;
                return self.arrow(params, true);
            }
            if Self::arrow_params_ok(&left) { return self.arrow(vec![left], false); }
            return self.err("bad arrow parameters");
        }
        let left = self.unparen(left)?;
        if let T::Punct(op) = self.cur.t.clone() {
            if matches!(op, "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "**=" | "<<=" | ">>=" | ">>>=" | "&=" | "|=" | "^=" | "&&=" | "||=" | "??=") {
                let target = if op == "=" && (left["type"] == "ObjectExpression" || left["type"] == "ArrayExpression") { self.to_pattern(left, false)? } else { left };
                self.next()?;
                let right = self.assign(no_in)?;
                return Ok(obj(vec![("type", J::from("AssignmentExpression")), ("operator", J::from(op)), ("left", J::from(target)), ("right", J::from(right))]));
            }
        }
        Ok(left)
    }

    /// A parenthesised group parsed for a possible arrow head: `{"__parens": [items]}`; anything else → the expression.
    fn unparen(&mut self, e: J) -> R<J> {
        match e {
            J::Object(mut m) if m.contains_key("__parens") => {
                let Some(J::Array(mut items)) = m.remove("__parens") else { return self.err("bad group") };
                if items.is_empty() || items.iter().any(|x| x["type"] == "RestElement") { return self.err("unexpected ()"); }
                if items.len() == 1 { return Ok(items.pop().unwrap()); }
                Ok(obj(vec![("type", J::from("SequenceExpression")), ("expressions", J::from(items))]))
            }
            e => Ok(e),
        }
    }

    fn arrow(&mut self, params: Vec<J>, is_async: bool) -> R<J> {
        self.expect("=>")?;
        let saved = (self.in_fn, self.in_async, self.in_gen, self.top_await);
        self.in_fn = true; self.in_async = is_async; self.in_gen = false; self.top_await = false;
        let r = if self.is("{") { self.fn_body().map(|b| (b, false)) } else { self.assign(false).map(|e| (e, true)) };
        (self.in_fn, self.in_async, self.in_gen, self.top_await) = saved;
        let (body, expression) = r?;
        Ok(crate::jscomp::lazy_fn(obj(vec![("type", J::from("ArrowFunctionExpression")), ("id", J::Null), ("expression", J::from(expression)), ("generator", J::from(false)), ("async", J::from(is_async)), ("params", J::from(params)), ("body", J::from(body))])))
    }

    /// Expression → pattern (§13.15.5 destructuring assignment, §15.3 arrow parameters).
    fn to_pattern(&mut self, mut e: J, binding: bool) -> R<J> {
        if e.get("__parens").is_some() { let x = self.unparen(e)?; return self.to_pattern(x, binding); }
        let t = e["type"].as_str().unwrap_or("").to_string();
        Ok(match t.as_str() {
            "Identifier" => e,
            "MemberExpression" if !binding => e,
            "AssignmentExpression" if e["operator"] == "=" => { let l = self.to_pattern(e["left"].take(), binding)?; obj(vec![("type", J::from("AssignmentPattern")), ("left", J::from(l)), ("right", J::from(e["right"].take()))]) }
            // already converted as an assignment target inside the parenthesised head (`({ a } = {}) =>`)
            "AssignmentPattern" | "ObjectPattern" | "ArrayPattern" => e,
            "ArrayExpression" => {
                let els = match e["elements"].take() { J::Array(v) => v, _ => vec![] };
                let mut v = Vec::with_capacity(els.len());
                for mut x in els { v.push(if x.is_null() { J::Null } else if x["type"] == "SpreadElement" { obj(vec![("type", J::from("RestElement")), ("argument", J::from(self.to_pattern(x["argument"].take(), binding)?))]) } else { self.to_pattern(x, binding)? }); }
                obj(vec![("type", J::from("ArrayPattern")), ("elements", J::from(v))])
            }
            "ObjectExpression" => {
                let props = match e["properties"].take() { J::Array(v) => v, _ => vec![] };
                let mut v = Vec::with_capacity(props.len());
                for mut p in props {
                    if p["type"] == "SpreadElement" { v.push(obj(vec![("type", J::from("RestElement")), ("argument", J::from(self.to_pattern(p["argument"].take(), binding)?))])); continue; }
                    let val = p["value"].take();
                    p["value"] = self.to_pattern(val, binding)?;
                    v.push(p);
                }
                obj(vec![("type", J::from("ObjectPattern")), ("properties", J::from(v))])
            }
            // `...x` as a parameter: `async(...e) =>` arrives as the spread argument of a call
            "RestElement" | "SpreadElement" => { let a = self.to_pattern(e["argument"].take(), binding)?; obj(vec![("type", J::from("RestElement")), ("argument", J::from(a))]) }
            _ => return self.err("invalid assignment target"),
        })
    }

    fn cond(&mut self, no_in: bool) -> R<J> {
        let test = self.binary(0, no_in)?;
        if self.eat("?")? {
            let test = self.unparen(test)?;
            let cons = self.assign(false)?;
            self.expect(":")?;
            let alt = self.assign(no_in)?;
            return Ok(obj(vec![("type", J::from("ConditionalExpression")), ("test", J::from(test)), ("consequent", J::from(cons)), ("alternate", J::from(alt))]));
        }
        Ok(test)
    }

    fn bin_prec(&self, no_in: bool) -> Option<(&'static str, u8)> {
        let op: &'static str = match &self.cur.t {
            T::Punct(p) => p,
            T::Name { s, esc: false } if s == "instanceof" => "instanceof",
            T::Name { s, esc: false } if s == "in" && !no_in => "in",
            _ => return None,
        };
        Some((op, match op {
            "??" => 1, "||" => 2, "&&" => 3, "|" => 4, "^" => 5, "&" => 6, "==" | "!=" | "===" | "!==" => 7,
            "<" | ">" | "<=" | ">=" | "instanceof" | "in" => 8, "<<" | ">>" | ">>>" => 9, "+" | "-" => 10, "*" | "/" | "%" => 11, "**" => 12,
            _ => return None,
        }))
    }

    fn binary(&mut self, min: u8, no_in: bool) -> R<J> {
        let mut left = if let T::Priv(s) = self.cur.t.clone() {
            // `#x in obj` (§13.10.1 RelationalExpression : PrivateIdentifier in ShiftExpression)
            self.next()?; obj(vec![("type", J::from("PrivateIdentifier")), ("name", J::from(s))])
        } else { self.unary()? };
        while let Some((op, p)) = self.bin_prec(no_in) {
            if p <= min { break; } // left-assoc: equal precedence ends this level; `**` recurses at p - 1 (right-assoc)
            left = self.unparen(left)?;
            self.next()?;
            let right = if op == "**" { self.binary(p - 1, no_in)? } else { self.binary(p, no_in)? };
            let right = self.unparen(right)?;
            let ty = if matches!(op, "||" | "&&" | "??") { "LogicalExpression" } else { "BinaryExpression" };
            left = obj(vec![("type", J::from(ty)), ("left", J::from(left)), ("operator", J::from(op)), ("right", J::from(right))]);
        }
        Ok(left)
    }

    fn unary(&mut self) -> R<J> {
        let op: Option<&'static str> = match &self.cur.t {
            T::Punct(p @ ("!" | "~" | "+" | "-")) => Some(p),
            T::Name { s, esc: false } if s == "typeof" => Some("typeof"),
            T::Name { s, esc: false } if s == "void" => Some("void"),
            T::Name { s, esc: false } if s == "delete" => Some("delete"),
            _ => None,
        };
        if let Some(op) = op {
            self.next()?;
            let a = self.unary()?;
            let a = self.unparen(a)?;
            return Ok(obj(vec![("type", J::from("UnaryExpression")), ("operator", J::from(op)), ("prefix", J::from(true)), ("argument", J::from(a))]));
        }
        if self.is("++") || self.is("--") {
            let op = if self.is("++") { "++" } else { "--" };
            self.next()?;
            let a = self.unary()?;
            let a = self.unparen(a)?;
            return Ok(obj(vec![("type", J::from("UpdateExpression")), ("operator", J::from(op)), ("prefix", J::from(true)), ("argument", J::from(a))]));
        }
        if self.is_name("await") && (self.in_async || (self.top_await && !self.in_fn)) {
            self.next()?;
            let a = self.unary()?;
            let a = self.unparen(a)?;
            return Ok(obj(vec![("type", J::from("AwaitExpression")), ("argument", J::from(a))]));
        }
        let e = self.lhs()?;
        if (self.is("++") || self.is("--")) && !self.cur.nl {
            let op = if self.is("++") { "++" } else { "--" };
            self.next()?;
            let e = self.unparen(e)?;
            return Ok(obj(vec![("type", J::from("UpdateExpression")), ("operator", J::from(op)), ("prefix", J::from(false)), ("argument", J::from(e))]));
        }
        Ok(e)
    }

    fn args(&mut self) -> R<Vec<J>> {
        // after '('
        let mut v = Vec::new();
        while !self.eat(")")? {
            if self.eat("...")? { let a = self.assign(false)?; v.push(obj(vec![("type", J::from("SpreadElement")), ("argument", J::from(a))])); }
            else { v.push(self.assign(false)?); }
            if !self.eat(",")? { self.expect(")")?; break; }
        }
        Ok(v)
    }

    /// LeftHandSideExpression: new / call / member / optional chains / tagged templates (§13.3).
    fn lhs(&mut self) -> R<J> {
        let mut e = if self.is_name("new") { self.new_expr()? } else { self.primary()? };
        let mut chain = false;
        loop {
            if self.is(".") {
                self.next()?;
                e = self.unparen(e)?;
                let prop = if let T::Priv(s) = self.cur.t.clone() { self.next()?; obj(vec![("type", J::from("PrivateIdentifier")), ("name", J::from(s))]) } else { obj(vec![("type", J::from("Identifier")), ("name", J::from(self.ident_name()?))]) };
                e = obj(vec![("type", J::from("MemberExpression")), ("object", J::from(e)), ("property", J::from(prop)), ("computed", J::from(false)), ("optional", J::from(false))]);
            } else if self.is("?.") {
                self.next()?;
                chain = true;
                e = self.unparen(e)?;
                if self.eat("(")? { let a = self.args()?; e = obj(vec![("type", J::from("CallExpression")), ("callee", J::from(e)), ("arguments", J::from(a)), ("optional", J::from(true))]); }
                else if self.eat("[")? { let p = self.expr(false)?; self.expect("]")?; e = obj(vec![("type", J::from("MemberExpression")), ("object", J::from(e)), ("property", J::from(p)), ("computed", J::from(true)), ("optional", J::from(true))]); }
                else {
                    let prop = if let T::Priv(s) = self.cur.t.clone() { self.next()?; obj(vec![("type", J::from("PrivateIdentifier")), ("name", J::from(s))]) } else { obj(vec![("type", J::from("Identifier")), ("name", J::from(self.ident_name()?))]) };
                    e = obj(vec![("type", J::from("MemberExpression")), ("object", J::from(e)), ("property", J::from(prop)), ("computed", J::from(false)), ("optional", J::from(true))]);
                }
            } else if self.is("[") {
                self.next()?;
                e = self.unparen(e)?;
                let p = self.expr(false)?; self.expect("]")?;
                e = obj(vec![("type", J::from("MemberExpression")), ("object", J::from(e)), ("property", J::from(p)), ("computed", J::from(true)), ("optional", J::from(false))]);
            } else if self.is("(") {
                self.next()?;
                if e.get("__parens").is_some() { e = self.unparen(e)?; }
                let a = self.args()?;
                e = obj(vec![("type", J::from("CallExpression")), ("callee", J::from(e)), ("arguments", J::from(a)), ("optional", J::from(false))]);
            } else if self.cur.t == T::Template {
                if chain { return self.err("tagged template in an optional chain"); }
                e = self.unparen(e)?;
                let q = self.template(true)?;
                e = obj(vec![("type", J::from("TaggedTemplateExpression")), ("tag", J::from(e)), ("quasi", J::from(q))]);
            } else { break; }
        }
        if chain { e = obj(vec![("type", J::from("ChainExpression")), ("expression", J::from(e))]); }
        Ok(e)
    }

    fn new_expr(&mut self) -> R<J> {
        self.next()?; // new
        if self.eat(".")? {
            let p = self.ident_name()?;
            return Ok(obj(vec![("type", J::from("MetaProperty")), ("meta", obj(vec![("type", J::from("Identifier")), ("name", J::from("new"))])), ("property", obj(vec![("type", J::from("Identifier")), ("name", J::from(p))]))]));
        }
        let mut callee = if self.is_name("new") { self.new_expr()? } else { self.primary()? };
        callee = self.unparen(callee)?;
        loop {
            if self.eat(".")? {
                let prop = if let T::Priv(s) = self.cur.t.clone() { self.next()?; obj(vec![("type", J::from("PrivateIdentifier")), ("name", J::from(s))]) } else { obj(vec![("type", J::from("Identifier")), ("name", J::from(self.ident_name()?))]) };
                callee = obj(vec![("type", J::from("MemberExpression")), ("object", J::from(callee)), ("property", J::from(prop)), ("computed", J::from(false)), ("optional", J::from(false))]);
            } else if self.eat("[")? {
                let p = self.expr(false)?; self.expect("]")?;
                callee = obj(vec![("type", J::from("MemberExpression")), ("object", J::from(callee)), ("property", J::from(p)), ("computed", J::from(true)), ("optional", J::from(false))]);
            } else if self.cur.t == T::Template {
                let q = self.template(true)?;
                callee = obj(vec![("type", J::from("TaggedTemplateExpression")), ("tag", J::from(callee)), ("quasi", J::from(q))]);
            } else { break; }
        }
        let args = if self.eat("(")? { self.args()? } else { vec![] };
        Ok(obj(vec![("type", J::from("NewExpression")), ("callee", J::from(callee)), ("arguments", J::from(args))]))
    }

    fn template(&mut self, tagged: bool) -> R<J> {
        // cur = the opening '`'; the lexer position is just after it
        let mut quasis = Vec::new(); let mut exprs = Vec::new();
        self.pos = self.cur.end;
        loop {
            let (cooked, tail) = self.template_chunk()?;
            if cooked.is_none() && !tagged { return self.err("bad escape in template"); }
            quasis.push(obj(vec![("type", J::from("TemplateElement")), ("value", obj(vec![("cooked", J::from(cooked))])), ("tail", J::from(tail))]));
            if tail { break; }
            self.next()?;
            exprs.push(self.expr(false)?);
            if !self.is("}") { return self.err("expected } in template"); }
            self.pos = self.cur.end;
        }
        self.cur = Tok { t: T::Punct(";"), start: self.pos, end: self.pos, nl: false }; // placeholder, replaced by next()
        self.next()?;
        Ok(obj(vec![("type", J::from("TemplateLiteral")), ("expressions", J::from(exprs)), ("quasis", J::from(quasis))]))
    }

    fn primary(&mut self) -> R<J> {
        let t = self.cur.t.clone();
        match t {
            T::Num(n, big) => { self.next()?; let mut l = obj(vec![("type", J::from("Literal")), ("value", J::from(num_json(n)))]); if let Some(b) = big { l["bigint"] = J::String(b); } Ok(l) }
            T::Str { v, .. } => { self.next()?; Ok(obj(vec![("type", J::from("Literal")), ("value", J::from(v))])) }
            T::Template => self.template(false),
            T::Punct("/") | T::Punct("/=") => {
                self.rescan_regex()?;
                let T::Regex(p, f) = self.cur.t.clone() else { unreachable!() };
                self.next()?;
                Ok(obj(vec![("type", J::from("Literal")), ("value", J::Null), ("regex", obj(vec![("pattern", J::from(p)), ("flags", J::from(f))]))]))
            }
            T::Punct("[") => {
                self.next()?;
                let mut els = Vec::new();
                while !self.eat("]")? {
                    if self.is(",") { self.next()?; els.push(J::Null); continue; }
                    let e = if self.eat("...")? { let a = self.assign(false)?; obj(vec![("type", J::from("SpreadElement")), ("argument", J::from(a))]) } else { self.assign(false)? };
                    els.push(e);
                    if !self.eat(",")? { self.expect("]")?; break; }
                }
                Ok(obj(vec![("type", J::from("ArrayExpression")), ("elements", J::from(els))]))
            }
            T::Punct("{") => self.object(),
            T::Punct("(") => {
                // parenthesised expression or arrow parameters (cover grammar §13.2.1)
                self.next()?;
                let mut items = Vec::new();
                while !self.eat(")")? {
                    if self.eat("...")? { let a = self.binding_target()?; items.push(obj(vec![("type", J::from("RestElement")), ("argument", J::from(a))])); self.eat(",")?; self.expect(")")?; break; }
                    let e = self.assign(false)?;
                    items.push(e);
                    if !self.eat(",")? { self.expect(")")?; break; }
                }
                if self.is("=>") && !self.cur.nl { return Ok(obj(vec![("__parens", J::from(items))])); }
                if items.len() == 1 && items[0]["type"] != "RestElement" { return Ok(obj(vec![("__parens", J::from(items))])); }
                self.unparen(obj(vec![("__parens", J::from(items))]))
            }
            T::Name { s, esc } => {
                if !esc {
                    match s.as_str() {
                        "function" => return self.function(false, false, false),
                        "async" => { let p = self.peek()?; if !p.nl && matches!(&p.t, T::Name { s, .. } if s == "function") { self.next()?; return self.function(false, true, false); } }
                        "class" => return self.class(false),
                        "this" => { self.next()?; return Ok(obj(vec![("type", J::from("ThisExpression"))])); }
                        "null" => { self.next()?; return Ok(obj(vec![("type", J::from("Literal")), ("value", J::Null)])); }
                        "true" | "false" => { self.next()?; return Ok(obj(vec![("type", J::from("Literal")), ("value", J::from(s == "true"))])); }
                        "super" => { self.next()?; return Ok(obj(vec![("type", J::from("Super"))])); }
                        "new" => return self.new_expr(),
                        "import" => {
                            self.next()?;
                            if self.eat(".")? { let p = self.ident_name()?; return Ok(obj(vec![("type", J::from("MetaProperty")), ("meta", obj(vec![("type", J::from("Identifier")), ("name", J::from("import"))])), ("property", obj(vec![("type", J::from("Identifier")), ("name", J::from(p))]))])); }
                            self.expect("(")?;
                            let src = self.assign(false)?;
                            let opts = if self.eat(",")? && !self.is(")") { let o = self.assign(false)?; self.eat(",")?; o } else { J::Null };
                            self.expect(")")?;
                            return Ok(obj(vec![("type", J::from("ImportExpression")), ("source", J::from(src)), ("options", J::from(opts))]));
                        }
                        _ => {}
                    }
                    if RESERVED.contains(&s.as_str()) { return self.err("unexpected keyword"); }
                }
                self.next()?;
                Ok(obj(vec![("type", J::from("Identifier")), ("name", J::from(s))]))
            }
            _ => self.err("unexpected token"),
        }
    }

    fn object(&mut self) -> R<J> {
        self.next()?; // {
        let mut props = Vec::new();
        while !self.eat("}")? {
            if self.eat("...")? { let a = self.assign(false)?; props.push(obj(vec![("type", J::from("SpreadElement")), ("argument", J::from(a))])); }
            else {
                let shorthand_name = match &self.cur.t { T::Name { s, .. } => Some(s.clone()), _ => None };
                let (kind, is_async, is_gen, key, computed) = self.method_head()?;
                if self.is("(") {
                    let (params, body) = self.fn_rest(is_async, is_gen)?;
                    let value = crate::jscomp::lazy_fn(obj(vec![("type", J::from("FunctionExpression")), ("id", J::Null), ("expression", J::from(false)), ("generator", J::from(is_gen)), ("async", J::from(is_async)), ("params", J::from(params)), ("body", J::from(body))]));
                    props.push(obj(vec![("type", J::from("Property")), ("method", J::from(kind == "method")), ("shorthand", J::from(false)), ("computed", J::from(computed)), ("key", J::from(key)), ("kind", J::from(if kind == "method" { "init" } else { kind })), ("value", J::from(value))]));
                } else if self.eat(":")? {
                    let value = self.assign(false)?;
                    props.push(obj(vec![("type", J::from("Property")), ("method", J::from(false)), ("shorthand", J::from(false)), ("computed", J::from(computed)), ("key", J::from(key)), ("value", J::from(value)), ("kind", J::from("init"))]));
                } else if key["type"] == "Identifier" && !computed && kind == "method" && !is_async && !is_gen && shorthand_name.is_some() {
                    let mut value = key.clone();
                    if self.eat("=")? { let d = self.assign(false)?; value = obj(vec![("type", J::from("AssignmentExpression")), ("operator", J::from("=")), ("left", J::from(value)), ("right", J::from(d))]); }
                    props.push(obj(vec![("type", J::from("Property")), ("method", J::from(false)), ("shorthand", J::from(true)), ("computed", J::from(false)), ("key", J::from(key)), ("kind", J::from("init")), ("value", J::from(value))]));
                } else { return self.err("bad object literal"); }
            }
            if !self.eat(",")? { self.expect("}")?; break; }
        }
        Ok(obj(vec![("type", J::from("ObjectExpression")), ("properties", J::from(props))]))
    }
}

/// Parse a classic script (`module` = false) or a module to the estree IR.
pub fn parse(src: &str, module: bool) -> Result<J, String> { let was = crate::jscomp::set_compact(std::env::var_os("OMR_COMPACT").map(|v| v != "0").unwrap_or(true)); let r = Parser::new(src, module).program(); crate::jscomp::set_compact(was); r }
/// The whole tree, no placeholders (oracle parity, printing the IR).
pub fn parse_full(src: &str, module: bool) -> Result<J, String> { let was = crate::jscomp::set_compact(false); let r = Parser::new(src, module).program(); crate::jscomp::set_compact(was); r }

/// `omni-render js-parse <file> [--module]` prints the IR; `--parity <oracle.jsonl>` compares with js2ir output lines
/// {"file", "module", "ir"} and prints per-file agreement.
pub fn cli(a: &[String]) {
    if a.first().map(|s| s.as_str()) == Some("--parity") {
        let text = std::fs::read_to_string(&a[1]).expect("oracle jsonl");
        let (mut same, mut diff, mut ours_err, mut both_err, mut total) = (0, 0, 0, 0, 0);
        let mut bytes = 0usize; let mut ms = 0f64;
        for line in text.lines() {
            let Ok(r) = serde_json::from_str::<J>(line) else { continue };
            let file = r["file"].as_str().unwrap_or("");
            let Ok(src) = std::fs::read_to_string(file) else { continue };
            total += 1;
            let t = std::time::Instant::now();
            let ours = parse_full(&src, r["module"].as_bool().unwrap_or(false));
            ms += t.elapsed().as_secs_f64() * 1e3; bytes += src.len();
            match (ours, r.get("ir")) {
                (Ok(o), Some(want)) if !want.is_null() => {
                    if norm(&o) == norm(want) { same += 1; } else { diff += 1; println!("DIFF {file}: {}", first_diff(&norm(&o), &norm(want), "$")); }
                }
                (Err(e), Some(want)) if !want.is_null() => { ours_err += 1; println!("OURS-ERR {file}: {e}"); }
                (Err(_), _) => both_err += 1,
                (Ok(_), _) => { diff += 1; println!("ORACLE-ERR-ONLY {file}"); }
            }
        }
        println!("{}", obj(vec![("files", J::from(total)), ("same", J::from(same)), ("different", J::from(diff)), ("ours_error", J::from(ours_err)), ("both_error", J::from(both_err)), ("kb", J::from(bytes / 1024)), ("parse_ms", J::from((ms * 10.0).round() / 10.0)), ("mb_per_s", J::from(((bytes as f64 / 1e6) / (ms / 1e3).max(1e-9) * 10.0).round() / 10.0))]));
        return;
    }
    if a.first().map(|s| s.as_str()) == Some("--feedcheck") {
        // parse (compact) → dump → fresh table → load → expand must equal the whole-tree parse, for every corpus file
        let text = std::fs::read_to_string(&a[1]).expect("oracle jsonl");
        let (mut ok, mut bad, mut bytes) = (0, 0, 0usize);
        for line in text.lines() {
            let Ok(r) = serde_json::from_str::<J>(line) else { continue };
            let Ok(src) = std::fs::read_to_string(r["file"].as_str().unwrap_or("")) else { continue };
            let module = r["module"].as_bool().unwrap_or(false);
            let Ok(full) = parse_full(&src, module) else { continue };
            crate::jscomp::begin();
            let Ok(prog) = parse(&src, module) else { bad += 1; continue };
            let blob = crate::jscomp::dump(&prog); bytes += blob.len();
            crate::jscomp::begin();
            // a second script already in the table, so the loaded one has a non-zero base
            let _ = parse("function pad(){return 1}", false);
            let loaded = crate::jscomp::load(&blob);
            match loaded { Some(l) if crate::jscomp::expand(&l) == full => ok += 1, _ => { bad += 1; println!("FEED-DIFF {}", r["file"]); } }
        }
        println!("{{\"feedcheck_ok\":{ok},\"bad\":{bad},\"blob_kb\":{}}}", bytes / 1024);
        return;
    }
    let src = std::fs::read_to_string(&a[0]).expect("file");
    if a.iter().any(|x| x == "--lex") {
        let t = std::time::Instant::now();
        let mut p = Parser::new(&src, false);
        let mut n = 0; while p.next().is_ok() && p.cur.t != T::Eof { n += 1; }
        let l = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        let r = parse(&src, a.iter().any(|x| x == "--module"));
        println!("tokens {n} lex {l:.1} ms · parse {:.1} ms ({}) · compact flags/compact/stash s = {:?}", t.elapsed().as_secs_f64() * 1e3, if r.is_ok() { "ok" } else { "err" }, crate::jscomp::TIMES.with(|t| t.get()));
        return;
    }
    match parse_full(&src, a.iter().any(|x| x == "--module")) { Ok(j) => println!("{j}"), Err(e) => { eprintln!("{e}"); std::process::exit(1); } }
}

/// Numbers compared by value (an oracle's 1 and our 1.0 are the same IR).
fn norm(v: &J) -> J {
    match v {
        J::Number(n) => { let f = n.as_f64().unwrap_or(0.0); num_json(f) }
        J::Array(a) => J::Array(a.iter().map(norm).collect()),
        J::Object(o) => J::Object(o.iter().map(|(k, x)| (k.clone(), norm(x))).collect::<Map<_, _>>()),
        _ => v.clone(),
    }
}

fn first_diff(a: &J, b: &J, path: &str) -> String {
    match (a, b) {
        (J::Object(x), J::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                match (x.get(k), y.get(k)) {
                    (Some(p), Some(q)) if p != q => return first_diff(p, q, &format!("{path}.{k}")),
                    (Some(_), None) => return format!("{path}.{k}: only ours"),
                    (None, Some(q)) => return format!("{path}.{k}: only oracle = {}", q.to_string().chars().take(120).collect::<String>()),
                    _ => {}
                }
            }
            format!("{path}: ?")
        }
        (J::Array(x), J::Array(y)) => {
            if x.len() != y.len() { return format!("{path}: len {} vs {}", x.len(), y.len()); }
            for (i, (p, q)) in x.iter().zip(y).enumerate() { if p != q { return first_diff(p, q, &format!("{path}[{i}]")); } }
            format!("{path}: ?")
        }
        _ => format!("{path}: ours {} vs oracle {}", a.to_string().chars().take(100).collect::<String>(), b.to_string().chars().take(100).collect::<String>()),
    }
}
