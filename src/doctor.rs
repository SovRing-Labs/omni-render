// SPDX-License-Identifier: LicenseRef-PolyForm-Small-Business-1.0.0 OR LicenseRef-PolyForm-Noncommercial-1.0.0
//! DOCTOR core D0/D1 (plan `an unpublished plan`): find the code that already solves the broken code's
//! problem. D0 cuts a source file into sections at its own joints (statements, blocks, short statement runs — an adaptive,
//! multi-scale partition: PIFS-RES-0 measured that block COUNT, not geometric shape, sets the cost, and ADAPT was fastest).
//! Each section gets a SHAPE (identifiers α-renamed, method names kept — the token-class stream; clean-room from the idea of
//! characteristic vectors over abstracted syntax, Jiang et al. "DECKARD", ICSE 2007) and a SUBJECT (its own names).
//! D1 = partitioned IFS run in reverse (Jacquin 1992: for each range block search the whole for the domain block + map that
//! reproduces it). Range = a broken section; domains = every other section; map = the identifier renaming that carries the
//! domain onto the range, read off an LCS alignment of the shape streams; collage error = 1 − Dice of the tokens that agree
//! after the map. Shortlist by the rendered sift over 16-phase shape hypervectors (R1: geometry where items are compared),
//! then the exact collage on the shortlist (R4: the exact step decides). The donor's unaligned lines, carried through the
//! inverse map, are what the range lacks — the input of the PATCH station (plastic-surgery hypothesis, Barr et al. FSE 2014).
//! Clean-room: from the published descriptions only; no code copied.
use crate::hv;
#[path = "doctor_shape.rs"] pub mod shape; // SHAPE station (author 10-06): angles ours vs Chrome, the square in the square hole
#[path = "doctor_fix.rs"] pub mod fix; // FIX (doctor-core P1–P4, author 10-07): classify → candidates → judge on the pristine map → site fix sheet
#[path = "doctor_abc.rs"] pub mod abc; // CODE-ABC (author 10-06): the code alphabet — equations in an e-graph
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Tok { pub k: u8, pub s: String, pub line: u32, pub p: usize, pub e: usize } // p..e = byte span in the source // k: 0 ident · 1 keyword · 2 number · 3 string/char · 4 punct

const KW: &[&str] = &["fn", "let", "mut", "if", "else", "for", "while", "loop", "match", "return", "in", "as", "break", "continue",
    "struct", "enum", "impl", "pub", "use", "self", "Self", "true", "false", "Some", "None", "Ok", "Err", "where", "move", "ref",
    "const", "static", "type", "mod", "trait", "dyn", "unsafe", "crate", "super"];
const OPS: &[&str] = &["..=", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "|=", "&=", "^=", "<<", ">>", ".."];

pub fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes(); let mut i = 0; let mut line = 1u32; let mut out: Vec<Tok> = Vec::new();
    let (mut nlen, mut tok_start) = (0usize, 0usize);
    loop {
        if out.len() > nlen { let t = out.last_mut().unwrap(); t.p = tok_start; t.e = i.min(b.len()); nlen = out.len(); }
        if i >= b.len() { break; }
        let c = b[i];
        if c == b'\n' { line += 1; i += 1; continue; }
        if c.is_ascii_whitespace() { i += 1; continue; }
        if c == b'/' && b.get(i + 1) == Some(&b'/') { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        if c == b'/' && b.get(i + 1) == Some(&b'*') { i += 2; while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { if b[i] == b'\n' { line += 1; } i += 1; } i += 2; continue; }
        let l0 = line; tok_start = i;
        // raw strings r"..." / r#"..."#
        if c == b'r' && (b.get(i + 1) == Some(&b'"') || (b.get(i + 1) == Some(&b'#') && b.get(i + 2) == Some(&b'"'))) {
            let mut j = i + 1; let mut h = 0; while b[j] == b'#' { h += 1; j += 1; } j += 1;
            loop { if j >= b.len() { break; } if b[j] == b'\n' { line += 1; } if b[j] == b'"' && (0..h).all(|q| b.get(j + 1 + q) == Some(&b'#')) { j += 1 + h; break; } j += 1; }
            out.push(Tok { k: 3, s: "S".into(), line: l0, p: 0, e: 0 }); i = j; continue;
        }
        if c == b'"' || (c == b'b' && b.get(i + 1) == Some(&b'"')) {
            let mut j = if c == b'b' { i + 2 } else { i + 1 };
            while j < b.len() && b[j] != b'"' { if b[j] == b'\\' { j += 1; } else if b[j] == b'\n' { line += 1; } j += 1; }
            out.push(Tok { k: 3, s: "S".into(), line: l0, p: 0, e: 0 }); i = j + 1; continue;
        }
        if c == b'\'' {
            // char literal 'x' / '\n' / '\u{..}' vs lifetime 'a
            if b.get(i + 1) == Some(&b'\\') { let mut j = i + 2; while j < b.len() && b[j] != b'\'' { j += 1; } out.push(Tok { k: 3, s: "S".into(), line, p: 0, e: 0 }); i = j + 1; continue; }
            let ch_len = src[i + 1..].chars().next().map_or(1, |ch| ch.len_utf8());
            if b.get(i + 1 + ch_len) == Some(&b'\'') { out.push(Tok { k: 3, s: "S".into(), line, p: 0, e: 0 }); i += 2 + ch_len; continue; }
            let mut j = i + 1; while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') { j += 1; }
            out.push(Tok { k: 4, s: "'".into(), line, p: 0, e: 0 }); i = j; continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let mut j = i; while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) { j += 1; }
            let w = &src[i..j]; out.push(Tok { k: if KW.contains(&w) { 1 } else { 0 }, s: w.into(), line, p: 0, e: 0 }); i = j; continue;
        }
        if c.is_ascii_digit() {
            let mut j = i; while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || (b[j] == b'.' && b.get(j + 1).is_some_and(|d| d.is_ascii_digit()))) { j += 1; }
            out.push(Tok { k: 2, s: src[i..j].into(), line, p: 0, e: 0 }); i = j; continue;
        }
        let op = OPS.iter().find(|o| src[i..].starts_with(**o)).map(|o| o.to_string()).unwrap_or_else(|| (c as char).to_string());
        i += op.len(); out.push(Tok { k: 4, s: op, line, p: 0, e: 0 });
    }
    out
}

const PY_KW: &[&str] = &["def", "class", "if", "elif", "else", "for", "while", "return", "in", "not", "and", "or", "is", "lambda", "True",
    "False", "None", "self", "pass", "break", "continue", "yield", "import", "from", "as", "with", "try", "except", "finally", "raise",
    "global", "nonlocal", "async", "await", "assert", "del"];
const JS_KW: &[&str] = &["function", "let", "const", "var", "if", "else", "for", "while", "do", "switch", "case", "default", "return", "new",
    "this", "class", "extends", "import", "export", "from", "true", "false", "null", "undefined", "typeof", "instanceof", "in", "of", "try",
    "catch", "finally", "throw", "async", "await", "break", "continue", "yield", "static", "get", "set"];

/// Language of a source file by extension (the index covers our own Rust, Python, JS/TS/Svelte and C/C++).
pub fn lang_of(path: &str) -> Option<&'static str> {
    let e = path.rsplit('.').next().unwrap_or("");
    Some(match e { "rs" => "rust", "py" => "python", "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => "js", "svelte" => "svelte",
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" => "c", _ => return None })
}
/// One lexer per language, all producing the same token stream the sectioner reads (braces and `;` delimit statements).
pub fn lex_lang(src: &str, lang: &str) -> Vec<Tok> {
    match lang {
        "python" => lex_py(src),
        "svelte" => {
            // keep only <script> bodies (same byte offsets and line numbers: everything else blanked to spaces / newlines)
            let mut b: Vec<u8> = src.bytes().map(|c| if c == b'\n' { b'\n' } else { b' ' }).collect();
            let lo = src.to_ascii_lowercase(); let mut k = 0;
            while let Some(a) = lo[k..].find("<script") { let a = k + a; let Some(gt) = lo[a..].find('>') else { break }; let body = a + gt + 1;
                let end = lo[body..].find("</script>").map(|x| body + x).unwrap_or(src.len());
                b[body..end].copy_from_slice(&src.as_bytes()[body..end]); k = end; }
            let t = lex(&String::from_utf8_lossy(&b)); retag(t, JS_KW)
        }
        "js" => retag(lex(src), JS_KW),
        _ => lex(src),
    }
}
fn retag(mut t: Vec<Tok>, kw: &[&str]) -> Vec<Tok> { for x in &mut t { if x.k <= 1 { x.k = if kw.contains(&x.s.as_str()) { 1 } else { 0 }; } } t }

/// Python lexer (clean-room from the language reference's lexical rules): `#` comments, single / triple-quoted strings, numbers,
/// names, operators; indentation becomes VIRTUAL braces (a block opener `:` + INDENT → `{`, DEDENT → `}`) and each logical line
/// ends with a virtual `;`, so the brace-based sectioner sees Python statements and blocks exactly like Rust ones.
pub fn lex_py(src: &str) -> Vec<Tok> {
    let b = src.as_bytes(); let mut out: Vec<Tok> = Vec::new();
    let mut i = 0usize; let mut line = 1u32; let mut depth = 0i32; // bracket depth: newlines inside brackets are not line ends
    let mut indents: Vec<usize> = vec![0]; let mut at_line_start = true;
    let vt = |s: &str, line: u32, p: usize| Tok { k: 4, s: s.into(), line, p, e: p };
    while i < b.len() {
        if at_line_start && depth == 0 {
            // measure indentation; blank / comment-only lines do not change blocks
            let mut j = i; let mut col = 0; while j < b.len() && (b[j] == b' ' || b[j] == b'\t') { col += if b[j] == b'\t' { 8 - col % 8 } else { 1 }; j += 1; }
            if j >= b.len() || b[j] == b'\n' || b[j] == b'#' || b[j] == b'\r' { while j < b.len() && b[j] != b'\n' { j += 1; } i = j + 1; line += 1; continue; }
            let cur = *indents.last().unwrap();
            if col > cur { indents.push(col); if out.last().is_some_and(|t| t.s == ";") && out.len() >= 2 && out[out.len() - 2].s == ":" { out.pop(); } out.push(vt("{", line, j)); }
            while col < *indents.last().unwrap() { indents.pop(); out.push(vt("}", line, j)); }
            at_line_start = false; i = j; continue;
        }
        let c = b[i];
        if c == b'\n' { if depth == 0 { if out.last().is_some_and(|t| t.s != ";" && t.s != "{" && t.s != "}") { out.push(vt(";", line, i)); } at_line_start = true; } line += 1; i += 1; continue; }
        if c == b'\\' && b.get(i + 1) == Some(&b'\n') { i += 2; line += 1; continue; }
        if c == b' ' || c == b'\t' || c == b'\r' { i += 1; continue; }
        if c == b'#' { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        let st = i; let l0 = line;
        // strings (prefixes r/b/f/u, triple quotes)
        let mut q = i; while q < b.len() && q < i + 2 && b[q].is_ascii_alphabetic() && matches!(b[q].to_ascii_lowercase(), b'r' | b'b' | b'f' | b'u') { q += 1; }
        if q < b.len() && (b[q] == b'"' || b[q] == b'\'') && (q == i || !b[i].is_ascii_alphanumeric() || q > i) && (q == i || matches!(b[i].to_ascii_lowercase(), b'r' | b'b' | b'f' | b'u')) {
            let qc = b[q]; let triple = b.get(q + 1) == Some(&qc) && b.get(q + 2) == Some(&qc);
            let mut j = q + if triple { 3 } else { 1 };
            loop { if j >= b.len() { break; } if b[j] == b'\\' { j += 2; continue; } if b[j] == b'\n' { line += 1; if !triple { break; } }
                if b[j] == qc && (!triple || (b.get(j + 1) == Some(&qc) && b.get(j + 2) == Some(&qc))) { j += if triple { 3 } else { 1 }; break; } j += 1; }
            out.push(Tok { k: 3, s: "S".into(), line: l0, p: st, e: j.min(b.len()) }); i = j; continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let mut j = i; while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) { j += 1; }
            let w = &src[i..j]; out.push(Tok { k: if PY_KW.contains(&w) { 1 } else { 0 }, s: w.into(), line, p: st, e: j }); i = j; continue;
        }
        if c.is_ascii_digit() { let mut j = i; while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.') { j += 1; } out.push(Tok { k: 2, s: src[i..j].into(), line, p: st, e: j }); i = j; continue; }
        let op = ["**=", "//=", ">>=", "<<=", "->", ":=", "==", "!=", "<=", ">=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**", "//", "<<", ">>"].iter()
            .find(|o| src[i..].starts_with(**o)).map(|o| o.to_string()).unwrap_or_else(|| (c as char).to_string());
        if op == "(" || op == "[" || op == "{" { depth += 1; } else if op == ")" || op == "]" || op == "}" { depth = (depth - 1).max(0); }
        // a bracket brace stays a real token but must not look like a block brace to the sectioner
        let s_op = match op.as_str() { "{" => "{d".to_string(), "}" => "}d".to_string(), _ => op.clone() };
        i += op.len(); out.push(Tok { k: 4, s: s_op, line, p: st, e: i });
    }
    if out.last().is_some_and(|t| t.s != ";" && t.s != "}") { out.push(vt(";", line, b.len())); }
    while indents.len() > 1 { indents.pop(); out.push(vt("}", line, b.len())); }
    out
}

/// Shape token: identifiers → "I" unless a method/function name being called after `.` (max, min, sum, iter, term, rule…:
/// those carry the shape of the computation); numbers 0/1 kept (identity/unit), others → "N"; strings → "S".
pub fn shape(t: &[Tok], i: usize) -> String {
    let k = &t[i];
    match k.k {
        0 => if i > 0 && t[i - 1].s == "." && t.get(i + 1).is_some_and(|n| n.s == "(") { format!(".{}", k.s) } else { "I".into() },
        2 => if k.s == "0" || k.s == "1" { k.s.clone() } else { "N".into() },
        3 => "S".into(),
        _ => k.s.clone(),
    }
}

#[derive(Clone, Debug)]
pub struct Section { pub a: usize, pub b: usize, pub kind: &'static str, pub depth: u32, pub func: String, pub l0: u32, pub l1: u32 }

/// Cut the token stream into sections: every fn, every brace block's statements (and the block-bearing statements whole),
/// plus runs of 2..=4 consecutive statements in the same block (the multi-scale range blocks).
pub fn sections(t: &[Tok], min_toks: usize) -> Vec<Section> {
    // matching braces
    let mut mate = vec![usize::MAX; t.len()]; let mut st = Vec::new();
    for (i, k) in t.iter().enumerate() { if k.k == 4 { match k.s.as_str() { "{" | "(" | "[" => st.push(i), "}" | ")" | "]" => { if let Some(o) = st.pop() { mate[o] = i; mate[i] = o; } } _ => {} } } }
    let mut out = Vec::new();
    // fns: `fn name ... {` → body
    let mut fn_spans: Vec<(usize, usize, String)> = Vec::new();
    for i in 0..t.len() {
        if matches!(t[i].s.as_str(), "fn" | "def" | "function") && t[i].k == 1 && t.get(i + 1).is_some_and(|n| n.k == 0) {
            let mut j = i + 2; while j < t.len() && t[j].s != "{" && t[j].s != ";" { if mate[j] != usize::MAX && (t[j].s == "(" || t[j].s == "[") { j = mate[j]; } j += 1; }
            if j < t.len() && t[j].s == "{" && mate[j] != usize::MAX { fn_spans.push((i, mate[j] + 1, t[i + 1].s.clone())); }
        }
    }
    let func_of = |a: usize| fn_spans.iter().filter(|f| f.0 <= a && a < f.1).min_by_key(|f| f.1 - f.0).map(|f| f.2.clone()).unwrap_or_default();
    for f in &fn_spans { out.push(Section { a: f.0, b: f.1, kind: "fn", depth: 0, func: f.2.clone(), l0: t[f.0].line, l1: t[f.1 - 1].line }); }
    // brace depth of every token, one pass (was recounted per block: quadratic on big files)
    let mut bdepth = vec![0u32; t.len()]; { let mut dd = 0u32; for (i, k) in t.iter().enumerate() { if k.k == 4 && k.s == "}" { dd = dd.saturating_sub(1); } bdepth[i] = dd; if k.k == 4 && k.s == "{" { dd += 1; } } }
    // statements of each brace block
    for o in 0..t.len() {
        if t[o].s != "{" || t[o].k != 4 || mate[o] == usize::MAX { continue; }
        let c = mate[o];
        let is_match = { let mut j = o; let mut found = false; let stop = o.saturating_sub(64); while j > stop { j -= 1; let s = t[j].s.as_str(); if s == ";" || s == "{" || s == "}" { break; } if s == "match" { found = true; break; } } found };
        let depth = bdepth[o];
        let mut stmts: Vec<(usize, usize)> = Vec::new();
        let mut i = o + 1;
        while i < c {
            let s0 = i;
            loop {
                if i >= c { break; }
                let s = t[i].s.as_str();
                if t[i].k == 4 && (s == "(" || s == "[") && mate[i] != usize::MAX { i = mate[i] + 1; continue; }
                if t[i].k == 4 && s == "{" && mate[i] != usize::MAX {
                    i = mate[i] + 1;
                    let nx = t.get(i).map(|n| n.s.as_str()).unwrap_or("");
                    if matches!(nx, "else" | "elif" | "except" | "finally" | "catch") || nx == "." || nx == "?" || nx == ")" || nx == ";" || nx == "," || nx == "as" { continue; }
                    // a block-statement ends at its closing brace unless it is the right-hand side of `let … =`
                    if t[s0].s == "let" || (t[s0].k == 0 && t.get(s0 + 1).is_some_and(|n| n.s.ends_with('='))) { continue; }
                    break;
                }
                i += 1;
                if s == ";" || (is_match && s == ",") { break; }
            }
            if i > s0 { stmts.push((s0, i.min(c))); }
        }
        let func = func_of(o);
        for &(a, b) in &stmts {
            if b - a >= min_toks { out.push(Section { a, b, kind: "stmt", depth, func: func.clone(), l0: t[a].line, l1: t[b - 1].line }); }
        }
        for w in 2..=4usize {
            for k in 0..stmts.len().saturating_sub(w - 1) {
                let (a, b) = (stmts[k].0, stmts[k + w - 1].1);
                if b - a >= min_toks && b - a <= 600 { out.push(Section { a, b, kind: "run", depth, func: func.clone(), l0: t[a].line, l1: t[b - 1].line }); }
            }
        }
    }
    out.sort_by_key(|s| (s.a, std::cmp::Reverse(s.b)));
    out.dedup_by(|x, y| x.a == y.a && x.b == y.b);
    out
}

/// Shape hypervector: bundle of the section's shape 3-grams (each n-gram a rendered symbol).
pub fn shape_hv(sh: &[String]) -> Vec<u8> {
    // symbols are cached per distinct 3-gram (a file has a few thousand; sections overlap heavily, so each 3-gram recurs)
    thread_local! { static SYM: std::cell::RefCell<HashMap<String, std::rc::Rc<Vec<u8>>>> = std::cell::RefCell::new(HashMap::new()); }
    if sh.len() < 3 { return hv::sym("empty"); }
    let (mut re, mut im) = (vec![0i32; hv::D], vec![0i32; hv::D]);
    SYM.with(|c| { let mut c = c.borrow_mut(); if c.len() > 200_000 { c.clear(); }
        for w in sh.windows(3) {
            let key = format!("3g:{} {} {}", w[0], w[1], w[2]);
            let v = c.entry(key.clone()).or_insert_with(|| std::rc::Rc::new(hv::sym(&key))).clone();
            for i in 0..hv::D { let k = (v[i] & 15) as usize; re[i] += crate::jit::sift::C8[k] as i32; im[i] += crate::jit::sift::S8[k] as i32; }
        } });
    // the same re-quantisation as hv::bundle (complex sum → nearest of 16 phases)
    (0..hv::D).map(|i| { let a = (im[i] as f64).atan2(re[i] as f64); (((a / (2.0 * std::f64::consts::PI) * 16.0).round() as i64).rem_euclid(16)) as u8 }).collect()
}

/// All sections' shape hypervectors at once: prefix sums of the per-position 3-gram phasors along the token stream, 64 dimensions
/// at a time (memory tokens × 64 × 8 B), so a section costs two reads per dimension instead of re-summing its 3-grams — sections
/// nest (fn ⊃ runs ⊃ statements), and re-summing was ~1.7 G adds for one 2.5k-line file. Identical result to `shape_hv` per section.
pub fn section_hvs(shp: &[String], secs: &[Section]) -> Vec<Vec<u8>> {
    let n = shp.len();
    let mut cache: HashMap<String, Vec<u8>> = HashMap::new();
    let tri: Vec<Option<Vec<u8>>> = (0..n).map(|i| if i + 3 <= n { let key = format!("3g:{} {} {}", shp[i], shp[i + 1], shp[i + 2]);
        Some(cache.entry(key.clone()).or_insert_with(|| hv::sym(&key)).clone()) } else { None }).collect();
    let mut re_im: Vec<(Vec<i32>, Vec<i32>)> = secs.iter().map(|_| (vec![0; hv::D], vec![0; hv::D])).collect();
    const W: usize = 64;
    let mut pr = vec![0i32; (n + 1) * W]; let mut pi = vec![0i32; (n + 1) * W];
    for c0 in (0..hv::D).step_by(W) {
        for i in 0..n {
            for k in 0..W { let (r, im) = match &tri[i] { Some(v) => { let ph = (v[c0 + k] & 15) as usize; (crate::jit::sift::C8[ph] as i32, crate::jit::sift::S8[ph] as i32) } None => (0, 0) };
                pr[(i + 1) * W + k] = pr[i * W + k] + r; pi[(i + 1) * W + k] = pi[i * W + k] + im; }
        }
        for (si, sc) in secs.iter().enumerate() {
            // 3-grams fully inside the section start at a .. b-3
            if sc.b < sc.a + 3 { continue; }
            let (a, e) = (sc.a, sc.b - 2);
            for k in 0..W { re_im[si].0[c0 + k] = pr[e * W + k] - pr[a * W + k]; re_im[si].1[c0 + k] = pi[e * W + k] - pi[a * W + k]; }
        }
    }
    secs.iter().zip(re_im).map(|(sc, (re, im))| if sc.b < sc.a + 3 { hv::sym("empty") } else {
        (0..hv::D).map(|i| { let a = (im[i] as f64).atan2(re[i] as f64); (((a / (2.0 * std::f64::consts::PI) * 16.0).round() as i64).rem_euclid(16)) as u8 }).collect() }).collect()
}

/// D9 cross-app donor search: the ranges of the local file against every section of every app shard in `ix` (one rows8 batch
/// recall over all rows, then the exact collage against the donor's own file, re-lexed and cached). Sections of the range's
/// own file overlapping the range are excluded. Prints the top donors per range with app · file · lines · collage error · map.
#[allow(clippy::too_many_arguments)]
fn donors_cross(args: &[String], ix: &str, src_path: &str, toks: &[Tok], shp: &[String], secs: &[Section], hvs: &[Vec<u8>], ranges: &[usize], top: usize) {
    let t0 = std::time::Instant::now();
    let shortlist: usize = arg(args, "--shortlist").and_then(|v| v.parse().ok()).unwrap_or(200);
    // --no-forks: donors may not come from a file with the range file's own name (a fork of the same file, e.g. layout.rs copies)
    let no_forks = args.iter().any(|a| a == "--no-forks");
    let mut rows: Vec<u8> = Vec::new(); let mut meta: Vec<(String, serde_json::Value)> = Vec::new();
    let mut apps: Vec<String> = std::fs::read_dir(ix).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".rows")).map(|n| n.trim_end_matches(".rows").to_string()).collect()).unwrap_or_default();
    apps.sort();
    for app in &apps {
        let packed = std::fs::read(format!("{ix}/{app}.rows")).unwrap_or_default();
        let m: Vec<serde_json::Value> = std::fs::read_to_string(format!("{ix}/{app}.meta.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        if packed.len() != m.len() * hv::D / 2 { eprintln!("  skip {app}: rows/meta mismatch"); continue; }
        rows.extend(packed.iter().flat_map(|b| [b & 15, b >> 4]));
        meta.extend(m.into_iter().map(|v| (app.clone(), v)));
    }
    let t_load = t0.elapsed().as_secs_f64();
    let me = std::fs::canonicalize(src_path).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let qs: Vec<Vec<u8>> = ranges.iter().map(|&ri| hvs[ri].clone()).collect();
    let scores = hv::sift_batch(&rows, &qs);
    let t_sift = t0.elapsed().as_secs_f64();
    let mut files: HashMap<String, (Vec<Tok>, Vec<String>)> = HashMap::new();
    let mut out = String::new();
    for (k, &ri) in ranges.iter().enumerate() {
        let r = &secs[ri]; let rl = r.b - r.a;
        let mut cand: Vec<usize> = (0..meta.len()).filter(|&j| { let m = &meta[j].1; let (a, b) = (m["a"].as_u64().unwrap_or(0) as usize, m["b"].as_u64().unwrap_or(0) as usize);
            let len = b.saturating_sub(a); let same = m["file"].as_str() == Some(me.as_str());
            let fork = no_forks && m["file"].as_str().and_then(|f| f.rsplit('/').next()) == std::path::Path::new(src_path).file_name().and_then(|x| x.to_str());
            m["kind"] != "fn" && len * 3 >= rl && len <= 3 * rl && !(same && a < r.b && b > r.a) && !fork }).collect();
        cand.sort_by_key(|&j| std::cmp::Reverse(scores[k][j])); cand.truncate(shortlist);
        let mut scored: Vec<(f64, usize, Collage)> = Vec::new();
        for j in cand {
            let m = &meta[j].1; let f = m["file"].as_str().unwrap_or("").to_string();
            if !files.contains_key(&f) { let src = std::fs::read_to_string(&f).unwrap_or_default(); let tk = lex_lang(&src, lang_of(&f).unwrap_or("rust")); let sh: Vec<String> = (0..tk.len()).map(|i| shape(&tk, i)).collect(); files.insert(f.clone(), (tk, sh)); }
            let (td, sd) = &files[&f];
            let d = Section { a: m["a"].as_u64().unwrap() as usize, b: m["b"].as_u64().unwrap() as usize, kind: "stmt", depth: 0, func: m["fn"].as_str().unwrap_or("").to_string(), l0: m["l0"].as_u64().unwrap_or(0) as u32, l1: m["l1"].as_u64().unwrap_or(0) as u32 };
            if d.b > td.len() { continue; } // file changed since indexing
            let c = collage2(toks, shp, r, td, sd, &d); scored.push((c.err, j, c));
        }
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        eprintln!("range L{}-{} [{} · {} tok]", r.l0, r.l1, r.func, rl);
        let mut seen = std::collections::HashSet::new(); let mut shown = 0;
        for (err, j, c) in &scored {
            let (app, m) = &meta[*j]; let key = (m["file"].as_str().unwrap_or("").to_string(), m["l0"].as_u64());
            if !seen.insert(key) { continue; }
            let mp: Vec<String> = c.map.iter().filter(|(a, b, n)| a != b && *n >= 2).take(6).map(|(a, b, n)| format!("{a}→{b}×{n}")).collect();
            let f = m["file"].as_str().unwrap_or("");
            eprintln!("  {app:16} {} L{}-{} [{} · {}] err {:.3} map {}", f.split("/projects/").nth(1).or_else(|| f.split("/packages/").nth(1)).unwrap_or(f), m["l0"], m["l1"], m["fn"].as_str().unwrap_or(""), m["lang"].as_str().unwrap_or(""), err, mp.join(" "));
            out += &format!("{}\n", serde_json::json!({"range": [r.l0, r.l1], "range_file": src_path, "app": app, "donor_file": f, "donor": [m["l0"], m["l1"]], "fn": m["fn"], "lang": m["lang"], "err": err}));
            shown += 1; if shown >= top { break; }
        }
    }
    if let Some(p) = arg(args, "--out") { std::fs::write(&p, out).unwrap(); }
    eprintln!("doctor cross-app donors: {} ranges × {} sections in {} apps · load {:.2}s · batch sift {:.2}s · collage {:.2}s",
        ranges.len(), meta.len(), apps.len(), t_load, t_sift - t_load, t0.elapsed().as_secs_f64() - t_sift);
}

/// D9 — CODE INDEX across our apps (agent recommendation B, `an unpublished review`): every
/// own source file (Rust, Python, JS/TS/Svelte, C/C++) cut into sections, each a shape hypervector, one shard per app:
/// `<out>/<app>.rows` (4-bit packed phases, 512 B per section) + `<out>/<app>.meta.jsonl` (file, token span, lines, kind, fn) +
/// `<out>/<app>.files.json` (file → FNV hash, for incremental rebuilds). Vendored trees are never indexed (they may teach
/// ideas, never donate code — clean-room). `omni-render core --index [--out results/doctor-index] [--kinds fn,stmt,run] <app-dir>…`
pub fn index(args: &[String]) {
    let out = arg(args, "--out").unwrap_or_else(|| "results/doctor-index".into());
    let kinds: Vec<String> = arg(args, "--kinds").unwrap_or_else(|| "fn,stmt,run".into()).split(',').map(|x| x.to_string()).collect();
    let skip_val: std::collections::HashSet<String> = ["--out", "--kinds"].iter().filter_map(|k| arg(args, k)).collect();
    let apps: Vec<String> = args.iter().filter(|a| !a.starts_with("--") && !skip_val.contains(*a)).cloned().collect();
    std::fs::create_dir_all(&out).unwrap();
    const DENY: &[&str] = &["target", "node_modules", ".git", "dist", "build", ".venv", "venv", "__pycache__", ".svelte-kit", "vendor",
        "third_party", "svelte-grounding-library", "omni-render-doctor", "omni-render-jit", "worktrees", ".weaver-pr", "site-packages",
        ".engine"];
    // DRX CONVERGE step 1 (clean-room law, 2026-10-06): a file whose header carries someone else's copyright / licence is NEVER a donor
    // (ONNX Runtime headers slipped past the name deny-list: 1,853 sections); a directory holding a `REV` pin is a snapshot of another
    // repo (omni-ui/.engine = omni-render) and is skipped so forks cannot pose as cross-app donors.
    fn foreign_header(src: &str) -> bool {
        let head: String = src.chars().take(3000).collect::<String>().to_ascii_lowercase();
        let marks = head.contains("copyright") || head.contains("spdx-license-identifier") || head.contains("licensed under");
        let ours = head.contains("de santiago") || head.contains("sovereign raptor") || head.contains("sovring");
        marks && !ours
    }
    let t0 = std::time::Instant::now();
    let results: Vec<(String, usize, usize, usize, usize, f64)> = std::thread::scope(|sc| {
        let hs: Vec<_> = apps.iter().map(|app| { let out = out.clone(); let kinds = kinds.clone(); sc.spawn(move || {
            let ta = std::time::Instant::now();
            let name = std::path::Path::new(app).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| app.clone());
            let mut files: Vec<std::path::PathBuf> = Vec::new(); let mut stack = vec![std::path::PathBuf::from(app)];
            while let Some(d) = stack.pop() { let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() { let p = e.path(); let n = e.file_name().to_string_lossy().to_string();
                    if DENY.contains(&n.as_str()) || n.starts_with("target") { continue; }
                    if p.is_dir() { if p.join("REV").is_file() { continue; } stack.push(p); } else if lang_of(&n).is_some() && e.metadata().map(|m| m.len() < 400_000).unwrap_or(false) { files.push(p); } } }
            files.sort();
            // incremental: reuse rows + meta of files whose hash did not change
            let old_files: HashMap<String, u64> = std::fs::read_to_string(format!("{out}/{name}.files.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let old_meta: Vec<serde_json::Value> = std::fs::read_to_string(format!("{out}/{name}.meta.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            let old_rows = std::fs::read(format!("{out}/{name}.rows")).unwrap_or_default();
            let mut by_file: HashMap<String, Vec<usize>> = HashMap::new(); for (i, m) in old_meta.iter().enumerate() { by_file.entry(m["file"].as_str().unwrap_or("").to_string()).or_default().push(i); }
            let (mut rows, mut meta, mut hashes) = (Vec::new(), Vec::new(), HashMap::new()); let (mut reused, mut lexed, mut foreign) = (0usize, 0usize, 0usize);
            for f in &files {
                let Ok(src) = std::fs::read_to_string(f) else { continue };
                if foreign_header(&src) { foreign += 1; continue; }
                let fs = f.to_string_lossy().to_string();
                let h = { let mut h = 0xcbf29ce484222325u64; for b in src.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } h };
                hashes.insert(fs.clone(), h);
                if old_files.get(&fs) == Some(&h) && old_rows.len() == old_meta.len() * hv::D / 2 {
                    if let Some(ix) = by_file.get(&fs) { for &i in ix { rows.extend_from_slice(&old_rows[i * hv::D / 2..(i + 1) * hv::D / 2]); meta.push(old_meta[i].clone()); } reused += 1; continue; }
                }
                lexed += 1;
                let lang = lang_of(&fs).unwrap();
                let toks = lex_lang(&src, lang);
                let shp: Vec<String> = (0..toks.len()).map(|i| shape(&toks, i)).collect();
                let secs: Vec<Section> = sections(&toks, 12).into_iter().filter(|sct| kinds.iter().any(|k| k == sct.kind)).collect();
                let hvs = section_hvs(&shp, &secs);
                for (sct, v) in secs.iter().zip(hvs) {
                    rows.extend(v.chunks(2).map(|c| (c[0] & 15) | ((c[1] & 15) << 4)));
                    meta.push(serde_json::json!({"file": fs, "lang": lang, "a": sct.a, "b": sct.b, "l0": sct.l0, "l1": sct.l1, "kind": sct.kind, "fn": sct.func}));
                }
            }
            std::fs::write(format!("{out}/{name}.rows"), &rows).unwrap();
            std::fs::write(format!("{out}/{name}.meta.jsonl"), meta.iter().map(|m| format!("{m}\n")).collect::<String>()).unwrap();
            std::fs::write(format!("{out}/{name}.files.json"), serde_json::to_string(&hashes).unwrap()).unwrap();
            if foreign > 0 { eprintln!("  {name}: {foreign} file(s) refused — foreign copyright/licence header (never a donor)"); }
            (name, files.len(), meta.len(), reused, lexed, ta.elapsed().as_secs_f64())
        }) }).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let (mut nf, mut ns, mut bytes) = (0, 0, 0u64);
    for (name, f, sct, reused, lexed, secs) in &results {
        let b = std::fs::metadata(format!("{out}/{name}.rows")).map(|m| m.len()).unwrap_or(0) + std::fs::metadata(format!("{out}/{name}.meta.jsonl")).map(|m| m.len()).unwrap_or(0);
        eprintln!("  {name:24} {f:5} files ({reused} reused, {lexed} lexed) → {sct:7} sections · {:6.1} MB · {secs:.1}s", b as f64 / 1e6);
        nf += f; ns += sct; bytes += b;
    }
    eprintln!("doctor index: {} apps, {nf} files, {ns} sections, {:.1} MB, {:.1}s → {out}", results.len(), bytes as f64 / 1e6, t0.elapsed().as_secs_f64());
}

pub struct Collage { pub err: f64, pub map: Vec<(String, String, u32)>, pub matched: usize, pub donor_unmatched_lines: Vec<(u32, f64)>, pub dmatch: Vec<bool>, pub anchor: Vec<Option<usize>> }

/// Reverse-PIFS collage of domain D onto range R: LCS-align the shape streams; the map = for each domain identifier its most
/// frequent aligned range identifier (greedy one-to-one by count); matched = aligned tokens that also agree under the map.
pub fn collage(t: &[Tok], r: &Section, d: &Section, shp: &[String]) -> Collage { collage2(t, shp, r, t, shp, d) }
/// The collage between a range in one token stream and a domain in another (another file or app — the cross-app donor search).
pub fn collage2(tr: &[Tok], shr: &[String], r: &Section, td: &[Tok], shd: &[String], d: &Section) -> Collage {
    let (ra, rb, da, db) = (r.a, r.b, d.a, d.b); let (n, m) = (rb - ra, db - da);
    let mut dp = vec![0u16; (n + 1) * (m + 1)];
    for i in (0..n).rev() { for j in (0..m).rev() {
        dp[i * (m + 1) + j] = if shr[ra + i] == shd[da + j] { dp[(i + 1) * (m + 1) + j + 1] + 1 } else { dp[(i + 1) * (m + 1) + j].max(dp[i * (m + 1) + j + 1]) };
    } }
    let (mut i, mut j) = (0, 0); let mut pairs = Vec::new();
    while i < n && j < m {
        if shr[ra + i] == shd[da + j] { pairs.push((ra + i, da + j)); i += 1; j += 1; }
        else if dp[(i + 1) * (m + 1) + j] >= dp[i * (m + 1) + j + 1] { i += 1 } else { j += 1 }
    }
    let mut cnt: HashMap<(String, String), u32> = HashMap::new();
    for &(x, y) in &pairs { if tr[x].k == 0 && td[y].k == 0 { *cnt.entry((td[y].s.clone(), tr[x].s.clone())).or_default() += 1; } }
    let mut cv: Vec<_> = cnt.into_iter().collect(); cv.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let (mut used_d, mut used_r) = (std::collections::HashSet::new(), std::collections::HashSet::new()); let mut map = Vec::new();
    for ((dn, rn), c) in cv { if used_d.contains(&dn) || used_r.contains(&rn) { continue; } used_d.insert(dn.clone()); used_r.insert(rn.clone()); map.push((dn, rn, c)); }
    let mm: HashMap<&str, &str> = map.iter().map(|(a, b, _)| (a.as_str(), b.as_str())).collect();
    let mut matched = 0; let mut dmatch = vec![false; m];
    for &(x, y) in &pairs { let ok = tr[x].k != 0 || mm.get(td[y].s.as_str()).is_some_and(|v| *v == tr[x].s); if ok { matched += 1; dmatch[y - da] = true; } }
    let err = 1.0 - 2.0 * matched as f64 / (n + m) as f64;
    let mut per_line: std::collections::BTreeMap<u32, (u32, u32)> = Default::default();
    for j in 0..m { let e = per_line.entry(td[da + j].line).or_default(); e.1 += 1; if !dmatch[j] { e.0 += 1; } }
    let donor_unmatched_lines = per_line.into_iter().filter(|(_, (u, a))| *a >= 3 && *u as f64 / *a as f64 >= 0.5).map(|(l, (u, a))| (l, u as f64 / a as f64)).collect();
    // anchor[j] = the range token aligned with the last agreeing donor token at or before donor token j (where a transplant goes)
    let mut anchor = vec![None; m]; let mut last = None; let pm: HashMap<usize, usize> = pairs.iter().map(|&(x, y)| (y, x)).collect();
    for j in 0..m { if dmatch[j] { last = pm.get(&(da + j)).copied(); } anchor[j] = last; }
    Collage { err, map, matched, donor_unmatched_lines, dmatch, anchor }
}

/// `OMR_GATE_SHOW=1`: for every lost / gained box print tag, class, its text (first 70 chars) and the per-axis error
/// (x, y, w, h vs Chrome) under the base and the candidate program — what a lost box looks like, before reading code.
fn gate_show(pages: &[std::path::PathBuf], b: &HashMap<std::path::PathBuf, std::collections::HashSet<u32>>, c: &HashMap<std::path::PathBuf, std::collections::HashSet<u32>>,
             base: &HashMap<String, HashMap<String, Vec<i8>>>, cand: &HashMap<String, HashMap<String, Vec<i8>>>) {
    for p in pages {
        let (Some(x), Some(y)) = (b.get(p), c.get(p)) else { continue };
        let (lost, gained): (Vec<u32>, Vec<u32>) = (x.difference(y).copied().collect(), y.difference(x).copied().collect());
        if lost.is_empty() && gained.is_empty() { continue; }
        crate::layout::set_program(crate::layout::Program::from_map(base.clone())); let fb = super::score_full_pub(p);
        crate::layout::set_program(crate::layout::Program::from_map(cand.clone())); let fc = super::score_full_pub(p);
        let (Some(fb), Some(fc)) = (fb, fc) else { continue };
        let Some(cc) = super::cached_pub(p) else { continue };
        let d = &cc.l.dom;
        for (tag, list) in [("LOST", &lost), ("GAIN", &gained)] { for &n in list.iter().take(12) {
            let mut t = String::new(); let mut st = vec![n];
            while let Some(k) = st.pop() { if d.nodes[k as usize].kind == crate::dom::Kind::Text { t.push_str(d.text(k)); t.push('|'); } if t.len() > 70 { break; } let mut ks: Vec<u32> = d.children(k).collect(); ks.reverse(); st.extend(ks); }
            let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
            eprintln!("{tag} {} n{n} <{} .{}> base {:?} cand {:?} «{t}»", p.file_name().unwrap().to_string_lossy(), d.tag(n), d.attr(n, "class").unwrap_or(""), fb.1.get(&n), fc.1.get(&n));
        } }
    }
}

/// D2 gate for a ported patch (the core's gate, reason-gen form): with the base program vs the base program + one clause
/// (`patch.<key>[Ctx]=-1` turns the dormant port on), every box that agreed with Chrome on every replay AND holdout page must
/// still agree (lost = 0), and some box must gain. `omni-render core corpus --patch-gate --program P --clause K[C]=W [--workers 3]`
/// Exit 0 = PASS. Program is thread-local, so each worker sets it before scoring its pages.
pub fn patch_gate(args: &[String]) {
    let dir = args.first().filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog_path = arg(args, "--program").unwrap_or_else(|| "omr.program.json".into());
    let clause = arg(args, "--clause").expect("--clause key[Ctx]=w");
    let workers: usize = arg(args, "--workers").and_then(|v| v.parse().ok()).unwrap_or(3);
    let (kc, w) = clause.rsplit_once('=').expect("key[ctx]=w"); let w: i8 = w.parse().expect("ternary weight");
    let (key, ctx) = kc.split_once('[').map(|(k, c)| (k.to_string(), c.trim_end_matches(']').to_string())).expect("key[ctx]");
    let base: HashMap<String, HashMap<String, Vec<i8>>> = crate::layout::Program::load(&prog_path).map();
    let mut cand = base.clone(); cand.entry(format!("k:{key}")).or_default().insert(ctx.clone(), vec![w]);
    let kk: HashMap<String, HashMap<String, i8>> = cand.iter().filter_map(|(p, c)| p.strip_prefix("k:").map(|k| (k.to_string(), c.iter().map(|(x, v)| (x.clone(), v[0])).collect()))).collect();
    let ww: HashMap<String, HashMap<String, Vec<i8>>> = cand.iter().filter(|(p, _)| !p.starts_with("k:")).map(|(p, c)| (p.clone(), c.clone())).collect();
    let errs = crate::layout::verify_program(&kk, &ww); if !errs.is_empty() { eprintln!("patch-gate: candidate program refused by the verifier: {errs:?}"); std::process::exit(2); }
    let stem = prog_path.trim_end_matches(".json").to_string();
    let (replay, holdout, ysha) = super::frozen_sets(&dir, &format!("{stem}.sets.json"));
    let pages: Vec<std::path::PathBuf> = replay.iter().chain(&holdout).cloned().collect();
    let t0 = std::time::Instant::now();
    let score_all = |m: &HashMap<String, HashMap<String, Vec<i8>>>| -> HashMap<std::path::PathBuf, std::collections::HashSet<u32>> {
        let chunks: Vec<Vec<std::path::PathBuf>> = (0..workers).map(|k| pages.iter().skip(k).step_by(workers).cloned().collect()).collect();
        std::thread::scope(|s| {
            let hs: Vec<_> = chunks.into_iter().map(|ch| { let m = m.clone(); s.spawn(move || {
                crate::layout::set_program(crate::layout::Program::from_map(m));
                ch.into_iter().filter_map(|p| super::score(&p).map(|(g, _, _)| (p, g))).collect::<Vec<_>>() }) }).collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        })
    };
    let b = score_all(&base); let c = score_all(&cand);
    let (mut lost, mut gained) = ((0usize, 0usize), (0usize, 0usize)); let mut per_page = Vec::new();
    for p in &pages {
        let (Some(x), Some(y)) = (b.get(p), c.get(p)) else { continue };
        let (l, g) = (x.difference(y).count(), y.difference(x).count());
        let h = holdout.contains(p);
        if h { lost.1 += l; gained.1 += g; } else { lost.0 += l; gained.0 += g; }
        if l + g > 0 { per_page.push(serde_json::json!({"page": p.file_name().unwrap().to_string_lossy(), "holdout": h, "lost": l, "gained": g, "lost_nodes": x.difference(y).take(12).collect::<Vec<_>>()})); }
    }
    let sum = |m: &HashMap<std::path::PathBuf, std::collections::HashSet<u32>>, ps: &[std::path::PathBuf]| -> usize { ps.iter().filter_map(|p| m.get(p)).map(|s| s.len()).sum() };
    if std::env::var("OMR_GATE_SHOW").is_ok() { gate_show(&pages, &b, &c, &base, &cand); }
    let pass = lost == (0, 0) && gained.0 + gained.1 > 0;
    let j = serde_json::json!({"clause": clause, "program": prog_path, "sets": &ysha[..12], "pass": pass,
        "base": [sum(&b, &replay), sum(&b, &holdout)], "cand": [sum(&c, &replay), sum(&c, &holdout)],
        "lost": [lost.0, lost.1], "gained": [gained.0, gained.1], "pages": per_page, "secs": t0.elapsed().as_secs_f64()});
    println!("{j}");
    eprintln!("patch-gate {clause}: base replay {} holdout {} → cand {} / {} · lost {}/{} · gained {}/{} → {} ({:.1}s)", sum(&b, &replay), sum(&b, &holdout), sum(&c, &replay), sum(&c, &holdout), lost.0, lost.1, gained.0, gained.1, if pass { "PASS" } else { "FAIL" }, t0.elapsed().as_secs_f64());
    std::process::exit(if pass { 0 } else { 1 });
}

type Agree = HashMap<std::path::PathBuf, std::collections::HashSet<u32>>;
fn score_map(pages: &[std::path::PathBuf], m: &HashMap<String, HashMap<String, Vec<i8>>>, workers: usize) -> Agree {
    let chunks: Vec<Vec<std::path::PathBuf>> = (0..workers).map(|k| pages.iter().skip(k).step_by(workers).cloned().collect()).collect();
    std::thread::scope(|s| {
        let hs: Vec<_> = chunks.into_iter().map(|ch| { let m = m.clone(); s.spawn(move || {
            crate::layout::set_program(crate::layout::Program::from_map(m));
            ch.into_iter().filter_map(|p| super::score(&p).map(|(g, _, _)| (p, g))).collect::<Vec<_>>() }) }).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}
fn diff(b: &Agree, c: &Agree) -> (usize, usize) {
    let (mut l, mut g) = (0, 0);
    for (p, x) in b { if let Some(y) = c.get(p) { l += x.difference(y).count(); g += y.difference(x).count(); } }
    (l, g)
}

/// D2c CONDITION SEARCH for a dormant patch (the SAT station's move applied to ported code; clean-room, greedy set-cover
/// style search over the same node features `knob_at` reads): gate `key[Ctx&lit]=-1` for every literal `f=v` / `f!=v` seen
/// on Ctx nodes, then conjunctions of the best lossy literal with literals that also gain; keep the best clause with 0 lost.
/// `omni-render core corpus --patch-search --key patch.K --ctx Table --program P [--pairs 4] [--top 4]`
pub fn patch_search(args: &[String]) {
    let dir = args.first().filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog_path = arg(args, "--program").unwrap_or_else(|| "omr.program.json".into());
    let key = arg(args, "--key").expect("--key patch.K"); let ctx = arg(args, "--ctx").expect("--ctx Display");
    let npairs: usize = arg(args, "--pairs").and_then(|v| v.parse().ok()).unwrap_or(4);
    let topv: usize = arg(args, "--top").and_then(|v| v.parse().ok()).unwrap_or(4);
    let workers: usize = arg(args, "--workers").and_then(|v| v.parse().ok()).unwrap_or(3);
    // greedy cover (set-cover style): each accepted zero-loss clause joins the base, the next round searches what is left
    let rounds: usize = arg(args, "--rounds").and_then(|v| v.parse().ok()).unwrap_or(1);
    let stem = prog_path.trim_end_matches(".json").to_string();
    let (replay, holdout, _) = super::frozen_sets(&dir, &format!("{stem}.sets.json"));
    let pages: Vec<std::path::PathBuf> = replay.iter().chain(&holdout).cloned().collect();
    let mut base_m = crate::layout::Program::load(&prog_path).map();
    let t0 = std::time::Instant::now();
    let mut seen: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for p in &pages {
        let Some(c) = super::cached(p) else { continue };
        let (d, st) = (&c.l.dom, &c.l.styled);
        for n in 0..d.nodes.len() as u32 {
            if d.nodes[n as usize].kind != crate::dom::Kind::Element { continue; }
            if format!("{:?}", st.table[st.ids[n as usize] as usize].display) != ctx { continue; }
            for f in crate::layout::FEATURES { *seen.entry(f.to_string()).or_default().entry(crate::layout::node_feature(d, st, n, f)).or_default() += 1; }
        }
    }
    let mut lits: Vec<String> = Vec::new();
    for (f, vs) in &seen {
        if vs.len() < 2 { continue; } // a feature constant on every Ctx node cannot separate anything
        let mut v: Vec<_> = vs.iter().collect(); v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (val, _) in v.iter().take(topv) { if !val.contains('&') && !val.contains('=') { lits.push(format!("{f}={val}")); } }
        if let Some((val, _)) = v.first() { if !val.contains('&') { lits.push(format!("{f}!={val}")); } }
    }
    lits.sort();
    let out = format!("{stem}.patch-search.jsonl");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&out).unwrap();
    use std::io::Write;
    let mut accepted: Vec<(String, usize)> = Vec::new(); let mut gates = 0usize;
    for round in 1..=rounds {
        let base = score_map(&pages, &base_m, workers);
        let gate = |clause_ctx: &str| -> (usize, usize) {
            let mut m = base_m.clone(); m.entry(format!("k:{key}")).or_default().insert(clause_ctx.to_string(), vec![-1]);
            diff(&base, &score_map(&pages, &m, workers))
        };
        let mut results: Vec<(String, usize, usize)> = Vec::new();
        let (l0, g0) = gate(&ctx); results.push((ctx.clone(), l0, g0));
        eprintln!("round {round} {key}[{ctx}]: unconditioned lost {l0} gained {g0}; {} literals", lits.len());
        for lit in &lits {
            let c = format!("{ctx}&{lit}"); if accepted.iter().any(|a| a.0 == c) { continue; }
            let (l, g) = gate(&c); if g > 0 { eprintln!("  {c:50} lost {l:4} gained {g:4}"); } results.push((c, l, g));
        }
        let mut lossy: Vec<_> = results.iter().filter(|r| r.1 > 0 && r.2 > r.1 && r.0 != ctx).cloned().collect(); // the bare ctx has no literal to pair lossy.sort_by(|a, b| (b.2 - b.1).cmp(&(a.2 - a.1)));
        let gainers: Vec<String> = results.iter().filter(|r| r.2 > 0 && r.0 != ctx).map(|r| r.0[ctx.len() + 1..].to_string()).collect();
        for (c, _, _) in lossy.iter().take(npairs) {
            let f0 = c[ctx.len() + 1..].split(['=', '!']).next().unwrap_or("").to_string();
            for lit in &gainers {
                if lit.split(['=', '!']).next() == Some(f0.as_str()) { continue; }
                let cc = format!("{c}&{lit}"); let (l, g) = gate(&cc); if g > 0 { eprintln!("  {cc:50} lost {l:4} gained {g:4}"); } results.push((cc, l, g));
            }
        }
        gates += results.len();
        for (c, l, g) in &results { writeln!(f, "{}", serde_json::json!({"key": key, "ctx": c, "lost": l, "gained": g, "round": round})).ok(); }
        let Some((c, _, g)) = results.iter().filter(|r| r.1 == 0 && r.2 > 0).max_by_key(|r| (r.2, std::cmp::Reverse(r.0.len()))).cloned() else {
            eprintln!("round {round}: no zero-loss clause — cover complete"); break };
        eprintln!("round {round} ACCEPT {key}[{c}]=-1: gained {g}, lost 0");
        base_m.entry(format!("k:{key}")).or_default().insert(c.clone(), vec![-1]);
        accepted.push((c, g));
    }
    let total: usize = accepted.iter().map(|a| a.1).sum();
    eprintln!("patch-search {key}: {} clause(s) accepted, +{total} boxes, 0 lost ({gates} gates, {:.0}s) → {out}", accepted.len(), t0.elapsed().as_secs_f64());
    for (c, g) in &accepted { eprintln!("  {key}[{c}]=-1  (+{g})"); }
    if let Some(w) = arg(args, "--write") { if !accepted.is_empty() { crate::layout::Program::from_map(base_m.clone()).save(&w); eprintln!("program with the accepted clauses → {w}"); } }
}

/// D2′ BINARY gate — for engine changes the program cannot switch (style parsing is cached per page, independent of the
/// program): `--agree-snapshot OUT` (old binary) writes every frozen page's agreeing box set; `--binary-gate BASE` (new binary)
/// scores the same pages with the same program and applies the same rule: lost 0 on replay + holdout, gained > 0 (exit 0).
pub fn binary_gate(args: &[String]) {
    let dir = args.first().filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog_path = arg(args, "--program").unwrap_or_else(|| "omr.program.json".into());
    let workers: usize = arg(args, "--workers").and_then(|v| v.parse().ok()).unwrap_or(3);
    let stem = prog_path.trim_end_matches(".json").to_string();
    let (replay, holdout, ysha) = super::frozen_sets(&dir, &format!("{stem}.sets.json"));
    let pages: Vec<std::path::PathBuf> = replay.iter().chain(&holdout).cloned().collect();
    let m = crate::layout::Program::load(&prog_path).map();
    let t0 = std::time::Instant::now();
    // ids: a new snapshot is written with stable ids (real-node index, pseudo-elements excluded); a gate reads the base's kind
    let base_j: Option<serde_json::Value> = arg(args, "--binary-gate").map(|b| serde_json::from_str(&std::fs::read_to_string(&b).unwrap()).unwrap());
    let stable = base_j.as_ref().map_or(true, |j| j["ids"].as_str() == Some("stable"));
    let chunks: Vec<Vec<std::path::PathBuf>> = (0..workers).map(|k| pages.iter().skip(k).step_by(workers).cloned().collect()).collect();
    let now: HashMap<String, std::collections::BTreeSet<u32>> = std::thread::scope(|s| {
        let hs: Vec<_> = chunks.into_iter().map(|ch| { let m = m.clone(); s.spawn(move || {
            crate::layout::set_program(crate::layout::Program::from_map(m));
            ch.into_iter().filter_map(|p| { let g = if stable { super::score_stable(&p) } else { super::score(&p).map(|(g, _, _)| g) };
                g.map(|g| (p.file_name().unwrap().to_string_lossy().to_string(), g.into_iter().collect())) }).collect::<Vec<_>>() }) }).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    if let Some(out) = arg(args, "--agree-snapshot") {
        std::fs::write(&out, serde_json::json!({"program": prog_path, "sets": &ysha[..12], "ids": if stable { "stable" } else { "arena" }, "agree": now}).to_string()).unwrap();
        eprintln!("agree snapshot → {out}: {} pages, {} agreeing boxes ({:.1}s)", now.len(), now.values().map(|v| v.len()).sum::<usize>(), t0.elapsed().as_secs_f64());
        return;
    }
    let base_path = arg(args, "--binary-gate").expect("--binary-gate BASE.json");
    let j: serde_json::Value = base_j.unwrap();
    if j["sets"].as_str() != Some(&ysha[..12]) { eprintln!("binary-gate: snapshot was taken on a different yardstick"); std::process::exit(2); }
    let (mut lost, mut gained) = ((0usize, 0usize), (0usize, 0usize)); let mut per_page = Vec::new();
    let (mut b_r, mut b_h, mut c_r, mut c_h) = (0, 0, 0, 0);
    for p in &pages {
        let name = p.file_name().unwrap().to_string_lossy().to_string(); let h = holdout.contains(p);
        let x: std::collections::BTreeSet<u32> = j["agree"][&name].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|v| v as u32)).collect()).unwrap_or_default();
        let y = now.get(&name).cloned().unwrap_or_default();
        let (l, g) = (x.difference(&y).count(), y.difference(&x).count());
        if h { lost.1 += l; gained.1 += g; b_h += x.len(); c_h += y.len(); } else { lost.0 += l; gained.0 += g; b_r += x.len(); c_r += y.len(); }
        if l + g > 0 { per_page.push(serde_json::json!({"page": name, "holdout": h, "lost": l, "gained": g, "lost_nodes": x.difference(&y).take(12).collect::<Vec<_>>()})); }
    }
    let pass = lost == (0, 0) && gained.0 + gained.1 > 0;
    println!("{}", serde_json::json!({"base": [b_r, b_h], "cand": [c_r, c_h], "lost": [lost.0, lost.1], "gained": [gained.0, gained.1], "pages": per_page, "pass": pass, "sets": &ysha[..12]}));
    eprintln!("binary-gate vs {base_path}: replay {b_r} → {c_r} · holdout {b_h} → {c_h} · lost {}/{} · gained {}/{} → {} ({:.1}s)", lost.0, lost.1, gained.0, gained.1, if pass { "PASS" } else { "FAIL" }, t0.elapsed().as_secs_f64());
    std::process::exit(if pass { 0 } else { 1 });
}

/// Property names the engine's style code handles: string literals in `style.rs` that open a match arm (`"x" =>` or
/// `"x" | "y" =>`), read from the source at compile time so the list cannot drift from the code (as `layout::known_keys`).
pub fn handled_props() -> std::collections::HashSet<String> {
    const SRC: &str = include_str!("style.rs");
    let mut out = std::collections::HashSet::new();
    for line in SRC.lines() {
        let Some(arrow) = line.find("=>") else { continue };
        // walk every `"lit" =>` / `"a" | "b" =>` group on the line
        let mut rest = line; let mut base = 0;
        while let Some(q) = rest.find('"') {
            let start = base + q + 1; let Some(e) = line[start..].find('"') else { break };
            let lit = &line[start..start + e]; let after = line[start + e + 1..].trim_start();
            if (after.starts_with("=>") || after.starts_with('|')) && start < arrow + 400 && !lit.is_empty() && lit.chars().all(|c| c.is_ascii_lowercase() || c == '-') { out.insert(lit.to_string()); }
            base = start + e + 1; rest = &line[base..];
        }
    }
    out
}
/// Guard arms (`n if n.starts_with("border-") && (n.ends_with("-width") || …) =>`): (prefix, suffixes) read from `style.rs`.
pub fn handled_guards() -> Vec<(String, Vec<String>)> {
    const SRC: &str = include_str!("style.rs");
    let lit = |s: &str, k: &str| -> Vec<String> { s.match_indices(k).filter_map(|(i, _)| { let r = &s[i + k.len()..]; r.find('"').map(|e| r[..e].to_string()) }).collect() };
    SRC.lines().filter(|l| l.contains("n if n.starts_with(\"") && l.contains("=>")).filter_map(|l| {
        let pre = lit(l, "starts_with(\"").into_iter().next()?; Some((pre, lit(l, "ends_with(\""))) }).collect()
}
fn is_handled(name: &str, lits: &std::collections::HashSet<String>, guards: &[(String, Vec<String>)]) -> bool {
    lits.contains(name) || guards.iter().any(|(p, sfx)| name.starts_with(p.as_str()) && (sfx.is_empty() || sfx.iter().any(|x| name.ends_with(x.as_str()))))
}

/// D1b MISSING-FEATURE station: a problem no knob and no donor reaches may be a CSS feature the engine never implemented.
/// For every broken origin box on the replay pages, the winning declarations (explain's cascade provenance) on the box and its
/// ancestors (up to `--up 4`) minus the properties `style.rs` handles; ranked by count × lift against a sample of agreeing
/// boxes (lift = P(prop | broken) / P(prop | agreeing)), so a property every box carries does not rank. One card per property.
/// `omni-render core corpus --missing [--program P] [--up 4] [--out results/omr.program.missing.jsonl]`
pub fn missing(args: &[String]) {
    let dir = args.first().filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog_path = arg(args, "--program").unwrap_or_else(|| "omr.program.json".into());
    let up: usize = arg(args, "--up").and_then(|v| v.parse().ok()).unwrap_or(4);
    let stem = prog_path.trim_end_matches(".json").to_string();
    let out_path = arg(args, "--out").unwrap_or_else(|| format!("{stem}.missing.jsonl"));
    crate::layout::set_program(crate::layout::Program::load(&prog_path));
    let (replay, _holdout, _) = super::frozen_sets(&dir, &format!("{stem}.sets.json"));
    let handled = handled_props(); let guards = handled_guards();
    let t0 = std::time::Instant::now();
    #[derive(Default)] struct Card { broken: usize, agree: usize, pages: std::collections::BTreeSet<String>, sigs: HashMap<String, usize>, values: HashMap<String, usize>, sample: Vec<(String, u32, String)> }
    let mut cards: HashMap<String, Card> = HashMap::new();
    let (mut n_broken, mut n_agree) = (0usize, 0usize);
    for p in &replay {
        let Some(full) = super::score_full(p) else { continue };
        let Some(c) = super::cached(p) else { continue };
        let (d, sheet) = (&c.l.dom, &c.l.sheet);
        let page = p.file_name().unwrap().to_string_lossy().to_string();
        let props_up = |n: u32| -> HashMap<String, String> {
            let mut m = HashMap::new(); let mut x = n;
            for _ in 0..=up {
                if x == crate::dom::NONE || d.nodes[x as usize].kind != crate::dom::Kind::Element { break; }
                for (name, val, _) in crate::explain::provenance(d, sheet, x) {
                    // paint-only longhands (radius, colour) cannot move a box
                    if is_handled(&name, &handled, &guards) || !crate::explain::layout_prop(&name) || name.ends_with("-radius") || name.ends_with("-color") { continue; }
                    m.entry(name).or_insert(val);
                }
                x = d.nodes[x as usize].parent;
            }
            m
        };
        for o in &full.origins {
            n_broken += 1;
            for (name, val) in props_up(o.n) {
                let cd = cards.entry(name).or_default(); cd.broken += 1; cd.pages.insert(page.clone());
                *cd.sigs.entry(o.sig.clone()).or_default() += 1; *cd.values.entry(val.clone()).or_default() += 1;
                if cd.sample.len() < 4 { cd.sample.push((page.clone(), o.n, val)); }
            }
        }
        // negatives: every 8th agreeing box (deterministic order)
        let mut ag: Vec<u32> = full.agree.iter().copied().collect(); ag.sort();
        for &n in ag.iter().step_by(8) { n_agree += 1; for (name, _) in props_up(n) { cards.entry(name).or_default().agree += 1; } }
    }
    // Ochiai suspiciousness (spectrum-based fault localisation; Abreu et al. 2007, clean-room from the formula):
    // ef / sqrt((ef + nf) · (ef + ep)) with ef = broken boxes carrying the property, nf = broken boxes without it, ep = agreeing
    // boxes carrying it (scaled ×8: one in eight agreeing boxes is sampled). Replaces the earlier count × lift score.
    let mut rows: Vec<(f64, String, Card)> = cards.into_iter().filter(|(_, c)| c.broken > 0).map(|(k, c)| {
        let ef = c.broken as f64; let ep = c.agree as f64 * 8.0;
        (ef / ((n_broken.max(1) as f64) * (ef + ep)).sqrt(), k, c) }).collect();
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    // negative knowledge: properties already tested (gate 0/0) or known inert in this engine are listed, never ranked
    let neg: HashMap<String, String> = std::fs::read_to_string(format!("{stem}.missing.negative.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter_map(|j| Some((j["property"].as_str()?.to_string(), j["reason"].as_str().unwrap_or("").to_string()))).collect();
    let (skipped, kept): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| neg.contains_key(&r.1));
    let rows = kept;
    for (_, k, c) in &skipped { eprintln!("  (negative) {k}: {} broken — {}", c.broken, neg[k]); }
    let mut out = String::new();
    eprintln!("doctor missing: {} broken origins, {} agreeing sampled, {} handled properties + {} guard arms, {} replay pages, {:.1}s", n_broken, n_agree, handled.len(), guards.len(), replay.len(), t0.elapsed().as_secs_f64());
    for (score, k, c) in rows.iter().take(25) {
        let lift = (c.broken as f64 / n_broken.max(1) as f64) / ((c.agree as f64 + 0.5) / n_agree.max(1) as f64);
        let mut sigs: Vec<_> = c.sigs.iter().collect(); sigs.sort_by(|a, b| b.1.cmp(a.1));
        let mut vals: Vec<_> = c.values.iter().collect(); vals.sort_by(|a, b| b.1.cmp(a.1));
        eprintln!("  {k:28} ochiai {score:.3} broken {:4} agree {:4} lift {:6.2} pages {:2} · top {} · values {}", c.broken, c.agree, lift, c.pages.len(),
            sigs.iter().take(2).map(|(s, n)| format!("{s}×{n}")).collect::<Vec<_>>().join(" "), vals.iter().take(3).map(|(v, n)| format!("{v}×{n}")).collect::<Vec<_>>().join(" "));
        out += &format!("{}\n", serde_json::json!({"property": k, "score": score, "broken": c.broken, "agree_sampled": c.agree, "lift": lift, "pages": c.pages,
            "sigs": c.sigs, "values": vals.iter().take(8).map(|(v, n)| serde_json::json!([v, n])).collect::<Vec<_>>(),
            "samples": c.sample.iter().map(|(p, n, v)| serde_json::json!({"page": p, "node": n, "value": v})).collect::<Vec<_>>(),
            "missing": "declared on the broken box or an ancestor, and no style.rs match arm handles it"}));
    }
    std::fs::write(&out_path, out).unwrap(); eprintln!("→ {out_path}");
}

/// Names DECLARED where the insertion point (token index `at`) can see them: fn parameters (`name:`), `let` / `let mut` / `for`
/// patterns, closure parameters and `if let` / `while let` pattern bindings, each only while its enclosing block is still open at `at`.
fn declared_in_scope(t: &[Tok], fn_start: usize, at: usize) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new(); out.insert("self".to_string());
    let mut stack: Vec<Vec<String>> = vec![Vec::new()]; // declarations per open brace
    let mut pending: Vec<String> = Vec::new(); // let/if-let/for/closure bindings take effect at the statement / block they open
    let mut i = fn_start;
    while i < at.min(t.len()) {
        let s = t[i].s.as_str();
        match s {
            "{" => { let mut v = Vec::new(); v.append(&mut pending); stack.push(v); }
            "}" => { stack.pop(); if stack.is_empty() { stack.push(Vec::new()); } }
            "let" | "for" => {
                // pattern identifiers up to `=` / `in` / `:`-type annotation end
                let mut j = i + 1; let mut depth = 0;
                while j < t.len() && !(depth == 0 && (t[j].s == "=" || t[j].s == "in" || t[j].s == ";")) {
                    if t[j].s == "(" || t[j].s == "[" { depth += 1; } else if t[j].s == ")" || t[j].s == "]" { depth -= 1; }
                    if t[j].k == 0 && t[j].s.chars().next().is_some_and(|c| c.is_lowercase() || c == '_') && t.get(j + 1).is_none_or(|n| n.s != "(" && n.s != "::") && !(j > 0 && t[j - 1].s == ":") {
                        // `let x: Type` — the annotation's lowercase words are types/paths, skip after ':'
                        if t.get(j.wrapping_sub(1)).is_some_and(|p| p.s == ":") { j += 1; continue; }
                        if s == "let" && t[i + 1].s != "(" && t[i + 1].s != "mut" && j > i + 1 && t[j - 1].s != "(" && t[j - 1].s != "," && t[j - 1].s != "let" && t[j - 1].s != "mut" { j += 1; continue; }
                        pending.push(t[j].s.clone());
                    }
                    j += 1;
                }
                // a plain `let` binds from the end of its statement in the CURRENT block
                if s == "let" { if let Some(top) = stack.last_mut() { top.append(&mut pending); } }
            }
            "|" => { // closure parameters |a, b|
                let mut j = i + 1; while j < t.len() && t[j].s != "|" { if t[j].k == 0 { pending.push(t[j].s.clone()); } j += 1; }
                if let Some(top) = stack.last_mut() { top.extend(pending.drain(..)); }
                i = j;
            }
            _ => {
                // fn parameters: `name:` inside the signature parens before the body
                if t[i].k == 0 && t.get(i + 1).is_some_and(|n| n.s == ":") && stack.len() == 1 { if let Some(top) = stack.last_mut() { top.push(t[i].s.clone()); } }
            }
        }
        i += 1;
    }
    for v in stack { out.extend(v); }
    out
}

/// Rename identifiers in a code fragment through the donor→range map (word boundaries; strings and comments untouched is not
/// guaranteed — the build and the gate decide).
fn rename(text: &str, map: &[(String, String, u32)]) -> String {
    // only renames seen at least twice in the alignment (a single aligned pair is often a coincidence: n→margin)
    let mm: HashMap<&str, &str> = map.iter().filter(|(a, b, c)| a != b && *c >= 2).map(|(a, b, _)| (a.as_str(), b.as_str())).collect();
    let mut out = String::new(); let mut word = String::new();
    let flush = |w: &mut String, o: &mut String| { if !w.is_empty() { o.push_str(mm.get(w.as_str()).copied().unwrap_or(w.as_str())); w.clear(); } };
    for ch in text.chars() { if ch.is_alphanumeric() || ch == '_' { word.push(ch); } else { flush(&mut word, &mut out); out.push(ch); } }
    flush(&mut word, &mut out); out
}
/// Edit class of a transplant unit (the agent-fix mining classes, tools/seatfix.py: guard / term-add / clamp / loop / binding / other).
fn edit_class(text: &str) -> &'static str {
    let t = text.trim_start();
    if t.starts_with("let ") { "binding" } else if t.starts_with("if ") || t.contains(" if ") { "guard" }
    else if t.starts_with("for ") || t.starts_with("while ") { "loop" }
    else if t.contains(".max(") || t.contains(".min(") || t.contains(".clamp(") { "clamp" }
    else if t.contains("+=") || t.contains("-=") { "term-add" } else { "other" }
}
/// SimFix-style prior (Jiang et al. ISSTA 2018, clean-room: prefer the kinds of change that worked before): per edit class,
/// (passes + α·seed) / (tries + α); seed = the class share among the 28 mined agent fixes (guard 14, term-add 4, clamp 3, other 2),
/// updated by every transplant outcome in the ledger.
fn class_prior(ledger: &str) -> HashMap<String, f64> {
    let seed: HashMap<&str, f64> = [("guard", 15.0), ("term-add", 5.0), ("clamp", 4.0), ("loop", 1.0), ("binding", 0.5), ("other", 3.0)].into_iter().map(|(k, v)| (k, v / 34.0)).collect();
    let mut tp: HashMap<String, (f64, f64)> = HashMap::new();
    for l in std::fs::read_to_string(ledger).unwrap_or_default().lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let e = tp.entry(j["class"].as_str().unwrap_or("other").to_string()).or_default(); e.1 += 1.0; if j["outcome"] == "pass" { e.0 += 1.0; }
    }
    let alpha = 4.0;
    seed.iter().map(|(k, s0)| { let (p, t) = tp.get(*k).copied().unwrap_or((0.0, 0.0)); (k.to_string(), (p + alpha * s0) / (t + alpha)) }).collect()
}

fn arg(args: &[String], k: &str) -> Option<String> { args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned() }

/// `omni-render core --sections [--src src/layout.rs]` (D0) ·
/// `omni-render core --donors --range L0-L1 [--src …] [--top 5] [--shortlist 300] [--out results/doctor-donors.jsonl]` (D1)
pub fn run(args: &[String]) {
    let src_path = arg(args, "--src").unwrap_or_else(|| "src/layout.rs".into());
    let src = std::fs::read_to_string(&src_path).unwrap_or_else(|e| panic!("{src_path}: {e}"));
    let t0 = std::time::Instant::now();
    let toks = lex_lang(&src, lang_of(&src_path).unwrap_or("rust"));
    let t_lex = t0.elapsed().as_secs_f64();
    let min_toks: usize = arg(args, "--min").and_then(|v| v.parse().ok()).unwrap_or(12);
    let secs = sections(&toks, min_toks);
    let t_sec = t0.elapsed().as_secs_f64();
    let shp: Vec<String> = (0..toks.len()).map(|i| shape(&toks, i)).collect();
    let hvs: Vec<Vec<u8>> = section_hvs(&shp, &secs);
    let t_hv = t0.elapsed().as_secs_f64();
    if args.iter().any(|a| a == "--time") { eprintln!("time: lex {:.3}s · sections {:.3}s · shape+hv {:.3}s", t_lex, t_sec - t_lex, t_hv - t_sec); }
    if args.iter().any(|a| a == "--hv-check") { let bad = secs.iter().zip(&hvs).filter(|(sc, h)| shape_hv(&shp[sc.a..sc.b]) != **h).count(); eprintln!("hv-check: prefix-sum vectors = direct vectors on {} of {} sections", secs.len() - bad, secs.len()); }
    let by_kind = |k: &str| secs.iter().filter(|s| s.kind == k).count();
    let lines = src.lines().count();
    let covered = { let mut c = vec![false; lines + 2]; for s in secs.iter().filter(|s| s.kind != "fn") { for l in s.l0..=s.l1 { c[l as usize] = true; } } c.iter().filter(|x| **x).count() };
    eprintln!("doctor sections: {src_path} {lines} lines, {} tokens → {} sections (fn {} · stmt {} · run {}), {} lines inside a sub-fn section, {:.2}s",
        toks.len(), secs.len(), by_kind("fn"), by_kind("stmt"), by_kind("run"), covered, t0.elapsed().as_secs_f64());
    if !args.iter().any(|a| a == "--donors") { return; }
    let rng = arg(args, "--range").expect("--range L0-L1");
    let (r0, r1) = rng.split_once('-').map(|(a, b)| (a.parse::<u32>().unwrap(), b.parse::<u32>().unwrap())).expect("L0-L1");
    let top: usize = arg(args, "--top").and_then(|v| v.parse().ok()).unwrap_or(5);
    let shortlist: usize = arg(args, "--shortlist").and_then(|v| v.parse().ok()).unwrap_or(300);
    // ranges: the maximal non-fn sections inside the line span (the largest blocks first, like a quadtree's top level)
    let mut ranges: Vec<usize> = (0..secs.len()).filter(|&i| secs[i].kind != "fn" && secs[i].l0 >= r0 && secs[i].l1 <= r1).collect();
    ranges.sort_by_key(|&i| std::cmp::Reverse(secs[i].b - secs[i].a));
    if let Some(n) = arg(args, "--ranges").and_then(|v| v.parse::<usize>().ok()) { ranges.truncate(n); }
    if let Some(ix) = arg(args, "--index") { return donors_cross(args, &ix, &src_path, &toks, &shp, &secs, &hvs, &ranges, top); }
    let rows: Vec<u8> = hvs.concat();
    let out_path = arg(args, "--out");
    let mut out_rows = Vec::new(); let mut sift_s = 0.0f64;
    // all ranges' shape queries recalled in ONE batch call (rows8: 8 queries per row load); --sift-check compares every score
    // with the one-query kernel
    let tb = std::time::Instant::now();
    let qs: Vec<Vec<u8>> = ranges.iter().map(|&ri| hvs[ri].clone()).collect();
    let batch = hv::sift_batch(&rows, &qs);
    let batch_ms = tb.elapsed().as_secs_f64() * 1e3;
    let mut check_bad = 0usize;
    let priors = class_prior("results/doctor.transplants.jsonl");
    let mut units: Vec<serde_json::Value> = Vec::new(); let mut scope_drop = 0usize;
    for &ri in &ranges {
        let r = &secs[ri];
        let k = ranges.iter().position(|&x| x == ri).unwrap();
        let scores = batch[k].clone();
        if args.iter().any(|a| a == "--sift-check") {
            let ts = std::time::Instant::now();
            let one = hv::sift(&rows, &hvs[ri]);
            sift_s += ts.elapsed().as_secs_f64();
            check_bad += one.iter().zip(&scores).filter(|(a, b)| a != b).count();
        }
        let mut cand: Vec<usize> = (0..secs.len()).filter(|&j| { let d = &secs[j]; d.kind != "fn" && (d.b <= r.a || d.a >= r.b) && !(d.l0 >= r0 && d.l1 <= r1)
            && (d.b - d.a) * 3 >= (r.b - r.a) && (d.b - d.a) <= 3 * (r.b - r.a) }).collect();
        cand.sort_by_key(|&j| std::cmp::Reverse(scores[j]));
        cand.truncate(shortlist);
        let mut scored: Vec<(f64, usize, Collage)> = cand.into_iter().map(|j| { let c = collage(&toks, r, &secs[j], &shp); (c.err, j, c) }).collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // one donor per (function, start line): drop nested duplicates of the same donor
        let mut seen = std::collections::HashSet::new(); let mut keep = Vec::new();
        for s in scored { let d = &secs[s.1]; if seen.insert((d.func.clone(), d.l0)) { keep.push(s); } if keep.len() >= top { break; } }
        eprintln!("range L{}-{} [{} · {} · {} tok]", r.l0, r.l1, r.func, r.kind, r.b - r.a);
        for (err, j, c) in &keep {
            let d = &secs[*j];
            let mp: Vec<String> = c.map.iter().filter(|(a, b, _)| a != b).take(8).map(|(a, b, n)| format!("{a}→{b}×{n}")).collect();
            eprintln!("  donor L{}-{} [{} · {} · {} tok] collage err {:.3} (sift {}) map {}", d.l0, d.l1, d.func, d.kind, d.b - d.a, err, scores[*j], mp.join(" "));
            // transplant units (ssFix-style): the outermost donor statements whose tokens mostly do not align with the range,
            // renamed through the map, to be inserted after the range statement that holds the last aligned token before them
            let mut taken: Vec<(usize, usize)> = Vec::new();
            for u in secs.iter().filter(|u| u.kind == "stmt" && u.a >= d.a && u.b <= d.b) {
                if taken.iter().any(|&(a, b)| u.a >= a && u.b <= b) { continue; }
                // a transplant unit must be a whole STATEMENT: ends with `;`, or is a block statement (if/for/while/match …) that is
                // followed by another statement or the block's end AND is not the block's tail value (no `;` needed means it yields ())
                let last = toks[u.b - 1].s.as_str(); let first = toks[u.a].s.as_str();
                let next = toks.get(u.b).map(|t| t.s.as_str()).unwrap_or("");
                let stmt_ok = last == ";" || (last == "}" && matches!(first, "if" | "for" | "while" | "loop" | "match") && next != "}" && next != ")" && next != ",");
                if !stmt_ok { continue; }
                let un = (u.a..u.b).filter(|&k| !c.dmatch[k - d.a]).count();
                if (un as f64) < 0.6 * (u.b - u.a) as f64 { continue; }
                let Some(x) = (if u.a > d.a { c.anchor[u.a - d.a - 1] } else { None }) else { continue };
                let Some(host) = secs.iter().filter(|h| h.kind == "stmt" && h.a <= x && x < h.b && h.a >= r.a && h.b <= r.b).min_by_key(|h| h.b - h.a) else { continue };
                taken.push((u.a, u.b));
                let text = rename(&src[toks[u.a].p..toks[u.b - 1].e], &c.map);
                // scope check (ssFix maps only to variables available at the fix point): every free local name of the renamed unit
                // must appear in the host function before the insertion point; units that fail are dropped before any build
                let fn_start = secs.iter().filter(|f| f.kind == "fn" && f.a <= host.a && host.b <= f.b).map(|f| f.a).max().unwrap_or(r.a);
                let in_scope = declared_in_scope(&toks, fn_start, host.b);
                let ut = lex(&text);
                let free: Vec<String> = (0..ut.len()).filter(|&i| ut[i].k == 0 && ut[i].s.chars().next().is_some_and(|c| c.is_lowercase() || c == '_')
                    && !(i > 0 && (ut[i - 1].s == "." || ut[i - 1].s == "::")) && !ut.get(i + 1).is_some_and(|nx| nx.s == "(" || nx.s == "!" || nx.s == "::")
                    && !(i > 0 && (ut[i - 1].s == "let" || ut[i - 1].s == "mut" || ut[i - 1].s == "for" || ut[i - 1].s == "|"))).map(|i| ut[i].s.clone()).collect();
                let bound: std::collections::HashSet<String> = (0..ut.len()).filter(|&i| i > 0 && matches!(ut[i - 1].s.as_str(), "let" | "mut" | "for" | "|" | ",") && ut[i].k == 0).map(|i| ut[i].s.clone()).collect();
                let missing: Vec<&String> = free.iter().filter(|w| !in_scope.contains(w.as_str()) && !bound.contains(*w)).collect();
                if !missing.is_empty() { scope_drop += 1; continue; }
                let class = edit_class(&text);
                let prior = priors.get(class).copied().unwrap_or(0.05);
                units.push(serde_json::json!({"id": format!("t{:08x}", { let mut h = 0xcbf29ce484222325u64; for b in format!("{}:{}:{}", d.l0, u.l0, host.l1).bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } h as u32 }),
                    "range": [r.l0, r.l1], "donor": [d.l0, d.l1], "unit": [u.l0, u.l1], "insert_after": host.l1, "class": class, "prior": prior,
                    "score": (1.0 - err) * prior, "err": err, "text": text}));
            }
            out_rows.push(serde_json::json!({"range": [r.l0, r.l1], "range_fn": r.func, "donor": [d.l0, d.l1], "donor_fn": d.func, "err": err,
                "range_bytes": [toks[r.a].p, toks[r.b - 1].e], "donor_bytes": [toks[d.a].p, toks[d.b - 1].e], "src_sha": format!("{:016x}", { let mut h = 0xcbf29ce484222325u64; for b in src.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } h }),
                "sift": scores[*j], "map": c.map.iter().map(|(a, b, n)| serde_json::json!([a, b, n])).collect::<Vec<_>>(),
                "donor_lacking_lines": c.donor_unmatched_lines.iter().map(|(l, f)| serde_json::json!([l, f])).collect::<Vec<_>>()}));
        }
    }
    if let Some(p) = out_path { let s: String = out_rows.iter().map(|r| format!("{r}\n")).collect(); std::fs::write(&p, s).unwrap(); eprintln!("→ {p}"); }
    // one unit per (insertion point, text): best score first
    units.sort_by(|a, b| b["score"].as_f64().partial_cmp(&a["score"].as_f64()).unwrap());
    let mut seen_u = std::collections::HashSet::new(); units.retain(|u| seen_u.insert((u["insert_after"].as_u64(), u["text"].as_str().map(|t| t.to_string()))));
    let mut pr: Vec<_> = priors.iter().collect(); pr.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    eprintln!("transplant units: {} ({scope_drop} dropped by the scope check) · class priors {}", units.len(), pr.iter().map(|(k, v)| format!("{k} {v:.3}")).collect::<Vec<_>>().join(" · "));
    for u in units.iter().take(6) { eprintln!("  {} {:8} score {:.3} insert after L{} ← donor L{:?}: {}", u["id"].as_str().unwrap(), u["class"].as_str().unwrap(), u["score"].as_f64().unwrap(), u["insert_after"], u["unit"], u["text"].as_str().unwrap().replace('\n', " ").chars().take(90).collect::<String>()); }
    if let Some(p) = arg(args, "--units-out") { let s: String = units.iter().map(|r| format!("{r}\n")).collect(); std::fs::write(&p, s).unwrap(); eprintln!("units → {p}"); }
    eprintln!("doctor donors: {} ranges, {:.2}s · batch sift {batch_ms:.2} ms ({} queries × {} rows){}", ranges.len(), t0.elapsed().as_secs_f64(), ranges.len(), secs.len(),
        if args.iter().any(|a| a == "--sift-check") { format!(" · one-query sift {:.2} ms · mismatches {check_bad} of {}", sift_s * 1e3, ranges.len() * secs.len()) } else { String::new() });
}
