// SPDX-License-Identifier: MIT OR Apache-2.0
//! PARSE-DOCK P1/P2 (author 10-07, ~/Downloads/Parse.txt: the parse stage as a transform tier that hands the renderer a compact
//! feed; plan an unpublished plan § PARSE-DOCK).
//! A finished function node (params + body) leaves the parser's mutable serde tree for a content-addressed, hash-consed
//! tree: every subtree is stored once (a minified bundle repeats 67 % of its nodes, measured on YouTube's kevlar_base) and
//! the function is lowered only when first called (4.1 % of the functions on that page ever run). The parent keeps a small
//! placeholder: the function header, its flags, and `$t` = the address of the stored node.
//! Reference: hash-consing (Ershov 1958; Filliâtre & Conchon 2006), V8 lazy function compilation.
use serde_json::{Map, Value as J};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// FxHash (rustc's): the interner hashes millions of short strings; SipHash is the wrong tool for a private table.
#[derive(Default)]
pub struct Fx(u64);
impl Hasher for Fx {
    fn write(&mut self, b: &[u8]) { for c in b.chunks(8) { let mut w = [0u8; 8]; w[..c.len()].copy_from_slice(c); self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(0x517c_c1b7_2722_0a95); } }
    fn write_u64(&mut self, x: u64) { self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95); }
    fn write_u8(&mut self, x: u8) { self.write_u64(x as u64) }
    fn write_usize(&mut self, x: usize) { self.write_u64(x as u64) }
    fn finish(&self) -> u64 { self.0 }
}
type FxB = BuildHasherDefault<Fx>;
use std::rc::Rc;

/// An immutable, interned tree node. Strings and objects are unique per content, so equal subtrees are one allocation.
#[derive(Clone)]
pub enum Cn { Null, Bool(bool), Num(f64), Str(Rc<str>), Arr(Rc<Vec<Cn>>), Obj(Rc<On>) }
pub struct On { kv: Vec<(Rc<str>, Cn)> }

impl Cn {
    /// identity of this node (children are interned, so pointer equality is structural equality)
    fn id(&self) -> u64 {
        match self {
            Cn::Null => 1, Cn::Bool(b) => 2 + *b as u64, Cn::Num(f) => f.to_bits().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 4,
            Cn::Str(s) => Rc::as_ptr(s) as *const u8 as u64 ^ 5, Cn::Arr(a) => Rc::as_ptr(a) as u64 ^ 6, Cn::Obj(o) => Rc::as_ptr(o) as u64 ^ 7,
        }
    }
}
fn same(a: &Cn, b: &Cn) -> bool { a.id() == b.id() && std::mem::discriminant(a) == std::mem::discriminant(b) }
fn mix(h: u64, x: u64) -> u64 { (h ^ x).wrapping_mul(0x0000_0100_0000_01B3).rotate_left(17) }

#[derive(Default)]
struct Interner { strs: HashMap<Rc<str>, (), FxB>, arrs: HashMap<u64, Vec<Rc<Vec<Cn>>>, FxB>, objs: HashMap<u64, Vec<Rc<On>>, FxB> }

thread_local! {
    pub static TIMES: Cell<[f64; 3]> = const { Cell::new([0.0; 3]) };
    static COMPACT: Cell<bool> = const { Cell::new(false) };
    static IN: RefCell<Interner> = RefCell::new(Interner::default());
    /// stored functions; the second field is the base their nested placeholders' addresses are relative to (0 = absolute)
    static TABLE: RefCell<Vec<(Cn, u64)>> = const { RefCell::new(Vec::new()) };
    static BY_NODE: RefCell<HashMap<u64, usize, FxB>> = RefCell::new(HashMap::default());
}

/// The process is about to exit: keep the parse store instead of freeing it node by node (thread-local destructors spent
/// a visible share of a YouTube renderer's life on that teardown, JS-COMPILE-0 page profile). Only for exiting processes —
/// a long-lived thread that called this would keep its store for good.
pub fn forget_store() {
    IN.with(|i| std::mem::forget(std::mem::take(&mut *i.borrow_mut())));
    TABLE.with(|t| std::mem::forget(std::mem::take(&mut *t.borrow_mut())));
    BY_NODE.with(|b| std::mem::forget(std::mem::take(&mut *b.borrow_mut())));
}

/// Parse with function compaction on (the page / runtime path) or off (the oracle parity path, which compares the whole tree).
pub fn set_compact(on: bool) -> bool { COMPACT.with(|c| c.replace(on)) }
pub fn compacting() -> bool { COMPACT.with(|c| c.get()) }

fn istr(s: &str) -> Rc<str> {
    IN.with(|i| { let mut i = i.borrow_mut(); if let Some((k, _)) = i.strs.get_key_value(s) { return k.clone(); } let r: Rc<str> = Rc::from(s); i.strs.insert(r.clone(), ()); r })
}

pub fn compact(v: &J) -> Cn {
    match v {
        J::Null => Cn::Null,
        J::Bool(b) => Cn::Bool(*b),
        J::Number(n) => Cn::Num(n.as_f64().unwrap_or(0.0)),
        J::String(s) => Cn::Str(istr(s)),
        J::Array(a) => mk_arr(a.iter().map(compact).collect()),
        J::Object(o) => mk_obj(o.iter().map(|(k, x)| (istr(k), compact(x))).collect()),
    }
}

fn mk_arr(items: Vec<Cn>) -> Cn {
    let h = items.iter().fold(11u64, |h, c| mix(h, c.id()));
    IN.with(|i| { let mut i = i.borrow_mut(); let b = i.arrs.entry(h).or_default();
        if let Some(r) = b.iter().find(|r| r.len() == items.len() && r.iter().zip(&items).all(|(x, y)| same(x, y))) { return Cn::Arr(r.clone()); }
        let r = Rc::new(items); b.push(r.clone()); Cn::Arr(r) })
}
fn mk_obj(kv: Vec<(Rc<str>, Cn)>) -> Cn {
    let h = kv.iter().fold(13u64, |h, (k, c)| mix(mix(h, Rc::as_ptr(k) as *const u8 as u64), c.id()));
    IN.with(|i| { let mut i = i.borrow_mut(); let b = i.objs.entry(h).or_default();
        if let Some(r) = b.iter().find(|r| r.kv.len() == kv.len() && r.kv.iter().zip(&kv).all(|(x, y)| Rc::ptr_eq(&x.0, &y.0) && same(&x.1, &y.1))) { return Cn::Obj(r.clone()); }
        let r = Rc::new(On { kv }); b.push(r.clone()); Cn::Obj(r) })
}

fn num_j(f: f64) -> J {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 && !(f == 0.0 && f.is_sign_negative()) {
        if f >= 0.0 { J::from(f as u64) } else { J::from(f as i64) }
    } else { serde_json::Number::from_f64(f).map(J::Number).unwrap_or(J::Null) }
}

pub fn to_j(c: &Cn) -> J {
    match c {
        Cn::Null => J::Null, Cn::Bool(b) => J::Bool(*b), Cn::Num(f) => num_j(*f), Cn::Str(s) => J::String(s.to_string()),
        Cn::Arr(a) => J::Array(a.iter().map(to_j).collect()),
        Cn::Obj(o) => { let mut m = Map::with_capacity(o.kv.len()); for (k, x) in &o.kv { m.insert(k.to_string(), to_j(x)); } J::Object(m) }
    }
}

/// Store a node and return its address (the same interned node always gets the same address).
fn stash(c: Cn) -> usize {
    let key = c.id();
    if let Some(i) = BY_NODE.with(|b| b.borrow().get(&key).copied()) { return i; }
    let i = TABLE.with(|t| { let mut t = t.borrow_mut(); t.push((c, 0)); t.len() - 1 });
    BY_NODE.with(|b| b.borrow_mut().insert(key, i)); i
}
pub fn fetch(i: usize) -> J { TABLE.with(|t| { let t = t.borrow(); let (c, base) = &t[i]; let mut j = to_j(c); if *base != 0 { bump(&mut j, *base); } j }) }
/// (functions stored, distinct interned objects) — diagnostics
pub fn stats() -> (usize, usize) { (TABLE.with(|t| t.borrow().len()), IN.with(|i| i.borrow().objs.values().map(|v| v.len()).sum())) }

/// A function node just finished by the parser → its placeholder (when compaction is on).
pub fn lazy_fn(n: J) -> J {
    if !compacting() { return n; }
    // own = the body; all = the whole node (what an enclosing function's scan used to see): own + params + id + the node's own type
    let t0 = std::time::Instant::now();
    let own = crate::jsi_ast::body_flags(&n["body"]);
    let (pf, idf) = (crate::jsi_ast::body_flags(&n["params"]), crate::jsi_ast::body_flags(&n["id"]));
    let all = (own.0 | pf.0 | idf.0, own.1 | pf.1 | idf.1 | (n["type"] == "ArrowFunctionExpression"), own.2 | pf.2 | idf.2, own.3 | pf.3 | idf.3);
    let own_strict = n["body"]["type"] == "BlockStatement" && crate::jsi_ast::has_use_strict(&n["body"]["body"]);
    let (mut np_all, mut np_len, mut seen_default) = (0u32, 0u32, false);
    if let Some(ps) = n["params"].as_array() {
        for p in ps {
            if p["type"] == "RestElement" { continue; }
            np_all += 1;
            if p["type"] == "AssignmentPattern" { seen_default = true; }
            if !seen_default { np_len += 1; }
        }
    }
    let t1 = std::time::Instant::now();
    // one small number per field group (the parent tree holds a placeholder per function — 30 k on YouTube — so it stays thin):
    // m = own flags (bits 0-3: arguments this super meta) | whole-node flags (4-7) | own "use strict" (8) | async (9) | generator (10)
    let bit = |x: bool, i: u32| (x as u64) << i;
    let m_bits = bit(own.0, 0) | bit(own.1, 1) | bit(own.2, 2) | bit(own.3, 3) | bit(all.0, 4) | bit(all.1, 5) | bit(all.2, 6) | bit(all.3, 7)
        | bit(own_strict, 8) | bit(n["async"].as_bool().unwrap_or(false), 9) | bit(n["generator"].as_bool().unwrap_or(false), 10);
    let mut m = Map::new();
    for k in ["type", "id"] { m.insert(k.to_string(), n[k].clone()); }
    let cn = compact(&n); let t2 = std::time::Instant::now();
    m.insert("$t".into(), J::from(stash(cn) as u64));
    let t3 = std::time::Instant::now();
    TIMES.with(|t| { let mut a = t.get(); a[0] += (t1 - t0).as_secs_f64(); a[1] += (t2 - t1).as_secs_f64(); a[2] += (t3 - t2).as_secs_f64(); t.set(a); });
    m.insert("m".into(), J::from(m_bits));
    m.insert("n".into(), J::from(np_all as u64 | (np_len as u64) << 16));
    J::Object(m)
}

// ---- the feed: a parsed script as bytes (the broker parses and caches; the renderer loads) ------------------------------

/// Start a self-contained parse: the function table holds only this script's functions (addresses 0..n).
pub fn begin() { TABLE.with(|t| t.borrow_mut().clear()); BY_NODE.with(|b| b.borrow_mut().clear()); }

fn put_var(out: &mut Vec<u8>, mut x: u64) { while x >= 0x80 { out.push((x as u8) | 0x80); x >>= 7; } out.push(x as u8); }
fn get_var(b: &[u8], p: &mut usize) -> Option<u64> { let (mut x, mut sh) = (0u64, 0); loop { let c = *b.get(*p)?; *p += 1; x |= ((c & 0x7f) as u64) << sh; if c & 0x80 == 0 { return Some(x); } sh += 7; if sh > 63 { return None; } } }

struct Writer { out: Vec<u8>, memo: HashMap<(u8, u64), u32, FxB>, n: u32 }
impl Writer {
    fn node(&mut self, c: &Cn) -> u32 {
        let key = (match c { Cn::Null => 0, Cn::Bool(_) => 1, Cn::Num(_) => 2, Cn::Str(_) => 3, Cn::Arr(_) => 4, Cn::Obj(_) => 5 }, c.id());
        if let Some(&i) = self.memo.get(&key) { return i; }
        match c {
            Cn::Null => self.out.push(0),
            Cn::Bool(b) => self.out.push(1 + *b as u8),
            Cn::Num(f) => { self.out.push(3); self.out.extend_from_slice(&f.to_le_bytes()); }
            Cn::Str(s) => { self.out.push(4); put_var(&mut self.out, s.len() as u64); self.out.extend_from_slice(s.as_bytes()); }
            Cn::Arr(a) => { let ids: Vec<u32> = a.iter().map(|x| self.node(x)).collect(); self.out.push(5); put_var(&mut self.out, ids.len() as u64); for i in ids { put_var(&mut self.out, i as u64); } }
            Cn::Obj(o) => {
                let ids: Vec<(u32, u32)> = o.kv.iter().map(|(k, x)| { let kid = self.node(&Cn::Str(k.clone())); (kid, self.node(x)) }).collect();
                self.out.push(6); put_var(&mut self.out, ids.len() as u64); for (k, v) in ids { put_var(&mut self.out, k as u64); put_var(&mut self.out, v as u64); }
            }
        }
        let i = self.n; self.n += 1; self.memo.insert(key, i); i
    }
}

/// The program just parsed (after `begin()`) and every function it stored, as one byte string. Each distinct subtree is
/// written once; ids are post-order, so a reader rebuilds bottom-up.
/// The magic is the format version: bump it whenever dump's layout or the placeholder fields (jsi_ast reads them) change —
/// the broker keeps these on disk across builds.
pub fn dump(prog: &J) -> Vec<u8> {
    let root = compact(prog);
    let funcs: Vec<Cn> = TABLE.with(|t| t.borrow().iter().map(|(c, _)| c.clone()).collect());
    let mut w = Writer { out: Vec::new(), memo: HashMap::default(), n: 0 };
    let r = w.node(&root);
    let fids: Vec<u32> = funcs.iter().map(|f| w.node(f)).collect();
    let mut out = b"OMJ2".to_vec();
    put_var(&mut out, w.n as u64); out.extend_from_slice(&w.out);
    put_var(&mut out, r as u64); put_var(&mut out, fids.len() as u64); for f in fids { put_var(&mut out, f as u64); }
    out
}

fn bump(j: &mut J, base: u64) {
    match j {
        J::Object(o) => { if let Some(t) = o.get_mut("$t") { if let Some(x) = t.as_u64() { *t = J::from(x + base); } } for v in o.values_mut() { bump(v, base); } }
        J::Array(a) => for v in a { bump(v, base); },
        _ => {}
    }
}

/// Inverse of `dump`: rebuild the nodes through this thread's interner (equal subtrees collapse with anything already
/// loaded), append the script's functions to the table, and return the program with its placeholders re-addressed.
pub fn load(b: &[u8]) -> Option<J> {
    if b.get(..4)? != b"OMJ2" { return None; }
    let mut p = 4usize;
    let n = get_var(b, &mut p)? as usize;
    let mut nodes: Vec<Cn> = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = *b.get(p)?; p += 1;
        let c = match tag {
            0 => Cn::Null, 1 => Cn::Bool(false), 2 => Cn::Bool(true),
            3 => { let f = f64::from_le_bytes(b.get(p..p + 8)?.try_into().ok()?); p += 8; Cn::Num(f) }
            4 => { let l = get_var(b, &mut p)? as usize; let s = std::str::from_utf8(b.get(p..p + l)?).ok()?; p += l; Cn::Str(istr(s)) }
            5 => { let l = get_var(b, &mut p)? as usize; let mut v = Vec::with_capacity(l); for _ in 0..l { v.push(nodes.get(get_var(b, &mut p)? as usize)?.clone()); } mk_arr(v) }
            6 => { let l = get_var(b, &mut p)? as usize; let mut v = Vec::with_capacity(l);
                for _ in 0..l { let k = match nodes.get(get_var(b, &mut p)? as usize)? { Cn::Str(s) => s.clone(), _ => return None }; v.push((k, nodes.get(get_var(b, &mut p)? as usize)?.clone())); } mk_obj(v) }
            _ => return None,
        };
        nodes.push(c);
    }
    let root = nodes.get(get_var(b, &mut p)? as usize)?.clone();
    let nf = get_var(b, &mut p)? as usize;
    let base = TABLE.with(|t| t.borrow().len()) as u64;
    for _ in 0..nf { let f = nodes.get(get_var(b, &mut p)? as usize)?.clone(); TABLE.with(|t| t.borrow_mut().push((f, base))); }
    let mut j = to_j(&root);
    bump(&mut j, base);
    Some(j)
}

/// The renderer's source of parsed scripts (set by pageproc): url, module → program.
pub static FEED: std::sync::OnceLock<fn(&str, bool) -> Option<J>> = std::sync::OnceLock::new();
pub fn feed(url: &str, module: bool) -> Option<J> { (FEED.get()?)(url, module) }

/// Every placeholder replaced by the function node it stands for (recursively): the tree the parser would have built whole.
pub fn expand(j: &J) -> J {
    match j {
        J::Object(o) => {
            if let Some(t) = o.get("$t").and_then(|t| t.as_u64()) { return expand(&fetch(t as usize)); }
            J::Object(o.iter().map(|(k, v)| (k.clone(), expand(v))).collect())
        }
        J::Array(a) => J::Array(a.iter().map(expand).collect()),
        v => v.clone(),
    }
}
