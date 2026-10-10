// SPDX-License-Identifier: MIT OR Apache-2.0
//! CSS — tokenizer (CSS Syntax Level 3), rule parser, selectors (Selectors Level 4 subset) and matching.
//! Written from the W3C specs; no code copied from any engine.
use crate::dom::{Dom, Kind, NONE};
use std::collections::HashMap;

// ───────────────────────── tokenizer ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Func(String),
    At(String),
    Hash(String),
    Str(String),
    Url(String),
    Num(f32),
    Pct(f32),
    Dim(f32, String),
    Delim(char),
    Ws,
    Colon,
    Semi,
    Comma,
    LBrack,
    RBrack,
    LParen,
    RParen,
    LBrace,
    RBrace,
}

fn is_name_start(c: char) -> bool { c.is_ascii_alphabetic() || c == '_' || !c.is_ascii() }
fn is_name(c: char) -> bool { is_name_start(c) || c.is_ascii_digit() || c == '-' }

pub fn tokenize(src: &str) -> Vec<Tok> {
    // ASCII-only sources (the vast majority of CSS) are indexed as bytes: 4× less scratch than Vec<char>
    if src.is_ascii() { return tokenize_chars(&ByteChars(src.as_bytes())); }
    let v: Vec<char> = src.chars().collect();
    tokenize_chars(&v[..])
}

trait Chars { fn len(&self) -> usize; fn at(&self, i: usize) -> char; fn starts(&self, i: usize, p: &[char]) -> bool { p.iter().enumerate().all(|(k, c)| i + k < self.len() && self.at(i + k) == *c) } }
struct ByteChars<'a>(&'a [u8]);
impl Chars for ByteChars<'_> { fn len(&self) -> usize { self.0.len() } fn at(&self, i: usize) -> char { self.0[i] as char } }
impl Chars for [char] { fn len(&self) -> usize { <[char]>::len(self) } fn at(&self, i: usize) -> char { self[i] } }

fn tokenize_chars<C: Chars + ?Sized>(s: &C) -> Vec<Tok> {
    let n = s.len();
    let mut i = 0;
    let mut out = Vec::with_capacity(n / 3);
    let starts_ident = |i: usize| -> bool {
        let c0 = (if i < n { s.at(i) } else { '\0' });
        let c1 = (if i + 1 < n { s.at(i + 1) } else { '\0' });
        let c2 = if i + 2 < n { s.at(i + 2) } else { '\0' };
        // CSS Syntax §4.3.8: a backslash starts an identifier only as a valid escape (not at EOF, not before a newline)
        let esc = |a: char, b: char| a == '\\' && b != '\n' && b != '\0';
        if c0 == '-' { is_name_start(c1) || c1 == '-' || esc(c1, c2) } else { is_name_start(c0) || esc(c0, c1) }
    };
    let starts_num = |i: usize| -> bool {
        let c0 = (if i < n { s.at(i) } else { '\0' });
        let c1 = (if i + 1 < n { s.at(i + 1) } else { '\0' });
        let c2 = (if i + 2 < n { s.at(i + 2) } else { '\0' });
        c0.is_ascii_digit() || (c0 == '.' && c1.is_ascii_digit())
            || ((c0 == '+' || c0 == '-') && (c1.is_ascii_digit() || (c1 == '.' && c2.is_ascii_digit())))
    };
    let read_name = |i: &mut usize| -> String {
        let mut r = String::new();
        while *i < n {
            let c = s.at(*i);
            if is_name(c) { r.push(c); *i += 1; }
            else if c == '\\' && *i + 1 < n && s.at(*i + 1) != '\n' {
                *i += 1;
                let mut hex = String::new();
                while *i < n && hex.len() < 6 && s.at(*i).is_ascii_hexdigit() { hex.push(s.at(*i)); *i += 1; }
                if hex.is_empty() { r.push(s.at(*i)); *i += 1; }
                else {
                    if *i < n && s.at(*i).is_whitespace() { *i += 1; }
                    r.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD)).unwrap_or('\u{FFFD}'));
                }
            } else { break; }
        }
        r
    };
    while i < n {
        let c = s.at(i);
        if c == '/' && i + 1 < n && s.at(i + 1) == '*' {
            i += 2;
            while i + 1 < n && !(s.at(i) == '*' && s.at(i + 1) == '/') { i += 1; }
            i += 2;
            continue;
        }
        if c.is_whitespace() {
            while i < n && s.at(i).is_whitespace() { i += 1; }
            out.push(Tok::Ws);
            continue;
        }
        if c == '"' || c == '\'' {
            i += 1;
            let mut r = String::new();
            while i < n && s.at(i) != c {
                if s.at(i) == '\\' && i + 1 < n {
                    i += 1;
                    if s.at(i).is_ascii_hexdigit() {
                        // CSS Syntax §4.3.7: up to 6 hex digits + one optional whitespace → that code point
                        let mut hex = String::new();
                        while i < n && hex.len() < 6 && s.at(i).is_ascii_hexdigit() { hex.push(s.at(i)); i += 1; }
                        if i < n && s.at(i).is_whitespace() { i += 1; }
                        let cp = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                        r.push(if cp == 0 || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) { '\u{FFFD}' } else { char::from_u32(cp).unwrap_or('\u{FFFD}') });
                        continue;
                    }
                    if s.at(i) != '\n' { r.push(s.at(i)); }
                } else if s.at(i) == '\n' { break; } else { r.push(s.at(i)); }
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(r));
            continue;
        }
        if starts_num(i) {
            let st = i;
            if s.at(i) == '+' || s.at(i) == '-' { i += 1; }
            while i < n && s.at(i).is_ascii_digit() { i += 1; }
            if i + 1 < n && s.at(i) == '.' && s.at(i + 1).is_ascii_digit() { i += 1; while i < n && s.at(i).is_ascii_digit() { i += 1; } }
            if i + 1 < n && (s.at(i) == 'e' || s.at(i) == 'E') && (s.at(i + 1).is_ascii_digit() || ((s.at(i + 1) == '-' || s.at(i + 1) == '+') && i + 2 < n && s.at(i + 2).is_ascii_digit())) {
                i += 2; while i < n && s.at(i).is_ascii_digit() { i += 1; }
            }
            let v: f32 = (st..i).map(|k| s.at(k)).collect::<String>().parse().unwrap_or(0.0);
            if i < n && s.at(i) == '%' { i += 1; out.push(Tok::Pct(v)); }
            else if starts_ident(i) { let u = read_name(&mut i); out.push(Tok::Dim(v, u.to_ascii_lowercase())); }
            else { out.push(Tok::Num(v)); }
            continue;
        }
        if starts_ident(i) {
            let before = i;
            let name = read_name(&mut i);
            if i == before { i += 1; out.push(Tok::Delim(c)); continue; } // no progress: never loop
            if i < n && s.at(i) == '(' {
                i += 1;
                if name.eq_ignore_ascii_case("url") {
                    let mut j = i; while j < n && s.at(j).is_whitespace() { j += 1; }
                    if j < n && s.at(j) != '"' && s.at(j) != '\'' {
                        let mut r = String::new();
                        while j < n && s.at(j) != ')' { r.push(s.at(j)); j += 1; }
                        i = j + 1;
                        out.push(Tok::Url(r.trim().to_string()));
                        continue;
                    }
                }
                out.push(Tok::Func(name.to_ascii_lowercase()));
            } else { out.push(Tok::Ident(name)); }
            continue;
        }
        i += 1;
        out.push(match c {
            '#' if i < n && (is_name(s.at(i)) || s.at(i) == '\\') => Tok::Hash(read_name(&mut i)),
            '@' if starts_ident(i) => Tok::At(read_name(&mut i).to_ascii_lowercase()),
            ':' => Tok::Colon, ';' => Tok::Semi, ',' => Tok::Comma,
            '[' => Tok::LBrack, ']' => Tok::RBrack, '(' => Tok::LParen, ')' => Tok::RParen,
            '{' => Tok::LBrace, '}' => Tok::RBrace,
            '<' if s.starts(i, &['!', '-', '-']) => { i += 3; Tok::Ws }
            '-' if s.starts(i, &['-', '>']) => { i += 2; Tok::Ws }
            _ => Tok::Delim(c),
        });
    }
    out
}

// ───────────────────────── selectors ─────────────────────────

#[derive(Clone, Debug)]
pub enum AttrOp { Exists, Eq, Word, Dash, Prefix, Suffix, Contains }

const FORM_CTL: [&str; 7] = ["input", "button", "select", "textarea", "option", "optgroup", "fieldset"];

#[derive(Clone, Debug)]
pub enum Simple {
    Tag(String),
    Any,
    Id(String),
    Class(String),
    Attr(String, AttrOp, String, bool),
    Root,
    Checked,
    Disabled,
    Enabled,
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    NthChild(i32, i32, bool, bool), // a, b, from_end, of_type
    Empty,
    Link,
    Not(Vec<Vec<Compound>>),
    Is(Vec<Vec<Compound>>),
    /// `:has(<relative selector list>)` (Selectors 4 §4.5): each chain's first compound carries the leading
    /// combinator relative to the anchor element (None = descendant)
    Has(Vec<Vec<Compound>>),
    Never, // dynamic/unsupported states: :hover, :focus, ::before ...
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Comb { Desc, Child, Next, Later }

/// One compound selector and the combinator linking it to the compound on its LEFT.
#[derive(Clone, Debug)]
pub struct Compound { pub simples: Vec<Simple>, pub comb: Option<Comb> }

#[derive(Clone, Debug)]
pub struct Selector { pub parts: Vec<Compound>, pub spec: u32, pub pseudo: u8 } // parts left→right; pseudo 1 = ::before, 2 = ::after

fn parse_compound_list(t: &[Tok]) -> Option<Vec<Vec<Compound>>> {
    split_commas(t).into_iter().map(|p| parse_complex(p).map(|s| s.parts)).collect()
}

/// A relative selector list (the argument of `:has()`): each item may open with `>`, `+` or `~`; the leading
/// combinator is stored on the first compound. Returns the chains and the most specific item's specificity.
fn parse_relative_list(t: &[Tok]) -> Option<(Vec<Vec<Compound>>, u32)> {
    let mut out = Vec::new();
    let mut spec = 0u32;
    for item in split_commas(t) {
        let mut i = 0;
        while i < item.len() && item[i] == Tok::Ws { i += 1; }
        let lead = match item.get(i) { Some(Tok::Delim('>')) => Some(Comb::Child), Some(Tok::Delim('+')) => Some(Comb::Next), Some(Tok::Delim('~')) => Some(Comb::Later), _ => None };
        if lead.is_some() { i += 1; }
        let sel = parse_complex(&item[i..])?;
        if sel.pseudo != 0 || has_relative_parts(&sel.parts) { return None; } // no pseudo-elements, no nested :has()
        spec = spec.max(sel.spec);
        let mut parts = sel.parts;
        parts[0].comb = Some(lead.unwrap_or(Comb::Desc));
        out.push(parts);
    }
    Some((out, spec))
}

fn has_relative_parts(parts: &[Compound]) -> bool {
    parts.iter().any(|c| c.simples.iter().any(|x| match x {
        Simple::Has(_) => true,
        Simple::Not(l) | Simple::Is(l) => l.iter().any(|p| has_relative_parts(p)),
        _ => false,
    }))
}

/// Any `:has()` in the selector: its match depends on descendants / later siblings, so it breaks
/// sibling style sharing and the incremental style memo (both assume ancestors + earlier siblings only).
pub fn has_relative(sel: &Selector) -> bool { has_relative_parts(&sel.parts) }

/// The most specific selector's specificity in a selector list (packed a<<16 | b<<8 | c).
fn list_spec(t: &[Tok]) -> u32 { split_commas(t).into_iter().filter_map(parse_complex).map(|s| s.spec).max().unwrap_or(0) }

fn split_commas(t: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut st = 0;
    for (i, x) in t.iter().enumerate() {
        match x {
            Tok::LParen | Tok::Func(_) | Tok::LBrack => depth += 1,
            Tok::RParen | Tok::RBrack => depth -= 1,
            Tok::Comma if depth == 0 => { out.push(&t[st..i]); st = i + 1; }
            _ => {}
        }
    }
    out.push(&t[st..]);
    out
}

fn nth(args: &[Tok]) -> Option<(i32, i32)> {
    let s: String = args.iter().map(|t| match t {
        Tok::Ident(x) => x.clone(), Tok::Num(v) => format!("{v}"), Tok::Dim(v, u) => format!("{v}{u}"),
        Tok::Delim(c) => c.to_string(), Tok::Ws => String::new(), _ => "?".into() }).collect::<String>().to_ascii_lowercase();
    let s = s.split(" of ").next().unwrap_or("").to_string();
    if s == "odd" { return Some((2, 1)); }
    if s == "even" { return Some((2, 0)); }
    if let Some(p) = s.find('n') {
        let a = match &s[..p] { "" | "+" => 1, "-" => -1, x => x.parse().ok()? };
        let b = if p + 1 < s.len() { s[p + 1..].parse().ok()? } else { 0 };
        Some((a, b))
    } else { Some((0, s.parse().ok()?)) }
}

fn take_paren(t: &[Tok], i: usize) -> (usize, &[Tok]) {
    // t[i] is a Func; return (index after matching ')', inner tokens)
    let mut d = 1;
    let mut j = i + 1;
    while j < t.len() {
        match t[j] { Tok::Func(_) | Tok::LParen => d += 1, Tok::RParen => { d -= 1; if d == 0 { break; } } _ => {} }
        j += 1;
    }
    (j + 1, &t[i + 1..j.min(t.len())])
}

pub fn parse_complex(t: &[Tok]) -> Option<Selector> {
    let mut pseudo = 0u8;
    let mut parts: Vec<Compound> = Vec::new();
    let mut cur: Vec<Simple> = Vec::new();
    let mut pending: Option<Comb> = None;
    let mut i = 0;
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    let flush = |cur: &mut Vec<Simple>, parts: &mut Vec<Compound>, pending: &mut Option<Comb>| {
        if !cur.is_empty() {
            let comb = if parts.is_empty() { None } else { Some(pending.take().unwrap_or(Comb::Desc)) };
            parts.push(Compound { simples: std::mem::take(cur), comb });
            *pending = None;
        }
    };
    while i < t.len() {
        match &t[i] {
            Tok::Ws => { if !cur.is_empty() { flush(&mut cur, &mut parts, &mut pending); pending = pending.or(Some(Comb::Desc)); } i += 1; }
            Tok::Delim('>') | Tok::Delim('+') | Tok::Delim('~') => {
                flush(&mut cur, &mut parts, &mut pending);
                pending = Some(match t[i] { Tok::Delim('>') => Comb::Child, Tok::Delim('+') => Comb::Next, _ => Comb::Later });
                if parts.is_empty() { return None; }
                i += 1;
            }
            Tok::Ident(x) => { cur.push(Simple::Tag(x.to_ascii_lowercase())); c += 1; i += 1; }
            Tok::Delim('*') => { cur.push(Simple::Any); i += 1; }
            Tok::Hash(x) => { cur.push(Simple::Id(x.clone())); a += 1; i += 1; }
            Tok::Delim('.') => {
                if let Some(Tok::Ident(x)) = t.get(i + 1) { cur.push(Simple::Class(x.clone())); b += 1; i += 2; } else { return None; }
            }
            Tok::LBrack => {
                let mut j = i + 1;
                while j < t.len() && t[j] != Tok::RBrack { j += 1; }
                let inner: Vec<&Tok> = t[i + 1..j.min(t.len())].iter().filter(|x| **x != Tok::Ws).collect();
                let name = match inner.first() { Some(Tok::Ident(x)) => x.to_ascii_lowercase(), _ => return None };
                let mut ci = false;
                let (op, val) = if inner.len() == 1 { (AttrOp::Exists, String::new()) } else {
                    let (op, k) = match (inner.get(1), inner.get(2)) {
                        (Some(Tok::Delim('=')), _) => (AttrOp::Eq, 2),
                        (Some(Tok::Delim('~')), Some(Tok::Delim('='))) => (AttrOp::Word, 3),
                        (Some(Tok::Delim('|')), Some(Tok::Delim('='))) => (AttrOp::Dash, 3),
                        (Some(Tok::Delim('^')), Some(Tok::Delim('='))) => (AttrOp::Prefix, 3),
                        (Some(Tok::Delim('$')), Some(Tok::Delim('='))) => (AttrOp::Suffix, 3),
                        (Some(Tok::Delim('*')), Some(Tok::Delim('='))) => (AttrOp::Contains, 3),
                        _ => return None,
                    };
                    let v = match inner.get(k) { Some(Tok::Ident(x)) | Some(Tok::Str(x)) => x.clone(), Some(Tok::Num(v)) => format!("{v}"), _ => return None };
                    if let Some(Tok::Ident(f)) = inner.get(k + 1) { ci = f.eq_ignore_ascii_case("i"); }
                    (op, v)
                };
                cur.push(Simple::Attr(name, op, val, ci));
                b += 1;
                i = j + 1;
            }
            Tok::Colon => {
                let pe = matches!(t.get(i + 1), Some(Tok::Colon));
                let k = if pe { i + 2 } else { i + 1 };
                match t.get(k) {
                    Some(Tok::Ident(x)) => {
                        let x = x.to_ascii_lowercase();
                        let s = if (x == "before" || x == "after") && pseudo == 0 {
                            // generated content: the selector matches the element; the rule styles its pseudo box
                            c += 1; pseudo = if x == "before" { 1 } else { 2 }; i = k + 1; continue;
                        } else if pe || matches!(x.as_str(), "first-line" | "first-letter" | "selection" | "placeholder" | "marker" | "backdrop") {
                            c += 1; Simple::Never
                        } else {
                            b += 1;
                            match x.as_str() {
                                "root" => Simple::Root, "first-child" => Simple::FirstChild, "last-child" => Simple::LastChild,
                                "only-child" => Simple::OnlyChild, "first-of-type" => Simple::FirstOfType, "last-of-type" => Simple::LastOfType,
                                "empty" => Simple::Empty, "link" | "any-link" => Simple::Link,
                                "checked" | "default" => Simple::Checked, "disabled" => Simple::Disabled, "enabled" => Simple::Enabled,
                                _ => Simple::Never,
                            }
                        };
                        cur.push(s);
                        i = k + 1;
                    }
                    Some(Tok::Func(f)) => {
                        let f = f.clone();
                        let (next, inner) = take_paren(t, k);
                        let s = if pe { c += 1; Simple::Never } else {
                            match f.as_str() {
                                // Selectors 4 §17: :is() / :not() / :has() count as their most specific argument, :where() as zero
                                // (MDN `:is(:is(.left-sidebar,.content-section) .icon.icon-baseline)~.icon{display:none}` is (0,4,0)
                                // and beats `.left-sidebar span:not(…)` (0,2,1); a flat (0,1,0) per pseudo lost it)
                                "not" => { let l = parse_compound_list(inner)?; if crate::layout::patch_on(crate::layout::PATCH_IS_SPEC) { let sp = list_spec(inner); a += sp >> 16; b += (sp >> 8) & 255; c += sp & 255; } else { b += 1; } Simple::Not(l) }
                                "is" | "matches" | "where" | "-webkit-any" => {
                                    match parse_compound_list(inner) { Some(l) => { if f != "where" { if crate::layout::patch_on(crate::layout::PATCH_IS_SPEC) { let sp = list_spec(inner); a += sp >> 16; b += (sp >> 8) & 255; c += sp & 255; } else { b += 1; } } Simple::Is(l) } None => Simple::Never }
                                }
                                "has" if crate::layout::patch_on(crate::layout::PATCH_HAS) => {
                                    match parse_relative_list(inner) { Some((l, sp)) => { a += sp >> 16; b += (sp >> 8) & 255; c += sp & 255; Simple::Has(l) } None => Simple::Never }
                                }
                                "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                                    let (aa, bb) = nth(inner)?; b += 1;
                                    Simple::NthChild(aa, bb, f.contains("last"), f.contains("of-type"))
                                }
                                _ => Simple::Never,
                            }
                        };
                        cur.push(s);
                        i = next;
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    if cur.is_empty() && pseudo != 0 && (parts.is_empty() || pending.is_some()) { cur.push(Simple::Any); }
    flush(&mut cur, &mut parts, &mut pending);
    if parts.is_empty() { return None; }
    Some(Selector { parts, spec: (a.min(255) << 16) | (b.min(255) << 8) | c.min(255), pseudo })
}

// ───────────────────────── rules ─────────────────────────

#[derive(Clone, Debug)]
pub struct Decl { pub name: String, pub value: Vec<Tok>, pub important: bool }

#[derive(Debug)]
pub struct Rule { pub sel: Selector, pub decls: std::rc::Rc<Vec<Decl>>, pub order: u32, pub anc: Bloom, pub sib: bool, pub scope: u32 }

/// 256-bit Bloom set of ancestor keys (tag / #id / .class), 2 bits per key.
pub type Bloom = [u64; 4];

pub fn bloom_add(b: &mut Bloom, prefix: u8, key: &str) {
    let mut h: u64 = 0xcbf29ce484222325 ^ prefix as u64;
    for &c in key.as_bytes() { h ^= c as u64; h = h.wrapping_mul(0x100000001b3); }
    for k in [h & 255, (h >> 8) & 255] { b[(k >> 6) as usize] |= 1 << (k & 63); }
}

enum Key { Id(String), Class(String), Tag(String), Attr(String) }

/// The bucket keys under which a rule must be indexed so every element it can match will find it.
fn compound_keys(c: &Compound) -> Vec<Key> {
    for x in &c.simples { if let Simple::Id(v) = x { return vec![Key::Id(v.clone())]; } }
    for x in &c.simples { if let Simple::Class(v) = x { return vec![Key::Class(v.clone())]; } }
    for x in &c.simples { if let Simple::Tag(v) = x { return vec![Key::Tag(v.clone())]; } }
    for x in &c.simples { if let Simple::Attr(n, ..) = x { return vec![Key::Attr(n.clone())]; } }
    for x in &c.simples {
        if let Simple::Is(alts) = x {
            let mut all = Vec::new();
            for parts in alts {
                let k = match parts.last() { Some(last) => compound_keys(last), None => Vec::new() };
                if k.is_empty() { all.clear(); break; }
                all.extend(k);
            }
            if !all.is_empty() { return all; }
        }
    }
    Vec::new()
}

/// Can this selector distinguish two siblings with identical tag/attributes? (structural pseudos, sibling
/// combinators, :empty — anywhere in the selector, including inside :not()/:is()).
fn sibling_sensitive(sel: &Selector) -> bool {
    fn simples(v: &[Simple]) -> bool {
        v.iter().any(|x| match x {
            Simple::FirstChild | Simple::LastChild | Simple::OnlyChild | Simple::FirstOfType | Simple::LastOfType | Simple::NthChild(..) | Simple::Empty => true,
            Simple::Not(l) | Simple::Is(l) => l.iter().any(|parts| parts.iter().any(|c| simples(&c.simples) || matches!(c.comb, Some(Comb::Next) | Some(Comb::Later)))),
            Simple::Has(_) => true,
            _ => false,
        })
    }
    sel.parts.iter().any(|c| simples(&c.simples) || matches!(c.comb, Some(Comb::Next) | Some(Comb::Later)))
}

/// Keys every match must find among the ancestors: compounds reached through descendant/child combinators only.
fn ancestor_bits(sel: &Selector) -> Bloom {
    let mut b = [0u64; 4];
    let p = &sel.parts;
    let mut k = p.len() - 1;
    while k > 0 {
        match p[k].comb { Some(Comb::Desc) | Some(Comb::Child) => {} _ => break }
        for x in &p[k - 1].simples {
            match x { Simple::Tag(t) => bloom_add(&mut b, b't', t), Simple::Id(v) => bloom_add(&mut b, b'#', v), Simple::Class(v) => bloom_add(&mut b, b'.', v), _ => {} }
        }
        k -= 1;
    }
    b
}

pub struct Media { pub width: f32, pub height: f32 }

pub fn parse_decls(t: &[Tok]) -> Vec<Decl> {
    let mut out = Vec::new();
    for d in split_top(t, |x| *x == Tok::Semi) {
        let d: Vec<Tok> = d.to_vec();
        let mut k = 0;
        while k < d.len() && d[k] == Tok::Ws { k += 1; }
        let name = match d.get(k) { Some(Tok::Ident(x)) => x.clone(), _ => continue };
        k += 1;
        while k < d.len() && d[k] == Tok::Ws { k += 1; }
        if d.get(k) != Some(&Tok::Colon) { continue; }
        let mut v: Vec<Tok> = d[k + 1..].to_vec();
        while v.last() == Some(&Tok::Ws) { v.pop(); }
        let mut important = false;
        if v.len() >= 2 {
            if let (Tok::Delim('!'), Tok::Ident(x)) = (&v[v.len() - 2], &v[v.len() - 1]) {
                if x.eq_ignore_ascii_case("important") { important = true; v.truncate(v.len() - 2); }
            }
        }
        while v.first() == Some(&Tok::Ws) { v.remove(0); }
        while v.last() == Some(&Tok::Ws) { v.pop(); }
        let name = if name.starts_with("--") { name } else { name.to_ascii_lowercase() };
        out.push(Decl { name, value: v, important });
    }
    out
}

fn split_top<'a>(t: &'a [Tok], sep: impl Fn(&Tok) -> bool) -> Vec<&'a [Tok]> {
    let mut out = Vec::new();
    let mut d = 0i32;
    let mut st = 0;
    for (i, x) in t.iter().enumerate() {
        match x {
            Tok::LParen | Tok::Func(_) | Tok::LBrack | Tok::LBrace => d += 1,
            Tok::RParen | Tok::RBrack | Tok::RBrace => d -= 1,
            _ if d == 0 && sep(x) => { out.push(&t[st..i]); st = i + 1; }
            _ => {}
        }
    }
    out.push(&t[st..]);
    out
}

/// Media Queries Level 4: comma-separated queries; each `[not|only] type [and cond]` or a bare condition
/// with `not` / `and` / `or` and nested parentheses.
/// `@supports` condition (CSS Conditional 3 §6), answered as Chrome would: standard and -webkit- features are supported,
/// -ms-/-moz-/-o- ones are not; `selector()` is supported when we can parse it; `not` / `and` / `or` combine.
pub fn supports_ok(t: &[Tok]) -> bool {
    let t: Vec<&Tok> = t.iter().filter(|x| !matches!(x, Tok::Ws)).collect();
    fn foreign(x: &str) -> bool { let x = x.to_ascii_lowercase(); x.starts_with("-ms-") || x.starts_with("-moz-") || x.starts_with("-o-") }
    // one parenthesised / functional term starting at i → (value, next index)
    fn term(t: &[&Tok], i: usize) -> (bool, usize) {
        match t.get(i) {
            Some(Tok::Ident(k)) if k.eq_ignore_ascii_case("not") => { let (v, j) = term(t, i + 1); (!v, j) }
            Some(Tok::LParen) => {
                // find the matching paren
                let mut d = 0; let mut j = i;
                while j < t.len() { match t[j] { Tok::LParen | Tok::Func(_) => d += 1, Tok::RParen => { d -= 1; if d == 0 { break; } } _ => {} } j += 1; }
                let inner = &t[i + 1..j.min(t.len())];
                let is_decl = matches!(inner.first(), Some(Tok::Ident(k)) if !k.eq_ignore_ascii_case("not")) && matches!(inner.get(1), Some(Tok::Colon));
                let v = if is_decl {
                    // (property: value)
                    let prop = match inner.first() { Some(Tok::Ident(p)) => p.clone(), _ => String::new() };
                    let val_foreign = inner.iter().skip(2).any(|x| matches!(x, Tok::Ident(v) if foreign(v)) || matches!(x, Tok::Func(f) if foreign(f)));
                    !prop.is_empty() && !foreign(&prop) && !val_foreign
                } else { cond(inner) };
                (v, j + 1)
            }
            Some(Tok::Func(f)) => {
                let mut d = 1; let mut j = i + 1;
                while j < t.len() { match t[j] { Tok::LParen | Tok::Func(_) => d += 1, Tok::RParen => { d -= 1; if d == 0 { break; } } _ => {} } j += 1; }
                let v = match f.to_ascii_lowercase().as_str() {
                    "selector" => { let inner: Vec<Tok> = t[i + 1..j.min(t.len())].iter().map(|x| (*x).clone()).collect(); parse_complex(&inner).is_some() }
                    "font-tech" | "font-format" => true,
                    _ => false,
                };
                (v, j + 1)
            }
            _ => (false, i + 1),
        }
    }
    fn cond(t: &[&Tok]) -> bool {
        let (mut v, mut i) = term(t, 0);
        while i < t.len() {
            match t.get(i) {
                Some(Tok::Ident(k)) if k.eq_ignore_ascii_case("and") => { let (w, j) = term(t, i + 1); v = v && w; i = j; }
                Some(Tok::Ident(k)) if k.eq_ignore_ascii_case("or") => { let (w, j) = term(t, i + 1); v = v || w; i = j; }
                _ => break,
            }
        }
        v
    }
    if t.is_empty() { return true; }
    cond(&t)
}

pub fn media_ok(prelude: &[Tok], m: &Media) -> bool {
    for q in split_commas(prelude) {
        let t: Vec<&Tok> = q.iter().filter(|x| **x != Tok::Ws).collect();
        if t.is_empty() { return true; }
        let mut i = 0;
        let mut neg = false;
        let mut ok = true;
        if let Some(Tok::Ident(x)) = t.first() {
            let x = x.to_ascii_lowercase();
            if x == "not" && !matches!(t.get(1), Some(Tok::LParen)) { neg = true; i = 1; }
            else if x == "only" { i = 1; }
        }
        if let Some(Tok::Ident(ty)) = t.get(i) {
            let ty = ty.to_ascii_lowercase();
            if ty != "not" {
                ok = matches!(ty.as_str(), "screen" | "all");
                i += 1;
                if let Some(Tok::Ident(a)) = t.get(i) { if a.eq_ignore_ascii_case("and") { i += 1; } }
            }
        }
        if i < t.len() { let mut k = i; ok = ok && cond(&t, &mut k, m); }
        if ok != neg { return true; }
    }
    false
}

fn cond(t: &[&Tok], i: &mut usize, m: &Media) -> bool {
    if let Some(Tok::Ident(x)) = t.get(*i) {
        if x.eq_ignore_ascii_case("not") { *i += 1; return !term(t, i, m); }
    }
    let mut v = term(t, i, m);
    while let Some(Tok::Ident(op)) = t.get(*i) {
        let op = op.to_ascii_lowercase();
        if op != "and" && op != "or" { break; }
        *i += 1;
        let r = term(t, i, m);
        v = if op == "and" { v && r } else { v || r };
    }
    v
}

fn term(t: &[&Tok], i: &mut usize, m: &Media) -> bool {
    match t.get(*i) {
        Some(Tok::LParen) => {
            *i += 1;
            let nested = matches!(t.get(*i), Some(Tok::LParen)) || matches!(t.get(*i), Some(Tok::Ident(x)) if x.eq_ignore_ascii_case("not"));
            let v = if nested { cond(t, i, m) } else {
                let st = *i;
                let mut d = 0;
                while *i < t.len() { match t[*i] { Tok::LParen => d += 1, Tok::RParen => { if d == 0 { break; } d -= 1; } _ => {} } *i += 1; }
                feature_ok(&t[st..*i], m)
            };
            if matches!(t.get(*i), Some(Tok::RParen)) { *i += 1; }
            v
        }
        Some(Tok::Func(_)) => { // e.g. a stray function; skip it
            let mut d = 1; *i += 1;
            while *i < t.len() && d > 0 { match t[*i] { Tok::LParen | Tok::Func(_) => d += 1, Tok::RParen => d -= 1, _ => {} } *i += 1; }
            false
        }
        _ => { *i += 1; false }
    }
}

fn len_px(t: &Tok) -> Option<f32> {
    match t {
        Tok::Dim(v, u) => Some(match u.as_str() { "px" => *v, "em" | "rem" => v * 16.0, "pt" => v * 4.0 / 3.0, _ => return None }),
        Tok::Num(v) if *v == 0.0 => Some(0.0),
        _ => None,
    }
}

fn feature_ok(f: &[&Tok], m: &Media) -> bool {
    let name = match f.first() { Some(Tok::Ident(x)) => x.to_ascii_lowercase(), _ => return false };
    // range syntax: (width >= 600px), (400px <= width <= 700px) — reduce the common forms
    if f.len() >= 3 && matches!(f[1], Tok::Delim('<') | Tok::Delim('>')) {
        let gt = matches!(f[1], Tok::Delim('>'));
        let eq = matches!(f.get(2), Some(Tok::Delim('=')));
        let k = if eq { 3 } else { 2 };
        // the value may be a math function: (width >= calc(1rem * 2 + (15rem + 2rem) * 2 + 31rem)) — MDN's layout breakpoints
        // (Media Queries 4 §2.4.4 range form; calc() per CSS Values 4: rem/em = the initial 16 px in a media query)
        let px = match f.get(k) {
            Some(Tok::Func(_)) => { let toks: Vec<Tok> = f[k..].iter().map(|t| (*t).clone()).collect(); match crate::style::eval_len_px(&toks, 16 * 64, 0) { Some(v) => v as f32 / 64.0, None => return false } }
            Some(t) => match len_px(t) { Some(p) => p, None => return false },
            None => return false,
        };
        let dim = match name.as_str() { "width" => m.width, "height" => m.height, _ => return false };
        return match (gt, eq) { (true, true) => dim >= px, (true, false) => dim > px, (false, true) => dim <= px, (false, false) => dim < px };
    }
    let val = f.iter().skip(2).next();
    match (name.as_str(), val) {
        ("min-width", Some(v)) => len_px(v).map_or(false, |px| m.width >= px),
        ("max-width", Some(v)) => len_px(v).map_or(false, |px| m.width <= px),
        ("min-height", Some(v)) => len_px(v).map_or(false, |px| m.height >= px),
        ("max-height", Some(v)) => len_px(v).map_or(false, |px| m.height <= px),
        ("orientation", Some(Tok::Ident(o))) => o == "landscape",
        ("prefers-color-scheme", Some(Tok::Ident(o))) => o == "light",
        ("prefers-reduced-motion", Some(Tok::Ident(o))) => o == "reduce",
        ("hover", Some(Tok::Ident(o))) | ("any-hover", Some(Tok::Ident(o))) => o == "hover",
        ("pointer", Some(Tok::Ident(o))) | ("any-pointer", Some(Tok::Ident(o))) => o == "fine",
        ("color", _) => true,
        ("prefers-contrast", Some(Tok::Ident(o))) => o == "no-preference",
        ("forced-colors", Some(Tok::Ident(o))) => o == "none",
        ("display-mode", Some(Tok::Ident(o))) => o == "browser",
        ("scripting", Some(Tok::Ident(o))) => o == "none",
        ("min-resolution", _) | ("-webkit-min-device-pixel-ratio", _) | ("min--moz-device-pixel-ratio", _) => false,
        ("max-resolution", _) | ("-webkit-max-device-pixel-ratio", _) => true,
        _ => false,
    }
}

/// A parsed stylesheet: rules bucketed by their rightmost compound's key (id / class / tag / universal).
pub struct Sheet {
    pub rules: Vec<Rule>,
    pub by_id: HashMap<String, Vec<u32>>,
    pub by_class: HashMap<String, Vec<u32>>,
    pub by_tag: HashMap<String, Vec<u32>>,
    pub by_attr: HashMap<String, Vec<u32>>,
    pub universal: Vec<u32>,
    pub imports: Vec<String>,
    pub font_faces: usize,
    pub ua_rules: u32, // rules with order < this came from the UA sheet
    pub cur_scope: u32, // rules being added belong to this declarative shadow root (u32::MAX = the document)
    pub scoped: bool,   // any shadow-scoped rule present
    /// Keys present in the document: a rule whose rightmost id/class/tag never occurs is skipped at parse
    /// time, before its declarations are even tokenized into values (YouTube: 21k rules → a few hundred).
    pub filter: Option<KeyFilter>,
    pub skipped: usize,
    /// URL of the sheet being added (relative url() in @font-face resolves against it)
    pub base: String,
    pub faces: Vec<FontFaceRule>,
    /// explain mode only: rule order → (stylesheet url, selector text as written)
    pub trace: Option<HashMap<u32, (String, String)>>,
}

#[derive(Clone, Debug)]
pub struct FontFaceRule { pub family: String, pub srcs: Vec<String>, pub w_lo: u16, pub w_hi: u16, pub italic: bool, pub size_adjust: u32, pub latin: bool }

#[derive(Default)]
pub struct KeyFilter { pub ids: std::collections::HashSet<String>, pub classes: std::collections::HashSet<String>, pub tags: std::collections::HashSet<String> }

impl KeyFilter {
    pub fn from_dom(d: &Dom) -> KeyFilter {
        let mut f = KeyFilter::default();
        for i in 0..d.nodes.len() as u32 {
            if d.nodes[i as usize].kind != Kind::Element { continue; }
            f.tags.insert(d.tag(i).to_ascii_lowercase());
            if let Some(v) = d.attr(i, "id") { f.ids.insert(v.to_string()); }
            if let Some(v) = d.attr(i, "class") { for c in v.split_ascii_whitespace() { f.classes.insert(c.to_string()); } }
        }
        f
    }
    /// Decide from raw selector text, without tokenizing, that no comma part's rightmost compound can match.
    /// Conservative: anything it cannot read (escapes, functional pseudos in the key) is left to the full parser.
    pub fn reject_raw(&self, sel: &str) -> bool {
        let b = sel.as_bytes();
        let mut parts = Vec::new();
        let (mut d, mut st) = (0i32, 0usize);
        for (i, &c) in b.iter().enumerate() {
            match c { b'(' | b'[' => d += 1, b')' | b']' => d -= 1, b',' if d == 0 => { parts.push(&sel[st..i]); st = i + 1; } b'\\' => return false, _ => {} }
        }
        parts.push(&sel[st..]);
        for p in parts {
            let p = p.trim();
            // rightmost compound: after the last top-level whitespace or combinator
            let pb = p.as_bytes();
            let mut d = 0i32;
            let mut cut = 0usize;
            for (i, &c) in pb.iter().enumerate() {
                match c { b'(' | b'[' => d += 1, b')' | b']' => d -= 1, b' ' | b'\t' | b'\n' | b'>' | b'+' | b'~' if d == 0 => cut = i + 1, _ => {} }
            }
            let comp = &p[cut..];
            if comp.is_empty() || comp.contains('(') { return false; }
            if !self.raw_compound_ok(comp) { continue; } else { return false; }
        }
        true
    }

    fn raw_compound_ok(&self, comp: &str) -> bool {
        let b = comp.as_bytes();
        let ident_end = |mut i: usize| { while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'_' || b[i] >= 0x80) { i += 1; } i };
        let mut i = 0;
        let e = ident_end(0);
        if e > 0 { if !self.tags.contains(&comp[..e].to_ascii_lowercase()) { return false; } i = e; }
        while i < b.len() {
            match b[i] {
                b'#' => { let e = ident_end(i + 1); if !self.ids.contains(&comp[i + 1..e]) { return false; } i = e; }
                b'.' => { let e = ident_end(i + 1); if !self.classes.contains(&comp[i + 1..e]) { return false; } i = e; }
                b'[' => { while i < b.len() && b[i] != b']' { i += 1; } i += 1; }
                b':' => { i += 1; if i < b.len() && b[i] == b':' { i += 1; } i = ident_end(i); }
                _ => i += 1,
            }
        }
        true
    }

    fn may_match(&self, key: &Compound) -> bool {
        for x in &key.simples {
            match x {
                Simple::Id(v) if !self.ids.contains(v) => return false,
                Simple::Class(v) if !self.classes.contains(v) => return false,
                Simple::Tag(v) if !self.tags.contains(v) => return false,
                _ => {}
            }
        }
        true
    }
}

/// Set before loading a page to keep rule provenance (`omni-render explain`); off = no cost.
pub static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Sheet {
    pub fn new() -> Sheet {
        Sheet { rules: Vec::new(), by_id: HashMap::new(), by_class: HashMap::new(), by_tag: HashMap::new(), by_attr: HashMap::new(),
                universal: Vec::new(), imports: Vec::new(), font_faces: 0, ua_rules: 0, cur_scope: u32::MAX, scoped: false, filter: None, skipped: 0, base: String::new(), faces: Vec::new(),
                trace: if TRACE.load(std::sync::atomic::Ordering::Relaxed) { Some(HashMap::new()) } else { None } }
    }
    /// Append a stylesheet's rules (document order is the `order` counter, shared across sheets).
    /// Streams rule by rule over the raw text: a selector is tokenized first and the declaration block only if
    /// the rule can match this document — peak memory is one rule, not the whole multi-MB sheet.
    pub fn add(&mut self, src: &str, m: &Media, order: &mut u32) {
        let b = src.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let (pre, blk, next) = next_rule(src, i);
            i = next;
            let pre_owned = strip_comments(pre);
            let pre_t = pre_owned.trim();
            if pre_t.is_empty() && blk.is_none() { continue; }
            if pre_t.starts_with('@') {
                let name: String = pre_t[1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>().to_ascii_lowercase();
                let prelude = tokenize(&pre_t[1 + name.len()..]);
                match blk {
                    None => if name == "import" {
                        // @import url|string [layer(...)] [supports(...)] [media-query-list]
                        let mut url = None;
                        let mut k = 0;
                        while k < prelude.len() {
                            match &prelude[k] {
                                Tok::Str(u) | Tok::Url(u) => { url = Some(u.clone()); k += 1; break; }
                                Tok::Func(f) if f == "url" => { if let Some(Tok::Str(u)) = prelude.get(k + 1) { url = Some(u.clone()); } k += 3; break; }
                                _ => k += 1,
                            }
                        }
                        let rest: Vec<Tok> = strip_funcs(&prelude[k.min(prelude.len())..], &["layer", "supports"]);
                        if let Some(u) = url { if media_ok(&rest, m) { self.imports.push(u); } }
                    },
                    Some(body) => match name.as_str() {
                        "media" => if media_ok(&prelude, m) { self.add(body, m, order) },
                        "supports" => if supports_ok(&prelude) { self.add(body, m, order) },
                        "layer" | "document" | "-moz-document" | "container" | "scope" => self.add(body, m, order),
                        "font-face" => { self.font_faces += 1; self.font_face(body); }
                        _ => {}
                    },
                }
                continue;
            }
            let body = match blk { Some(x) => x, None => continue };
            if let Some(f) = &self.filter { if f.reject_raw(pre_t) { self.skipped += 1; *order += 1; continue; } }
            let sel_t = tokenize(pre_t);
            let sels: Vec<Selector> = split_commas(&sel_t).into_iter().filter_map(|sel| parse_complex(trim_ws(sel)))
                .filter(|s| match &self.filter { Some(f) => f.may_match(s.parts.last().unwrap()), None => true }).collect();
            if sels.is_empty() { self.skipped += 1; *order += 1; continue; }
            let decls = std::rc::Rc::new(parse_decls(&tokenize(body)));
            if decls.is_empty() { continue; }
            for s in sels {
                if let Some(t) = &mut self.trace { t.insert(*order, (if self.base.is_empty() { "<style>".into() } else { self.base.clone() }, pre_t.to_string())); }
                self.push_rule(s, decls.clone(), order);
            }
        }
    }

    fn font_face(&mut self, body: &str) {
        let decls = parse_decls(&tokenize(body));
        let mut f = FontFaceRule { family: String::new(), srcs: Vec::new(), w_lo: 400, w_hi: 400, italic: false, size_adjust: 1000, latin: true };
        for d in &decls {
            match d.name.as_str() {
                "font-family" => f.family = d.value.iter().filter_map(|t| match t { Tok::Str(x) | Tok::Ident(x) => Some(x.clone()), _ => None }).collect::<Vec<_>>().join(" "),
                "src" => {
                    // url(...) [format(...)] , ... — keep formats we decode (sfnt / WOFF / WOFF2); local() is skipped (privacy: no font fingerprinting)
                    for part in d.value.split(|t| *t == Tok::Comma) {
                        let url = part.iter().find_map(|t| match t { Tok::Url(u) => Some(u.clone()), _ => None })
                            .or_else(|| part.windows(2).find_map(|w| match (&w[0], &w[1]) { (Tok::Func(fn_), Tok::Str(u)) if fn_ == "url" => Some(u.clone()), _ => None }));
                        let fmt = part.windows(2).find_map(|w| match (&w[0], &w[1]) { (Tok::Func(fn_), Tok::Str(x)) | (Tok::Func(fn_), Tok::Ident(x)) if fn_ == "format" => Some(x.to_ascii_lowercase()), _ => None });
                        if let Some(u) = url {
                            let ok = match &fmt { Some(x) => x.contains("woff") || x.contains("truetype") || x.contains("opentype"), None => !u.ends_with(".eot") && !u.ends_with(".svg") };
                            if ok && !u.starts_with("data:") { f.srcs.push(crate::page::resolve(&self.base, &u)); }
                        }
                    }
                }
                "font-weight" => {
                    let n: Vec<u16> = d.value.iter().filter_map(|t| match t { Tok::Num(v) => Some(*v as u16), Tok::Ident(x) if x == "bold" => Some(700), Tok::Ident(x) if x == "normal" => Some(400), _ => None }).collect();
                    if let Some(&a) = n.first() { f.w_lo = a; f.w_hi = *n.get(1).unwrap_or(&a); }
                }
                "font-style" => f.italic = d.value.iter().any(|t| matches!(t, Tok::Ident(x) if x == "italic" || x == "oblique")),
                // unicode-range (CSS Fonts 4 §4.5): does this subset cover basic Latin ('A' U+0041)? A page often declares the same
                // family/weight once per subset (latin-ext first, then latin); text in Latin must use the latin one
                "unicode-range" => {
                    let raw: String = d.value.iter().map(|t| match t { Tok::Ident(x) => x.clone(), Tok::Num(n) => format!("{n}"), Tok::Dim(n, u) => format!("{n}{u}"), Tok::Delim(c) => c.to_string(), Tok::Comma => ",".into(), _ => String::new() }).collect::<String>().to_ascii_lowercase();
                    f.latin = raw.split(',').any(|r| { let r = r.trim().trim_start_matches(|c: char| c == 'u' || c == '+'); let (lo, hi) = match r.split_once('-') { Some((a, b)) => (a.replace('?', "0"), b.to_string()), None => (r.replace('?', "0"), r.replace('?', "f")) };
                        match (u32::from_str_radix(&lo, 16), u32::from_str_radix(&hi, 16)) { (Ok(a), Ok(b)) => a <= 0x41 && 0x41 <= b, _ => false } });
                }
                // CSS Fonts 5 §4.11 size-adjust: scales the face's glyph outlines and metrics (‰)
                "size-adjust" => if let Some(v) = d.value.iter().find_map(|t| match t { Tok::Pct(v) => Some(*v), _ => None }) { if v > 0.0 { f.size_adjust = (v * 10.0).round() as u32; } },
                _ => {}
            }
        }
        if !f.family.is_empty() && !f.srcs.is_empty() { self.faces.push(f); }
    }

    fn push_rule(&mut self, s: Selector, decls: std::rc::Rc<Vec<Decl>>, order: &mut u32) {
        let ri = self.rules.len() as u32;
        let key = s.parts.last().unwrap();
        // bucket key: id > class > tag > attribute name > every alternative of an :is()/:where() > universal
        let keys = compound_keys(key);
        if keys.is_empty() { self.universal.push(ri); }
        for k in keys {
            match k {
                Key::Id(v) => self.by_id.entry(v).or_default().push(ri),
                Key::Class(v) => self.by_class.entry(v).or_default().push(ri),
                Key::Tag(v) => self.by_tag.entry(v).or_default().push(ri),
                Key::Attr(v) => self.by_attr.entry(v).or_default().push(ri),
            }
        }
        let anc = ancestor_bits(&s);
        let sib = sibling_sensitive(&s);
        self.rules.push(Rule { sel: s, decls, order: *order, anc, sib, scope: self.cur_scope });
        *order += 1;
    }
    fn add_toks(&mut self, t: &[Tok], m: &Media, order: &mut u32) {
        let mut i = 0;
        while i < t.len() {
            match &t[i] {
                Tok::Ws | Tok::Semi => { i += 1; }
                Tok::At(name) => {
                    let name = name.clone();
                    let st = i + 1;
                    let mut j = st;
                    while j < t.len() && t[j] != Tok::LBrace && t[j] != Tok::Semi { j += 1; }
                    let prelude = &t[st..j.min(t.len())];
                    if j >= t.len() { break; }
                    if t[j] == Tok::Semi {
                        if name == "import" {
                            let mut url = None;
                            let mut k = 0;
                            while k < prelude.len() {
                                match &prelude[k] {
                                    Tok::Str(u) | Tok::Url(u) if url.is_none() => url = Some(u.clone()),
                                    Tok::Func(f) if f == "url" => { if let Some(Tok::Str(u)) = prelude.get(k + 1) { url = Some(u.clone()); k += 2; } }
                                    _ => {}
                                }
                                k += 1;
                                if url.is_some() { break; }
                            }
                            // remaining prelude = [layer(...)] [supports(...)] media-query-list
                            let rest: Vec<Tok> = prelude[k.min(prelude.len())..].iter().cloned()
                                .skip_while(|x| matches!(x, Tok::Ws)).collect();
                            let rest: Vec<Tok> = strip_funcs(&rest, &["layer", "supports"]);
                            if let Some(u) = url { if media_ok(&rest, m) { self.imports.push(u); } }
                        }
                        i = j + 1;
                        continue;
                    }
                    let (end, body) = block(t, j);
                    match name.as_str() {
                        "media" => if media_ok(prelude, m) { self.add_toks(body, m, order) },
                        "supports" | "layer" | "document" | "-moz-document" | "container" | "scope" => self.add_toks(body, m, order),
                        "font-face" => self.font_faces += 1,
                        _ => {}
                    }
                    i = end;
                }
                _ => {
                    let st = i;
                    let mut j = i;
                    while j < t.len() && t[j] != Tok::LBrace { j += 1; }
                    if j >= t.len() { break; }
                    let (end, body) = block(t, j);
                    let sels: Vec<Selector> = split_commas(&t[st..j]).into_iter().filter_map(|sel| parse_complex(trim_ws(sel)))
                        .filter(|s| match &self.filter { Some(f) => f.may_match(s.parts.last().unwrap()), None => true }).collect();
                    if sels.is_empty() { self.skipped += 1; *order += 1; i = end; continue; }
                    let decls = std::rc::Rc::new(parse_decls(body));
                    if !decls.is_empty() {
                        for s in sels {
                            {
                                let ri = self.rules.len() as u32;
                                let key = s.parts.last().unwrap();
                                let mut placed = false;
                                for x in &key.simples {
                                    match x {
                                        Simple::Id(v) => { self.by_id.entry(v.clone()).or_default().push(ri); placed = true; break; }
                                        _ => {}
                                    }
                                }
                                if !placed { for x in &key.simples { if let Simple::Class(v) = x { self.by_class.entry(v.clone()).or_default().push(ri); placed = true; break; } } }
                                if !placed { for x in &key.simples { if let Simple::Tag(v) = x { self.by_tag.entry(v.clone()).or_default().push(ri); placed = true; break; } } }
                                if !placed { self.universal.push(ri); }
                                let anc = ancestor_bits(&s);
                                let sib = sibling_sensitive(&s);
                                self.rules.push(Rule { sel: s, decls: decls.clone(), order: *order, anc, sib, scope: self.cur_scope });
                                *order += 1;
                            }
                        }
                    }
                    i = end;
                }
            }
        }
    }
}

fn strip_funcs(t: &[Tok], names: &[&str]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if let Tok::Func(f) = &t[i] {
            if names.contains(&f.as_str()) {
                let mut d = 1; i += 1;
                while i < t.len() && d > 0 { match t[i] { Tok::Func(_) | Tok::LParen => d += 1, Tok::RParen => d -= 1, _ => {} } i += 1; }
                continue;
            }
        }
        if let Tok::Ident(x) = &t[i] { if x.eq_ignore_ascii_case("layer") { i += 1; continue; } }
        out.push(t[i].clone());
        i += 1;
    }
    out
}

/// Remove /* … */ from a prelude (a comment before `@import` or a selector must not hide it).
fn strip_comments(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("/*") { return std::borrow::Cow::Borrowed(s); }
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find("/*") {
        o.push_str(&rest[..a]);
        rest = match rest[a + 2..].find("*/") { Some(b) => &rest[a + 2 + b + 2..], None => "" };
        o.push(' ');
    }
    o.push_str(rest);
    std::borrow::Cow::Owned(o)
}

/// Raw-text rule scanner: (prelude, Some(block) | None for `;`-terminated at-rules, next index).
/// Skips comments and respects strings and nested braces/parens.
fn next_rule(src: &str, mut i: usize) -> (&str, Option<&str>, usize) {
    let b = src.as_bytes();
    let n = b.len();
    let st = i;
    let mut paren = 0i32;
    while i < n {
        match b[i] {
            b'/' if i + 1 < n && b[i + 1] == b'*' => { i = src[i + 2..].find("*/").map(|k| i + 2 + k + 2).unwrap_or(n); continue; }
            q @ (b'"' | b'\'') => { i += 1; while i < n && b[i] != q { if b[i] == b'\\' { i += 1; } i += 1; } }
            b'(' => paren += 1,
            b')' => paren -= 1,
            b';' if paren <= 0 => return (&src[st..i], None, i + 1),
            b'{' if paren <= 0 => {
                let bs = i + 1;
                let mut d = 1;
                i += 1;
                while i < n {
                    match b[i] {
                        b'/' if i + 1 < n && b[i + 1] == b'*' => { i = src[i + 2..].find("*/").map(|k| i + 2 + k + 2).unwrap_or(n); continue; }
                        q @ (b'"' | b'\'') => { i += 1; while i < n && b[i] != q { if b[i] == b'\\' { i += 1; } i += 1; } }
                        b'{' => d += 1,
                        b'}' => { d -= 1; if d == 0 { break; } }
                        _ => {}
                    }
                    i += 1;
                }
                return (&src[st..bs - 1], Some(&src[bs..i.min(n)]), (i + 1).min(n.max(i + 1)));
            }
            b'}' => return ("", None, i + 1), // stray close brace
            _ => {}
        }
        i += 1;
    }
    (&src[st..n.min(src.len())], None, n)
}

fn trim_ws(t: &[Tok]) -> &[Tok] {
    let mut a = 0;
    let mut b = t.len();
    while a < b && t[a] == Tok::Ws { a += 1; }
    while b > a && t[b - 1] == Tok::Ws { b -= 1; }
    &t[a..b]
}

fn block(t: &[Tok], open: usize) -> (usize, &[Tok]) {
    let mut d = 0;
    let mut j = open;
    while j < t.len() {
        match t[j] { Tok::LBrace => d += 1, Tok::RBrace => { d -= 1; if d == 0 { break; } } _ => {} }
        j += 1;
    }
    (j + 1, &t[open + 1..j.min(t.len())])
}

// ───────────────────────── matching ─────────────────────────

/// Per-element facts precomputed once per document so matching never re-parses attributes.
pub struct ElemFacts {
    pub id: Vec<Option<String>>,
    pub classes: Vec<Vec<String>>,
    pub elem_index: Vec<u32>, // 1-based position among element siblings
    pub elem_count: Vec<u32>, // element siblings total
}

impl ElemFacts {
    pub fn new(d: &Dom) -> ElemFacts {
        let n = d.nodes.len();
        let mut id = vec![None; n];
        let mut classes = vec![Vec::new(); n];
        let mut elem_index = vec![0u32; n];
        let mut elem_count = vec![0u32; n];
        for i in 0..n as u32 {
            if d.nodes[i as usize].kind != Kind::Element { continue; }
            if let Some(v) = d.attr(i, "id") { id[i as usize] = Some(v.to_string()); }
            if let Some(v) = d.attr(i, "class") { classes[i as usize] = v.split_ascii_whitespace().map(|s| s.to_string()).collect(); }
        }
        // one pass: preorder visits each parent's element children in order — count them per parent,
        // then a second sweep writes each child's sibling total (no per-node allocation)
        let mut per_parent = vec![0u32; n];
        for c in 0..n {
            if d.nodes[c].kind != Kind::Element { continue; }
            let p = d.nodes[c].parent;
            if p == NONE { continue; }
            per_parent[p as usize] += 1;
            elem_index[c] = per_parent[p as usize];
        }
        for c in 0..n {
            if d.nodes[c].kind != Kind::Element { continue; }
            let p = d.nodes[c].parent;
            if p != NONE { elem_count[c] = per_parent[p as usize]; }
        }
        ElemFacts { id, classes, elem_index, elem_count }
    }
}

fn prev_elem(d: &Dom, e: u32) -> u32 {
    let p = d.nodes[e as usize].parent;
    if p == NONE { return NONE; }
    let mut last = NONE;
    for c in d.children(p) { if c == e { return last; } if d.nodes[c as usize].kind == Kind::Element { last = c; } }
    NONE
}

fn match_simple(d: &Dom, f: &ElemFacts, e: u32, s: &Simple) -> bool {
    match s {
        Simple::Any => true,
        Simple::Tag(t) => d.tag(e).eq_ignore_ascii_case(t),
        Simple::Id(v) => f.id[e as usize].as_deref() == Some(v.as_str()),
        Simple::Class(v) => f.classes[e as usize].iter().any(|c| c == v),
        Simple::Attr(n, op, v, ci) => match d.attr(e, n) {
            None => false,
            Some(a) => {
                let (a, v) = if *ci { (a.to_ascii_lowercase(), v.to_ascii_lowercase()) } else { (a.to_string(), v.clone()) };
                match op {
                    AttrOp::Exists => true,
                    AttrOp::Eq => a == v,
                    AttrOp::Word => a.split_ascii_whitespace().any(|w| w == v),
                    AttrOp::Dash => a == v || a.starts_with(&format!("{v}-")),
                    AttrOp::Prefix => !v.is_empty() && a.starts_with(&v),
                    AttrOp::Suffix => !v.is_empty() && a.ends_with(&v),
                    AttrOp::Contains => !v.is_empty() && a.contains(&v),
                }
            }
        },
        Simple::Root => { let p = d.nodes[e as usize].parent; p != NONE && d.nodes[p as usize].kind == Kind::Document }
        Simple::FirstChild => f.elem_index[e as usize] == 1,
        Simple::LastChild => f.elem_index[e as usize] == f.elem_count[e as usize],
        Simple::OnlyChild => f.elem_count[e as usize] == 1,
        Simple::FirstOfType | Simple::LastOfType | Simple::NthChild(_, _, _, true) => {
            let p = d.nodes[e as usize].parent;
            let tag = d.tag(e);
            let sibs: Vec<u32> = d.children(p).filter(|&c| d.nodes[c as usize].kind == Kind::Element && d.tag(c) == tag).collect();
            let pos = sibs.iter().position(|&c| c == e).unwrap_or(0) as i32 + 1;
            let cnt = sibs.len() as i32;
            match s {
                Simple::FirstOfType => pos == 1,
                Simple::LastOfType => pos == cnt,
                Simple::NthChild(a, b, end, _) => nth_ok(*a, *b, if *end { cnt - pos + 1 } else { pos }),
                _ => false,
            }
        }
        Simple::NthChild(a, b, end, false) => {
            let pos = f.elem_index[e as usize] as i32;
            let cnt = f.elem_count[e as usize] as i32;
            nth_ok(*a, *b, if *end { cnt - pos + 1 } else { pos })
        }
        Simple::Empty => d.children(e).all(|c| d.nodes[c as usize].kind == Kind::Comment || (d.nodes[c as usize].kind == Kind::Text && d.text(c).is_empty())),
        // form state from content attributes (HTML §4.16.3): no script runs, so the parsed state is the live one
        Simple::Checked => match d.tag(e) { "input" => d.attr(e, "checked").is_some() && matches!(d.attr(e, "type").map(|t| t.to_ascii_lowercase()).as_deref(), Some("checkbox") | Some("radio")), "option" => d.attr(e, "selected").is_some(), _ => false },
        Simple::Disabled => FORM_CTL.contains(&d.tag(e)) && d.attr(e, "disabled").is_some(),
        Simple::Enabled => FORM_CTL.contains(&d.tag(e)) && d.attr(e, "disabled").is_none(),
        Simple::Link => (d.tag(e) == "a" || d.tag(e) == "area") && d.attr(e, "href").is_some(),
        Simple::Not(l) => !l.iter().any(|parts| match_parts(d, f, e, parts)),
        Simple::Is(l) => l.iter().any(|parts| match_parts(d, f, e, parts)),
        Simple::Has(l) => l.iter().any(|parts| has_from(d, f, e, parts, 0)),
        Simple::Never => false,
    }
}

/// `:has()` forward walk: some element reached from `anchor` by `parts[k].comb` matches `parts[k]`, and the
/// rest of the chain continues from it (left to right, the reverse of `match_from`).
fn has_from(d: &Dom, f: &ElemFacts, anchor: u32, parts: &[Compound], k: usize) -> bool {
    let step = |c: u32| match_compound(d, f, c, &parts[k]) && (k + 1 == parts.len() || has_from(d, f, c, parts, k + 1));
    let elem = |c: u32| d.nodes[c as usize].kind == Kind::Element;
    match parts[k].comb.unwrap_or(Comb::Desc) {
        Comb::Child => d.children(anchor).any(|c| elem(c) && step(c)),
        Comb::Desc => {
            // pre-order over the subtree: a node's descendants are the ids up to its `end`
            let end = d.nodes[anchor as usize].end;
            (anchor + 1..end).any(|c| elem(c) && step(c))
        }
        Comb::Next => {
            let mut s = d.next_sibling(anchor);
            while s != NONE && !elem(s) { s = d.next_sibling(s); }
            s != NONE && step(s)
        }
        Comb::Later => {
            let mut s = d.next_sibling(anchor);
            while s != NONE { if elem(s) && step(s) { return true; } s = d.next_sibling(s); }
            false
        }
    }
}

fn nth_ok(a: i32, b: i32, pos: i32) -> bool {
    if a == 0 { pos == b } else { let k = pos - b; k % a == 0 && k / a >= 0 }
}

fn match_compound(d: &Dom, f: &ElemFacts, e: u32, c: &Compound) -> bool {
    c.simples.iter().all(|s| match_simple(d, f, e, s))
}

/// Right-to-left match of a complex selector ending at element `e`.
pub fn match_parts(d: &Dom, f: &ElemFacts, e: u32, parts: &[Compound]) -> bool {
    let last = parts.len() - 1;
    if !match_compound(d, f, e, &parts[last]) { return false; }
    match_from(d, f, e, parts, last)
}

fn match_from(d: &Dom, f: &ElemFacts, e: u32, parts: &[Compound], k: usize) -> bool {
    if k == 0 { return true; }
    let comb = parts[k].comb.unwrap_or(Comb::Desc);
    let left = &parts[k - 1];
    match comb {
        Comb::Child => {
            let p = d.nodes[e as usize].parent;
            p != NONE && d.nodes[p as usize].kind == Kind::Element && match_compound(d, f, p, left) && match_from(d, f, p, parts, k - 1)
        }
        Comb::Desc => {
            let mut p = d.nodes[e as usize].parent;
            while p != NONE && d.nodes[p as usize].kind == Kind::Element {
                if match_compound(d, f, p, left) && match_from(d, f, p, parts, k - 1) { return true; }
                p = d.nodes[p as usize].parent;
            }
            false
        }
        Comb::Next => {
            let s = prev_elem(d, e);
            s != NONE && match_compound(d, f, s, left) && match_from(d, f, s, parts, k - 1)
        }
        Comb::Later => {
            let mut s = prev_elem(d, e);
            while s != NONE {
                if match_compound(d, f, s, left) && match_from(d, f, s, parts, k - 1) { return true; }
                s = prev_elem(d, s);
            }
            false
        }
    }
}

impl Sheet {
    /// Matching rules for element `e`, as (specificity, order, rule index).
    pub fn matching(&self, d: &Dom, f: &ElemFacts, e: u32, out: &mut Vec<(u32, u32, u32)>) {
        self.matching_bloom(d, f, e, &[u64::MAX; 4], out)
    }

    /// Fingerprint of the sibling-position-sensitive rules that match `e` (0 = none). Two identical siblings
    /// with the same fingerprint get identical cascades.
    pub fn sibling_fingerprint(&self, d: &Dom, f: &ElemFacts, e: u32, anc: &Bloom) -> u64 {
        let mut h: u64 = 0;
        let mut eval = |v: &[u32]| for &ri in v {
            let r = &self.rules[ri as usize];
            if !r.sib || (0..4).any(|k| r.anc[k] & !anc[k] != 0) { continue; }
            if match_parts(d, f, e, &r.sel.parts) { h = h.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(ri as u64 + 1); }
        };
        if let Some(id) = &f.id[e as usize] { if let Some(v) = self.by_id.get(id) { eval(v); } }
        for c in &f.classes[e as usize] { if let Some(v) = self.by_class.get(c) { eval(v); } }
        if let Some(v) = self.by_tag.get(&d.tag(e).to_ascii_lowercase()) { eval(v); }
        for a in d.attr_list(e) { if let Some(v) = self.by_attr.get(&*d.names[a.name as usize].local) { eval(v); } }
        eval(&self.universal);
        h
    }

    /// Could any rule that might match `e` tell it apart from an identical sibling?
    pub fn sibling_sensitive_for(&self, d: &Dom, f: &ElemFacts, e: u32) -> bool {
        let chk = |v: Option<&Vec<u32>>| v.map_or(false, |v| v.iter().any(|&ri| self.rules[ri as usize].sib));
        if let Some(id) = &f.id[e as usize] { if chk(self.by_id.get(id)) { return true; } }
        for c in &f.classes[e as usize] { if chk(self.by_class.get(c)) { return true; } }
        chk(self.by_tag.get(&d.tag(e).to_ascii_lowercase())) || self.universal.iter().any(|&ri| self.rules[ri as usize].sib)
    }

    /// As `matching`, rejecting a rule at once when its required ancestor keys are absent from `anc`.
    pub fn matching_bloom(&self, d: &Dom, f: &ElemFacts, e: u32, anc: &Bloom, out: &mut Vec<(u32, u32, u32)>) {
        // candidate rules from the element's buckets; small per-element sets → reuse one scratch buffer
        thread_local! { static CAND: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::with_capacity(256)); }
        CAND.with(|c| {
            let mut cand = c.borrow_mut();
            cand.clear();
            if let Some(id) = &f.id[e as usize] { if let Some(v) = self.by_id.get(id) { cand.extend_from_slice(v); } }
            for cl in &f.classes[e as usize] { if let Some(v) = self.by_class.get(cl) { cand.extend_from_slice(v); } }
            let tag = d.tag(e);
            let tv = if tag.bytes().any(|b| b.is_ascii_uppercase()) { self.by_tag.get(&tag.to_ascii_lowercase()) } else { self.by_tag.get(tag) };
            if let Some(v) = tv { cand.extend_from_slice(v); }
            if !self.by_attr.is_empty() { for a in d.attr_list(e) { if let Some(v) = self.by_attr.get(&*d.names[a.name as usize].local) { cand.extend_from_slice(v); } } }
            cand.extend_from_slice(&self.universal);
            if cand.len() > 1 { cand.sort_unstable(); cand.dedup(); }
            for &ri in cand.iter() {
                let r = &self.rules[ri as usize];
                if (0..4).any(|k| r.anc[k] & !anc[k] != 0) { continue; }
                if match_parts(d, f, e, &r.sel.parts) { out.push((r.sel.spec, r.order, ri)); }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokenizes_and_matches() {
        let d = Dom::parse(b"<div id=a class='x y'><p class=y>hi</p><p>2</p></div>");
        let f = ElemFacts::new(&d);
        let m = Media { width: 1280.0, height: 800.0 };
        let mut s = Sheet::new();
        let mut o = 0;
        s.add("#a > p.y { color: red } div p:nth-child(2) { color: blue !important } @media (max-width: 600px) { p { x: 1 } }", &m, &mut o);
        assert_eq!(s.rules.len(), 2);
        let ps: Vec<u32> = (0..d.nodes.len() as u32).filter(|&i| d.tag(i) == "p").collect();
        let mut out = Vec::new();
        s.matching(&d, &f, ps[0], &mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        s.matching(&d, &f, ps[1], &mut out);
        assert_eq!(out.len(), 1);
        assert!(s.rules[1].decls[0].important);
    }
}
