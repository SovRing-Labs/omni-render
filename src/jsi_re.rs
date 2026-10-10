// SPDX-License-Identifier: MIT OR Apache-2.0
//! jsi_re — a backtracking regular-expression engine for jsi (ECMAScript subset): literals, escapes (\d \w \s and
//! negations, \b, \n \t, \uXXXX), `.`, classes with ranges and negation, ^ $ (multiline with m), groups (capturing,
//! (?:…), named (?<name>…), lookahead (?=…) (?!…), lookbehind (?<=…) (?<!…)), backreferences (\1, \k<name>), alternation,
//! greedy and lazy quantifiers (* + ? {n} {n,} {n,m}), flags g i m s (ECMA-262 §22.2).
use crate::jsi::{st, Ctl, Interp, Kind, Val};

#[derive(Debug, Clone)]
enum Node { Char(char), Any, Class(Vec<(char, char)>, bool), Start, End, WordB(bool), Group(Box<Node>, Option<usize>), Look(Box<Node>, bool), Alt(Vec<Node>), Seq(Vec<Node>), Rep(Box<Node>, usize, usize, bool), Back(usize),
    /// lookbehind (?<=…) / (?<!…) (§22.2.2.4 Assertion with direction backward)
    LookB(Box<Node>, bool) }

struct P<'a> { c: &'a [char], i: usize, groups: usize, names: Vec<(String, usize)>, unicode: bool }

/// Sorted, merged ranges, so a big class (a \p{…} property) is searched by bisection.
fn norm_ranges(mut v: Vec<(char, char)>) -> Vec<(char, char)> {
    v.retain(|r| r.0 <= r.1); v.sort();
    let mut out: Vec<(char, char)> = Vec::with_capacity(v.len());
    for (lo, hi) in v { match out.last_mut() { Some(l) if (lo as u32) <= l.1 as u32 + 1 => { if hi > l.1 { l.1 = hi; } } _ => out.push((lo, hi)) } }
    out
}
#[inline] fn in_ranges(r: &[(char, char)], c: char) -> bool {
    if r.len() <= 8 { return r.iter().any(|&(lo, hi)| lo <= c && c <= hi); }
    let i = r.partition_point(|&(lo, _)| lo <= c);
    i > 0 && c <= r[i - 1].1
}
#[inline] fn class_hit(r: &[(char, char)], c: char, icase: bool) -> bool {
    in_ranges(r, c) || (icase && { let l = c.to_ascii_lowercase(); let u = c.to_ascii_uppercase(); (l != c && in_ranges(r, l)) || (u != c && in_ranges(r, u)) })
}
thread_local! { static PROPS: std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<Vec<(char, char)>>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }
/// Unicode property escapes \p{…} (§22.2.2.9 UnicodeMatchProperty) without the UCD tables: general categories, binary
/// properties and scripts approximated from Rust's char predicates and block ranges, enumerated once into ranges.
fn prop_ranges(name: &str) -> Option<std::rc::Rc<Vec<(char, char)>>> {
    if let Some(r) = PROPS.with(|p| p.borrow().get(name).cloned()) { return Some(r); }
    let (key, val) = match name.split_once('=') { Some((k, v)) => (k, v), None => ("", name) };
    let blocks = |bs: &[(u32, u32)]| -> Vec<(char, char)> { bs.iter().filter_map(|&(a, b)| Some((char::from_u32(a)?, char::from_u32(b)?))).collect() };
    let marks: &[(u32, u32)] = &[(0x0300, 0x036F), (0x0483, 0x0489), (0x0591, 0x05BD), (0x05BF, 0x05BF), (0x05C1, 0x05C2), (0x05C4, 0x05C5), (0x05C7, 0x05C7), (0x0610, 0x061A), (0x064B, 0x065F), (0x0670, 0x0670), (0x06D6, 0x06DC), (0x06DF, 0x06E4), (0x06E7, 0x06E8), (0x06EA, 0x06ED), (0x0900, 0x0903), (0x093A, 0x094F), (0x0951, 0x0957), (0x0962, 0x0963), (0x0981, 0x0983), (0x09BC, 0x09D7), (0x0E31, 0x0E31), (0x0E34, 0x0E3A), (0x0E47, 0x0E4E), (0x1AB0, 0x1AFF), (0x1DC0, 0x1DFF), (0x20D0, 0x20FF), (0x302A, 0x302F), (0x3099, 0x309A), (0xFE00, 0xFE0F), (0xFE20, 0xFE2F), (0xE0100, 0xE01EF)];
    let script = |sc: &str| -> Option<Vec<(char, char)>> { Some(blocks(match sc {
        "Latin" | "Latn" => &[(0x41, 0x5A), (0x61, 0x7A), (0xAA, 0xAA), (0xBA, 0xBA), (0xC0, 0xD6), (0xD8, 0xF6), (0xF8, 0x24F), (0x1E00, 0x1EFF), (0x2C60, 0x2C7F), (0xA720, 0xA7FF), (0xFF21, 0xFF3A), (0xFF41, 0xFF5A)],
        "Greek" | "Grek" => &[(0x370, 0x373), (0x375, 0x3FF), (0x1F00, 0x1FFF)], "Cyrillic" | "Cyrl" => &[(0x400, 0x52F), (0x1C80, 0x1C8F), (0x2DE0, 0x2DFF), (0xA640, 0xA69F)],
        "Han" | "Hani" => &[(0x2E80, 0x2FDF), (0x3005, 0x3005), (0x3007, 0x3007), (0x3021, 0x3029), (0x3038, 0x303B), (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF), (0x20000, 0x3134F)],
        "Hiragana" | "Hira" => &[(0x3041, 0x309F)], "Katakana" | "Kana" => &[(0x30A0, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)], "Hangul" | "Hang" => &[(0x1100, 0x11FF), (0x3130, 0x318F), (0xAC00, 0xD7AF)],
        "Arabic" | "Arab" => &[(0x600, 0x6FF), (0x750, 0x77F), (0x8A0, 0x8FF), (0xFB50, 0xFDFF), (0xFE70, 0xFEFF)], "Hebrew" | "Hebr" => &[(0x591, 0x5F4), (0xFB1D, 0xFB4F)],
        "Thai" => &[(0xE01, 0xE5B)], "Devanagari" | "Deva" => &[(0x900, 0x97F), (0xA8E0, 0xA8FF)], "Armenian" | "Armn" => &[(0x531, 0x58F)], "Georgian" | "Geor" => &[(0x10A0, 0x10FF), (0x2D00, 0x2D2F)],
        "Common" | "Zyyy" => &[(0x0, 0x40), (0x5B, 0x60), (0x7B, 0xA9), (0x2000, 0x206F), (0x20A0, 0x20CF), (0x2100, 0x2BFF), (0x3000, 0x3004), (0xFF01, 0xFF20), (0x1F000, 0x1FAFF)],
        _ => return None })) };
    let pred = |f: &dyn Fn(char) -> bool| -> Vec<(char, char)> { let mut v: Vec<(char, char)> = Vec::new(); for u in 0..=0x10FFFFu32 { let Some(c) = char::from_u32(u) else { continue }; if f(c) { match v.last_mut() { Some(l) if l.1 as u32 + 1 == u => l.1 = c, _ => v.push((c, c)) } } } v };
    let is_mark = |c: char| marks.iter().any(|&(a, b)| (a..=b).contains(&(c as u32)));
    let emoji: &[(u32, u32)] = &[(0x00A9, 0x00A9), (0x00AE, 0x00AE), (0x203C, 0x203C), (0x2049, 0x2049), (0x2122, 0x2122), (0x2139, 0x2139), (0x2194, 0x21AA), (0x231A, 0x23FF), (0x24C2, 0x24C2), (0x25AA, 0x25FE), (0x2600, 0x27BF), (0x2934, 0x2935), (0x2B05, 0x2B55), (0x3030, 0x3030), (0x303D, 0x303D), (0x3297, 0x3299), (0x1F000, 0x1FAFF)];
    let v: Vec<(char, char)> = match (key, val) {
        ("Script" | "sc" | "Script_Extensions" | "scx", sc) => script(sc)?,
        (_, "L" | "Letter" | "Alphabetic" | "Alpha") => pred(&|c| c.is_alphabetic()),
        (_, "ID_Start" | "IDS" | "XID_Start" | "XIDS") => pred(&|c| c.is_alphabetic() || matches!(c, '\u{2118}' | '\u{212E}' | '\u{309B}' | '\u{309C}')),
        (_, "ID_Continue" | "IDC" | "XID_Continue" | "XIDC") => pred(&|c| c.is_alphanumeric() || c == '_' || is_mark(c) || matches!(c, '\u{203F}' | '\u{2040}' | '\u{2054}' | '\u{FE33}' | '\u{FE34}' | '\u{FE4D}'..='\u{FE4F}' | '\u{FF3F}' | '\u{B7}' | '\u{387}' | '\u{19DA}')),
        (_, "Lu" | "Uppercase_Letter" | "Uppercase" | "Upper") => pred(&|c| c.is_uppercase()),
        (_, "Ll" | "Lowercase_Letter" | "Lowercase" | "Lower") => pred(&|c| c.is_lowercase()),
        (_, "Lo" | "Other_Letter") => pred(&|c| c.is_alphabetic() && !c.is_uppercase() && !c.is_lowercase() && !is_mark(c)),
        (_, "Lt" | "Titlecase_Letter") => blocks(&[(0x1C5, 0x1C5), (0x1C8, 0x1C8), (0x1CB, 0x1CB), (0x1F2, 0x1F2), (0x1F88, 0x1F8F), (0x1F98, 0x1F9F), (0x1FA8, 0x1FAF), (0x1FBC, 0x1FBC), (0x1FCC, 0x1FCC), (0x1FFC, 0x1FFC)]),
        (_, "Lm" | "Modifier_Letter") => blocks(&[(0x2B0, 0x2C1), (0x2C6, 0x2D1), (0x2E0, 0x2E4), (0x2EC, 0x2EC), (0x2EE, 0x2EE), (0x374, 0x374), (0x37A, 0x37A), (0x3005, 0x3005), (0x309D, 0x309E), (0x30FC, 0x30FE)]),
        (_, "N" | "Number") => pred(&|c| c.is_numeric()),
        (_, "Nd" | "Decimal_Number" | "digit") => pred(&|c| c.is_numeric() && { let u = c as u32; (0x30..=0x39).contains(&u) || (0x660..=0x669).contains(&u) || (0x6F0..=0x6F9).contains(&u) || (0x966..=0x96F).contains(&u) || (0xFF10..=0xFF19).contains(&u) || (u > 0x9FF && u < 0x1D7FF && !(0x2150..=0x218F).contains(&u) && !(0x2460..=0x24FF).contains(&u) && !(0x2070..=0x209F).contains(&u) && !(0x3000..=0x33FF).contains(&u) && !(0xB2..=0xBE).contains(&u)) }),
        (_, "M" | "Mark" | "Combining_Mark" | "Mn" | "Nonspacing_Mark" | "Mc" | "Spacing_Mark") => blocks(marks),
        (_, "P" | "Punctuation" | "punct") => pred(&|c| (c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)) || matches!(c as u32, 0xA1 | 0xA7 | 0xAB | 0xB6 | 0xB7 | 0xBB | 0xBF | 0x2010..=0x2027 | 0x2030..=0x205E | 0x3001..=0x3003 | 0x3008..=0x3011 | 0x3014..=0x301F | 0xFF01..=0xFF0F)),
        (_, "S" | "Symbol") => pred(&|c| "$+<=>^`|~".contains(c) || matches!(c as u32, 0xA2..=0xA6 | 0xA8 | 0xA9 | 0xAC | 0xAE..=0xB1 | 0xB4 | 0xB8 | 0xD7 | 0xF7 | 0x20A0..=0x20CF | 0x2100..=0x214F | 0x2190..=0x23FF | 0x2500..=0x27BF | 0x2900..=0x2BFF | 0x1F000..=0x1FAFF)),
        (_, "Sc" | "Currency_Symbol") => pred(&|c| matches!(c as u32, 0x24 | 0xA2..=0xA5 | 0x58F | 0x60B | 0x9F2 | 0x9F3 | 0xE3F | 0x17DB | 0x20A0..=0x20CF | 0xFDFC | 0xFE69 | 0xFF04 | 0xFFE0 | 0xFFE1 | 0xFFE5 | 0xFFE6)),
        (_, "Z" | "Separator" | "Zs" | "Space_Separator") => blocks(&[(0x20, 0x20), (0xA0, 0xA0), (0x1680, 0x1680), (0x2000, 0x200A), (0x2028, 0x2029), (0x202F, 0x202F), (0x205F, 0x205F), (0x3000, 0x3000)]),
        (_, "White_Space" | "space") => pred(&|c| c.is_whitespace()),
        (_, "Cc" | "Control" | "cntrl") => pred(&|c| c.is_control()),
        (_, "Emoji" | "Emoji_Presentation" | "EPres" | "Extended_Pictographic" | "ExtPict") => { let mut v = blocks(emoji); if val == "Emoji" { v.extend(blocks(&[(0x23, 0x23), (0x2A, 0x2A), (0x30, 0x39)])); } v }
        (_, "Emoji_Modifier" | "EMod") => blocks(&[(0x1F3FB, 0x1F3FF)]),
        (_, "Emoji_Component" | "EComp") => blocks(&[(0x23, 0x23), (0x2A, 0x2A), (0x30, 0x39), (0x200D, 0x200D), (0x20E3, 0x20E3), (0xFE0F, 0xFE0F), (0x1F1E6, 0x1F1FF), (0x1F3FB, 0x1F3FF), (0xE0020, 0xE007F)]),
        (_, "Regional_Indicator" | "RI") => blocks(&[(0x1F1E6, 0x1F1FF)]),
        (_, "ASCII") => blocks(&[(0, 0x7F)]),
        (_, "Any" | "Assigned") => blocks(&[(0, 0xD7FF), (0xE000, 0x10FFFF)]),
        (_, "ASCII_Hex_Digit" | "AHex" | "Hex_Digit") => blocks(&[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)]),
        (_, sc) => script(sc)?,
    };
    let r = std::rc::Rc::new(norm_ranges(v));
    PROPS.with(|p| p.borrow_mut().insert(name.to_string(), r.clone()));
    Some(r)
}

/// GroupSpecifier names in pattern order (§22.2.1.2 static semantics), so `\k<name>` can refer forward.
fn group_names(c: &[char]) -> Vec<(String, usize)> {
    let (mut out, mut n, mut i, mut in_cls) = (Vec::new(), 0usize, 0usize, false);
    while i < c.len() {
        match c[i] {
            '\\' => { i += 2; continue; }
            '[' => in_cls = true, ']' => in_cls = false,
            '(' if !in_cls => {
                if c.get(i + 1) != Some(&'?') { n += 1; }
                else if c.get(i + 2) == Some(&'<') && !matches!(c.get(i + 3), Some('=') | Some('!')) { n += 1; let s = i + 3; let mut e = s; while e < c.len() && c[e] != '>' { e += 1; } out.push((c[s..e].iter().collect(), n)); }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn class_escape(e: char) -> Option<(Vec<(char, char)>, bool)> {
    let d = vec![('0', '9')]; let w = vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
    let s = vec![(' ', ' '), ('\t', '\r'), ('\u{a0}', '\u{a0}'), ('\u{2028}', '\u{2029}'), ('\u{feff}', '\u{feff}')];
    Some(match e { 'd' => (d, false), 'D' => (d, true), 'w' => (w, false), 'W' => (w, true), 's' => (s, false), 'S' => (s, true), _ => return None })
}
/// The complement of a set of code-point ranges over 0..=U+10FFFF (a negated class escape inside a class, `[\D…]`).
fn complement(r: &[(char, char)]) -> Vec<(char, char)> {
    let mut v: Vec<(u32, u32)> = r.iter().map(|&(a, b)| (a as u32, b as u32)).collect(); v.sort();
    let mut out = Vec::new(); let mut next = 0u32;
    for (a, b) in v { if a > next { out.push((next, a - 1)); } next = next.max(b + 1); }
    if next <= 0x10FFFF { out.push((next, 0x10FFFF)); }
    out.into_iter().filter_map(|(a, b)| { let a = char::from_u32(a).unwrap_or('\u{E000}'); let b = char::from_u32(b).unwrap_or('\u{D7FF}'); if a <= b { Some((a, b)) } else { None } }).collect()
}
fn simple_escape(e: char) -> char { match e { 'n' => '\n', 't' => '\t', 'r' => '\r', 'f' => '\u{c}', 'v' => '\u{b}', '0' => '\0', x => x } }

impl P<'_> {
    fn peek(&self) -> Option<char> { self.c.get(self.i).copied() }
    /// `\p{Name}` / `\p{Key=Value}` after the `p`: the property's ranges.
    fn prop(&mut self) -> Result<std::rc::Rc<Vec<(char, char)>>, String> {
        self.i += 1; let s = self.i;
        while self.peek().map_or(false, |c| c != '}') { self.i += 1; }
        let name: String = self.c[s..self.i].iter().collect(); self.i += 1;
        prop_ranges(&name).ok_or_else(|| format!("Invalid property name {name}"))
    }
    fn hex(&mut self, n: usize) -> Option<u32> { let h: String = self.c.get(self.i..self.i + n)?.iter().collect(); let v = u32::from_str_radix(&h, 16).ok()?; self.i += n; Some(v) }
    /// CharacterEscape (§22.2.1): \xHH, \uHHHH (a surrogate pair joins), \u{H…}, \cX, \0, the control letters; others stand for themselves.
    fn esc_char(&mut self, e: char) -> char {
        match e {
            'x' => match self.hex(2) { Some(v) => char::from_u32(v).unwrap_or('x'), None => 'x' },
            'u' => {
                if self.peek() == Some('{') { let s = self.i; self.i += 1; let st = self.i; while self.peek().map_or(false, |c| c.is_ascii_hexdigit()) { self.i += 1; } let h: String = self.c[st..self.i].iter().collect(); if self.peek() == Some('}') { self.i += 1; if let Some(c) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) { return c; } } self.i = s; return 'u'; }
                match self.hex(4) {
                    Some(hi) if (0xD800..0xDC00).contains(&hi) && self.c.get(self.i) == Some(&'\\') && self.c.get(self.i + 1) == Some(&'u') => { let save = self.i; self.i += 2; match self.hex(4) { Some(lo) if (0xDC00..0xE000).contains(&lo) => char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)).unwrap_or('\u{fffd}'), _ => { self.i = save; char::from_u32(0xF0000 + hi).unwrap() } } }
                    Some(v) if (0xD800..0xE000).contains(&v) => char::from_u32(0xF0000 + v).unwrap(),
                    Some(v) => char::from_u32(v).unwrap_or('\u{fffd}'),
                    None => 'u',
                }
            }
            'c' => match self.peek() { Some(l) if l.is_ascii_alphabetic() => { self.i += 1; char::from_u32(l as u32 % 32).unwrap() } _ => '\\' },
            e => simple_escape(e),
        }
    }
    fn alt(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.seq()?];
        while self.peek() == Some('|') { self.i += 1; alts.push(self.seq()?); }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }
    fn seq(&mut self) -> Result<Node, String> {
        let mut v = Vec::new();
        while let Some(c) = self.peek() { if c == '|' || c == ')' { break; } let a = self.atom()?; v.push(self.quant(a)?); }
        Ok(Node::Seq(v))
    }
    fn num(&mut self) -> Option<usize> { let s = self.i; while self.peek().map_or(false, |c| c.is_ascii_digit()) { self.i += 1; } if s == self.i { None } else { self.c[s..self.i].iter().collect::<String>().parse().ok() } }
    fn quant(&mut self, a: Node) -> Result<Node, String> {
        let (lo, hi) = match self.peek() {
            Some('*') => { self.i += 1; (0, usize::MAX) } Some('+') => { self.i += 1; (1, usize::MAX) } Some('?') => { self.i += 1; (0, 1) }
            Some('{') => {
                let save = self.i; self.i += 1;
                match self.num() {
                    Some(lo) => { let hi = if self.peek() == Some(',') { self.i += 1; self.num().unwrap_or(usize::MAX) } else { lo }; if self.peek() != Some('}') { self.i = save; return Ok(a); } self.i += 1; (lo, hi) }
                    None => { self.i = save; return Ok(a); }
                }
            }
            _ => return Ok(a),
        };
        let lazy = if self.peek() == Some('?') { self.i += 1; true } else { false };
        Ok(Node::Rep(Box::new(a), lo, hi, lazy))
    }
    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or("unexpected end")?; self.i += 1;
        Ok(match c {
            '.' => Node::Any, '^' => Node::Start, '$' => Node::End,
            '(' => {
                let (cap, look) = if self.c[self.i..].starts_with(&['?', ':']) { self.i += 2; (false, None) }
                    else if self.c[self.i..].starts_with(&['?', '=']) { self.i += 2; (false, Some(false)) }
                    else if self.c[self.i..].starts_with(&['?', '!']) { self.i += 2; (false, Some(true)) }
                    else if self.c[self.i..].starts_with(&['?', '<', '=']) { self.i += 3; (false, Some(false)) }
                    else if self.c[self.i..].starts_with(&['?', '<', '!']) { self.i += 3; (false, Some(true)) }
                    else if self.c[self.i..].starts_with(&['?', '<']) { while self.peek().map_or(false, |c| c != '>') { self.i += 1; } self.i += 1; (true, None) }
                    else { (true, None) };
                let behind = look.is_some() && self.c[..self.i].ends_with(&['<', '=']) || look == Some(true) && self.c[..self.i].ends_with(&['<', '!']);
                let idx = if cap { self.groups += 1; Some(self.groups) } else { None };
                let inner = self.alt()?;
                if self.peek() != Some(')') { return Err("missing )".into()); }
                self.i += 1;
                match look { Some(neg) if behind => Node::LookB(Box::new(inner), neg), Some(neg) => Node::Look(Box::new(inner), neg), None => Node::Group(Box::new(inner), idx) }
            }
            '[' => {
                let neg = if self.peek() == Some('^') { self.i += 1; true } else { false };
                let mut ranges = Vec::new(); let mut first = true;
                loop {
                    let ch = self.peek().ok_or("unterminated class")?;
                    // `]` always closes: `[]` matches nothing and `[^]` any character (§22.2.1 ClassContents may be empty)
                    if ch == ']' { self.i += 1; let _ = first; break; }
                    first = false; self.i += 1;
                    let lo = if ch == '\\' { let e = self.peek().ok_or("bad escape")?; self.i += 1; if let Some((r, n)) = class_escape(e) { if n { ranges.extend(complement(&r)); } else { ranges.extend(r); } continue; }
                        if (e == 'p' || e == 'P') && self.unicode && self.peek() == Some('{') { let r = self.prop()?; if e == 'P' { ranges.extend(complement(&r)); } else { ranges.extend(r.iter().copied()); } continue; } if e == 'b' { '\u{8}' } else { self.esc_char(e) } } else { ch };
                    if self.peek() == Some('-') && self.c.get(self.i + 1).map_or(false, |&x| x != ']') {
                        self.i += 1; let h = self.peek().unwrap(); self.i += 1;
                        let hi = if h == '\\' { let e = self.peek().unwrap_or('\\'); self.i += 1; self.esc_char(e) } else { h };
                        ranges.push((lo, hi));
                    } else { ranges.push((lo, lo)); }
                }
                Node::Class(norm_ranges(ranges), neg)
            }
            '\\' => {
                let e = self.peek().ok_or("trailing backslash")?; self.i += 1;
                if let Some((r, n)) = class_escape(e) { Node::Class(norm_ranges(r), n) }
                else if (e == 'p' || e == 'P') && self.unicode && self.peek() == Some('{') { let r = self.prop()?; Node::Class((*r).clone(), e == 'P') }
                else if e == 'b' { Node::WordB(true) } else if e == 'B' { Node::WordB(false) }
                else if e.is_ascii_digit() && e != '0' { Node::Back(e.to_digit(10).unwrap() as usize) }
                else if e == 'k' && self.peek() == Some('<') { let s = self.i + 1; let mut j = s; while j < self.c.len() && self.c[j] != '>' { j += 1; } let name: String = self.c[s..j].iter().collect(); self.i = j + 1; match self.names.iter().find(|(n, _)| *n == name) { Some((_, g)) => Node::Back(*g), None => return Err(format!("invalid named reference {name}")) } }
                else { Node::Char(self.esc_char(e)) }
            }
            x => Node::Char(x),
        })
    }
}

pub struct Re { node: Node, groups: usize, icase: bool, multi: bool, dotall: bool, backref: bool, counted: bool, lead: Lead, pub names: Vec<(String, usize)> }

/// Counted repetition makes the failure memo unsound (B1 differential, 2026-10-06: 4 / 160,000 false "no match"): a repeat's
/// continuation depends on its iteration count unless every repeat is `*`, `+` or `?` (lo ≤ 1 and hi ∈ {1, ∞}).
fn has_counted(n: &Node) -> bool {
    match n {
        Node::Rep(inner, lo, hi, _) => *lo > 1 || (*hi != 1 && *hi != usize::MAX) || has_counted(inner),
        Node::Group(inner, _) | Node::Look(inner, _) | Node::LookB(inner, _) => has_counted(inner),
        Node::Alt(v) | Node::Seq(v) => v.iter().any(has_counted),
        _ => false,
    }
}

/// JSI-PERF-2: what a match must start with, so `exec_at` skips start offsets that cannot match without entering the matcher.
/// Derived from the tree's first consuming element: a literal char (case-insensitive only when it has no case) or a `^` anchor.
#[derive(Debug, Clone, PartialEq)]
enum Lead { Any, Char(char), Anchor, Set(Box<[bool; 128]>, bool) }   // Set = possible first chars (ASCII bitmap, any non-ASCII)
/// First-char analysis: (ASCII bitmap, non-ASCII possible, nullable). None = cannot bound the first char (Any / backreference).
fn first_of(n: &Node, icase: bool) -> Option<([bool; 128], bool, bool)> {
    let mut set = [false; 128];
    let add = |set: &mut [bool; 128], c: char| -> bool { if (c as u32) < 128 { set[c as usize] = true; if icase { set[c.to_ascii_lowercase() as usize] = true; set[c.to_ascii_uppercase() as usize] = true; } false } else { true } };
    match n {
        Node::Char(c) => { let na = add(&mut set, *c) || icase; Some((set, na, false)) }   // icase: non-ASCII may fold to ASCII (U+212A → k)
        Node::Class(r, neg) => {
            for b in 0..128u8 { let c = b as char; set[b as usize] = class_hit(r, c, icase) != *neg; }
            Some((set, true, false))                               // non-ASCII: conservatively possible
        }
        Node::Start | Node::End | Node::WordB(_) | Node::Look(..) | Node::LookB(..) => Some((set, false, true)),   // zero-width: consumes nothing
        Node::Group(inner, _) => first_of(inner, icase),
        Node::Rep(inner, lo, ..) => { let (s2, na, nl) = first_of(inner, icase)?; Some((s2, na, nl || *lo == 0)) }
        Node::Alt(v) => { let mut na = false; let mut nl = false; for a in v { let (s2, n2, l2) = first_of(a, icase)?; for i in 0..128 { set[i] |= s2[i]; } na |= n2; nl |= l2; } Some((set, na, nl)) }
        Node::Seq(v) => { let mut na = false; for e in v { let (s2, n2, l2) = first_of(e, icase)?; for i in 0..128 { set[i] |= s2[i]; } na |= n2; if !l2 { return Some((set, na, false)); } } Some((set, na, true)) }
        Node::Any | Node::Back(_) => None,
    }
}
fn lead_of(n: &Node, icase: bool) -> Lead {
    match n {
        Node::Seq(v) => v.first().map_or(Lead::Any, |h| lead_of(h, icase)),
        Node::Group(inner, _) => lead_of(inner, icase),
        Node::Rep(inner, lo, ..) if *lo >= 1 => lead_of(inner, icase),
        Node::Char(c) if !icase || (c.to_lowercase().eq(c.to_uppercase())) => Lead::Char(*c),
        Node::Start => Lead::Anchor,
        _ => Lead::Any,
    }
}

/// Does this tree contain a `\1`-style backreference? Failure memoization is keyed on
/// (node, position) and is only sound when match success does not depend on the capture
/// table — which `Node::Back` is the one way to violate. Patterns without backrefs get
/// memoized; patterns with them keep the plain backtracking path.
fn has_backref(n: &Node) -> bool {
    match n {
        Node::Back(_) => true,
        Node::Group(inner, _) | Node::Look(inner, _) | Node::LookB(inner, _) | Node::Rep(inner, ..) => has_backref(inner),
        Node::Alt(v) | Node::Seq(v) => v.iter().any(has_backref),
        _ => false,
    }
}

fn has_lookbehind(n: &Node) -> bool {
    match n { Node::LookB(..) => true, Node::Group(inner, _) | Node::Look(inner, _) | Node::Rep(inner, ..) => has_lookbehind(inner), Node::Alt(v) | Node::Seq(v) => v.iter().any(has_lookbehind), _ => false }
}

pub fn compile(pat: &str, flags: &str) -> Result<Re, String> {
    let c: Vec<char> = pat.chars().collect();
    let mut p = P { c: &c, i: 0, groups: 0, names: group_names(&c), unicode: flags.contains('u') || flags.contains('v') };
    let node = p.alt()?;
    if p.i != c.len() { return Err(format!("unexpected ) at {}", p.i)); }
    // Struct-literal fields evaluate in written order, so `node` would be moved before
    // the borrow below. Decide memo-eligibility first.
    // lookbehind bodies run under a position-dependent continuation, so the (node, offset) failure memo is off for them too
    let backref = has_backref(&node) || has_lookbehind(&node);
    let mut lead = lead_of(&node, flags.contains('i'));
    if lead == Lead::Any { if let Some((set, na, false)) = first_of(&node, flags.contains('i')) { lead = Lead::Set(Box::new(set), na); } }
    let counted = has_counted(&node);
    Ok(Re { node, groups: p.groups, icase: flags.contains('i'), multi: flags.contains('m'), dotall: flags.contains('s'), backref, counted, lead, names: p.names })
}

fn is_word(c: Option<&char>) -> bool { c.map_or(false, |c| c.is_ascii_alphanumeric() || *c == '_') }

/// Continuation-passing matcher: m(node, pos, caps, k) → does node match at pos and then k succeed?
fn m(re: &Re, n: &Node, s: &[char], i: usize, caps: &mut Vec<Option<(usize, usize)>>, k: &mut dyn FnMut(usize, &mut Vec<Option<(usize, usize)>>) -> bool, budget: &mut u64) -> bool {
    *budget += 1; if *budget > 5_000_000 { BUDGET_HIT.with(|b| b.set(true)); return false; }
    // Arm the failure memo only once a scan proves itself expensive (see memo_note), so
    // linear patterns never pay a hash. See the block comment above for why the key is sound.
    memo_note();
    if memo_on() {
        let key = (n as *const Node as usize, i);
        if MEMO.with(|g| g.borrow().contains(&key)) { return false; }
        if m_inner(re, n, s, i, caps, k, budget) { return true; }
        memo_record(key);
        return false;
    }
    m_inner(re, n, s, i, caps, k, budget)
}

fn m_inner(re: &Re, n: &Node, s: &[char], i: usize, caps: &mut Vec<Option<(usize, usize)>>, k: &mut dyn FnMut(usize, &mut Vec<Option<(usize, usize)>>) -> bool, budget: &mut u64) -> bool {
    let eqc = |a: char, b: char| if re.icase { a.to_lowercase().eq(b.to_lowercase()) } else { a == b };
    match n {
        Node::Char(c) => i < s.len() && eqc(s[i], *c) && k(i + 1, caps),
        Node::Any => i < s.len() && (re.dotall || !matches!(s[i], '\n' | '\r' | '\u{2028}' | '\u{2029}')) && k(i + 1, caps),
        Node::Class(r, neg) => {
            if i >= s.len() { return false; }
            let c = s[i];
            class_hit(r, c, re.icase) != *neg && k(i + 1, caps)
        }
        Node::Start => (i == 0 || (re.multi && s[i - 1] == '\n')) && k(i, caps),
        Node::End => (i == s.len() || (re.multi && s[i] == '\n')) && k(i, caps),
        Node::WordB(want) => { let b = is_word(if i > 0 { s.get(i - 1) } else { None }) != is_word(s.get(i)); b == *want && k(i, caps) }
        Node::Back(g) => { match caps.get(*g).copied().flatten() { Some((a, b)) => { let l = b - a; i + l <= s.len() && (0..l).all(|j| eqc(s[i + j], s[a + j])) && k(i + l, caps) } None => k(i, caps) } }
        Node::Group(inner, idx) => {
            let saved = idx.map(|g| caps[g]);
            let r = m(re, inner, s, i, caps, &mut |j, c: &mut Vec<Option<(usize, usize)>>| { if let Some(g) = idx { let old = c[*g]; c[*g] = Some((i, j)); if k(j, c) { return true; } c[*g] = old; false } else { k(j, c) } }, budget);
            if !r { if let (Some(g), Some(sv)) = (idx, saved) { caps[*g] = sv; } }
            r
        }
        Node::Look(inner, neg) => { let mut c2 = caps.clone(); let ok = m(re, inner, s, i, &mut c2, &mut |_, _| true, budget); if ok != *neg { if ok { *caps = c2; } k(i, caps) } else { false } }
        // lookbehind: some start j ≤ i from which the body matches exactly up to i (nearest start first)
        Node::LookB(inner, neg) => {
            let mut found = None;
            for j in (0..=i).rev() { let mut c3 = caps.clone(); if m(re, inner, s, j, &mut c3, &mut |e, _| e == i, budget) { found = Some(c3); break; } }
            if found.is_some() != *neg { if let Some(c3) = found { *caps = c3; } k(i, caps) } else { false }
        }
        Node::Alt(v) => v.iter().any(|a| m(re, a, s, i, caps, k, budget)),
        Node::Seq(v) => seq(re, v, s, i, caps, k, budget),
        Node::Rep(inner, lo, hi, lazy) => rep(re, inner, *lo, *hi, *lazy, s, i, caps, k, budget, 0, usize::MAX),
    }
}

fn seq(re: &Re, v: &[Node], s: &[char], i: usize, caps: &mut Vec<Option<(usize, usize)>>, k: &mut dyn FnMut(usize, &mut Vec<Option<(usize, usize)>>) -> bool, budget: &mut u64) -> bool {
    match v.split_first() { None => k(i, caps), Some((h, t)) => m(re, h, s, i, caps, &mut |j, c: &mut Vec<Option<(usize, usize)>>| seq(re, t, s, j, c, k, budget_ref()), budget) }
}
// a shared budget would need a RefCell through the closures; continuation nesting carries its own soft budget
fn budget_ref() -> &'static mut u64 { thread_local! { static B: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; } unsafe { &mut *B.with(|b| b.as_ptr()) } }

// ── failure memoization ────────────────────────────────────────────────────────────────────
// `exec_at` retries the whole matcher from every start offset, so a pattern whose match
// attempt can walk far (the launcher's markdown FENCE, `` /```([^\n]*)\n?([\s\S]*?)(?:```|$)/g ``,
// whose lazy star runs to end-of-string when a fence is never closed) costs
// O(offsets x walk) per scan. Caching "(this node failed at this offset)" collapses that
// to O(nodes x offsets) — the classic memoized-backtracker bound.
//
// Two deliberate limits keep this off the hot path of ordinary patterns:
//   * it arms only after MEMO_TRIGGER steps inside one scan, so linear patterns such as
//     esc()'s four single-character replaces never allocate or hash anything;
//   * it is disabled entirely for patterns containing a backreference, where match success
//     depends on the capture table and (node, offset) is not a sound key.
// `exec_at` clears the memo on entry: the key is a raw node pointer, and a stack-reused
// `Re` from an earlier call could otherwise land on the same address.
const MEMO_TRIGGER: u64 = 20_000;
const MEMO_MAX: usize = 1_000_000;

thread_local! {
    static MEMO: std::cell::RefCell<std::collections::HashSet<(usize, usize)>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    static MEMO_ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static MEMO_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static MEMO_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// set when a scan exhausts the step budget: its "no match" is then a give-up, not an answer
    static BUDGET_HIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NO_MEMO_ENV: bool = std::env::var_os("JSI_RE_NO_MEMO").is_some();
}

fn memo_reset(backref: bool) {
    // `backref` here = "the (node, offset) failure key is unsound for this pattern" (backreference OR counted repetition, see
    // memo_unsound); JSI_RE_NO_MEMO=1 also keeps the memo off (A/B). MEMO_OFF makes memo_note skip arming, so it never turns on.
    let backref = backref || NO_MEMO_ENV.with(|v| *v);
    MEMO_OFF.with(|c| c.set(backref));
    MEMO_ON.with(|c| c.set(false));
    MEMO_STEPS.with(|c| c.set(0));
    if backref { return; }
    // clearing an empty table is not free (it rewrites the control bytes of its whole capacity), and a huge table left by
    // one pathological scan would tax every later exec: skip the empty case, drop an oversized table
    MEMO.with(|m| { let mut m = m.borrow_mut(); if !m.is_empty() { if m.capacity() > 1 << 16 { *m = std::collections::HashSet::new(); } else { m.clear(); } } });
}

#[inline]
fn memo_on() -> bool { MEMO_ON.with(|c| c.get()) }

#[inline]
fn memo_note() {
    if memo_on() || MEMO_OFF.with(|c| c.get()) { return; }
    let n = MEMO_STEPS.with(|c| { let v = c.get() + 1; c.set(v); v });
    if n >= MEMO_TRIGGER { MEMO_ON.with(|c| c.set(true)); }
}

fn memo_record(key: (usize, usize)) {
    if !memo_on() { return; }
    // All work happens inside the closure: a RefMut borrowed from a thread_local cannot
    // outlive it, so hoisting the guard out is a lifetime error, not a style choice.
    MEMO.with(|m| {
        let mut g = m.borrow_mut();
        if g.len() >= MEMO_MAX { g.clear(); MEMO_ON.with(|c| c.set(false)); } else { g.insert(key); }
    });
}

#[allow(clippy::too_many_arguments)]
fn rep(re: &Re, n: &Node, lo: usize, hi: usize, lazy: bool, s: &[char], i: usize, caps: &mut Vec<Option<(usize, usize)>>, k: &mut dyn FnMut(usize, &mut Vec<Option<(usize, usize)>>) -> bool, budget: &mut u64, count: usize, last: usize) -> bool {
    if count >= lo && lazy && k(i, caps) { return true; }
    if count < hi && last != i {
        let ok = m(re, n, s, i, caps, &mut |j, c: &mut Vec<Option<(usize, usize)>>| rep(re, n, lo, hi, lazy, s, j, c, k, budget_ref(), count + 1, i), budget);
        if ok { return true; }
    }
    count >= lo && !lazy && k(i, caps)
}

fn exec_at(re: &Re, s: &[char], from: usize) -> Option<Vec<Option<(usize, usize)>>> {
    // The memo key is a raw node pointer into `re`; a stack-reused Re from a previous
    // call can share its address, so the memo must not survive the call. exec_at is not
    // re-entrant (callbacks run after it returns), so clearing on entry is sound.
    memo_reset(re.backref || re.counted);
    BUDGET_HIT.with(|b| b.set(false));
    let mut caps = vec![None; re.groups + 1];                      // one capture table per scan, cleared per start (JSI-PERF-2)
    for start in from..=s.len() {
        thread_local! { static NO_LEAD: bool = std::env::var("JSI_RE_NO_LEAD").is_ok(); }   // A/B switch: prefilter off
        let lead = if NO_LEAD.with(|v| *v) { &Lead::Any } else { &re.lead };
        match lead {                                               // start offsets that cannot begin a match never enter m()
            Lead::Char(c) => if start >= s.len() || s[start] != *c { continue; },
            Lead::Set(set, na) => if start >= s.len() || !(if (s[start] as u32) < 128 { set[s[start] as usize] } else { *na }) { continue; },
            Lead::Anchor => if !(start == 0 || (re.multi && s[start - 1] == '\n')) { continue; },
            Lead::Any => {}
        }
        caps.iter_mut().for_each(|x| *x = None);
        let mut end = None;
        let mut b = 0u64;
        unsafe { *budget_ref() = 0; }
        if m(re, &re.node, s, start, &mut caps, &mut |j, _c| { end = Some(j); true }, &mut b) {
            // re-run to capture final groups (caps hold the successful path)
            caps[0] = Some((start, end.unwrap()));
            return Some(caps);
        }
    }
    None
}

// JSI-PERF-2: compiled patterns are cached per (pattern, flags) — re_of used to recompile on every call — and the last subject's
// Vec<char> is reused when the next call scans the same string (`while ((m = re.exec(s)))` re-collected s once per match).
thread_local! {
    static RE_CACHE: std::cell::RefCell<std::collections::HashMap<(String, String), std::rc::Rc<Re>>> = std::cell::RefCell::new(std::collections::HashMap::new());
    static SUBJECT: std::cell::RefCell<(String, std::rc::Rc<Vec<char>>)> = std::cell::RefCell::new((String::new(), std::rc::Rc::new(Vec::new())));
}
fn compiled(p: &str, f: &str) -> Result<std::rc::Rc<Re>, String> {
    if let Some(r) = RE_CACHE.with(|c| c.borrow().get(&(p.to_string(), f.to_string())).cloned()) { return Ok(r); }
    let r = std::rc::Rc::new(compile(p, f)?);
    RE_CACHE.with(|c| { let mut c = c.borrow_mut(); if c.len() >= 512 { c.clear(); } c.insert((p.to_string(), f.to_string()), r.clone()); });
    Ok(r)
}
fn chars_of(s: &str) -> std::rc::Rc<Vec<char>> {
    SUBJECT.with(|c| { let mut c = c.borrow_mut(); if c.0 != s { *c = (s.to_string(), std::rc::Rc::new(s.chars().collect())); } c.1.clone() })
}
fn re_of(it: &mut Interp, v: &Val) -> Result<(std::rc::Rc<Re>, bool), Ctl> {
    let (p, f) = match v { Val::Obj(o) => match &o.borrow().kind { Kind::RegExp(p, f) => (p.clone(), f.clone()), _ => (String::new(), String::new()) }, Val::Str(s) => (escape(s), String::new()), _ => (String::new(), String::new()) };
    let g = f.contains('g');
    compiled(&p, &f).map(|r| (r, g)).map_err(|e| Ctl::Throw(it.error("SyntaxError", &format!("Invalid regular expression: {e}"))))
}
fn escape(s: &str) -> String { s.chars().map(|c| if "\\^$.|?*+()[]{}/".contains(c) { format!("\\{c}") } else { c.to_string() }).collect() }
fn sub(s: &[char], r: Option<(usize, usize)>) -> Val { match r { Some((a, b)) => st(&s[a..b].iter().collect::<String>()), None => Val::Undef } }

pub fn test(it: &mut Interp, re: &Val, s: &str) -> Result<Val, Ctl> {
    // a global or sticky regexp is stateful: test is RegExpExec and moves lastIndex (§22.2.6.16)
    if re_of_flag(re, 'g') || re_of_flag(re, 'y') { return Ok(Val::Bool(!matches!(exec(it, s, re)?, Val::Null))); }
    let (r, _) = re_of(it, re)?; let cr = chars_of(s); let c: &[char] = &cr;
    if let Some(b) = dfa_test(&r, c) { return Ok(Val::Bool(b)); }                // B1: linear-time answer when the pattern qualifies
    Ok(Val::Bool(exec_at(&r, &c, 0).is_some())) }

/// `RegExp.prototype.exec`: one match from `lastIndex`, advancing it (global/sticky only). The old wiring
/// returned *every* match for a /g/ pattern, so `while ((m = re.exec(s)) !== null)` never terminated.
pub fn exec(it: &mut Interp, s: &str, re: &Val) -> Result<Val, Ctl> {
    let (r, g) = re_of(it, re)?;
    let cr = chars_of(s); let c: &[char] = &cr;
    let sticky = re_of_flag(re, 'y');
    let stateful = g || sticky;
    let mut start = if stateful { last_index(it, re)? } else { 0 };
    // Spec: an empty match leaves lastIndex at its end (§22.2.7.2 step 15), and callers advance. Engine-only guard
    // (tests/re/engine-only/zeroWidth-drain.js): when the caller did NOT advance — the previous exec on this object was an
    // empty match ending exactly here — start one further, so a bare `while (re.exec(s))` drain terminates instead of
    // wedging the renderer. Callers that advance themselves (lastIndex++) see spec behaviour.
    if stateful && empty_at(re) == Some(start) { start += 1; }
    if start > c.len() { if stateful { set_last_index(it, re, 0)?; set_empty_at(re, None); } return Ok(Val::Null); }
    // exec_at already scans forward from `from` and yields the first match at or after it,
    // so the old `(start..=c.len()).find_map(|i| exec_at(&r, &c, i))` retried the whole tail
    // once per offset: O(n^2) exec_at calls, each O(n) — cubic. One call is equivalent.
    // sticky (§22.2.7.2 step 13): a match must begin exactly at lastIndex
    let caps = if sticky { exec_at(&r, &c, start).filter(|x| x[0].map_or(false, |m| m.0 == start)) } else { exec_at(&r, &c, start) };
    match caps {
        Some(caps) => {
            let (a, b) = caps[0].unwrap();
            // only /g/ and /y/ carry lastIndex forward; a non-global exec must not touch it
            // lastIndex = the match end even for an empty match (§22.2.7.2 step 15); callers advance past empty matches
            if stateful { set_last_index(it, re, b)?; set_empty_at(re, if a == b { Some(b) } else { None }); }
            Ok(match_array(it, &r, &c, &caps, a, s))
        }
        None => { if stateful { set_last_index(it, re, 0)?; set_empty_at(re, None); } Ok(Val::Null) }
    }
}

/// The match result array (§22.2.7.2 RegExpBuiltinExec steps 22–35): captures, index, input, groups.
fn match_array(it: &mut Interp, r: &Re, c: &[char], caps: &[Option<(usize, usize)>], at: usize, s: &str) -> Val {
    let arr = Val::array(caps.iter().map(|x| sub(c, *x)).collect());
    if let Val::Obj(o) = &arr {
        let groups = groups_obj(r, c, caps);
        let mut b = o.borrow_mut();
        let idx = if s.len() == c.len() { at } else { c[..at].iter().map(|ch| { let u = *ch as u32; if u >= 0x10000 && !(0xF0000 + 0xD800..=0xF0000 + 0xDFFF).contains(&u) { 2 } else { 1 } }).sum() };
        b.props.push((std::rc::Rc::from("index"), Val::Num(idx as f64)));
        b.props.push((std::rc::Rc::from("input"), st(s)));
        b.props.push((std::rc::Rc::from("groups"), groups));
    }
    let _ = it;
    arr
}
/// The groups object: named captures (undefined when the pattern has none).
fn groups_obj(r: &Re, c: &[char], caps: &[Option<(usize, usize)>]) -> Val {
    if r.names.is_empty() { return Val::Undef; }
    let g = Val::obj_p(Kind::Plain, None);
    if let Val::Obj(o) = &g { for (n, i) in &r.names { o.borrow_mut().props.push((std::rc::Rc::from(n.as_str()), sub(c, caps.get(*i).copied().flatten()))); } }
    g
}

/// `String.prototype.search`: index of the first match, or -1.
pub fn search(it: &mut Interp, s: &str, re: &Val) -> Result<Val, Ctl> {
    let (r, _) = re_of(it, re)?;
    let cr = chars_of(s); let c: &[char] = &cr;
    Ok(match exec_at(&r, &c, 0) {
        Some(caps) => Val::Num(caps[0].unwrap().0 as f64),
        None => Val::Num(-1.0),
    })
}

/// `String.prototype.matchAll`: every match from `lastIndex`, as match arrays (an iterator upstream; an array here).
/// Like node, a non-global pattern is a TypeError rather than a silent coercion.
pub fn match_all(it: &mut Interp, s: &str, re: &Val) -> Result<Val, Ctl> {
    let (r, g) = re_of(it, re)?;
    if !g && matches!(re, Val::Obj(o) if matches!(o.borrow().kind, Kind::RegExp(..))) {
        return Err(Ctl::Throw(it.error("TypeError", "String.prototype.matchAll called with a non-global RegExp argument")));
    }
    let cr = chars_of(s); let c: &[char] = &cr;
    let mut out = Vec::new();
    let mut i = if g { last_index(it, re)? } else { 0 };
    while i <= c.len() {
        let Some(caps) = exec_at(&r, &c, i) else { break };
        let (a, b) = caps[0].unwrap();
        out.push(match_array(it, &r, &c, &caps, a, s));
        i = if b == a { b + 1 } else { b };
    }
    Ok(Val::array(out))
}

/// Where this regexp object's last exec ended with an empty match (hidden own slot), for the drain guard in exec.
fn empty_at(re: &Val) -> Option<usize> { match re { Val::Obj(o) => o.borrow().props.iter().find(|(k, _)| &**k == "\u{0}empty").and_then(|(_, v)| if let Val::Num(n) = v { Some(*n as usize) } else { None }), _ => None } }
fn set_empty_at(re: &Val, at: Option<usize>) {
    if let Val::Obj(o) = re { let mut b = o.borrow_mut(); let v = at.map_or(Val::Undef, |n| Val::Num(n as f64)); match b.props.iter_mut().find(|(k, _)| &**k == "\u{0}empty") { Some(p) => p.1 = v, None => if at.is_some() { b.props.push((std::rc::Rc::from("\u{0}empty"), v)); } } }
}
fn re_of_flag(re: &Val, f: char) -> bool { match re { Val::Obj(o) => matches!(&o.borrow().kind, Kind::RegExp(_, fl) if fl.contains(f)), _ => false } }
fn last_index(it: &mut Interp, re: &Val) -> Result<usize, Ctl> {
    let v = it.get(re, "lastIndex").unwrap_or(Val::Num(0.0));
    Ok(match v { Val::Num(n) if n.is_finite() && n >= 0.0 => n as usize, _ => 0 })
}
fn set_last_index(it: &mut Interp, re: &Val, n: usize) -> Result<(), Ctl> { it.set(re, "lastIndex", Val::Num(n as f64)).map(|_| ()) }

pub fn match_(it: &mut Interp, s: &str, re: &Val) -> Result<Val, Ctl> {
    let (r, g) = re_of(it, re)?;
    let cr = chars_of(s); let c: &[char] = &cr;
    if g {
        let mut out = Vec::new(); let mut i = 0;
        while i <= c.len() { match exec_at(&r, &c, i) { Some(caps) => { let (a, b) = caps[0].unwrap(); out.push(sub(&c, caps[0])); i = if b == a { b + 1 } else { b }; } None => break } }
        return Ok(if out.is_empty() { Val::Null } else { Val::array(out) });
    }
    Ok(match exec_at(&r, &c, 0) {
        Some(caps) => { let a = caps[0].unwrap().0; match_array(it, &r, &c, &caps, a, s) }
        None => Val::Null,
    })
}

/// `String.prototype.split(/re/)` = RegExp.prototype[@@split] (§22.2.6.14): from p, find the next match at q ≥ p (the
/// sticky splitter stepped over q); an empty match at p advances q, any other match cuts s[p..q] and its captures.
pub fn split(it: &mut Interp, s: &str, re: &Val) -> Result<Val, Ctl> {
    let (r, _) = re_of(it, re)?;
    let cr = chars_of(s); let c: &[char] = &cr;
    let size = c.len();
    let mut out: Vec<Val> = Vec::new();
    if size == 0 { return Ok(Val::array(if exec_at(&r, c, 0).map_or(false, |caps| caps[0].map_or(false, |m| m.0 == 0)) { vec![] } else { vec![st("")] })); }
    let (mut p, mut q) = (0usize, 0usize);
    while q < size {
        let Some(caps) = exec_at(&r, c, q) else { break };
        let (a, e) = caps[0].unwrap();
        if a >= size { break; }
        if e == p { q = a + 1; continue; }
        out.push(sub(c, Some((p, a))));
        for cap in caps.iter().skip(1) { out.push(sub(c, *cap)); }
        p = e; q = p;
    }
    out.push(sub(c, Some((p, size))));
    let _ = it;
    Ok(Val::array(out))
}

pub fn replace(it: &mut Interp, s: &str, re: &Val, rep: &Val) -> Result<Val, Ctl> {
    let (r, g) = re_of(it, re)?;
    let cr = chars_of(s); let c: &[char] = &cr;
    let mut out = String::new(); let mut i = 0; let mut last = 0;
    while i <= c.len() {
        let Some(caps) = exec_at(&r, &c, i) else { break };
        let (a, b) = caps[0].unwrap();
        out.extend(&c[last..a]);
        if rep.is_callable() {
            let mut args: Vec<Val> = caps.iter().map(|x| sub(&c, *x)).collect();
            args.push(Val::Num(a as f64)); args.push(st(s));
            if !r.names.is_empty() { args.push(groups_obj(&r, &c, &caps)); }
            let v = it.call(rep, Val::Undef, args)?; out += &it.to_str(&v)?;
        } else {
            let t: Vec<char> = it.to_str(rep)?.chars().collect();
            let mut j = 0;
            while j < t.len() {
                if t[j] == '$' && j + 1 < t.len() {
                    match t[j + 1] {
                        '$' => { out.push('$'); j += 2; continue; }
                        '&' => { out.extend(&c[a..b]); j += 2; continue; }
                        '`' => { out.extend(&c[..a]); j += 2; continue; }
                        '\'' => { out.extend(&c[b..]); j += 2; continue; }
                        d if d.is_ascii_digit() => {
                            // $nn when that group exists, else $n (§22.1.3.19.1 GetSubstitution)
                            let two = t.get(j + 2).and_then(|x| x.to_digit(10)).map(|x| d.to_digit(10).unwrap() as usize * 10 + x as usize).filter(|&g| g >= 1 && g <= r.groups);
                            let (gi, w) = match two { Some(g) => (g, 3), None => (d.to_digit(10).unwrap() as usize, 2) };
                            if gi >= 1 && gi <= r.groups { if let Some((x, y)) = caps[gi] { out.extend(&c[x..y]); } j += w; continue; }
                        }
                        '<' if !r.names.is_empty() => { if let Some(e) = t[j + 2..].iter().position(|&x| x == '>') { let name: String = t[j + 2..j + 2 + e].iter().collect(); if let Some((_, gi)) = r.names.iter().find(|(n, _)| *n == name) { if let Some((x, y)) = caps[*gi] { out.extend(&c[x..y]); } } j += 3 + e; continue; } }
                        _ => {}
                    }
                }
                out.push(t[j]); j += 1;
            }
        }
        last = b;
        if !g { break; }
        i = if b == a { if b < c.len() { out.push(c[b]); } last = b + 1; b + 1 } else { b };
    }
    if last < c.len() { out.extend(&c[last..]); }
    Ok(st(&out))
}

// ── B1 (IR-JIT-NEXT): a lazy DFA for "is there a match" — RegExp.prototype.test and grep ──────────────────────────────────────
// Clean-room from the published constructions (Thompson 1968 NFA; Rabin–Scott subset construction built lazily per input, as in
// RE2 / Hyperscan descriptions). Linear time, no backtracking budget. Exact by construction for the patterns it accepts:
//   * atoms keep the backtracker's own predicates (eqc case folding, `.` vs line terminators, class ranges incl. icase variants);
//   * input characters are grouped into classes by WHICH atoms accept them (ASCII precomputed, others cached on first sight);
//   * accepted: no backreference, no lookaround, no \b, `^` only as the first and `$` only as the last top-level element without m.
// Anything else returns None from `build` and the backtracker answers, unchanged.
enum Ni { Atom(usize, usize), Split(usize, usize), Match }
#[derive(Clone)]
enum Atom { Ch(char), Any, Cls(Vec<(char, char)>, bool) }
pub struct Dfa {
    insts: Vec<Ni>, atoms: Vec<Atom>, start: usize, anch_s: bool, anch_e: bool, icase: bool, dotall: bool,
    ascii: Vec<u16>, other: std::collections::HashMap<char, u16>, sigs: std::collections::HashMap<Vec<bool>, u16>, class_atoms: Vec<Vec<bool>>,
    sets: Vec<(Vec<usize>, bool)>, ids: std::collections::HashMap<Vec<usize>, u32>, trans: Vec<Vec<u32>>, init: u32,
}
const NONE: u32 = u32::MAX;
impl Dfa {
    pub fn build(re: &Re) -> Option<Dfa> {
        if re.multi { return None; }
        let mut d = Dfa { insts: Vec::new(), atoms: Vec::new(), start: 0, anch_s: false, anch_e: false, icase: re.icase, dotall: re.dotall,
            ascii: Vec::new(), other: Default::default(), sigs: Default::default(), class_atoms: Vec::new(), sets: Vec::new(), ids: Default::default(), trans: Vec::new(), init: 0 };
        let m = d.push(Ni::Match);
        let top: Vec<&Node> = match &re.node { Node::Seq(v) => v.iter().collect(), n => vec![n] };
        let (mut a, mut b) = (0usize, top.len());
        if b > a && matches!(top[a], Node::Start) { d.anch_s = true; a += 1; }
        if b > a && matches!(top[b - 1], Node::End) { d.anch_e = true; b -= 1; }
        let mut next = m;
        for n in top[a..b].iter().rev() { next = d.emit(n, next)?; }
        d.start = next;
        if d.insts.len() > 20_000 { return None; }
        let n_atoms = d.atoms.len();
        d.ascii = (0..128u32).map(|c| d.class_of_sig((0..n_atoms).map(|k| d.accepts(k, char::from_u32(c).unwrap())).collect())).collect();
        let s0 = d.closure(&[d.start]); d.init = d.state(s0);
        Some(d)
    }
    fn push(&mut self, i: Ni) -> usize { self.insts.push(i); self.insts.len() - 1 }
    fn emit(&mut self, n: &Node, next: usize) -> Option<usize> {
        if self.insts.len() > 20_000 { return None; }
        Some(match n {
            Node::Char(c) => { self.atoms.push(Atom::Ch(*c)); let k = self.atoms.len() - 1; self.push(Ni::Atom(k, next)) }
            Node::Any => { self.atoms.push(Atom::Any); let k = self.atoms.len() - 1; self.push(Ni::Atom(k, next)) }
            Node::Class(r, neg) => { self.atoms.push(Atom::Cls(r.clone(), *neg)); let k = self.atoms.len() - 1; self.push(Ni::Atom(k, next)) }
            Node::Group(inner, _) => self.emit(inner, next)?,
            Node::Seq(v) => { let mut x = next; for e in v.iter().rev() { x = self.emit(e, x)?; } x }
            Node::Alt(v) => { let mut x = self.emit(&v[v.len() - 1], next)?; for e in v[..v.len() - 1].iter().rev() { let s = self.emit(e, next)?; x = self.push(Ni::Split(s, x)); } x }
            Node::Rep(inner, lo, hi, _) => {
                let mut x = next;
                if *hi == usize::MAX { let p = self.push(Ni::Split(0, 0)); let body = self.emit(inner, p)?; self.insts[p] = Ni::Split(body, next); x = p; }
                else { if *hi - *lo > 1000 { return None; } for _ in *lo..*hi { let body = self.emit(inner, x)?; x = self.push(Ni::Split(body, next)); } }
                if *lo > 1000 { return None; }
                for _ in 0..*lo { x = self.emit(inner, x)?; }
                x
            }
            Node::Start | Node::End | Node::WordB(_) | Node::Look(..) | Node::LookB(..) | Node::Back(_) => return None,
        })
    }
    /// the backtracker's own atom predicates (m_inner)
    fn accepts(&self, k: usize, c: char) -> bool {
        match &self.atoms[k] {
            Atom::Ch(x) => if self.icase { c.to_lowercase().eq(x.to_lowercase()) } else { c == *x },
            Atom::Any => self.dotall || !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'),
            Atom::Cls(r, neg) => class_hit(r, c, self.icase) != *neg,
        }
    }
    fn class_of_sig(&mut self, sig: Vec<bool>) -> u16 {
        if let Some(&c) = self.sigs.get(&sig) { return c; }
        let c = self.class_atoms.len() as u16; self.class_atoms.push(sig.clone()); self.sigs.insert(sig, c);
        for t in self.trans.iter_mut() { t.push(NONE); }
        c
    }
    fn class(&mut self, c: char) -> u16 {
        if (c as u32) < 128 { return self.ascii[c as usize]; }
        if let Some(&k) = self.other.get(&c) { return k; }
        let sig = (0..self.atoms.len()).map(|k| self.accepts(k, c)).collect(); let k = self.class_of_sig(sig); self.other.insert(c, k); k
    }
    fn closure(&self, from: &[usize]) -> Vec<usize> {
        let mut seen = vec![false; self.insts.len()]; let mut st: Vec<usize> = from.to_vec(); let mut out = Vec::new();
        while let Some(i) = st.pop() { if seen[i] { continue; } seen[i] = true;
            match self.insts[i] { Ni::Split(a, b) => { st.push(b); st.push(a); } _ => out.push(i) } }
        out.sort_unstable(); out
    }
    fn state(&mut self, mut set: Vec<usize>) -> u32 {
        if !self.anch_s { let s0 = self.closure(&[self.start]); set.extend(s0); set.sort_unstable(); set.dedup(); }   // unanchored: a match may start anywhere
        if let Some(&id) = self.ids.get(&set) { return id; }
        let is_m = set.iter().any(|&i| matches!(self.insts[i], Ni::Match));
        let id = self.sets.len() as u32; self.ids.insert(set.clone(), id); self.sets.push((set, is_m)); self.trans.push(vec![NONE; self.class_atoms.len()]); id
    }
    fn step(&mut self, s: u32, cls: u16) -> u32 {
        let t = self.trans[s as usize][cls as usize]; if t != NONE { return t; }
        let sig = self.class_atoms[cls as usize].clone();
        let nexts: Vec<usize> = self.sets[s as usize].0.iter().filter_map(|&i| match self.insts[i] { Ni::Atom(k, nx) if sig[k] => Some(nx), _ => None }).collect();
        let set = self.closure(&nexts); let t = self.state(set); self.trans[s as usize][cls as usize] = t; t
    }
    /// The same answer over a &str without collecting chars (grep). None = too many states.
    pub fn is_match_str(&mut self, s: &str) -> Option<bool> {
        let mut st = self.init;
        if !self.anch_e && self.sets[st as usize].1 { return Some(true); }
        for c in s.chars() {
            if self.sets.len() > 20_000 { return None; }
            let k = self.class(c); st = self.step(st, k);
            if self.anch_s && self.sets[st as usize].0.is_empty() { return Some(false); }
            if !self.anch_e && self.sets[st as usize].1 { return Some(true); }
        }
        Some(self.sets[st as usize].1)
    }
    /// Is there a match anywhere in `s` (exactly `exec_at(re, s, 0).is_some()` for the accepted patterns)? None = too many states.
    pub fn is_match(&mut self, s: &[char]) -> Option<bool> {
        let mut st = self.init;
        if !self.anch_e && self.sets[st as usize].1 { return Some(true); }
        for &c in s {
            if self.sets.len() > 20_000 { return None; }
            let k = self.class(c); st = self.step(st, k);
            if self.anch_s && self.sets[st as usize].0.is_empty() { return Some(false); }
            if !self.anch_e && self.sets[st as usize].1 { return Some(true); }
        }
        Some(self.sets[st as usize].1)
    }
}
thread_local! { static DFAS: std::cell::RefCell<std::collections::HashMap<usize, Option<Dfa>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }
/// test() through the DFA when the pattern qualifies (keyed by the cached Re's address; the RE_CACHE keeps it alive).
fn dfa_test(r: &std::rc::Rc<Re>, s: &[char]) -> Option<bool> {
    if std::env::var_os("JSI_RE_NO_DFA").is_some() { return None; }
    let key = std::rc::Rc::as_ptr(r) as usize;
    DFAS.with(|m| { let mut m = m.borrow_mut(); if m.len() > 512 { m.clear(); }
        m.entry(key).or_insert_with(|| Dfa::build(r)).as_mut().and_then(|d| d.is_match(s)) })
}

/// `omni-render re-dfa-check [cases]`: random patterns × random subjects — the DFA's answer must equal the backtracker's
/// `exec_at(.., 0).is_some()` on every case the DFA accepts; reports how many patterns it accepted.
pub fn dfa_check(cases: usize) {
    let mut s = 0x9E3779B97F4A7C15u64; let mut rnd = move |n: u64| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s % n };
    fn gen_pat(rnd: &mut dyn FnMut(u64) -> u64, depth: u32) -> String {
        let atoms = ["a", "b", "c", ".", "[ab]", "[^a]", "\\d", "\\w", "\\s", "A", "_", "1", "[a-c]", "\\n"];
        let mut out = String::new(); let n = 1 + rnd(4);
        for _ in 0..n {
            let mut a = if depth < 2 && rnd(5) == 0 { let alt = if rnd(2) == 0 { format!("{}|{}", gen_pat(rnd, depth + 1), gen_pat(rnd, depth + 1)) } else { gen_pat(rnd, depth + 1) };
                format!("({}{})", if rnd(2) == 0 { "?:" } else { "" }, alt) } else { atoms[rnd(atoms.len() as u64) as usize].to_string() };
            match rnd(8) { 0 => a.push('*'), 1 => a.push('+'), 2 => a.push('?'), 3 => a.push_str("{1,3}"), 4 => a.push_str("*?"), _ => {} }
            out += &a;
        }
        out
    }
    let (mut accepted, mut cmp, mut bad, mut pats, mut gave_up) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let flags_all = ["", "i", "s", "is"];
    for _ in 0..cases {
        let mut p = gen_pat(&mut rnd, 0);
        if rnd(6) == 0 { p = format!("^{p}"); } if rnd(6) == 0 { p.push('$'); }
        let f = flags_all[rnd(4) as usize];
        let Ok(re) = compile(&p, f) else { continue };
        pats += 1;
        let Some(mut d) = Dfa::build(&re) else { continue };
        accepted += 1;
        for _ in 0..8 {
            let alpha = ['a', 'b', 'c', 'A', 'B', ' ', '1', '_', '\n', 'é', '\u{212A}'];
            let subj: Vec<char> = (0..rnd(12)).map(|_| alpha[rnd(alpha.len() as u64) as usize]).collect();
            let want = exec_at(&re, &subj, 0).is_some();
            if BUDGET_HIT.with(|b| b.get()) { gave_up += 1; continue; }   // the backtracker gave up: its false is not an answer
            let got = d.is_match(&subj);
            cmp += 1;
            if got != Some(want) { bad += 1; if bad <= 5 { eprintln!("  MISMATCH /{p}/{f} on {:?}: dfa {got:?} backtracker {want}", subj.iter().collect::<String>()); } }
        }
    }
    println!("RE-DFA-CHECK: patterns {pats} · accepted by the DFA {accepted} · comparisons {cmp} · mismatches {bad} · backtracker gave up (budget) {gave_up} · {}", if bad == 0 && cmp > 0 { "PASS" } else { "FAIL" });
}

/// `omni-render grep [-i] [-l|-c] <pattern> <file|dir>...` (B1): JavaScript-regex grep, line by line, through the lazy DFA when
/// the pattern qualifies (else the backtracker). -l = names of files with a match; -c = count per file; default = matching lines.
/// Prints a summary line on stderr: files, lines scanned, matching lines, engine, ms.
pub fn grep_main(argv: &[String]) {
    let (mut flags, mut mode, mut pos) = (String::new(), 'p', Vec::new());
    for a in argv { match a.as_str() { "-i" => flags.push('i'), "-l" => mode = 'l', "-c" => mode = 'c', _ => pos.push(a.clone()) } }
    assert!(!pos.is_empty(), "usage: omni-render grep [-i] [-l|-c] <pattern> <file|dir>...");
    let re = compile(&pos[0], &flags).unwrap_or_else(|e| panic!("bad pattern: {e}"));
    let mut dfa = Dfa::build(&re);
    let engine = if dfa.is_some() { "dfa" } else { "backtracker" };
    // required literal (exact prefilter): the longest run of consecutive literal chars in the TOP-LEVEL sequence must appear in
    // every match, so a file / line without it cannot match. Case-sensitive patterns only.
    let lit: Option<String> = if re.icase { None } else {
        let top: Vec<&Node> = match &re.node { Node::Seq(v) => v.iter().collect(), n => vec![n] };
        let (mut best, mut cur) = (String::new(), String::new());
        for n in top { if let Node::Char(c) = n { cur.push(*c); } else { if cur.chars().count() > best.chars().count() { best = cur.clone(); } cur.clear(); } }
        if cur.chars().count() > best.chars().count() { best = cur; }
        if best.chars().count() >= 2 { Some(best) } else { None } };
    fn walk(p: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        // like os.walk / rg: symlinked directories are not followed (a top-level argument is, as given)
        let md = match std::fs::symlink_metadata(p) { Ok(m) => m, Err(_) => return };
        if md.is_dir() { if let Ok(rd) = std::fs::read_dir(p) { let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect(); v.sort(); for q in v { walk(&q, out); } } }
        else if md.is_file() { out.push(p.to_path_buf()); }
    }
    let mut files = Vec::new(); for p in &pos[1..] { walk(std::path::Path::new(p), &mut files); }
    let t0 = std::time::Instant::now(); let (mut nl, mut nm) = (0usize, 0usize);
    use std::io::Write; let so = std::io::stdout(); let mut so = std::io::BufWriter::new(so.lock());
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue }; let text = String::from_utf8_lossy(&bytes);
        let mut cnt = 0usize;
        if let Some(l) = &lit { if !text.contains(l.as_str()) { nl += text.split('\n').count(); if mode == 'c' { let _ = writeln!(so, "{}:0", f.display()); } continue; } }
        for line in text.split('\n') {
            nl += 1;
            if let Some(l) = &lit { if !line.contains(l.as_str()) { continue; } }
            let hit = match dfa.as_mut().and_then(|d| d.is_match_str(line)) { Some(b) => b, None => { let c: Vec<char> = line.chars().collect(); exec_at(&re, &c, 0).is_some() } };
            if hit { cnt += 1; nm += 1; if mode == 'p' { let _ = writeln!(so, "{}:{line}", f.display()); } else if mode == 'l' { break; } }
        }
        if mode == 'l' && cnt > 0 { let _ = writeln!(so, "{}", f.display()); }
        if mode == 'c' { let _ = writeln!(so, "{}:{cnt}", f.display()); }
    }
    let _ = so.flush();
    eprintln!("GREP: files {} · lines {nl} · matching lines {nm} · engine {engine}{} · {:.1} ms", files.len(), lit.as_ref().map(|l| format!("+literal {l:?}")).unwrap_or_default(), t0.elapsed().as_secs_f64() * 1e3);
}
