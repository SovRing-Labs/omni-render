// SPDX-License-Identifier: MIT OR Apache-2.0
//! jsi — JavaScript semantics on the Omniglyph IR (author 10-04: "could we also just add js to the omniglyph?").
//! Programs arrive as estree IR (tools/js2ir.mjs at build time; svelte2ir for components; jsparse.rs at run time) and run
//! here: heap objects with reference semantics and real prototype chains (ECMA-262 §10.1 ordinary objects: [[Prototype]],
//! accessor properties, attributes, extensibility), lexical scopes + closures, return/break/continue/throw as control
//! values (labelled), classes (§15.7: super, fields, #private, static blocks), symbols, iterators, generators and async
//! functions on a resumable evaluator (§27.5 / §27.7), promises with a real microtask queue (§9.5 jobs; HTML §8.1.7.3
//! checkpoint after each script and task), and the standard library in jsi_lib.rs. Correctness is not asserted — it is
//! cemented: the interpreter is the `omniir` language in Omniglyph (bin/omniglyph), running every law's `js` form on the
//! same inputs node runs; a law holds for omniir only when both agree on every case.
use serde_json::Value as J;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use crate::jsi_ast as A;
use crate::jsi_ast::{Body, Expr, Pat, Stmt};

pub type Native = Rc<dyn Fn(&mut Interp, Val, Vec<Val>) -> Result<Val, Ctl>>;
pub type ORef = Rc<RefCell<Obj>>;

#[derive(Clone)]
pub enum Val { Undef, Null, Bool(bool), Num(f64), Str(Rc<str>), Obj(ORef), Sym(Rc<Sym>), Big(Rc<crate::jsi_lib::Big>) }

/// ECMA-262 §6.1.5 Symbol: a unique property key. `key` is its internal property-name spelling ("\0…": no source string
/// can produce it, and string-keyed enumeration skips it).
pub struct Sym { pub key: Rc<str>, pub desc: Option<Rc<str>> }

pub struct Obj { pub props: Vec<(Rc<str>, Val)>, pub kind: Kind, pub proto: Option<ORef>, pub flags: u8, pub attrs: Vec<(Rc<str>, u8)> }

// object flags
pub const F_NULLPROTO: u8 = 1; pub const F_NOEXT: u8 = 2; pub const F_SEALED: u8 = 4; pub const F_FROZEN: u8 = 8;
/// holds an accessor property (props values of Kind::Accessor are getter/setter pairs)
pub const F_ACC: u8 = 16;
/// holds a non-writable property (see attrs)
pub const F_RO: u8 = 32;
pub const F_CLASS: u8 = 64;
/// an array made by Array(n) / concat of one: its `undefined` elements read as holes in map/forEach/filter/some/every
pub const F_HOLES: u8 = 128;
// property attributes (sparse: only non-default ones are listed in Obj::attrs)
pub const A_NOENUM: u8 = 1; pub const A_NOWRITE: u8 = 2; pub const A_NOCONF: u8 = 4;
// well-known symbol keys (ECMA-262 §6.1.5.1)
pub const K_ITER: &str = "\u{0}@@iterator"; pub const K_ASYNC_ITER: &str = "\u{0}@@asyncIterator"; pub const K_HAS_INSTANCE: &str = "\u{0}@@hasInstance";
pub const K_TO_PRIM: &str = "\u{0}@@toPrimitive"; pub const K_TAG: &str = "\u{0}@@toStringTag"; pub const K_SPECIES: &str = "\u{0}@@species";
pub const WELL_KNOWN: &[&str] = &["iterator", "asyncIterator", "hasInstance", "toPrimitive", "toStringTag", "species", "isConcatSpreadable", "unscopables", "match", "matchAll", "replace", "search", "split"];

pub enum Kind { Plain, Array(Vec<Val>), Func(Rc<Closure>), Native(Native, &'static str), Error, Map(Box<MapData>), Set(Box<MapData>), Promise(Box<PState>), RegExp(String, String),
    /// a platform object of the page (jsdom.rs): (live node id, role) — element, style, classList, dataset, window, location, storage
    Host(u32, u8),
    /// an accessor property's (getter, setter), stored as the property's value
    Accessor(Val, Val),
    /// Number / String / Boolean / Symbol / BigInt wrapper object (ECMA-262 §7.1.18 ToObject)
    Prim(Val),
    Date(f64),
    Iter(Box<IterSt>),
    Gen(Rc<RefCell<Coro>>),
    Proxy(Val, Val),
    WeakMap(HashMap<usize, (Weak<RefCell<Obj>>, Val)>), WeakSet(HashMap<usize, Weak<RefCell<Obj>>>), WeakRef(Weak<RefCell<Obj>>),
    Buffer(Rc<RefCell<Vec<u8>>>), Typed(Box<TArr>), View(Box<TArr>),
    /// the global object: its properties are the global scope's bindings
    Global }

/// Map / Set storage: insertion-ordered entries with tombstones (iterators keep their position across deletes, ECMA-262
/// §24.1.3.5) and a SameValueZero hash index.
pub struct MapData { pub ents: Vec<Option<(Val, Val)>>, pub idx: HashMap<HKey, usize>, pub live: usize }
#[derive(Hash, PartialEq, Eq, Clone)]
pub enum HKey { U, Nl, B(bool), N(u64), S(Rc<str>), P(usize), Bg(String) }
impl MapData {
    pub fn new() -> MapData { MapData { ents: Vec::new(), idx: HashMap::new(), live: 0 } }
    pub fn hkey(k: &Val) -> HKey {
        match k { Val::Undef => HKey::U, Val::Null => HKey::Nl, Val::Bool(b) => HKey::B(*b), Val::Num(n) => HKey::N(if *n == 0.0 { 0 } else if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() }),
            Val::Str(s) => HKey::S(s.clone()), Val::Obj(o) => HKey::P(Rc::as_ptr(o) as *const u8 as usize), Val::Sym(s) => HKey::P(Rc::as_ptr(s) as *const u8 as usize), Val::Big(b) => HKey::Bg(b.to_string()) }
    }
    pub fn get(&self, k: &Val) -> Option<&Val> { self.idx.get(&Self::hkey(k)).and_then(|&i| self.ents[i].as_ref()).map(|e| &e.1) }
    pub fn has(&self, k: &Val) -> bool { self.idx.contains_key(&Self::hkey(k)) }
    pub fn set(&mut self, k: Val, v: Val) {
        let k = match k { Val::Num(n) if n == 0.0 => Val::Num(0.0), k => k };
        let h = Self::hkey(&k);
        if let Some(&i) = self.idx.get(&h) { if let Some(e) = &mut self.ents[i] { e.1 = v; } return; }
        self.idx.insert(h, self.ents.len()); self.ents.push(Some((k, v))); self.live += 1;
    }
    pub fn delete(&mut self, k: &Val) -> bool { match self.idx.remove(&Self::hkey(k)) { Some(i) => { self.ents[i] = None; self.live -= 1; true } None => false } }
    pub fn clear(&mut self) { for e in self.ents.iter_mut() { *e = None; } self.idx.clear(); self.live = 0; }
    pub fn entries(&self) -> impl Iterator<Item = &(Val, Val)> { self.ents.iter().flatten() }
}

/// Promise state (ECMA-262 §27.2.6): 0 pending, 1 fulfilled, 2 rejected; reactions = (onFulfilled, onRejected, derived promise).
pub struct PState { pub st: u8, pub v: Val, pub reacts: Vec<(Val, Val, Option<Val>)>, pub handled: bool }

/// Built-in iterator state (Array / Map / Set / String iterators, §23.1.5, §24.1.5, §24.2.5, §22.1.5): mode 0 keys, 1 values, 2 entries.
pub enum IterSt { Arr(Val, usize, u8), Map(Val, usize, u8), Str(Rc<str>, usize), List(Vec<Val>, usize), Done }

/// Typed array / DataView view over an ArrayBuffer (§25.1, §23.2): ty = element type index, off in bytes, len in elements (bytes for a view).
pub struct TArr { pub buf: Rc<RefCell<Vec<u8>>>, pub bufobj: Val, pub ty: u8, pub off: usize, pub len: usize }

/// A generator / async function activation that can suspend (ECMA-262 §27.5.3 GeneratorStart … GeneratorResume).
/// kind: 0 generator, 1 async function, 2 async generator (its pending next/throw/return requests queue up, §27.6.3).
pub struct Coro { pub clo: Rc<Closure>, pub scope: Scope, pub frames: Vec<Slot>, pub state: u8, pub kind: u8, pub promise: Option<Val>,
    pub queue: VecDeque<(Resume, Val)>, pub busy: bool, pub awaiting: bool }
pub const CO_START: u8 = 0; pub const CO_SUSP: u8 = 1; pub const CO_RUN: u8 = 2; pub const CO_DONE: u8 = 3;

/// `module`: the module whose code made this function (HTML §8.1.5.5 "active script": a later import() inside it resolves
/// against that module's URL, not the page's)
pub struct Closure { pub func: Rc<crate::jsi_ast::Func>, pub scope: Scope, pub this: Option<Val>, pub name: String, pub home: Option<Val>, pub cls: Option<Rc<ClassRt>>, pub module: Option<Rc<str>> }
/// Per-class run-time data a constructor needs: instance fields (key, member index) and whether it derives.
pub struct ClassRt { pub def: Rc<A::ClassDef>, pub scope: Scope, pub derived: bool, pub fields: Vec<(Rc<str>, usize)>, pub home: Val }

pub type Scope = Rc<RefCell<Frame>>;
/// Scope maps hash identifier names from the program itself (not attacker-chosen keys): a multiply-rotate hash
/// (FxHash family) instead of SipHash cuts every variable lookup several-fold.
#[derive(Default, Clone, Copy)]
pub struct Fx(u64);
impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, b: &[u8]) { for c in b.chunks(8) { let mut w = [0u8; 8]; w[..c.len()].copy_from_slice(c); self.0 = (self.0.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95); } }
    fn write_u8(&mut self, i: u8) { self.0 = (self.0.rotate_left(5) ^ i as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95); }
    fn write_usize(&mut self, i: usize) { self.0 = (self.0.rotate_left(5) ^ i as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95); }
}
pub type FxMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;

/// `with`: an object environment record (§9.1.1.2) — names resolve on the object first.
/// `fixed`: the bindings are immutable and assignments to them are ignored (a named function expression's own name, §15.2.5).
/// `slots` / `lay`: an activation frame's bindings by slot (JS-COMPILE-0 C1, layout from jsi_ast `FuncBody.layout`); `None`
/// = not bound yet (a lexical name before its declaration runs: lookups continue outward, as for an absent map key).
pub struct Frame { pub vars: FxMap<Rc<str>, Val>, pub slots: Vec<Option<Val>>, pub lay: Option<Rc<A::Layout>>, pub parent: Option<Scope>, pub with: Option<Val>, pub fixed: bool }
impl Frame {
    pub fn map(parent: Option<Scope>) -> Frame { Frame { vars: FxMap::default(), slots: Vec::new(), lay: None, parent, with: None, fixed: false } }
    #[inline] fn slot_of(&self, name: &str) -> Option<usize> { match &self.lay { Some(l) => l.find(name), None => None } }
    /// the binding `name` holds in this frame (slot or map)
    #[inline] pub fn get(&self, name: &str) -> Option<&Val> {
        if let Some(i) = self.slot_of(name) { return self.slots[i].as_ref(); }
        self.vars.get(name)
    }
    #[inline] pub fn get_mut(&mut self, name: &str) -> Option<&mut Val> {
        if let Some(i) = self.slot_of(name) { return self.slots[i].as_mut(); }
        self.vars.get_mut(name)
    }
    #[inline] pub fn has(&self, name: &str) -> bool { self.get(name).is_some() }
    /// bind `name` in this frame (CreateMutableBinding + InitializeBinding)
    #[inline] pub fn put(&mut self, name: &Rc<str>, v: Val) {
        if let Some(i) = self.slot_of(name) { self.slots[i] = Some(v); return; }
        if self.lay.is_some() && slotcheck_on() { eprintln!("SLOTCHECK overflow: '{name}' bound in a slotted frame outside its layout {:?}", self.lay.as_ref().map(|l| l.names.iter().take(12).map(|n| n.to_string()).collect::<Vec<_>>())); if blame_on() { BS.with(|b| eprintln!("  in [{}]", b.borrow().iter().rev().take(8).cloned().collect::<Vec<_>>().join(" < "))); } }
        self.vars.insert(name.clone(), v);
    }
    /// a per-iteration copy (§14.7.4.4 CreatePerIterationEnvironment)
    pub(crate) fn copy_bindings(&mut self, o: &Frame) { self.vars = o.vars.clone(); self.slots = o.slots.clone(); self.lay = o.lay.clone(); }
}
/// OMR_SLOTCHECK=1: every resolved slot access is cross-checked against the by-name walk; a mismatch panics.
pub fn slotcheck_on() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var_os("OMR_SLOTCHECK").is_some()) }

/// Completion records (ECMA-262 §6.2.4) as control values; Suspend = a generator/async body paused at yield/await
/// (the value travels in the coroutine context, the resume path in the saved slots).
#[derive(Clone)]
pub enum Ctl { Return(Val), Break(Option<Rc<str>>), Continue(Option<Rc<str>>), Throw(Val), Suspend }

/// How a suspended body is resumed (§27.5.3.3 GeneratorResume / §27.5.3.4 GeneratorResumeAbrupt).
#[derive(Clone)]
pub enum Resume { Next(Val), Throw(Val), Return(Val) }

/// One construct's saved state on the path to a suspension point: stage, partial values, scopes, an open iterator, a
/// pending completion (try/finally).
#[derive(Default)]
pub struct Slot { st: u32, n: usize, vals: Vec<Val>, sc: Option<Scope>, sc2: Option<Scope>, it: Option<Box<IterRec>>, ctl: Option<Box<Ctl>> }
impl Slot { fn st(st: u32) -> Slot { Slot { st, ..Default::default() } } }

struct CoCtx { resume: Vec<Slot>, rpos: usize, saving: Vec<Slot>, sent: Option<Resume>, out: Val, awaiting: bool }

/// An open iteration (§7.4 Iterator Records), with fast paths for arrays, our generators and built-in iterators.
pub enum IterRec { Arr(ORef, usize), List(Vec<Val>, usize), Gen(Rc<RefCell<Coro>>), Bi(ORef), Proto(Val, Val), Async(Val, Val) }

/// Microtasks (ECMA-262 §9.5 HostEnqueuePromiseJob).
pub enum Job { React { handler: Val, arg: Val, derived: Option<Val>, ok: bool }, Thenable { p: Val, obj: Val, then: Val }, Call(Val, Vec<Val>) }

fn collect_names(p: &J, out: &mut Vec<String>) {
    match s(p, "type") {
        "Identifier" => out.push(s(p, "name").to_string()),
        "ObjectPattern" => for pr in arr(&p["properties"]) { if pr["type"] == "RestElement" { collect_names(&pr["argument"], out) } else { collect_names(&pr["value"], out) } },
        "ArrayPattern" => for e in arr(&p["elements"]) { if !e.is_null() { collect_names(e, out) } },
        "AssignmentPattern" => collect_names(&p["left"], out),
        "RestElement" => collect_names(&p["argument"], out),
        _ => {}
    }
}
#[allow(dead_code)] fn _keep_collect(p: &J) -> Vec<String> { let mut v = Vec::new(); collect_names(p, &mut v); v }

fn s<'j>(n: &'j J, k: &str) -> &'j str { n.get(k).and_then(|x| x.as_str()).unwrap_or("") }
fn arr(n: &J) -> &[J] { n.as_array().map(|v| v.as_slice()).unwrap_or(&[]) }

pub fn st(x: &str) -> Val { Val::Str(Rc::from(x)) }

// JS strings are UTF-16 and may hold lone surrogates; ours are UTF-8. A lone surrogate unit u is carried as the private-use
// code point 0xF0000 + u (plane 15), converted back on the way to UTF-16, and re-paired after concatenation.
const LONE: u32 = 0xF0000;
pub fn utf16_of(x: &str) -> Vec<u16> {
    let mut v = Vec::with_capacity(x.len());
    for c in x.chars() { let u = c as u32; if (LONE + 0xD800..=LONE + 0xDFFF).contains(&u) { v.push((u - LONE) as u16); } else { let mut b = [0u16; 2]; v.extend_from_slice(c.encode_utf16(&mut b)); } }
    v
}
pub fn str_of16(v: &[u16]) -> String {
    char::decode_utf16(v.iter().copied()).map(|r| r.unwrap_or_else(|e| char::from_u32(LONE + e.unpaired_surrogate() as u32).unwrap())).collect()
}
thread_local! { static UNITS: RefCell<Vec<(Rc<str>, Option<Rc<Vec<u16>>>)>> = const { RefCell::new(Vec::new()) }; }
/// The UTF-16 code units of a string, or None when it is ASCII (its bytes are its units). Long strings are cached by
/// identity (a small LRU holding the Rc, so a cached address cannot be reused by another string): a charCodeAt loop over a
/// 60 KB source no longer converts the whole source per call.
pub fn units(s: &Rc<str>) -> Option<Rc<Vec<u16>>> {
    if s.len() < 64 { return if s.is_ascii() { None } else { Some(Rc::new(utf16_of(s))) }; }
    UNITS.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(pos) = c.iter().position(|(k, _)| Rc::ptr_eq(k, s)) { if pos > 0 { let e = c.remove(pos); c.insert(0, e); } return c[0].1.clone(); }
        let v = if s.is_ascii() { None } else { Some(Rc::new(utf16_of(s))) };
        c.insert(0, (s.clone(), v.clone())); c.truncate(8);
        v
    })
}
/// UTF-16 length without building the unit vector.
pub fn len16(x: &str) -> usize { if x.is_ascii() { x.len() } else { x.chars().map(|c| { let u = c as u32; if u >= 0x10000 && !(LONE + 0xD800..=LONE + 0xDFFF).contains(&u) { 2 } else { 1 } }).sum() } }
/// `a + b` for strings: only the seam can re-pair a carried high half with a carried low half, so only the seam is
/// inspected (a full fix_pairs scan per concatenation made `s += c` loops quadratic in the scan as well as the copy).
pub fn concat(a: &str, b: &str) -> Rc<str> {
    let hi = a.chars().next_back().map(|c| c as u32).filter(|u| (LONE + 0xD800..LONE + 0xDC00).contains(u));
    let lo = b.chars().next().map(|c| c as u32).filter(|u| (LONE + 0xDC00..=LONE + 0xDFFF).contains(u));
    let mut s = String::with_capacity(a.len() + b.len());
    match (hi, lo) {
        (Some(h), Some(l)) => { let (h, l) = (h - LONE, l - LONE); s.push_str(&a[..a.len() - 4]); s.push(char::from_u32(0x10000 + ((h - 0xD800) << 10) + (l - 0xDC00)).unwrap()); s.push_str(&b[4..]); }
        _ => { s.push_str(a); s.push_str(b); }
    }
    Rc::from(s)
}
/// After joining strings: two carried halves that now sit side by side become one real character again.
pub fn fix_pairs(x: String) -> String { if x.chars().any(|c| (LONE + 0xD800..=LONE + 0xDFFF).contains(&(c as u32))) { str_of16(&utf16_of(&x)) } else { x } }
/// "is this a canonical array index" (ECMA-262 §6.1.7): fast reject on the first byte.
#[inline] pub fn index_of(k: &str) -> Option<usize> { let b = k.as_bytes(); if b.is_empty() || !b[0].is_ascii_digit() || (b[0] == b'0' && b.len() > 1) || b.len() > 15 { return None; } k.parse::<usize>().ok() }

/// The `this` a native constructor sees when called through `new` (natives decide object-vs-function behaviour by it).
pub fn new_marker() -> Val { Val::Str(Rc::from("\u{1}new")) }
pub fn is_new(v: &Val) -> bool { matches!(v, Val::Str(s) if &**s == "\u{1}new") }

impl Val {
    pub fn obj(kind: Kind) -> Val { Val::Obj(Rc::new(RefCell::new(Obj { props: Vec::new(), kind, proto: None, flags: 0, attrs: Vec::new() }))) }
    pub fn obj_p(kind: Kind, proto: Option<ORef>) -> Val { let v = Val::obj(kind); if let Val::Obj(o) = &v { match proto { Some(p) => o.borrow_mut().proto = Some(p), None => o.borrow_mut().flags |= F_NULLPROTO } } v }
    pub fn array(v: Vec<Val>) -> Val { Val::obj(Kind::Array(v)) }
    pub fn plain(kv: Vec<(&str, Val)>) -> Val { let o = Val::obj(Kind::Plain); if let Val::Obj(r) = &o { r.borrow_mut().props = kv.into_iter().map(|(k, v)| (Rc::from(k), v)).collect(); } o }
    pub fn native(name: &'static str, f: impl Fn(&mut Interp, Val, Vec<Val>) -> Result<Val, Ctl> + 'static) -> Val { Val::obj(Kind::Native(Rc::new(f), name)) }
    pub fn truthy(&self) -> bool { match self { Val::Undef | Val::Null => false, Val::Bool(b) => *b, Val::Num(n) => *n != 0.0 && !n.is_nan(), Val::Str(x) => !x.is_empty(), Val::Obj(_) | Val::Sym(_) => true, Val::Big(b) => !b.is_zero() } }
    pub fn is_callable(&self) -> bool { matches!(self, Val::Obj(o) if { let b = o.borrow(); match &b.kind { Kind::Func(_) | Kind::Native(..) => true, Kind::Proxy(t, _) => t.is_callable(), _ => false } }) }
    pub fn is_nullish(&self) -> bool { matches!(self, Val::Undef | Val::Null) }
    pub fn type_of(&self) -> &'static str {
        match self { Val::Undef => "undefined", Val::Null => "object", Val::Bool(_) => "boolean", Val::Num(_) => "number", Val::Str(_) => "string", Val::Sym(_) => "symbol", Val::Big(_) => "bigint",
            Val::Obj(_) => if self.is_callable() { "function" } else { "object" } }
    }
    pub fn as_obj(&self) -> Option<&ORef> { if let Val::Obj(o) = self { Some(o) } else { None } }
}

/// JS Number::toString(10).
pub fn num_str(n: f64) -> String {
    if n.is_nan() { return "NaN".into(); }
    if n.is_infinite() { return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() }; }
    if n == 0.0 { return "0".into(); }
    if n.fract() == 0.0 && n.abs() < 1e15 { return format!("{}", n as i64); }
    let a = n.abs();
    if a >= 1e21 || a < 1e-6 {
        let e = format!("{n:e}");
        let (m, x) = e.split_once('e').unwrap();
        let x: i32 = x.parse().unwrap();
        return format!("{m}e{}{x}", if x >= 0 { "+" } else { "" });
    }
    format!("{n}")
}

/// WhiteSpace ∪ LineTerminator (ECMA-262 §12.2, §12.3) — not Unicode White_Space (U+0085 and U+180E are not JS white space).
pub fn es_ws(c: char) -> bool { matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}') }
/// JS ToNumber on a string.
pub fn str_num(x: &str) -> f64 {
    let t = x.trim_matches(es_ws);
    if t.is_empty() { return 0.0; }
    let radix = |h: &str, r: u32| -> f64 { if h.is_empty() || !h.chars().all(|c| c.is_digit(r)) { return f64::NAN; } h.chars().fold(0f64, |a, c| a * r as f64 + c.to_digit(r).unwrap() as f64) };
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { return radix(h, 16); }
    if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) { return radix(b, 2); }
    if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) { return radix(o, 8); }
    match t { "Infinity" | "+Infinity" => return f64::INFINITY, "-Infinity" => return f64::NEG_INFINITY, _ => {} }
    if !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) { return f64::NAN; }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

pub fn to_int32(n: f64) -> i32 { if !n.is_finite() { 0 } else { (n.trunc().rem_euclid(4294967296.0) as u32) as i32 } }
pub fn to_uint32(n: f64) -> u32 { if !n.is_finite() { 0 } else { n.trunc().rem_euclid(4294967296.0) as u32 } }
/// Function.prototype.toString text (§20.2.3.5): the IR keeps no source, so the head is re-spelled from the parameters
/// (DI-style code reads parameter names from it) and the body stands in as `[code]`.
fn fn_source(c: &Closure) -> String {
    fn p(x: &Pat) -> String { match x { Pat::Ident(n) => n.to_string(), Pat::Default(q, _) => p(q), Pat::Array(..) => "[]".into(), Pat::Object(..) => "{}".into(), Pat::Member(..) => "_".into() } }
    let f = &c.func;
    let mut ps: Vec<String> = f.b().params.iter().map(p).collect();
    if let Some(r) = &f.b().rest { ps.push(format!("...{}", p(r))); }
    if c.cls.is_some() { return format!("class {} {{ [code] }}", c.name); }
    let head = if f.arrow { format!("{}({}) =>", if f.is_async { "async " } else { "" }, ps.join(", ")) } else { format!("{}function{} {}({})", if f.is_async { "async " } else { "" }, if f.is_gen { "*" } else { "" }, c.name, ps.join(", ")) };
    format!("{head} {{ [code] }}")
}
/// IsAnonymousFunctionDefinition (§8.4.3): only these take the binding's name.
fn is_anon_fn(e: &Expr) -> bool { match e { Expr::Func(f) => f.name.is_empty(), Expr::Class(c) => c.name.is_empty(), _ => false } }
#[inline] fn brk(e: &Ctl, labels: &[Rc<str>]) -> bool { match e { Ctl::Break(None) => true, Ctl::Break(Some(l)) => labels.iter().any(|x| x == l), _ => false } }
#[inline] fn cont(e: &Ctl, labels: &[Rc<str>]) -> bool { match e { Ctl::Continue(None) => true, Ctl::Continue(Some(l)) => labels.iter().any(|x| x == l), _ => false } }
#[inline] fn accessor_of(x: &Val) -> Option<(Val, Val)> { if let Val::Obj(o) = x { if let Kind::Accessor(g, s) = &o.borrow().kind { return Some((g.clone(), s.clone())); } } None }
pub fn attr_of(b: &Obj, k: &str) -> u8 { if b.attrs.is_empty() { 0 } else { b.attrs.iter().find(|(x, _)| &**x == k).map_or(0, |x| x.1) } }

/// The realm's intrinsic prototypes (ECMA-262 §9.3 Realms). An object whose `proto` is None (and not F_NULLPROTO) inherits
/// the intrinsic for its kind, so objects built without an Interp (Val::array, Val::plain) still have the right chain.
pub struct Realm { pub object: ORef, pub function: ORef, pub array: ORef, pub string: ORef, pub number: ORef, pub boolean: ORef, pub symbol: ORef, pub bigint: ORef,
    pub error: ORef, pub errors: Vec<(&'static str, ORef)>, pub map: ORef, pub set: ORef, pub weakmap: ORef, pub weakset: ORef, pub weakref: ORef, pub promise: ORef,
    pub regexp: ORef, pub date: ORef, pub iter: ORef, pub array_iter: ORef, pub generator: ORef, pub async_gen: ORef, pub typed: ORef, pub typed_each: Vec<ORef>,
    pub buffer: ORef, pub view: ORef }
fn proto_obj(parent: Option<&ORef>) -> ORef { let o = Rc::new(RefCell::new(Obj { props: Vec::new(), kind: Kind::Plain, proto: parent.cloned(), flags: if parent.is_none() { F_NULLPROTO } else { 0 }, attrs: Vec::new() })); o }
impl Realm {
    fn new() -> Realm {
        let object = proto_obj(None);
        let p = || proto_obj(Some(&object));
        let error = p();
        let errors = ["TypeError", "RangeError", "SyntaxError", "ReferenceError", "EvalError", "URIError", "AggregateError"].iter().map(|n| (*n, proto_obj(Some(&error)))).collect();
        let iter = p();
        let typed = p();
        let typed_each = (0..11).map(|_| proto_obj(Some(&typed))).collect();
        Realm { function: p(), array: p(), string: p(), number: p(), boolean: p(), symbol: p(), bigint: p(), errors, error, map: p(), set: p(), weakmap: p(), weakset: p(), weakref: p(),
            promise: p(), regexp: p(), date: p(), array_iter: proto_obj(Some(&iter)), generator: proto_obj(Some(&iter)), async_gen: p(), iter, typed_each, typed, buffer: p(), view: p(), object }
    }
}

pub struct Interp { pub unhandled: Vec<Val>, pub rejection_log: Vec<String>, pub global: Scope, pub out: String, pub steps: u64, pub limit: u64, pub timers: Vec<(Val, f64, bool, Vec<Val>)>, pub host: HashMap<String, Native>,
    /// ES modules: translated IR files live in `module_dir` as <stem>.ir.json; cache by resolved key
    pub module_dir: Option<std::path::PathBuf>, pub modules: HashMap<String, Val>, exports: Vec<Val>, cur_module: Vec<String>, pub fetch_log: Vec<String>,
    /// Svelte lifecycle: onMount callbacks run after the first render; afterUpdate after every render
    pub mount_queue: Vec<Val>, pub after_update: Vec<Val>,
    /// the page's live document when jsi runs page scripts (BROWSER-BODY-0 JS-0)
    pub dom: Option<crate::jsdom::LiveRef>,
    /// lowered-AST caches for the author-based callers (svrt passes long-lived IR nodes): node address → typed node
    n_this: Rc<str>, n_args: Rc<str>, n_fn: Rc<str>, n_home: Rc<str>, n_nt: Rc<str>,
    cache_e: HashMap<usize, Rc<A::Expr>>, cache_s: HashMap<usize, Rc<A::Stmt>>, cache_p: HashMap<usize, Rc<A::Pat>>,
    pub realm: Realm,
    /// the microtask queue, drained after every script and every timer callback (HTML §8.1.7.3 perform a microtask checkpoint)
    pub jobs: VecDeque<Job>,
    /// PRNG state for Math.random / crypto (xorshift64*); never the step counter
    pub rng: u64,
    co: Vec<CoCtx>,
    /// web ES modules: (specifier, referrer key) → (resolved key, estree Program) — the page's module graph through the broker
    pub module_loader: Option<Rc<dyn Fn(&str, &str) -> Option<(String, J)>>>,
    /// symbol key → symbol value (Object.getOwnPropertySymbols / Reflect.ownKeys), Symbol.for registry
    pub syms: HashMap<Rc<str>, Val>, pub sym_for: HashMap<Rc<str>, Val>, sym_n: u64,
    /// new.target while a native constructor runs (subclassing built-ins)
    pub new_target: Option<Val>,
    pub eval_fn: Option<Val>,
    /// stack guard: address of the outermost entry frame and the bytes the interpreter may use below it
    pub stack_base: usize, pub stack_max: usize,
    join_stack: Vec<usize>,
    /// interval periods and fire counts by timer index; the virtual time inside run_timers
    timer_meta: HashMap<usize, (f64, u32)>, timer_now: f64,
    /// JS execution-context depth: when it returns to 0 the microtask queue is drained (HTML §8.1.4.4 clean up after running script)
    depth: u32, in_checkpoint: bool }

/// OMR_TRACE=1: name every closure/native call as it is entered (indented by nesting depth). A spin that is
/// inside one call shows up as a final "call X" line with no matching return; a spin that recurses shows as
/// unbounded depth. OMR_TRACE=foo,bar filters to those function names so a hot helper cannot bury the spin.
/// OMR_TRACE_MAX=N raises the entry cap (default 4000). Diagnostic only; gated so release runs are untouched.
/// OMR_BLAME=1 diagnostic, read once (it was read from the environment on every call)
fn blame_on() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var_os("OMR_BLAME").is_some()) }
fn trace_on() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var_os("OMR_TRACE").is_some()) }
fn trace_max() -> u32 { static M: std::sync::OnceLock<u32> = std::sync::OnceLock::new(); *M.get_or_init(|| std::env::var("OMR_TRACE_MAX").ok().and_then(|x| x.parse().ok()).unwrap_or(4000)) }
/// empty allowlist = trace everything; otherwise only names listed in OMR_TRACE
fn trace_want(name: &str) -> bool {
    static F: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let f = F.get_or_init(|| std::env::var("OMR_TRACE").unwrap_or_default().split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty() && x != "1").collect());
    f.is_empty() || f.iter().any(|x| name.contains(x.as_str()))
}
thread_local! { static FN_CALLED: std::cell::RefCell<std::collections::HashSet<usize>> = std::cell::RefCell::new(std::collections::HashSet::new()); static FN_TOTAL: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; static BS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) }; static TD: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; static TN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }

macro_rules! throw { ($i:expr, $($a:tt)*) => { return Err(Ctl::Throw($i.error("Error", &format!($($a)*)))) }; }
macro_rules! type_err { ($i:expr, $($a:tt)*) => { return Err(Ctl::Throw($i.error("TypeError", &format!($($a)*)))) }; }
/// Evaluate a resumable step: on Suspend, record this construct's slot (built lazily) and keep unwinding.
macro_rules! sus { ($s:expr, $e:expr, $slot:expr) => { match $e { Err(Ctl::Suspend) => { let sl = $slot; $s.save_slot(sl); return Err(Ctl::Suspend); } r => r? } }; }

impl Default for Interp { fn default() -> Self { Self::new() } }

impl Interp {
    pub fn new() -> Interp {
        let g = Rc::new(RefCell::new(Frame::map(None)));
        // OMR_CLOCK=virtual: a fixed Math.random seed (the clock and the frame address would differ per run)
        let seed = if crate::jsi_date::vclock() { 0x9E37_79B9_7F4A_7C15 } else { let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9); t ^ (&g as *const _ as u64).rotate_left(29) ^ 0x2545_F491_4F6C_DD1D };
        let mut it = Interp { unhandled: Vec::new(), rejection_log: Vec::new(), global: g, out: String::new(), steps: 0, limit: 200_000_000, timers: Vec::new(), host: HashMap::new(), module_dir: None, modules: HashMap::new(), exports: Vec::new(), cur_module: Vec::new(), fetch_log: Vec::new(), mount_queue: Vec::new(), after_update: Vec::new(), dom: None,
            n_this: Rc::from("this"), n_args: Rc::from("arguments"), n_fn: Rc::from("%fn"), n_home: Rc::from("%home"), n_nt: Rc::from("%nt"),
            cache_e: HashMap::new(), cache_s: HashMap::new(), cache_p: HashMap::new(), realm: Realm::new(), jobs: VecDeque::new(), rng: seed | 1, co: Vec::new(), module_loader: None,
            syms: HashMap::new(), sym_for: HashMap::new(), sym_n: 0, new_target: None, eval_fn: None, stack_base: 0, stack_max: 6 << 20, join_stack: Vec::new(), timer_meta: HashMap::new(), timer_now: 0.0, depth: 0, in_checkpoint: false };
        it.install_globals();
        it
    }

    /// xorshift64* (Vigna 2016) — Math.random / crypto randomness.
    pub fn next_rand(&mut self) -> u64 { let mut x = self.rng; x ^= x >> 12; x ^= x << 25; x ^= x >> 27; self.rng = x; x.wrapping_mul(0x2545_F491_4F6C_DD1D) }

    pub fn error(&self, name: &str, msg: &str) -> Val {
        let proto = if name == "Error" { Some(self.realm.error.clone()) } else { self.realm.errors.iter().find(|(n, _)| *n == name).map(|x| x.1.clone()) };
        let e = Val::obj(Kind::Error);
        if let Val::Obj(o) = &e {
            let mut b = o.borrow_mut();
            b.props = vec![(Rc::from("message"), st(msg)), (Rc::from("stack"), st(&format!("{name}: {msg}\n    at <anonymous>")))];
            b.attrs = vec![(Rc::from("message"), A_NOENUM), (Rc::from("stack"), A_NOENUM)];
            // names without an intrinsic (DOMException-style) keep their own name
            if proto.is_none() { b.props.push((Rc::from("name"), st(name))); b.attrs.push((Rc::from("name"), A_NOENUM)); }
            b.proto = Some(proto.unwrap_or_else(|| self.realm.error.clone()));
        }
        e
    }
    fn err(&self, name: &str, msg: &str) -> Ctl { Ctl::Throw(self.error(name, msg)) }

    // ── symbols ──
    pub fn new_symbol(&mut self, desc: Option<Rc<str>>) -> Val {
        self.sym_n += 1;
        let key: Rc<str> = Rc::from(format!("\u{0}S{}", self.sym_n).as_str());
        let v = Val::Sym(Rc::new(Sym { key: key.clone(), desc }));
        self.syms.insert(key, v.clone());
        v
    }
    /// The well-known symbol for `name` (Symbol.iterator, …), created on first use.
    pub fn well_known(&mut self, name: &str) -> Val {
        let key: Rc<str> = Rc::from(format!("\u{0}@@{name}").as_str());
        if let Some(v) = self.syms.get(&key) { return v.clone(); }
        let v = Val::Sym(Rc::new(Sym { key: key.clone(), desc: Some(Rc::from(format!("Symbol.{name}").as_str())) }));
        self.syms.insert(key, v.clone());
        v
    }

    // ── conversions ──
    pub fn to_str(&mut self, v: &Val) -> Result<Rc<str>, Ctl> {
        Ok(match v {
            Val::Undef => Rc::from("undefined"), Val::Null => Rc::from("null"), Val::Bool(b) => Rc::from(if *b { "true" } else { "false" }),
            Val::Num(n) => Rc::from(num_str(*n).as_str()), Val::Str(x) => x.clone(),
            Val::Sym(_) => type_err!(self, "Cannot convert a Symbol value to a string"),
            Val::Big(b) => Rc::from(b.to_string().as_str()),
            Val::Obj(_) => { let p = self.to_primitive(v, 2)?; return self.to_str(&p); }
        })
    }

    pub fn to_num(&mut self, v: &Val) -> Result<f64, Ctl> {
        Ok(match v {
            Val::Undef => f64::NAN, Val::Null => 0.0, Val::Bool(b) => *b as i32 as f64, Val::Num(n) => *n, Val::Str(x) => str_num(x),
            Val::Sym(_) => type_err!(self, "Cannot convert a Symbol value to a number"),
            Val::Big(_) => type_err!(self, "Cannot convert a BigInt value to a number"),
            Val::Obj(_) => { let p = self.to_primitive(v, 1)?; return self.to_num(&p); }
        })
    }

    /// ECMA-262 §7.1.19 ToPropertyKey: symbols use their internal key.
    pub fn to_key(&mut self, v: &Val) -> Result<Rc<str>, Ctl> {
        match v { Val::Str(s) => Ok(s.clone()), Val::Sym(s) => Ok(s.key.clone()), Val::Num(n) => Ok(Rc::from(num_str(*n).as_str())), Val::Obj(_) => { let p = self.to_primitive(v, 2)?; self.to_key(&p) } _ => self.to_str(v) }
    }

    /// ECMA-262 §7.1.1 ToPrimitive (hint 0 default, 1 number, 2 string) with §7.1.1.1 OrdinaryToPrimitive.
    pub fn to_primitive(&mut self, v: &Val, hint: u8) -> Result<Val, Ctl> {
        let Val::Obj(o) = v else { return Ok(v.clone()) };
        let fast: Option<Val> = {
            let b = o.borrow();
            match &b.kind {
                Kind::Prim(p) => Some(p.clone()),
                Kind::Func(c) => Some(st(&fn_source(c))),
                Kind::Native(_, n) => Some(st(&format!("function {n}() {{ [native code] }}"))),
                Kind::RegExp(p, f) => Some(st(&format!("/{p}/{f}"))),
                Kind::Host(_, 5) => self.dom.as_ref().map(|l| st(&l.borrow().url)),
                Kind::Host(_, 4) => Some(st("[object Window]")),
                Kind::Host(_, _) => Some(st("[object HTMLElement]")),
                _ => None,
            }
        };
        if let Some(x) = fast { return Ok(x); }
        let is_arr = matches!(o.borrow().kind, Kind::Array(_));
        if is_arr { return Ok(Val::Str(self.array_join(o, ",")?)); }
        let ex = self.get(v, K_TO_PRIM)?;
        if ex.is_callable() {
            let r = self.call(&ex, v.clone(), vec![st(match hint { 1 => "number", 2 => "string", _ => "default" })])?;
            if !matches!(r, Val::Obj(_)) { return Ok(r); }
            type_err!(self, "Cannot convert object to primitive value");
        }
        let order = if hint == 2 { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for m in order { let f = self.get(v, m)?; if f.is_callable() { let r = self.call(&f, v.clone(), vec![])?; if !matches!(r, Val::Obj(_)) { return Ok(r); } } }
        type_err!(self, "Cannot convert object to primitive value")
    }

    /// Array.prototype.join (ECMA-262 §23.1.3.18), cycle-safe as engines are (a cycle joins as "").
    pub fn array_join(&mut self, o: &ORef, sep: &str) -> Result<Rc<str>, Ctl> {
        let id = Rc::as_ptr(o) as *const u8 as usize;
        if self.join_stack.contains(&id) { return Ok(Rc::from("")); }
        let items: Vec<Val> = if let Kind::Array(a) = &o.borrow().kind { a.clone() } else { vec![] };
        self.join_stack.push(id);
        let mut out = String::new();
        let mut r = Ok(());
        for (i, x) in items.iter().enumerate() { if i > 0 { out += sep; } if !x.is_nullish() { match self.to_str(x) { Ok(s) => out += &s, Err(e) => { r = Err(e); break; } } } }
        self.join_stack.pop();
        r?;
        Ok(Rc::from(fix_pairs(out).as_str()))
    }

    pub fn strict_eq(a: &Val, b: &Val) -> bool {
        match (a, b) {
            (Val::Undef, Val::Undef) | (Val::Null, Val::Null) => true,
            (Val::Bool(x), Val::Bool(y)) => x == y, (Val::Num(x), Val::Num(y)) => x == y,
            (Val::Str(x), Val::Str(y)) => x == y, (Val::Obj(x), Val::Obj(y)) => Rc::ptr_eq(x, y),
            (Val::Sym(x), Val::Sym(y)) => Rc::ptr_eq(x, y), (Val::Big(x), Val::Big(y)) => x == y,
            _ => false,
        }
    }

    pub fn same_value_zero(a: &Val, b: &Val) -> bool { match (a, b) { (Val::Num(x), Val::Num(y)) if x.is_nan() && y.is_nan() => true, _ => Self::strict_eq(a, b) } }

    fn loose_eq(&mut self, a: &Val, b: &Val) -> Result<bool, Ctl> {
        Ok(match (a, b) {
            (Val::Undef | Val::Null, Val::Undef | Val::Null) => true,
            (Val::Undef | Val::Null, _) | (_, Val::Undef | Val::Null) => false,
            (Val::Big(x), Val::Num(y)) | (Val::Num(y), Val::Big(x)) => y.is_finite() && y.fract() == 0.0 && x.to_f64() == *y,
            (Val::Big(x), Val::Str(y)) | (Val::Str(y), Val::Big(x)) => crate::jsi_lib::Big::parse(y).map_or(false, |z| z == **x),
            (Val::Sym(_), Val::Sym(_)) => Self::strict_eq(a, b),
            (Val::Num(_), Val::Str(_)) | (Val::Str(_), Val::Num(_)) | (Val::Bool(_), _) | (_, Val::Bool(_)) if !matches!((a, b), (Val::Obj(_), _) | (_, Val::Obj(_))) => {
                if let (Val::Bool(x), Val::Big(_)) | (Val::Big(_), Val::Bool(x)) = (a, b) { let n = Val::Num(*x as i32 as f64); return self.loose_eq(&n, if matches!(a, Val::Big(_)) { a } else { b }); }
                self.to_num(a)? == self.to_num(b)?
            }
            (Val::Obj(_), Val::Obj(_)) => Self::strict_eq(a, b),
            (Val::Obj(_), _) => { let p = self.to_primitive(a, 0)?; return self.loose_eq(&p, b); }
            (_, Val::Obj(_)) => { let p = self.to_primitive(b, 0)?; return self.loose_eq(a, &p); }
            _ => Self::strict_eq(a, b),
        })
    }

    // ── scopes ──
    /// Lookup without building a ReferenceError when absent (hot paths: `this` in arrows).
    pub fn lookup_opt(&self, scope: &Scope, name: &str) -> Option<Val> {
        let mut cur = Some(scope.clone());
        while let Some(f) = cur { if let Some(v) = f.borrow().get(name) { return Some(v.clone()); } cur = f.borrow().parent.clone(); }
        None
    }

    pub fn lookup(&mut self, scope: &Scope, name: &str) -> Result<Val, Ctl> {
        {
            let mut cur = Some(scope.clone());
            while let Some(f) = cur {
                if let Some(v) = f.borrow().get(name) { return Ok(v.clone()); }
                let w = f.borrow().with.clone();
                if let Some(w) = w { if self.has_prop(&w, name)? { return self.get(&w, name); } }
                cur = f.borrow().parent.clone();
            }
        }
        match name { "undefined" => Ok(Val::Undef), "NaN" => Ok(Val::Num(f64::NAN)), "Infinity" => Ok(Val::Num(f64::INFINITY)), "window" => Ok(Val::Undef), _ => Err(Ctl::Throw(self.error("ReferenceError", &format!("{name} is not defined")))) }
    }
    pub(crate) fn assign_var(&mut self, scope: &Scope, name: &str, v: Val) -> Result<(), Ctl> {
        let mut cur = Some(scope.clone());
        while let Some(f) = cur {
            { let mut fb = f.borrow_mut(); let fixed = fb.fixed; if let Some(slot) = fb.get_mut(name) { if !fixed { *slot = v; } return Ok(()); } }
            let w = f.borrow().with.clone();
            if let Some(w) = w { if self.has_prop(&w, name)? { return self.set(&w, name, v); } }
            cur = f.borrow().parent.clone();
        }
        self.global.borrow_mut().vars.insert(Rc::from(name), v); // sloppy-mode implicit global
        Ok(())
    }
    pub fn declare(scope: &Scope, name: &str, v: Val) { scope.borrow_mut().put(&Rc::from(name), v); }
    pub fn child(scope: &Scope) -> Scope { Rc::new(RefCell::new(Frame::map(Some(scope.clone())))) }
    /// a block / loop / catch / switch frame: slotted when the resolver gave the construct a layout
    #[inline] pub(crate) fn child_l(scope: &Scope, lay: &A::Lay) -> Scope {
        let mut f = Frame::map(Some(scope.clone()));
        if let Some(l) = lay { f.slots = vec![None; l.names.len()]; f.lay = Some(l.clone()); }
        Rc::new(RefCell::new(f))
    }

    /// Run `f` on the frame `d` hops up, holding shared borrows on the frames between (no Rc clones on the walk).
    #[inline] fn at_depth<R>(s: &RefCell<Frame>, d: u32, f: impl FnOnce(&RefCell<Frame>) -> R) -> R {
        if d == 0 { return f(s); }
        let b = s.borrow(); Self::at_depth(b.parent.as_ref().expect("slot depth"), d - 1, f)
    }
    /// A resolved identifier read (`Expr::Slot`): `d` frames up, slot `i`; an unbound slot continues by name outward.
    #[inline] pub(crate) fn slot_get(&mut self, scope: &Scope, d: u32, i: u32, name: &Rc<str>) -> Result<Val, Ctl> {
        let r = match d {
            0 => scope.borrow().slots[i as usize].clone(),
            1 => { let b = scope.borrow(); let p = b.parent.as_ref().expect("slot depth").borrow(); p.slots[i as usize].clone() }
            _ => Self::at_depth(scope, d, |f| f.borrow().slots[i as usize].clone()),
        };
        match r { Some(v) if !slotcheck_on() => Ok(v), r => self.slot_get_slow(scope, d, i, name, r) }
    }
    #[cold] #[inline(never)] fn slot_get_slow(&mut self, scope: &Scope, d: u32, i: u32, name: &Rc<str>, r: Option<Val>) -> Result<Val, Ctl> {
        let r = match r { Some(v) => Ok(v), None => { let p = Self::frame_up(scope, d + 1); match p { Some(p) => self.lookup(&p, name), None => self.lookup(&self.global.clone(), name) } } };
        if slotcheck_on() { self.slot_check(scope, d, i, name, r.as_ref().ok()); }
        r
    }
    /// OMR_BLAME text for a nullish member read / method call (compiled code; the walker prints the same inline)
    pub(crate) fn blame_read(&self, obj: &Expr, key: &str) { if blame_on() { eprintln!("jsi blame: nullish read of {} (.{}) in {}", expr_desc(obj), key, BS.with(|b| { let b = b.borrow(); b[b.len().saturating_sub(8)..].join(" < ") })); } }
    pub(crate) fn blame_call(&self, obj: &Expr, key: &str) { if blame_on() { eprintln!("jsi blame: call of {}.{}(..) on nullish in {}", expr_desc(obj), key, BS.with(|b| { let b = b.borrow(); b[b.len().saturating_sub(8)..].join(" < ") })); } }
    /// `o.k(..)` where `o.k` is not callable (call_t's message)
    pub(crate) fn not_a_method(&self, obj: &Expr, key: &str) -> Ctl {
        let on = match obj { Expr::Ident(n) | Expr::Slot(_, _, n) => format!("{n}."), Expr::This => "this.".into(), _ => String::new() };
        Ctl::Throw(self.error("TypeError", &format!("{}{} is not a function", on, self.shown_key(key))))
    }
    fn frame_up(scope: &Scope, d: u32) -> Option<Scope> { let mut f = scope.clone(); for _ in 0..d { let p = f.borrow().parent.clone()?; f = p; } Some(f) }
    /// A resolved assignment: an unbound slot assigns by name outward (as a missing map key does).
    #[inline] pub(crate) fn slot_set(&mut self, scope: &Scope, d: u32, i: u32, name: &Rc<str>, v: Val) -> Result<(), Ctl> {
        if slotcheck_on() { return self.slot_set_slow(scope, d, i, name, v); }
        // a bound slot takes the value; an unbound one hands it back for the by-name path
        let back = Self::at_depth(scope, d, |f| { let mut b = f.borrow_mut(); match &mut b.slots[i as usize] { Some(x) => { *x = v; None } None => Some(v) } });
        match back { None => Ok(()), Some(v) => self.slot_set_slow(scope, d, i, name, v) }
    }
    #[cold] #[inline(never)] fn slot_set_slow(&mut self, scope: &Scope, d: u32, i: u32, name: &Rc<str>, v: Val) -> Result<(), Ctl> {
        let f = Self::frame_up(scope, d).expect("slot depth");
        let bound = { let mut b = f.borrow_mut(); match &mut b.slots[i as usize] { Some(x) => { *x = v.clone(); true } None => false } };
        if !bound { match f.borrow().parent.clone() { Some(p) => self.assign_var(&p, name, v.clone())?, None => self.assign_var(&self.global.clone(), name, v.clone())? } }
        if slotcheck_on() { self.slot_check(scope, d, i, name, Some(&v)); }
        Ok(())
    }
    fn slot_check(&mut self, scope: &Scope, d: u32, i: u32, name: &Rc<str>, got: Option<&Val>) {
        let want = self.lookup_opt_with(scope, name);
        let same = match (got, &want) { (Some(a), Some(b)) => Self::same_value_zero(a, b), (None, None) => true, _ => false };
        let lay_ok = Self::frame_up(scope, d).map_or(false, |f| f.borrow().lay.as_ref().map_or(false, |l| l.names.get(i as usize).map_or(false, |n| n == name)));
        if !same || !lay_ok { panic!("SLOTCHECK mismatch: '{name}' depth {d} slot {i} (layout ok {lay_ok}): slot gave {} by-name gave {}", got.map_or("<error>".to_string(), |v| v.type_of().to_string()), want.as_ref().map_or("<absent>".to_string(), |v| v.type_of().to_string())); }
    }
    /// by-name read including `with` objects, without side effects beyond getters (SLOTCHECK only)
    fn lookup_opt_with(&mut self, scope: &Scope, name: &str) -> Option<Val> { self.lookup(scope, name).ok() }

    // ── properties (ECMA-262 §10.1 ordinary object internal methods) ──
    /// [[GetPrototypeOf]] of an ordinary object, including the intrinsic default for its kind.
    pub fn proto_of_obj(&self, b: &Obj) -> Option<ORef> {
        if let Some(p) = &b.proto { return Some(p.clone()); }
        if b.flags & F_NULLPROTO != 0 { return None; }
        let r = &self.realm;
        Some(match &b.kind {
            Kind::Plain | Kind::Host(..) | Kind::Accessor(..) | Kind::Global => r.object.clone(),
            Kind::Array(_) => r.array.clone(), Kind::Func(_) | Kind::Native(..) => r.function.clone(), Kind::Error => r.error.clone(),
            Kind::Map(_) => r.map.clone(), Kind::Set(_) => r.set.clone(), Kind::Promise(_) => r.promise.clone(), Kind::RegExp(..) => r.regexp.clone(),
            Kind::Prim(p) => match p { Val::Str(_) => r.string.clone(), Val::Num(_) => r.number.clone(), Val::Bool(_) => r.boolean.clone(), Val::Sym(_) => r.symbol.clone(), Val::Big(_) => r.bigint.clone(), _ => r.object.clone() },
            Kind::Date(_) => r.date.clone(), Kind::Iter(_) => r.array_iter.clone(), Kind::Gen(_) => r.generator.clone(), Kind::Proxy(..) => return None,
            Kind::WeakMap(_) => r.weakmap.clone(), Kind::WeakSet(_) => r.weakset.clone(), Kind::WeakRef(_) => r.weakref.clone(),
            Kind::Buffer(_) => r.buffer.clone(), Kind::Typed(t) => r.typed_each[(t.ty as usize).min(10)].clone(), Kind::View(_) => r.view.clone(),
        })
    }
    /// Object.getPrototypeOf semantics for any value (primitives answer their wrapper's prototype).
    pub fn proto_of(&mut self, v: &Val) -> Result<Option<ORef>, Ctl> {
        match v {
            Val::Obj(o) => {
                let px = if let Kind::Proxy(t, h) = &o.borrow().kind { Some((t.clone(), h.clone())) } else { None };
                if let Some((t, h)) = px { let trap = self.get(&h, "getPrototypeOf")?; if trap.is_callable() { let r = self.call(&trap, h, vec![t])?; return Ok(r.as_obj().cloned()); } return self.proto_of(&t); }
                Ok(self.proto_of_obj(&o.borrow()))
            }
            Val::Str(_) => Ok(Some(self.realm.string.clone())), Val::Num(_) => Ok(Some(self.realm.number.clone())), Val::Bool(_) => Ok(Some(self.realm.boolean.clone())),
            Val::Sym(_) => Ok(Some(self.realm.symbol.clone())), Val::Big(_) => Ok(Some(self.realm.bigint.clone())),
            _ => type_err!(self, "Cannot convert undefined or null to object"),
        }
    }
    /// [[SetPrototypeOf]] (§10.1.2.1): refuses cycles and non-extensible targets.
    pub fn set_proto(&self, o: &ORef, p: Option<ORef>) -> bool {
        if let Some(pp) = &p { let mut cur = Some(pp.clone()); while let Some(c) = cur { if Rc::ptr_eq(&c, o) { return false; } cur = { let b = c.borrow(); if matches!(b.kind, Kind::Proxy(..)) { None } else { self.proto_of_obj(&b) } }; } }
        let mut b = o.borrow_mut();
        let same = match (&p, &b.proto) { (Some(x), Some(y)) => Rc::ptr_eq(x, y), _ => false };
        if b.flags & (F_NOEXT | F_SEALED | F_FROZEN) != 0 && !same { return false; }
        match p { Some(x) => { b.proto = Some(x); b.flags &= !F_NULLPROTO; } None => { b.proto = None; b.flags |= F_NULLPROTO; } }
        true
    }
    /// The Val a property key denotes (symbol keys map back to their Symbol).
    pub fn key_val(&self, key: &str) -> Val { if key.starts_with('\u{0}') { if let Some(s) = self.syms.get(key) { return s.clone(); } } st(key) }
    fn shown_key(&self, key: &str) -> String { match self.syms.get(key) { Some(Val::Sym(s)) => format!("Symbol({})", s.desc.as_deref().unwrap_or("")), _ => key.to_string() } }

    pub fn get(&mut self, v: &Val, key: &str) -> Result<Val, Ctl> {
        match v {
            Val::Obj(o) => { let o = o.clone(); self.get_obj(&o, key, v) }
            Val::Undef | Val::Null => { let k = self.shown_key(key); if blame_on() { BS.with(|b| eprintln!("jsi blame: read .{} of nullish in [{}]", k, b.borrow().iter().rev().take(8).cloned().collect::<Vec<_>>().join(" < "))); } type_err!(self, "Cannot read properties of {} (reading '{}')", if matches!(v, Val::Undef) { "undefined" } else { "null" }, k) }
            Val::Str(x) => {
                if key == "length" { return Ok(Val::Num(match units(x) { None => x.len(), Some(u) => u.len() } as f64)); }
                if let Some(i) = index_of(key) { return Ok(str_at_rc(x, i)); }
                let p = self.realm.string.clone(); self.get_obj(&p, key, v)
            }
            Val::Num(_) => { let p = self.realm.number.clone(); self.get_obj(&p, key, v) }
            Val::Bool(_) => { let p = self.realm.boolean.clone(); self.get_obj(&p, key, v) }
            Val::Sym(s) => { if key == "description" { return Ok(s.desc.clone().map(Val::Str).unwrap_or(Val::Undef)); } let p = self.realm.symbol.clone(); self.get_obj(&p, key, v) }
            Val::Big(_) => { let p = self.realm.bigint.clone(); self.get_obj(&p, key, v) }
        }
    }

    /// [[Get]] (§10.1.8.1): own property (accessors call their getter with the receiver), the kind's exotic own
    /// properties, then up the prototype chain. Property keys are unique in what the engine builds (set updates in place);
    /// a host that pushes a second entry for an existing key does not shadow the first (the page layer's older Object /
    /// Promise stand-ins pushed over complete built-ins).
    pub fn get_obj(&mut self, o: &ORef, key: &str, recv: &Val) -> Result<Val, Ctl> {
        enum G { Found(Val), Getter(Val), Host(u32, u8), Proxy(Val, Val), Next(Option<ORef>) }
        let mut cur = o.clone();
        let mut first = true;
        loop {
            let step = {
                let b = cur.borrow();
                if let Some((_, x)) = b.props.iter().find(|(k, _)| &**k == key) {
                    match if b.flags & F_ACC != 0 { accessor_of(x) } else { None } { Some((g, _)) => G::Getter(g), None => G::Found(x.clone()) }
                } else {
                    match &b.kind {
                        Kind::Plain => G::Next(self.proto_of_obj(&b)),
                        Kind::Array(a) => {
                            if key == "length" { G::Found(Val::Num(a.len() as f64)) }
                            else if let Some(i) = index_of(key) { G::Found(a.get(i).cloned().unwrap_or(Val::Undef)) }
                            else { G::Next(self.proto_of_obj(&b)) }
                        }
                        Kind::Func(c) => match key {
                            "name" => G::Found(st(&c.name)),
                            "length" => G::Found(Val::Num(c.func.np_len as f64)),
                            _ => G::Next(self.proto_of_obj(&b)),
                        },
                        Kind::Native(_, nm) => match key { "name" => G::Found(st(nm)), "length" => G::Found(Val::Num(crate::jsi_lib::native_len(nm))), _ => G::Next(self.proto_of_obj(&b)) },
                        Kind::RegExp(p, f) => match key {
                            "source" => G::Found(st(if p.is_empty() { "(?:)" } else { p })), "flags" => G::Found(st(f)),
                            "global" => G::Found(Val::Bool(f.contains('g'))), "ignoreCase" => G::Found(Val::Bool(f.contains('i'))), "multiline" => G::Found(Val::Bool(f.contains('m'))),
                            "sticky" => G::Found(Val::Bool(f.contains('y'))), "unicode" => G::Found(Val::Bool(f.contains('u'))), "dotAll" => G::Found(Val::Bool(f.contains('s'))), "hasIndices" => G::Found(Val::Bool(f.contains('d'))),
                            // own property, initialised to 0 when the literal is built; a RegExp() result may lack it
                            "lastIndex" if first => G::Found(Val::Num(0.0)),
                            _ => G::Next(self.proto_of_obj(&b)),
                        },
                        Kind::Map(m) | Kind::Set(m) if key == "size" => G::Found(Val::Num(m.live as f64)),
                        Kind::Prim(Val::Str(s)) if key == "length" => G::Found(Val::Num(len16(s) as f64)),
                        Kind::Prim(Val::Str(s)) if index_of(key).is_some() => G::Found(str_at(s, index_of(key).unwrap())),
                        Kind::Typed(t) => match key {
                            "length" => G::Found(Val::Num(t.len as f64)), "byteLength" => G::Found(Val::Num((t.len * crate::jsi_lib::elem_size(t.ty)) as f64)), "byteOffset" => G::Found(Val::Num(t.off as f64)), "buffer" => G::Found(t.bufobj.clone()),
                            _ => match index_of(key) { Some(i) => G::Found(crate::jsi_lib::typed_get(t, i)), None => G::Next(self.proto_of_obj(&b)) },
                        },
                        Kind::View(t) => match key { "byteLength" => G::Found(Val::Num(t.len as f64)), "byteOffset" => G::Found(Val::Num(t.off as f64)), "buffer" => G::Found(t.bufobj.clone()), _ => G::Next(self.proto_of_obj(&b)) },
                        Kind::Buffer(x) if key == "byteLength" => G::Found(Val::Num(x.borrow().len() as f64)),
                        Kind::Global => match self.global.borrow().vars.get(key) { Some(v) => G::Found(v.clone()), None => G::Next(self.proto_of_obj(&b)) },
                        Kind::Host(i, role) if first => G::Host(*i, *role),
                        Kind::Proxy(t, h) => G::Proxy(t.clone(), h.clone()),
                        _ => G::Next(self.proto_of_obj(&b)),
                    }
                }
            };
            match step {
                G::Found(v) => return Ok(v),
                G::Getter(g) => return if g.is_callable() { self.call(&g, recv.clone(), vec![]) } else { Ok(Val::Undef) },
                G::Host(i, role) => {
                    let r = crate::jsdom::get(self, recv, i, role, key)?;
                    if !matches!(r, Val::Undef) { return Ok(r); }
                    let n = self.proto_of_obj(&cur.borrow());
                    match n { Some(p) => cur = p, None => break }
                }
                G::Proxy(t, h) => return self.proxy_get(&t, &h, key, recv),
                G::Next(Some(p)) => cur = p,
                G::Next(None) => break,
            }
            first = false;
        }
        if key == "__proto__" { return Ok(match self.proto_of(recv)? { Some(p) => Val::Obj(p), None => Val::Null }); }
        Ok(Val::Undef)
    }

    fn proxy_get(&mut self, t: &Val, h: &Val, key: &str, recv: &Val) -> Result<Val, Ctl> {
        let trap = self.get(h, "get")?;
        if trap.is_callable() { let k = self.key_val(key); return self.call(&trap, h.clone(), vec![t.clone(), k, recv.clone()]); }
        self.get(t, key)
    }

    /// `o[k]` with a computed key: integer indexes into arrays, typed arrays and strings skip the key string.
    pub fn get_v(&mut self, v: &Val, k: &Val) -> Result<Val, Ctl> {
        if let Val::Num(n) = k {
            let i = *n as usize;
            if i as f64 == *n {
                match v {
                    Val::Obj(o) => { let b = o.borrow(); match &b.kind { Kind::Array(a) if i < a.len() => return Ok(a[i].clone()), Kind::Typed(t) => return Ok(crate::jsi_lib::typed_get(t, i)), _ => {} } }
                    Val::Str(s) => return Ok(str_at_rc(s, i)),
                    _ => {}
                }
            }
        }
        let key = self.to_key(k)?;
        self.get(v, &key)
    }
    pub fn set_v(&mut self, v: &Val, k: &Val, x: Val) -> Result<(), Ctl> {
        if let (Val::Num(n), Val::Obj(o)) = (k, v) {
            let i = *n as usize;
            if i as f64 == *n {
                let mut b = o.borrow_mut();
                if b.flags & (F_FROZEN | F_SEALED | F_NOEXT) == 0 { if let Kind::Array(a) = &mut b.kind { if i < a.len() { a[i] = x; return Ok(()); } if i == a.len() { a.push(x); return Ok(()); } } }
            }
        }
        let key = self.to_key(k)?;
        self.set(v, &key, x)
    }

    /// [[Set]] (§10.1.9.2 OrdinarySetWithOwnDescriptor): setters, non-writable and non-extensible objects fail silently (sloppy mode).
    pub fn set(&mut self, v: &Val, key: &str, x: Val) -> Result<(), Ctl> {
        let o = match v { Val::Obj(o) => o.clone(), Val::Undef | Val::Null => { let k = self.shown_key(key); type_err!(self, "Cannot set properties of {} (setting '{}')", if matches!(v, Val::Undef) { "undefined" } else { "null" }, k) } _ => return Ok(()) };
        let tag = { let b = o.borrow(); match &b.kind { Kind::Array(_) => 1u8, Kind::Typed(_) => 2, Kind::Host(..) => 3, Kind::Proxy(..) => 4, Kind::Global => 5, Kind::RegExp(..) => 6, _ => 0 } };
        match tag {
            1 => {
                if let Some(i) = index_of(key) {
                    let mut b = o.borrow_mut(); let fl = b.flags;
                    if fl & F_FROZEN != 0 { return Ok(()); }
                    if let Kind::Array(a) = &mut b.kind {
                        if i >= a.len() { if fl & (F_NOEXT | F_SEALED) != 0 { return Ok(()); } if i > a.len() + (1 << 26) { drop(b); return Err(self.err("RangeError", "Invalid array length")); } a.resize(i + 1, Val::Undef); }
                        a[i] = x;
                    }
                    return Ok(());
                }
                if key == "length" {
                    let n = self.to_num(&x)?;
                    if n < 0.0 || n.fract() != 0.0 || n > 4294967295.0 { return Err(self.err("RangeError", "Invalid array length")); }
                    if n > (1u64 << 28) as f64 { return Err(self.err("RangeError", "Invalid array length")); }
                    let mut b = o.borrow_mut(); if b.flags & F_FROZEN != 0 { return Ok(()); }
                    if let Kind::Array(a) = &mut b.kind { a.resize(n as usize, Val::Undef); }
                    return Ok(());
                }
            }
            2 => if let Some(i) = index_of(key) {
                let big = { let b = o.borrow(); matches!(&b.kind, Kind::Typed(t) if t.ty >= 9) };
                let xv = if big { match &x { Val::Big(_) => x.clone(), _ => type_err!(self, "Cannot convert to a BigInt") } } else { Val::Num(self.to_num(&x)?) };
                let b = o.borrow(); if let Kind::Typed(t) = &b.kind { crate::jsi_lib::typed_set(t, i, &xv); }
                return Ok(());
            },
            3 => { let (i, role) = if let Kind::Host(i, r) = o.borrow().kind { (i, r) } else { (0, 0) }; if crate::jsdom::set(self, i, role, key, &x)? { return Ok(()); } }
            4 => { let (t, h) = if let Kind::Proxy(t, h) = &o.borrow().kind { (t.clone(), h.clone()) } else { return Ok(()) }; return self.proxy_set(&t, &h, key, x, v); }
            5 => { self.global.borrow_mut().vars.insert(Rc::from(key), x); return Ok(()); }
            // a script writing lastIndex takes over the position: drop exec's empty-match drain marker (jsi_re::exec)
            6 if key == "lastIndex" => { o.borrow_mut().props.retain(|(k, _)| &**k != "\u{0}empty"); }
            _ => {}
        }
        self.set_ordinary(&o, key, x, v)
    }

    fn set_ordinary(&mut self, o: &ORef, key: &str, x: Val, recv: &Val) -> Result<(), Ctl> {
        let setter = {
            let mut b = o.borrow_mut();
            let fl = b.flags;
            match b.props.iter().position(|(k, _)| &**k == key) {
                Some(pos) => {
                    match if fl & F_ACC != 0 { accessor_of(&b.props[pos].1) } else { None } {
                        Some((_, s)) => Some(s),
                        None => { if fl & F_FROZEN != 0 || (fl & F_RO != 0 && attr_of(&b, key) & A_NOWRITE != 0) { return Ok(()); } b.props[pos].1 = x; return Ok(()); }
                    }
                }
                None => None,
            }
        };
        if let Some(s) = setter { if s.is_callable() { self.call(&s, recv.clone(), vec![x])?; } return Ok(()); }
        if key == "__proto__" { match &x { Val::Obj(p) => { self.set_proto(o, Some(p.clone())); } Val::Null => { self.set_proto(o, None); } _ => {} } return Ok(()); }
        // inherited setter / read-only data property: only prototypes that hold one are searched
        let mut cur = self.proto_of_obj(&o.borrow());
        while let Some(p) = cur {
            let step = {
                let b = p.borrow();
                if b.flags & (F_ACC | F_RO | F_FROZEN) != 0 {
                    match b.props.iter().find(|(k, _)| &**k == key) {
                        Some((_, val)) => match accessor_of(val) { Some((_, s)) => Some(Some(s)), None => if b.flags & F_FROZEN != 0 || attr_of(&b, key) & A_NOWRITE != 0 { Some(None) } else { None } },
                        None => None,
                    }
                } else { None }
            };
            match step { Some(Some(s)) => { if s.is_callable() { self.call(&s, recv.clone(), vec![x])?; } return Ok(()); } Some(None) => return Ok(()), None => {} }
            cur = { let b = p.borrow(); if matches!(b.kind, Kind::Proxy(..) | Kind::Host(..)) { None } else { self.proto_of_obj(&b) } };
        }
        let mut b = o.borrow_mut();
        if b.flags & (F_NOEXT | F_SEALED | F_FROZEN) != 0 { return Ok(()); }
        b.props.push((Rc::from(key), x));
        Ok(())
    }

    /// OrdinarySet with an explicit receiver (§10.1.9.2, Reflect.set): a setter on target's chain runs with the receiver;
    /// otherwise the receiver gets / updates an own writable data property. Returns the success flag.
    pub fn set_with_receiver(&mut self, target: &Val, key: &str, x: Val, recv: &Val) -> Result<bool, Ctl> {
        let same = matches!((target, recv), (Val::Obj(a), Val::Obj(b)) if Rc::ptr_eq(a, b));
        if same { let fl = target.as_obj().map_or(0, |o| o.borrow().flags); let ro = self.own_prop(target, key)?.map_or(false, |(v, a)| v.is_ok() && a & A_NOWRITE != 0); self.set(target, key, x)?; return Ok(fl & F_FROZEN == 0 && !ro); }
        let mut cur = target.as_obj().cloned();
        while let Some(p) = cur {
            let pv = Val::Obj(p.clone());
            match self.own_prop(&pv, key)? {
                Some((Err((_, s)), _)) => { if !s.is_callable() { return Ok(false); } self.call(&s, recv.clone(), vec![x])?; return Ok(true); }
                Some((Ok(_), a)) => { if a & A_NOWRITE != 0 { return Ok(false); } break; }
                None => {}
            }
            cur = self.proto_of_obj(&p.borrow());
        }
        let Val::Obj(r) = recv else { return Ok(false) };
        match self.own_prop(recv, key)? {
            Some((Err(_), _)) => Ok(false),
            Some((Ok(_), a)) => { if a & A_NOWRITE != 0 { return Ok(false); } self.set(recv, key, x)?; Ok(true) }
            None => { if r.borrow().flags & (F_NOEXT | F_SEALED | F_FROZEN) != 0 { return Ok(false); } Self::put(r, Rc::from(key), x); Ok(true) }
        }
    }
    fn proxy_set(&mut self, t: &Val, h: &Val, key: &str, x: Val, recv: &Val) -> Result<(), Ctl> {
        let trap = self.get(h, "set")?;
        if trap.is_callable() { let k = self.key_val(key); self.call(&trap, h.clone(), vec![t.clone(), k, x, recv.clone()])?; return Ok(()); }
        self.set(t, key, x)
    }

    /// CreateDataProperty on an object being built (literals, JSON, results): no setters, no checks.
    pub fn put(o: &ORef, key: Rc<str>, x: Val) {
        let mut b = o.borrow_mut();
        if let Kind::Array(a) = &mut b.kind { if let Some(i) = index_of(&key) { if i >= a.len() { a.resize(i + 1, Val::Undef); } a[i] = x; return; } }
        if let Some(p) = b.props.iter_mut().find(|(k, _)| *k == key) { p.1 = x; } else { b.props.push((key, x)); }
    }
    fn set_attr(b: &mut Obj, key: &str, a: u8) {
        if let Some(e) = b.attrs.iter_mut().find(|(k, _)| &**k == key) { e.1 = a; } else if a != 0 { b.attrs.push((Rc::from(key), a)); }
        if a & A_NOWRITE != 0 { b.flags |= F_RO; }
    }
    /// [[DefineOwnProperty]] for a data property with the given attributes.
    pub fn define(o: &ORef, key: &str, x: Val, attrs: u8) {
        let mut b = o.borrow_mut();
        if let Kind::Array(a) = &mut b.kind { if let Some(i) = index_of(key) { if i >= a.len() { a.resize(i + 1, Val::Undef); } a[i] = x; return; } }
        if let Some(p) = b.props.iter_mut().find(|(k, _)| &**k == key) { p.1 = x; } else { b.props.push((Rc::from(key), x)); }
        Self::set_attr(&mut b, key, attrs);
    }
    /// [[DefineOwnProperty]] for an accessor property; a missing getter/setter keeps the existing half.
    pub fn define_accessor(o: &ORef, key: &str, g: Option<Val>, s: Option<Val>, attrs: u8) {
        let mut b = o.borrow_mut();
        let pos = b.props.iter().position(|(k, _)| &**k == key);
        let (g0, s0) = pos.and_then(|p| accessor_of(&b.props[p].1)).unwrap_or((Val::Undef, Val::Undef));
        let acc = Val::obj(Kind::Accessor(g.unwrap_or(g0), s.unwrap_or(s0)));
        match pos { Some(p) => b.props[p].1 = acc, None => b.props.push((Rc::from(key), acc)) }
        b.flags |= F_ACC;
        Self::set_attr(&mut b, key, attrs);
    }
    /// The own property descriptor as (value | (getter, setter), attributes), or None.
    pub fn own_prop(&mut self, v: &Val, key: &str) -> Result<Option<(Result<Val, (Val, Val)>, u8)>, Ctl> {
        let Val::Obj(o) = v else { return Ok(match v { Val::Str(s) if key == "length" => Some((Ok(Val::Num(len16(s) as f64)), A_NOENUM | A_NOWRITE | A_NOCONF)), Val::Str(s) => index_of(key).filter(|&i| i < len16(s)).map(|i| (Ok(str_at(s, i)), A_NOWRITE | A_NOCONF)), _ => None }) };
        let r = {
            let b = o.borrow();
            let fl = b.flags;
            let frozen = if fl & F_FROZEN != 0 { A_NOWRITE | A_NOCONF } else if fl & F_SEALED != 0 { A_NOCONF } else { 0 };
            if let Some((_, x)) = b.props.iter().find(|(k, _)| &**k == key) {
                let a = attr_of(&b, key);
                Some(match accessor_of(x) { Some(gs) => (Err(gs), (a & !A_NOWRITE) | (frozen & A_NOCONF)), None => (Ok(x.clone()), a | frozen) })
            } else {
                match &b.kind {
                    Kind::Array(a) if key == "length" => Some((Ok(Val::Num(a.len() as f64)), A_NOENUM | A_NOCONF | (frozen & A_NOWRITE))),
                    Kind::Array(a) => index_of(key).filter(|&i| i < a.len()).map(|i| (Ok(a[i].clone()), frozen)),
                    Kind::Typed(t) => index_of(key).filter(|&i| i < t.len).map(|i| (Ok(crate::jsi_lib::typed_get(t, i)), A_NOCONF)),
                    Kind::Func(c) if key == "name" => Some((Ok(st(&c.name)), A_NOENUM | A_NOWRITE)),
                    Kind::Func(c) if key == "length" => Some((Ok(Val::Num(c.func.np_all as f64)), A_NOENUM | A_NOWRITE)),
                    Kind::Native(_, n) if key == "name" => Some((Ok(st(n)), A_NOENUM | A_NOWRITE)),
                    Kind::Prim(Val::Str(s)) if key == "length" => Some((Ok(Val::Num(len16(s) as f64)), A_NOENUM | A_NOWRITE | A_NOCONF)),
                    // String exotic objects' index properties (§10.4.3.5): enumerable, read-only
                    Kind::Prim(Val::Str(s)) => index_of(key).filter(|&i| i < len16(s)).map(|i| (Ok(str_at(s, i)), A_NOWRITE | A_NOCONF)),
                    Kind::Global => self.global.borrow().vars.get(key).map(|v| (Ok(v.clone()), 0)),
                    _ => None,
                }
            }
        };
        Ok(r)
    }
    pub fn has_own(&mut self, v: &Val, key: &str) -> Result<bool, Ctl> {
        if let Val::Obj(o) = v {
            let px = if let Kind::Proxy(t, h) = &o.borrow().kind { Some((t.clone(), h.clone())) } else { None };
            if let Some((t, h)) = px { let trap = self.get(&h, "getOwnPropertyDescriptor")?; if trap.is_callable() { let k = self.key_val(key); return Ok(!matches!(self.call(&trap, h, vec![t, k])?, Val::Undef)); } return self.has_own(&t, key); }
            let host = if let Kind::Host(i, r) = o.borrow().kind { Some((i, r)) } else { None };
            if self.own_prop(v, key)?.is_some() { return Ok(true); }
            if let Some((i, r)) = host { return Ok(!matches!(crate::jsdom::get(self, v, i, r, key)?, Val::Undef)); }
            return Ok(false);
        }
        Ok(self.own_prop(v, key)?.is_some())
    }

    /// HasProperty (§7.3.12) — the `in` operator.
    pub fn has_prop(&mut self, v: &Val, key: &str) -> Result<bool, Ctl> {
        let Val::Obj(o) = v else { let k = self.shown_key(key); let vs = self.to_str(v).map(|x| x.to_string()).unwrap_or_default(); type_err!(self, "Cannot use 'in' operator to search for '{}' in {}", k, vs) };
        enum H { Yes, Host(u32, u8), Proxy(Val, Val), Next(Option<ORef>) }
        let mut cur = o.clone();
        let mut first = true;
        loop {
            let h = {
                let b = cur.borrow();
                if b.props.iter().any(|(k, _)| &**k == key) { H::Yes } else {
                    match &b.kind {
                        Kind::Array(a) if key == "length" || index_of(key).map_or(false, |i| i < a.len()) => H::Yes,
                        Kind::Typed(t) if key == "length" || index_of(key).map_or(false, |i| i < t.len) => H::Yes,
                        Kind::Prim(Val::Str(s)) if key == "length" || index_of(key).map_or(false, |i| i < len16(s)) => H::Yes,
                        Kind::Func(_) | Kind::Native(..) if key == "name" || key == "length" => H::Yes,
                        Kind::Map(_) | Kind::Set(_) if key == "size" => H::Yes,
                        Kind::Global if self.global.borrow().vars.contains_key(key) => H::Yes,
                        Kind::Host(i, r) if first => H::Host(*i, *r),
                        Kind::Proxy(t, h) => H::Proxy(t.clone(), h.clone()),
                        _ => H::Next(self.proto_of_obj(&b)),
                    }
                }
            };
            match h {
                H::Yes => return Ok(true),
                H::Host(i, r) => { if !matches!(crate::jsdom::get(self, &Val::Obj(cur.clone()), i, r, key)?, Val::Undef) { return Ok(true); } let n = self.proto_of_obj(&cur.borrow()); match n { Some(p) => cur = p, None => return Ok(false) } }
                H::Proxy(t, hd) => { let trap = self.get(&hd, "has")?; if trap.is_callable() { let k = self.key_val(key); return Ok(self.call(&trap, hd, vec![t, k])?.truthy()); } return self.has_prop(&t, key); }
                H::Next(Some(p)) => cur = p,
                H::Next(None) => return Ok(false),
            }
            first = false;
        }
    }

    /// [[Delete]] (§10.1.10): non-configurable, sealed and frozen properties stay (false in sloppy mode).
    pub fn delete(&mut self, v: &Val, key: &str) -> Result<bool, Ctl> {
        let Val::Obj(o) = v else { return Ok(true) };
        let px = if let Kind::Proxy(t, h) = &o.borrow().kind { Some((t.clone(), h.clone())) } else { None };
        if let Some((t, h)) = px { let trap = self.get(&h, "deleteProperty")?; if trap.is_callable() { let k = self.key_val(key); return Ok(self.call(&trap, h, vec![t, k])?.truthy()); } return self.delete(&t, key); }
        let mut b = o.borrow_mut();
        let has = b.props.iter().any(|(k, _)| &**k == key);
        if b.flags & (F_SEALED | F_FROZEN) != 0 { return Ok(!has); }
        match &mut b.kind {
            Kind::Array(a) => { if let Some(i) = index_of(key) { if i < a.len() { a[i] = Val::Undef; } return Ok(true); } if key == "length" { return Ok(false); } }
            Kind::Global => { drop(b); self.global.borrow_mut().vars.remove(key); return Ok(true); }
            _ => {}
        }
        if attr_of(&b, key) & A_NOCONF != 0 { return Ok(false); }
        b.props.retain(|(k, _)| &**k != key);
        if !b.attrs.is_empty() { b.attrs.retain(|(k, _)| &**k != key); }
        Ok(true)
    }

    /// Own enumerable string keys in JS order: integer-like keys ascending, then insertion order (§10.1.11.1 OrdinaryOwnPropertyKeys).
    pub fn keys(&self, v: &Val) -> Vec<Rc<str>> { self.own_keys(v, true, false, false) }
    /// Own keys: strings and/or symbols, enumerable only unless `all`.
    pub fn own_keys(&self, v: &Val, strs: bool, syms: bool, all: bool) -> Vec<Rc<str>> {
        match v {
            Val::Obj(o) => {
                let b = o.borrow();
                match &b.kind {
                    Kind::Proxy(t, _) => { let t = t.clone(); drop(b); return self.own_keys(&t, strs, syms, all); }
                    Kind::Global => { let g = self.global.borrow(); return if strs { g.vars.keys().filter(|k| !k.starts_with('%') && &***k != "this").cloned().collect() } else { vec![] }; }
                    _ => {}
                }
                let mut ints: Vec<(u64, Rc<str>)> = Vec::new(); let mut rest: Vec<Rc<str>> = Vec::new(); let mut sy: Vec<Rc<str>> = Vec::new();
                if strs { match &b.kind {
                    Kind::Array(a) => for i in 0..a.len() { ints.push((i as u64, Rc::from(i.to_string().as_str()))); },
                    Kind::Typed(t) => for i in 0..t.len { ints.push((i as u64, Rc::from(i.to_string().as_str()))); },
                    Kind::Prim(Val::Str(s)) => for i in 0..len16(s) { ints.push((i as u64, Rc::from(i.to_string().as_str()))); },
                    _ => {}
                } }
                let many = b.props.len() > 16;
                let mut seen: std::collections::HashSet<Rc<str>> = std::collections::HashSet::new();
                for (k, _) in &b.props {
                    if k.starts_with('\u{0}') {
                        if syms && !k.starts_with("\u{0}#") && (all || attr_of(&b, k) & A_NOENUM == 0) && !sy.contains(k) { sy.push(k.clone()); }
                        continue;
                    }
                    if !strs || (!all && attr_of(&b, k) & A_NOENUM != 0) { continue; }
                    match index_of(k) { Some(i) => ints.push((i as u64, k.clone())), None => { if many { if seen.insert(k.clone()) { rest.push(k.clone()); } } else if !rest.contains(k) { rest.push(k.clone()); } } }
                }
                if all && strs { match &b.kind { Kind::Array(_) => rest.insert(0, Rc::from("length")), Kind::Func(_) | Kind::Native(..) => { for k in ["length", "name"] { if !rest.iter().any(|x| &**x == k) { rest.insert(0, Rc::from(k)); } } } _ => {} } }
                ints.sort_by_key(|x| x.0); ints.dedup_by_key(|x| x.0);
                ints.into_iter().map(|x| x.1).chain(rest).chain(sy).collect()
            }
            Val::Str(x) if strs => (0..len16(x)).map(|i| Rc::from(i.to_string().as_str())).collect(),
            _ => Vec::new(),
        }
    }
    /// for-in keys (§14.7.5.9 EnumerateObjectProperties): enumerable string keys along the prototype chain, shadowed ones once.
    pub fn for_in_keys(&self, v: &Val) -> Vec<Rc<str>> {
        let mut out = self.keys(v);
        let Val::Obj(o) = v else { return out };
        let mut seen: std::collections::HashSet<Rc<str>> = out.iter().cloned().collect();
        let mut cur = { let b = o.borrow(); if matches!(b.kind, Kind::Host(..) | Kind::Proxy(..)) { None } else { self.proto_of_obj(&b) } };
        while let Some(p) = cur {
            let pv = Val::Obj(p.clone());
            for k in self.own_keys(&pv, true, false, true) { if seen.insert(k.clone()) { if attr_of(&p.borrow(), &k) & A_NOENUM == 0 && !matches!(p.borrow().kind, Kind::Array(_)) { out.push(k); } } }
            cur = self.proto_of_obj(&p.borrow());
        }
        out
    }

    // ── calls (ECMA-262 §10.2 ECMAScript function objects) ──
    /// Entering script from the host: the outermost entry re-anchors the stack guard at the current frame.
    fn enter(&mut self) { if self.depth == 0 { let x = 0u8; self.stack_base = &x as *const u8 as usize; } self.depth += 1; }
    /// Leaving the outermost script or callback: perform a microtask checkpoint (HTML §8.1.4.4 / §8.1.7.3).
    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 && !self.jobs.is_empty() { self.run_jobs(); }
    }
    pub fn call(&mut self, f: &Val, this: Val, args: Vec<Val>) -> Result<Val, Ctl> {
        self.enter();
        let r = self.call_in(f, this, args);
        self.leave();
        r
    }
    fn call_in(&mut self, f: &Val, this: Val, args: Vec<Val>) -> Result<Val, Ctl> {
        let o = match f { Val::Obj(o) => o.clone(), _ => type_err!(self, "value is not a function") };
        enum C { Clo(Rc<Closure>), Nat(Native), Px(Val, Val), No }
        let c = { let b = o.borrow(); match &b.kind { Kind::Func(c) => C::Clo(c.clone()), Kind::Native(n, _) => C::Nat(n.clone()), Kind::Proxy(t, h) => C::Px(t.clone(), h.clone()), _ => C::No } };
        match c {
            C::Nat(n) => { if prof_on() { prof_enter(); let t = std::time::Instant::now(); let s0 = self.steps; let r = n(self, this, args); prof_leave(&format!("native:{}", match &o.borrow().kind { Kind::Native(_, nm) => *nm, _ => "?" }), t, self.steps.saturating_sub(s0)); r } else { n(self, this, args) } }
            C::Clo(c) => {
                if c.cls.is_some() && !c.func.method { type_err!(self, "Class constructor {} cannot be invoked without 'new'", c.name); }
                self.invoke(&c, f, this, args, None)
            }
            C::Px(t, h) => { let trap = self.get(&h, "apply")?; if trap.is_callable() { let a = Val::array(args); self.call(&trap, h, vec![t, this, a]) } else { self.call(&t, this, args) } }
            C::No => type_err!(self, "value is not a function"),
        }
    }

    fn stack_check(&mut self) -> Result<(), Ctl> {
        let x = 0u8; let sp = &x as *const u8 as usize;
        if self.stack_base == 0 { self.stack_base = sp; }
        if self.stack_base > sp && self.stack_base - sp > self.stack_max { return Err(self.err("RangeError", "Maximum call stack size exceeded")); }
        Ok(())
    }

    /// FunctionDeclarationInstantiation (§10.2.11): a fresh scope with this / arguments / home object / new.target and the parameters bound.
    fn activation(&mut self, c: &Rc<Closure>, fv: &Val, this: Val, args: &[Val], nt: Option<&Val>) -> Result<Scope, Ctl> {
        let func = &c.func;
        let fb = func.b();
        let mut fr = Frame::map(Some(c.scope.clone()));
        fr.slots = vec![None; fb.layout.names.len()]; fr.lay = Some(fb.layout.clone());
        if !func.arrow {
            // OrdinaryCallBindThis (§10.2.1.2): sloppy functions called without a receiver see the global object
            let this = if !func.strict && (func.uses_this || func.uses_super) && nt.is_none() && this.is_nullish() { let g = self.global.clone(); self.lookup_opt(&g, if self.dom.is_some() { "window" } else { "globalThis" }).unwrap_or(Val::Undef) } else { this };
            if func.uses_this || func.uses_super || nt.is_some() { fr.slots[A::SL_THIS] = Some(this); }
            if func.uses_args { fr.slots[A::SL_ARGS] = Some(Val::array(args.to_vec())); }
            if func.uses_super || c.cls.is_some() { if let Some(h) = &c.home { fr.slots[A::SL_HOME] = Some(h.clone()); } fr.slots[A::SL_FN] = Some(fv.clone()); }
            if func.uses_meta || c.cls.is_some() { fr.slots[A::SL_NT] = Some(nt.cloned().unwrap_or(Val::Undef)); }
        }
        let mut pat = false;
        for (i, p) in fb.params.iter().enumerate() {
            match p { Pat::Ident(_) => { fr.slots[fb.pslots[i] as usize] = Some(args.get(i).cloned().unwrap_or(Val::Undef)); } _ => { pat = true; } }
        }
        let scope = Rc::new(RefCell::new(fr));
        if pat || fb.rest.is_some() {
            // patterns bind in order after the simple parameters before them (defaults may read earlier parameters)
            for (i, p) in fb.params.iter().enumerate() { if !matches!(p, Pat::Ident(_)) { self.bind_pat(p, args.get(i).cloned().unwrap_or(Val::Undef), &scope)?; } }
            if let Some(r) = &fb.rest { self.bind_pat(r, Val::array(args.get(fb.params.len()..).map(|x| x.to_vec()).unwrap_or_default()), &scope)?; }
        }
        if !fb.vslots.is_empty() { let mut sb = scope.borrow_mut(); for &v in &fb.vslots { let x = &mut sb.slots[v as usize]; if x.is_none() { *x = Some(Val::Undef); } } }
        Ok(scope)
    }

    fn run_body(&mut self, func: &A::Func, scope: &Scope) -> Result<Val, Ctl> {
        if let Some(c) = crate::jsc::code_for(func.b()) { return (c.0)(self, scope); }
        match &func.b().body {
            Body::Block(b) => { let pr = if prof_on() { prof_enter(); Some(std::time::Instant::now()) } else { None }; self.hoist_fns(b, scope); if let Some(t) = pr { prof_leave("<hoist_fns>", t, 0); } match self.exec_block(b, scope) { Ok(()) => Ok(Val::Undef), Err(Ctl::Return(v)) => Ok(v), Err(e) => Err(e) } }
            Body::Expr(e) => self.ev(e, scope),
        }
    }

    fn invoke(&mut self, c: &Rc<Closure>, fv: &Val, this: Val, args: Vec<Val>, nt: Option<&Val>) -> Result<Val, Ctl> {
        // a function made by a module runs with that module as the active script (its import() / import.meta)
        match &c.module {
            Some(m) if self.cur_module.last().map(|x| x.as_str()) != Some(&**m) => {
                self.cur_module.push(m.to_string());
                let r = self.invoke_inner(c, fv, this, args, nt);
                self.cur_module.pop();
                r
            }
            _ => self.invoke_inner(c, fv, this, args, nt),
        }
    }
    fn invoke_inner(&mut self, c: &Rc<Closure>, fv: &Val, this: Val, args: Vec<Val>, nt: Option<&Val>) -> Result<Val, Ctl> {
        self.stack_check()?;
        let func = c.func.clone();
        if fnstat_on() { FN_CALLED.with(|h| { h.borrow_mut().insert(Rc::as_ptr(&func) as usize); }); }
        let tr = trace_on();
        let blame = blame_on();
        let prof_t = if prof_on() { prof_enter(); Some((std::time::Instant::now(), self.steps)) } else { None };
        if blame { BS.with(|b| b.borrow_mut().push(func.name.to_string())); }
        if tr {
            let d = TD.with(|x| { x.set(x.get() + 1); x.get() });
            if trace_want(&func.name) {
                let n = TN.with(|x| { let v = x.get(); x.set(v + 1); v });
                if n < trace_max() { eprintln!("{}jsi call {}", "  ".repeat(d as usize), func.name); } else if n == trace_max() { eprintln!("jsi call trace truncated"); }
            }
        }
        let this = if func.arrow { c.this.clone().unwrap_or(Val::Undef) } else { this };
        let r = (|| -> Result<Val, Ctl> {
            if func.is_gen {
                let scope = self.activation(c, fv, this, &args, nt)?;
                let co = Rc::new(RefCell::new(Coro { clo: c.clone(), scope, frames: Vec::new(), state: CO_START, kind: if func.is_async { 2 } else { 0 }, promise: None, queue: VecDeque::new(), busy: false, awaiting: false }));
                let g = Val::obj(Kind::Gen(co));
                let proto = self.get(fv, "prototype")?;
                if let (Val::Obj(go), Val::Obj(p)) = (&g, &proto) { go.borrow_mut().proto = Some(p.clone()); }
                return Ok(g);
            }
            if func.is_async {
                let scope = match self.activation(c, fv, this, &args, nt) { Ok(s) => s, Err(Ctl::Throw(e)) => return Ok(crate::jsi_lib::rejected(e)), Err(e) => return Err(e) };
                if !func.b().resumable {
                    return Ok(match self.run_body(&func, &scope) { Ok(v) => { let p = new_promise(); self.resolve_promise(&p, v); p } Err(Ctl::Throw(e)) => crate::jsi_lib::rejected(e), Err(e) => return Err(e) });
                }
                return Ok(self.async_start(c.clone(), scope));
            }
            let scope = self.activation(c, fv, this, &args, nt)?;
            self.run_body(&func, &scope)
        })();
        if tr { TD.with(|x| x.set(x.get() - 1)); }
        if blame { BS.with(|b| { b.borrow_mut().pop(); }); }
        if let Some((t, s0)) = prof_t { let d = self.steps.saturating_sub(s0); if func.name.is_empty() { prof_leave(&format!("<anon@{}>", func.lazy_addr().map(|x| x as i64).unwrap_or(-1)), t, d) } else { prof_leave(&func.name, t, d) } }
        r
    }

    pub fn construct(&mut self, f: &Val, args: Vec<Val>) -> Result<Val, Ctl> { let nt = f.clone(); self.enter(); let r = self.construct_nt(f, args, &nt); self.leave(); r }

    /// [[Construct]] (§10.2.2) with an explicit new.target: base constructors allocate `this` from newTarget.prototype,
    /// derived ones get it from super(); built-in (native) constructors are re-prototyped for subclasses.
    pub fn construct_nt(&mut self, f: &Val, args: Vec<Val>, nt: &Val) -> Result<Val, Ctl> {
        let o = match f { Val::Obj(o) => o.clone(), _ => { let s = self.to_str(f).map(|x| x.to_string()).unwrap_or_default(); type_err!(self, "{} is not a constructor", s) } };
        enum C { Clo(Rc<Closure>), Nat(Native), Px(Val, Val), No }
        let c = { let b = o.borrow(); match &b.kind { Kind::Func(c) => C::Clo(c.clone()), Kind::Native(n, _) => C::Nat(n.clone()), Kind::Proxy(t, h) => C::Px(t.clone(), h.clone()), _ => C::No } };
        match c {
            C::Nat(n) => {
                let prev = self.new_target.replace(nt.clone());
                let r = n(self, new_marker(), args);
                self.new_target = prev;
                let r = r?;
                let r = if matches!(r, Val::Obj(_)) { r } else { Val::obj(Kind::Plain) };
                let same = matches!((f, nt), (Val::Obj(a), Val::Obj(b)) if Rc::ptr_eq(a, b));
                // OrdinaryCreateFromConstructor (§10.1.13): a fresh plain object (or any object for a subclass) takes newTarget.prototype
                let fresh_plain = if let Val::Obj(ro) = &r { let b = ro.borrow(); matches!(b.kind, Kind::Plain) && b.proto.is_none() && b.flags & F_NULLPROTO == 0 } else { false };
                if !same || fresh_plain { if let Val::Obj(p) = self.get(nt, "prototype")? { if !Rc::ptr_eq(&p, &self.realm.object) { if let Val::Obj(ro) = &r { ro.borrow_mut().proto = Some(p); } } } }
                Ok(r)
            }
            C::Px(t, h) => { let trap = self.get(&h, "construct")?; if trap.is_callable() { let a = Val::array(args); self.call(&trap, h, vec![t, a, nt.clone()]) } else { let nt2 = if matches!((f, nt), (Val::Obj(a), Val::Obj(b)) if Rc::ptr_eq(a, b)) { t.clone() } else { nt.clone() }; self.construct_nt(&t, args, &nt2) } }
            C::No => {
                // a plain stand-in with a prototype object (see class_t) constructs an ordinary object
                if let Val::Obj(p) = self.get(f, "prototype")? { let t = Val::obj(Kind::Plain); let np = match self.get(nt, "prototype")? { Val::Obj(x) => x, _ => p }; if let Val::Obj(to) = &t { to.borrow_mut().proto = Some(np); } return Ok(t); }
                type_err!(self, "value is not a constructor")
            }
            C::Clo(c) => {
                if c.func.method || c.func.is_gen || c.func.is_async { type_err!(self, "{} is not a constructor", if c.name.is_empty() { "anonymous" } else { &c.name }); }
                self.stack_check()?;
                let derived = c.cls.as_ref().map_or(false, |k| k.derived);
                let this = if derived { Val::Undef } else {
                    let proto = self.get(nt, "prototype")?;
                    let t = Val::obj(Kind::Plain);
                    if let (Val::Obj(t), Val::Obj(p)) = (&t, &proto) { t.borrow_mut().proto = Some(p.clone()); }
                    t
                };
                let func = c.func.clone();
                let scope = self.activation(&c, f, this.clone(), &args, Some(nt))?;
                if !derived { if let Some(k) = &c.cls { self.init_fields(k, &this)?; } }
                let r = self.run_body(&func, &scope)?;
                if matches!(r, Val::Obj(_)) { return Ok(r); }
                let t = scope.borrow().get("this").cloned().unwrap_or(Val::Undef);
                if derived && t.is_nullish() { return Err(self.err("ReferenceError", "Must call super constructor in derived class before accessing 'this' or returning from derived constructor")); }
                Ok(t)
            }
        }
    }

    /// Instance fields (§7.3.34 InitializeInstanceElements): evaluated with `this` = the new object, in class scope.
    fn init_fields(&mut self, k: &Rc<ClassRt>, this: &Val) -> Result<(), Ctl> {
        let Val::Obj(t) = this else { return Ok(()) };
        for (key, mi) in &k.fields {
            let m = &k.def.members[*mi];
            let v = match &m.value {
                Some(e) => {
                    let fs = Self::child(&k.scope);
                    { let mut b = fs.borrow_mut(); b.put(&self.n_this, this.clone()); b.put(&self.n_home, k.home.clone()); }
                    let v = self.ev(e, &fs)?;
                    if is_anon_fn(e) { self.name_fn(&v, key); }
                    v
                }
                None => Val::Undef,
            };
            Self::define(t, key, v, 0);
        }
        Ok(())
    }

    /// SetFunctionName (§10.2.9) for anonymous function values bound to a key.
    pub(crate) fn name_fn(&self, v: &Val, key: &str) {
        if let Val::Obj(o) = v {
            let mut b = o.borrow_mut();
            if let Kind::Func(c) = &b.kind {
                if c.name.is_empty() {
                    let name = match self.syms.get(key) { Some(Val::Sym(s)) => format!("[{}]", s.desc.as_deref().unwrap_or("")), _ => key.trim_start_matches('\u{0}').to_string() };
                    let nc = Closure { func: c.func.clone(), scope: c.scope.clone(), this: c.this.clone(), name, home: c.home.clone(), cls: c.cls.clone(), module: c.module.clone() };
                    b.kind = Kind::Func(Rc::new(nc));
                }
            }
        }
    }

    pub fn make_fn_t(&self, f: &Rc<A::Func>, scope: &Scope, this: Option<Val>) -> Val { self.make_fn_x(f, scope, this, None, None) }
    fn make_fn_x(&self, f: &Rc<A::Func>, scope: &Scope, this: Option<Val>, home: Option<Val>, name: Option<&str>) -> Val {
        let v = Val::obj(Kind::Func(Rc::new(Closure { func: f.clone(), scope: scope.clone(), this, name: name.map(|n| n.to_string()).unwrap_or_else(|| f.name.to_string()), home, cls: None, module: self.cur_module.last().map(|m| Rc::from(m.as_str())) })));
        if f.is_gen {
            // each generator function has its own prototype object inheriting %GeneratorPrototype% (§27.3.4)
            let p = Val::obj_p(Kind::Plain, Some(if f.is_async { self.realm.async_gen.clone() } else { self.realm.generator.clone() }));
            if let Val::Obj(o) = &v { o.borrow_mut().props.push((Rc::from("prototype"), p)); o.borrow_mut().attrs.push((Rc::from("prototype"), A_NOENUM)); }
        } else if !f.method && !f.is_async {
            let p = Val::obj(Kind::Plain);
            if let (Val::Obj(o), Val::Obj(po)) = (&v, &p) { po.borrow_mut().props.push((Rc::from("constructor"), v.clone())); po.borrow_mut().attrs.push((Rc::from("constructor"), A_NOENUM)); o.borrow_mut().props.push((Rc::from("prototype"), p.clone())); o.borrow_mut().attrs.push((Rc::from("prototype"), A_NOENUM)); }
        }
        v
    }

    fn key(&mut self, k: &A::Key, scope: &Scope) -> Result<Rc<str>, Ctl> {
        match k { A::Key::Static(n) => Ok(n.clone()), A::Key::Computed(e) => { let v = self.ev(e, scope)?; self.to_key(&v) } }
    }

    /// ClassDefinitionEvaluation (§15.7.14).
    fn class_t(&mut self, c: &Rc<A::ClassDef>, scope: &Scope) -> Result<Val, Ctl> {
        let cscope = Self::child(scope);
        let (proto_parent, ctor_parent) = match &c.sup {
            None => (Some(self.realm.object.clone()), None),
            Some(e) => {
                let sv = self.ev(e, scope)?;
                match &sv {
                    Val::Null => (None, None),
                    // a page's plain stand-in constructor (jsdom's HTMLElement = { prototype }) is accepted as a base: `class X extends HTMLElement`
                    Val::Obj(so) => { let pp = self.get(&sv, "prototype")?; if !sv.is_callable() && !matches!(pp, Val::Obj(_)) { type_err!(self, "Class extends value is not a constructor or null"); } (match pp { Val::Obj(p) => Some(p), Val::Null => None, _ => type_err!(self, "Class extends value does not have valid prototype property") }, Some(so.clone())) }
                    _ => { let s = self.to_str(&sv).map(|x| x.to_string()).unwrap_or_default(); type_err!(self, "Class extends value {} is not a constructor or null", s) }
                }
            }
        };
        let proto = Val::obj_p(Kind::Plain, proto_parent);
        // computed keys are evaluated once, in order (§15.7.14 step 27)
        let mut keys = Vec::with_capacity(c.members.len());
        for m in &c.members { keys.push(match &m.key { A::Key::Static(n) => n.clone(), A::Key::Computed(e) => { let v = self.ev(e, &cscope)?; self.to_key(&v)? } }); }
        let fields: Vec<(Rc<str>, usize)> = c.members.iter().enumerate().filter(|(_, m)| m.kind == A::MKind::Field && !m.is_static).map(|(i, _)| (keys[i].clone(), i)).collect();
        let rt = Rc::new(ClassRt { def: c.clone(), scope: cscope.clone(), derived: c.sup.is_some(), fields, home: proto.clone() });
        let f = Val::obj(Kind::Func(Rc::new(Closure { func: c.ctor.clone(), scope: cscope.clone(), this: None, name: c.name.to_string(), home: Some(proto.clone()), cls: Some(rt), module: self.cur_module.last().map(|m| Rc::from(m.as_str())) })));
        let (Val::Obj(fo), Val::Obj(po)) = (&f, &proto) else { unreachable!() };
        { let mut b = fo.borrow_mut(); b.flags |= F_CLASS; if let Some(cp) = ctor_parent { b.proto = Some(cp); } }
        Self::define(fo, "prototype", proto.clone(), A_NOENUM | A_NOWRITE | A_NOCONF);
        Self::define(po, "constructor", f.clone(), A_NOENUM);
        if !c.name.is_empty() { Self::declare(&cscope, &c.name, f.clone()); }
        for (i, m) in c.members.iter().enumerate() {
            let target = if m.is_static { fo } else { po };
            let home = if m.is_static { f.clone() } else { proto.clone() };
            match m.kind {
                A::MKind::Method | A::MKind::Get | A::MKind::Set => {
                    let func = m.func.as_ref().unwrap();
                    let nm = match self.syms.get(&*keys[i]) { Some(Val::Sym(s)) => format!("[{}]", s.desc.as_deref().unwrap_or("")), _ => keys[i].trim_start_matches('\u{0}').to_string() };
                    let fv = self.make_fn_x(func, &cscope, None, Some(home), Some(&nm));
                    match m.kind { A::MKind::Method => Self::define(target, &keys[i], fv, A_NOENUM), A::MKind::Get => Self::define_accessor(target, &keys[i], Some(fv), None, A_NOENUM), _ => Self::define_accessor(target, &keys[i], None, Some(fv), A_NOENUM) }
                }
                _ => {}
            }
        }
        // static fields and blocks run in order with this = the class (§15.7.14 step 31)
        for (i, m) in c.members.iter().enumerate() {
            if !m.is_static || !matches!(m.kind, A::MKind::Field | A::MKind::StaticBlock) { continue; }
            let fs = Self::child(&cscope);
            { let mut b = fs.borrow_mut(); b.put(&self.n_this, f.clone()); b.put(&self.n_home, f.clone()); }
            if m.kind == A::MKind::StaticBlock { self.hoist_fns(&m.block, &fs); self.exec_block(&m.block, &fs)?; continue; }
            let v = match &m.value { Some(e) => { let v = self.ev(e, &fs)?; if is_anon_fn(e) { self.name_fn(&v, &keys[i]); } v } None => Val::Undef };
            Self::define(fo, &keys[i], v, 0);
        }
        Ok(f)
    }

    /// `super(...args)` (§13.3.7.1): construct the parent with this new.target, bind `this`, then run this class's field initializers.
    fn super_call(&mut self, args: Vec<Val>, scope: &Scope) -> Result<Val, Ctl> {
        let fv = self.lookup_opt(scope, "%fn").unwrap_or(Val::Undef);
        let nt = self.lookup_opt(scope, "%nt").unwrap_or(Val::Undef);
        let Val::Obj(fo) = &fv else { return Err(self.err("SyntaxError", "'super' keyword unexpected here")) };
        let parent = self.proto_of_obj(&fo.borrow());
        let parent = match parent { Some(p) => Val::Obj(p), _ => type_err!(self, "Super constructor is not a constructor") };
        let nt = if nt.is_nullish() { fv.clone() } else { nt };
        let r = self.construct_nt(&parent, args, &nt)?;
        self.assign_var(scope, "this", r.clone())?;
        let cls = if let Kind::Func(c) = &fo.borrow().kind { c.cls.clone() } else { None };
        if let Some(k) = cls { self.init_fields(&k, &r)?; }
        Ok(Val::Undef)
    }
    fn super_base(&mut self, scope: &Scope) -> Result<Val, Ctl> {
        let home = self.lookup_opt(scope, "%home").unwrap_or(Val::Undef);
        match &home { Val::Obj(h) => Ok(self.proto_of_obj(&h.borrow()).map(Val::Obj).unwrap_or(Val::Null)), _ => Err(self.err("SyntaxError", "'super' keyword unexpected here")) }
    }
    /// `super.k = v` (§13.3.7.3 MakeSuperPropertyReference + PutValue): a setter up the home object's chain runs with
    /// this = the receiver; otherwise the receiver gets an own data property.
    fn super_set(&mut self, k: &A::Key, v: Val, scope: &Scope) -> Result<(), Ctl> {
        let base = self.super_base(scope)?;
        let key = self.key(k, scope)?;
        let this = self.lookup_opt(scope, "this").unwrap_or(Val::Undef);
        let mut cur = base.as_obj().cloned();
        while let Some(p) = cur {
            let found = { let b = p.borrow(); b.props.iter().find(|(x, _)| **x == *key).map(|(_, x)| accessor_of(x)) };
            match found { Some(Some((_, s))) => { if s.is_callable() { self.call(&s, this, vec![v])?; } return Ok(()); } Some(None) => break, None => {} }
            cur = self.proto_of_obj(&p.borrow());
        }
        if let Val::Obj(t) = &this { Self::put(t, key, v); }
        Ok(())
    }
    fn super_get(&mut self, k: &A::Key, scope: &Scope) -> Result<(Val, Val), Ctl> {
        let base = self.super_base(scope)?;
        let key = self.key(k, scope)?;
        let this = self.lookup_opt(scope, "this").unwrap_or(Val::Undef);
        let v = match &base { Val::Obj(b) => { let b = b.clone(); self.get_obj(&b, &key, &this)? } _ => Val::Undef };
        Ok((v, this))
    }

    /// `a instanceof C` (§13.10.2 InstanceofOperator): C[@@hasInstance] first, else OrdinaryHasInstance.
    pub fn instance_of(&mut self, a: &Val, c: &Val) -> Result<bool, Ctl> {
        if !matches!(c, Val::Obj(_)) { type_err!(self, "Right-hand side of 'instanceof' is not callable"); }
        let h = self.get(c, K_HAS_INSTANCE)?;
        let default = match &h { Val::Obj(o) => matches!(&o.borrow().kind, Kind::Native(_, "[Symbol.hasInstance]")), _ => true };
        if !default && h.is_callable() { return Ok(self.call(&h, c.clone(), vec![a.clone()])?.truthy()); }
        if !c.is_callable() {
            // page stand-ins (jsdom's plain Node/Element objects) answer through their prototype property
            if let Val::Obj(p) = self.get(c, "prototype")? { let mut cur = self.proto_of(a).ok().flatten(); while let Some(x) = cur { if Rc::ptr_eq(&x, &p) { return Ok(true); } cur = self.proto_of(&Val::Obj(x))?; } return Ok(false); }
            type_err!(self, "Right-hand side of 'instanceof' is not callable");
        }
        self.ordinary_has_instance(c, a)
    }
    /// §7.3.21 OrdinaryHasInstance.
    pub fn ordinary_has_instance(&mut self, c: &Val, a: &Val) -> Result<bool, Ctl> {
        let Val::Obj(_) = a else { return Ok(false) };
        let p = self.get(c, "prototype")?;
        // a host constructor without a prototype object (the page layer's Event / KeyboardEvent natives) answers false
        // rather than throwing, as `x instanceof Event` must not take a script down
        let Val::Obj(p) = p else { if matches!(c, Val::Obj(o) if matches!(o.borrow().kind, Kind::Native(..))) { return Ok(false); } type_err!(self, "Function has non-object prototype in instanceof check") };
        let mut cur = self.proto_of(a)?;
        while let Some(x) = cur { if Rc::ptr_eq(&x, &p) { return Ok(true); } cur = self.proto_of(&Val::Obj(x))?; }
        Ok(false)
    }

    // ── patterns (§8.6.2 BindingInitialization, §13.15.5 DestructuringAssignmentEvaluation) ──
    pub fn bind_pat(&mut self, p: &Pat, v: Val, scope: &Scope) -> Result<(), Ctl> {
        match p {
            Pat::Ident(n) => { scope.borrow_mut().put(n, v); Ok(()) }
            Pat::Default(p, d) => { let v = if matches!(v, Val::Undef) { let x = self.ev(d, scope)?; if let (Pat::Ident(n), true) = (&**p, is_anon_fn(d)) { self.name_fn(&x, n); } x } else { v }; self.bind_pat(p, v, scope) }
            Pat::Array(els, rest) => {
                if v.is_nullish() { let s = self.to_str(&v)?; type_err!(self, "{} is not iterable", s); }
                let mut it = self.iter_open(&v)?;
                let mut done = false;
                for el in els.iter() {
                    let x = if done { Val::Undef } else { match self.iter_step(&mut it)? { Some(x) => x, None => { done = true; Val::Undef } } };
                    if let Some(el) = el { self.bind_pat(el, x, scope)?; }
                }
                if let Some(r) = rest {
                    let mut out = Vec::new();
                    if !done { while let Some(x) = self.iter_step(&mut it)? { out.push(x); } }
                    self.bind_pat(r, Val::array(out), scope)?;
                } else if !done { self.iter_close(&it)?; }
                Ok(())
            }
            Pat::Object(props, rest) => {
                if v.is_nullish() { let s = self.to_str(&v)?; type_err!(self, "Cannot destructure '{}' as it is {}.", s, s); }
                let mut used = Vec::new();
                for (k, pp) in props { let k = self.key(k, scope)?; let x = self.get(&v, &k)?; used.push(k); self.bind_pat(pp, x, scope)?; }
                if let Some(r) = rest {
                    let ro = Val::obj(Kind::Plain);
                    let Val::Obj(rob) = &ro else { unreachable!() };
                    for k in self.own_keys(&v, true, true, false) { if !used.contains(&k) { let x = self.get(&v, &k)?; Self::put(rob, k, x); } }
                    self.bind_pat(r, ro, scope)?;
                }
                Ok(())
            }
            Pat::Member(o, k) => { let ov = self.ev(o, scope)?; let k = self.key(k, scope)?; self.set(&ov, &k, v) }
        }
    }

    /// Assignment to an existing binding (not a declaration): identifiers resolve outward.
    pub(crate) fn assign_pat(&mut self, p: &Pat, v: Val, scope: &Scope) -> Result<(), Ctl> {
        match p {
            Pat::Ident(n) => self.assign_var(scope, n, v),
            Pat::Member(..) => self.bind_pat(p, v, scope),
            _ => { let sc = Self::child(scope); self.bind_pat(p, v, &sc)?; let vars: Vec<(Rc<str>, Val)> = sc.borrow().vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(); for (k, x) in vars { self.assign_var(scope, &k, x)?; } Ok(()) }
        }
    }

    fn literal_t(&self, l: &A::Lit) -> Val {
        match l { A::Lit::Undef => Val::Undef, A::Lit::Null => Val::Null, A::Lit::Bool(b) => Val::Bool(*b), A::Lit::Num(n) => Val::Num(*n), A::Lit::Str(s) => Val::Str(s.clone()),
            A::Lit::Big(d) => Val::Big(Rc::new(crate::jsi_lib::Big::parse(d).unwrap_or_default())),
            A::Lit::Regex(p, f) => {
                let o = Val::obj(Kind::RegExp(p.clone(), f.clone()));
                if let Val::Obj(r) = &o { r.borrow_mut().props = vec![(Rc::from("lastIndex"), Val::Num(0.0))]; r.borrow_mut().attrs = vec![(Rc::from("lastIndex"), A_NOENUM)]; }
                o
            } }
    }

    // ── iteration (§7.4) ──
    /// Values of an iterable, collected (spread, Array.from, Promise.all, …). Arrays, strings, Map, Set and typed arrays
    /// take the direct path; everything else goes through @@iterator.
    pub fn iterate(&mut self, v: &Val) -> Result<Vec<Val>, Ctl> {
        match v {
            Val::Str(x) => return Ok(x.chars().map(|c| { let mut b = [0u8; 4]; st(c.encode_utf8(&mut b)) }).collect()),
            Val::Obj(o) => {
                let b = o.borrow();
                match &b.kind {
                    Kind::Array(a) => return Ok(a.clone()),
                    Kind::Set(m) => return Ok(m.entries().map(|e| e.0.clone()).collect()),
                    Kind::Map(m) => return Ok(m.entries().map(|(k, v)| Val::array(vec![k.clone(), v.clone()])).collect()),
                    Kind::Typed(t) => return Ok((0..t.len).map(|i| crate::jsi_lib::typed_get(t, i)).collect()),
                    Kind::Prim(Val::Str(x)) => { let x = x.clone(); drop(b); return self.iterate(&Val::Str(x)); }
                    _ => {}
                }
            }
            _ => {}
        }
        let mut it = self.iter_open(v)?;
        let mut out = Vec::new();
        while let Some(x) = self.iter_step(&mut it)? { out.push(x); self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); } }
        Ok(out)
    }

    /// GetIterator (§7.4.2).
    pub fn iter_open(&mut self, v: &Val) -> Result<IterRec, Ctl> {
        match v {
            Val::Str(x) => return Ok(IterRec::List(x.chars().map(|c| { let mut b = [0u8; 4]; st(c.encode_utf8(&mut b)) }).collect(), 0)),
            Val::Obj(o) => {
                let b = o.borrow();
                let own_iter = b.props.iter().any(|(k, _)| &**k == K_ITER);
                if !own_iter {
                    match &b.kind {
                        Kind::Array(_) => return Ok(IterRec::Arr(o.clone(), 0)),
                        Kind::Gen(c) if b.proto.as_ref().map_or(true, |p| !p.borrow().props.iter().any(|(k, _)| &**k == K_ITER)) => return Ok(IterRec::Gen(c.clone())),
                        Kind::Iter(_) => return Ok(IterRec::Bi(o.clone())),
                        Kind::Map(_) => { drop(b); let i = Val::obj(Kind::Iter(Box::new(IterSt::Map(v.clone(), 0, 2)))); return Ok(IterRec::Bi(i.as_obj().unwrap().clone())); }
                        Kind::Set(_) => { drop(b); let i = Val::obj(Kind::Iter(Box::new(IterSt::Map(v.clone(), 0, 1)))); return Ok(IterRec::Bi(i.as_obj().unwrap().clone())); }
                        Kind::Typed(t) => return Ok(IterRec::List((0..t.len).map(|i| crate::jsi_lib::typed_get(t, i)).collect(), 0)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let f = self.get(v, K_ITER)?;
        if !f.is_callable() { let s = if v.is_nullish() { self.to_str(v)?.to_string() } else { v.type_of().to_string() }; type_err!(self, "{} is not iterable", if matches!(v, Val::Obj(_)) { "object" } else { &s }); }
        let it = self.call(&f, v.clone(), vec![])?;
        if !matches!(it, Val::Obj(_)) { type_err!(self, "Result of the Symbol.iterator method is not an object"); }
        if let Val::Obj(io) = &it { let b = io.borrow(); match &b.kind { Kind::Gen(c) if b.props.is_empty() => return Ok(IterRec::Gen(c.clone())), Kind::Iter(_) => return Ok(IterRec::Bi(io.clone())), _ => {} } }
        let next = self.get(&it, "next")?;
        Ok(IterRec::Proto(it, next))
    }
    /// IteratorStep + IteratorValue (§7.4.8).
    pub fn iter_step(&mut self, r: &mut IterRec) -> Result<Option<Val>, Ctl> {
        match r {
            IterRec::Arr(o, i) => { let b = o.borrow(); if let Kind::Array(a) = &b.kind { if *i < a.len() { let x = a[*i].clone(); *i += 1; return Ok(Some(x)); } } Ok(None) }
            IterRec::List(v, i) => { if *i < v.len() { *i += 1; Ok(Some(v[*i - 1].clone())) } else { Ok(None) } }
            IterRec::Gen(c) => { let c = c.clone(); let (x, done) = self.gen_resume(&c, Resume::Next(Val::Undef))?; Ok(if done { None } else { Some(x) }) }
            IterRec::Bi(o) => Ok(self.builtin_next(o)),
            IterRec::Proto(..) | IterRec::Async(..) => {
                let is_async = matches!(r, IterRec::Async(..));
                let (it, next) = match r { IterRec::Proto(a, b) | IterRec::Async(a, b) => (a.clone(), b.clone()), _ => unreachable!() };
                let res = self.call(&next, it, vec![])?;
                let res = if is_async { self.await_val(res)? } else { res };
                if !matches!(res, Val::Obj(_)) { let rs = self.to_str(&res).map(|x| x.to_string()).unwrap_or_default(); type_err!(self, "Iterator result {} is not an object", rs); }
                if self.get(&res, "done")?.truthy() { return Ok(None); }
                Ok(Some(self.get(&res, "value")?))
            }
        }
    }
    /// IteratorClose (§7.4.11): an early exit tells the iterator (generators run their finally blocks).
    pub fn iter_close(&mut self, r: &IterRec) -> Result<(), Ctl> {
        match r {
            IterRec::Gen(c) => { let c = c.clone(); self.gen_resume(&c, Resume::Return(Val::Undef))?; }
            IterRec::Proto(it, _) | IterRec::Async(it, _) => { let f = self.get(it, "return")?; if f.is_callable() { self.call(&f, it.clone(), vec![])?; } }
            _ => {}
        }
        Ok(())
    }
    /// %ArrayIteratorPrototype%.next and friends: one step of a built-in iterator.
    pub fn builtin_next(&mut self, o: &ORef) -> Option<Val> {
        let mut b = o.borrow_mut();
        let Kind::Iter(stt) = &mut b.kind else { return None };
        let r = match &mut **stt {
            IterSt::Arr(src, i, mode) => {
                let (len, item) = match src { Val::Obj(s) => { let sb = s.borrow(); match &sb.kind { Kind::Array(a) => (a.len(), a.get(*i).cloned()), Kind::Typed(t) => (t.len, if *i < t.len { Some(crate::jsi_lib::typed_get(t, *i)) } else { None }), _ => (0, None) } } Val::Str(s) => (len16(s), Some(str_at(s, *i))), _ => (0, None) };
                if *i >= len { None } else { let k = *i; *i += 1; Some(match mode { 0 => Val::Num(k as f64), 1 => item.unwrap_or(Val::Undef), _ => Val::array(vec![Val::Num(k as f64), item.unwrap_or(Val::Undef)]) }) }
            }
            IterSt::Map(src, i, mode) => {
                let Val::Obj(s) = src else { return None };
                let sb = s.borrow();
                let m = match &sb.kind { Kind::Map(m) | Kind::Set(m) => m, _ => return None };
                let is_set = matches!(sb.kind, Kind::Set(_));
                let mut out = None;
                while *i < m.ents.len() { let e = &m.ents[*i]; *i += 1; if let Some((k, v)) = e { out = Some(match mode { 0 => k.clone(), 1 => if is_set { k.clone() } else { v.clone() }, _ => Val::array(vec![k.clone(), if is_set { k.clone() } else { v.clone() }]) }); break; } }
                out
            }
            IterSt::Str(s, pos) => { let rest = &s[*pos..]; match rest.chars().next() { Some(c) => { *pos += c.len_utf8(); let mut bb = [0u8; 4]; Some(st(c.encode_utf8(&mut bb))) } None => None } }
            IterSt::List(v, i) => { if *i < v.len() { *i += 1; Some(v[*i - 1].clone()) } else { None } }
            IterSt::Done => None,
        };
        if r.is_none() { **stt = IterSt::Done; }
        r
    }
    pub fn iter_result(v: Val, done: bool) -> Val { Val::plain(vec![("value", v), ("done", Val::Bool(done))]) }

    // ── statements ──
    fn hoist_t(&mut self, body: &[Stmt], scope: &Scope) {
        for st in body {
            let st = match st { Stmt::ExportDecl(d, _) | Stmt::ExportDefaultDecl(d, _) => &**d, other => other };
            match st {
                Stmt::Func(n, f) => { let v = self.make_fn_t(f, scope, None); scope.borrow_mut().put(n, v); }
                Stmt::Var(A::VarKind::Var, decls) => for (p, _) in decls { if let Pat::Ident(n) = p { if !scope.borrow().has(n) { scope.borrow_mut().put(n, Val::Undef); } } },
                _ => {}
            }
        }
    }
    /// Function declarations of one statement list (block-level hoisting).
    pub(crate) fn hoist_fns(&mut self, body: &[Stmt], scope: &Scope) {
        for st in body {
            let st = match st { Stmt::ExportDecl(d, _) | Stmt::ExportDefaultDecl(d, _) => &**d, Stmt::Y(d) => &**d, other => other };
            if let Stmt::Func(n, f) = st { let v = self.make_fn_t(f, scope, None); scope.borrow_mut().put(n, v); }
        }
    }
    /// Script / module top level: functions plus every `var` anywhere in the body (§16.1.7 GlobalDeclarationInstantiation).
    fn hoist_program(&mut self, body: &[Stmt], scope: &Scope) {
        self.hoist_fns(body, scope);
        let mut vars = Vec::new(); A::collect_vars(body, &mut vars);
        let mut sb = scope.borrow_mut();
        for v in vars { if !sb.has(&v) { sb.put(&v, Val::Undef); } }
    }

    fn exec_block(&mut self, body: &[Stmt], scope: &Scope) -> Result<(), Ctl> { for st in body { self.ex(st, scope)?; } Ok(()) }

    pub(crate) fn ex(&mut self, n: &Stmt, scope: &Scope) -> Result<(), Ctl> {
        self.steps += 1;
        if self.steps > self.limit { throw!(self, "step limit"); }
        match n {
            Stmt::Expr(e) => { self.ev(e, scope)?; Ok(()) }
            Stmt::Var(kind, decls) => {
                for (p, init) in decls {
                    if matches!(kind, A::VarKind::Var) { if let Pat::Ident(nm) = p { if let Some(e) = init { let v = self.ev(e, scope)?; if is_anon_fn(e) { self.name_fn(&v, nm); } self.assign_var(scope, nm, v)?; } continue; } }
                    let v = match init { Some(e) => { let v = self.ev(e, scope)?; if let (Pat::Ident(nm), true) = (p, is_anon_fn(e)) { self.name_fn(&v, nm); } v } None => Val::Undef };
                    if matches!(kind, A::VarKind::Var) { self.assign_pat(p, v, scope)?; } else { self.bind_pat(p, v, scope)?; }
                }
                Ok(())
            }
            Stmt::Func(..) => Ok(()),
            Stmt::Class(nm, c) => { let v = self.class_t(c, scope)?; scope.borrow_mut().put(nm, v); Ok(()) }
            Stmt::Return(e) => Err(Ctl::Return(match e { Some(e) => self.ev(e, scope)?, None => Val::Undef })),
            Stmt::If(t, c, a) => { if self.ev(t, scope)?.truthy() { self.ex(c, scope) } else if let Some(a) = a { self.ex(a, scope) } else { Ok(()) } }
            Stmt::Block(b, scoped, lay) => { if *scoped { let sc = Self::child_l(scope, lay); self.hoist_fns(b, &sc); self.exec_block(b, &sc) } else { self.exec_block(b, scope) } }
            Stmt::For { .. } | Stmt::ForOf { .. } | Stmt::While(..) | Stmt::DoWhile(..) => self.ex_loop(n, scope, &[]),
            Stmt::Break(l) => Err(Ctl::Break(l.clone())),
            Stmt::Continue(l) => Err(Ctl::Continue(l.clone())),
            Stmt::Throw(e) => { let v = self.ev(e, scope)?; Err(Ctl::Throw(v)) }
            Stmt::Try { block, param, handler, finalizer, lays } => {
                let sc = Self::child_l(scope, &lays[0]);
                self.hoist_fns(block, &sc);
                let r = self.exec_block(block, &sc);
                let r = match r {
                    Err(Ctl::Throw(v)) if handler.is_some() => { let hs = Self::child_l(scope, &lays[1]); if let Some(p) = param { self.bind_pat(p, v, &hs)?; } let h = handler.as_ref().unwrap(); self.hoist_fns(h, &hs); self.exec_block(h, &hs) }
                    other => other,
                };
                if let Some(f) = finalizer { let fs = Self::child_l(scope, &lays[2]); self.hoist_fns(f, &fs); self.exec_block(f, &fs)?; }
                r
            }
            Stmt::Switch(d, cases, lay) => {
                let dv = self.ev(d, scope)?;
                let sc = Self::child_l(scope, lay);
                for (_, b) in cases { self.hoist_fns(b, &sc); }
                let mut start = None;
                for (i, (t, _)) in cases.iter().enumerate() { if let Some(t) = t { let tv = self.ev(t, &sc)?; if Self::strict_eq(&dv, &tv) { start = Some(i); break; } } }
                if start.is_none() { start = cases.iter().position(|c| c.0.is_none()); }
                if let Some(i0) = start { for (_, body) in &cases[i0..] { match self.exec_block(body, &sc) { Ok(()) => {}, Err(Ctl::Break(None)) => break, Err(e) => return Err(e) } } }
                Ok(())
            }
            Stmt::Labeled(l, b) => {
                let mut labels = vec![l.clone()]; let mut body = &**b;
                while let Stmt::Labeled(l2, b2) = body { labels.push(l2.clone()); body = b2; }
                let r = match body { Stmt::For { .. } | Stmt::ForOf { .. } | Stmt::While(..) | Stmt::DoWhile(..) => self.ex_loop(body, scope, &labels), _ => self.ex(body, scope) };
                match r { Err(Ctl::Break(Some(x))) if labels.contains(&x) => Ok(()), r => r }
            }
            Stmt::Empty => Ok(()),
            Stmt::With(o, b) => {
                let ov = self.ev(o, scope)?;
                if ov.is_nullish() { type_err!(self, "Cannot convert undefined or null to object"); }
                let ws = Rc::new(RefCell::new(Frame { with: Some(ov), ..Frame::map(Some(scope.clone())) }));
                self.ex(b, &ws)
            }
            Stmt::Y(s) => self.ex(s, scope),
            Stmt::Import(src, specs) => {
                let m = self.import(src)?;
                for (k, local) in specs {
                    let v = match k { A::ImportKind::Default => self.get(&m, "default")?, A::ImportKind::Namespace => m.clone(), A::ImportKind::Named(n) => self.get(&m, n)? };
                    scope.borrow_mut().put(local, v);
                }
                Ok(())
            }
            Stmt::ExportDecl(d, names) => {
                self.ex(d, scope)?;
                if let Some(e) = self.exports.last().cloned() { for nm in names { let v = self.lookup(scope, nm)?; self.set(&e, nm, v)?; } }
                Ok(())
            }
            Stmt::ExportDefault(e) => { let v = self.ev(e, scope)?; if let Some(ex) = self.exports.last().cloned() { self.set(&ex, "default", v)?; } Ok(()) }
            Stmt::ExportDefaultDecl(d, name) => { self.ex(d, scope)?; if let (Some(ex), Some(n)) = (self.exports.last().cloned(), name) { let v = self.lookup(scope, n)?; self.set(&ex, "default", v)?; } Ok(()) }
            Stmt::ExportNames(src, specs) => {
                let m = match src { Some(s) => Some(self.import(s)?), None => None };
                if let Some(ex) = self.exports.last().cloned() { for (local, exported) in specs { let v = match &m { Some(m) => if &**local == "*" { m.clone() } else { self.get(m, local)? }, None => self.lookup(scope, local)? }; self.set(&ex, exported, v)?; } }
                Ok(())
            }
            Stmt::ExportAll(src) => { let m = self.import(src)?; if let Some(e) = self.exports.last().cloned() { for k in self.keys(&m) { if &*k != "default" { let v = self.get(&m, &k)?; self.set(&e, &k, v)?; } } } Ok(()) }
            Stmt::Unsupported(k) => Err(self.err("SyntaxError", &format!("unsupported statement {k}"))),
        }
    }

    /// Loops with their label set (§14.7 IterationStatement, §14.13 LabelledEvaluation).
    fn ex_loop(&mut self, n: &Stmt, scope: &Scope, labels: &[Rc<str>]) -> Result<(), Ctl> {
        match n {
            Stmt::For { init, test, update, body, per_iter, lay } => {
                let sc = Self::child_l(scope, lay);
                if let Some(i) = init { self.ex(i, &sc)?; }
                loop {
                    if let Some(t) = test { if !self.ev(t, &sc)?.truthy() { break; } }
                    if *per_iter {
                        // closures in the body capture this iteration's bindings (§14.7.4.4 CreatePerIterationEnvironment)
                        let it = Self::child(&sc);
                        { let s = sc.borrow(); it.borrow_mut().copy_bindings(&s); }
                        match self.ex(body, &it) { Ok(()) => {}, Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) }
                        { let s = it.borrow(); sc.borrow_mut().copy_bindings(&s); }
                    } else {
                        match self.ex(body, &sc) { Ok(()) => {}, Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) }
                    }
                    if let Some(u) = update { self.ev(u, &sc)?; }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::ForOf { decl, pat, right, body, of, is_await, lay } => {
                let src = self.ev(right, scope)?;
                if !*of {
                    if src.is_nullish() { return Ok(()); }
                    for k in self.for_in_keys(&src) {
                        let it = Self::child_l(scope, lay);
                        if *decl == 2 { self.bind_pat(pat, Val::Str(k), &it)?; } else { self.assign_pat(pat, Val::Str(k), scope)?; }
                        match self.ex(body, &it) { Ok(()) => {}, Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) }
                    }
                    return Ok(());
                }
                let mut iter = if *is_await { self.async_iter_open(&src)? } else { self.iter_open(&src)? };
                loop {
                    let x = match self.iter_step(&mut iter)? { Some(x) => x, None => break };
                    let x = if *is_await { self.await_val(x)? } else { x };
                    let it = Self::child_l(scope, lay);
                    if *decl == 2 { self.bind_pat(pat, x, &it)?; } else { self.assign_pat(pat, x, scope)?; }
                    match self.ex(body, &it) {
                        Ok(()) => {}, Err(e) if cont(&e, labels) => {},
                        Err(e) if brk(&e, labels) => { self.iter_close(&iter)?; break; }
                        Err(Ctl::Throw(v)) => { let _ = self.iter_close(&iter); return Err(Ctl::Throw(v)); }
                        Err(e) => { self.iter_close(&iter)?; return Err(e); }
                    }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::While(t, b) => {
                while self.ev(t, scope)?.truthy() {
                    match self.ex(b, scope) { Ok(()) => {}, Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::DoWhile(b, t) => {
                loop {
                    match self.ex(b, scope) { Ok(()) => {}, Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) }
                    if !self.ev(t, scope)?.truthy() { break; }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::Y(s) => self.ex_loop(s, scope, labels),
            other => self.ex(other, scope),
        }
    }

    // ── expressions ──
    fn args_t(&mut self, a: &[Expr], spread: &[bool], scope: &Scope) -> Result<Vec<Val>, Ctl> {
        let mut out = Vec::with_capacity(a.len());
        for (i, x) in a.iter().enumerate() { let v = self.ev(x, scope)?; if spread.get(i).copied().unwrap_or(false) { out.extend(self.iterate(&v)?); } else { out.push(v); } }
        Ok(out)
    }

    fn object_lit(&mut self, props: &[A::Prop], scope: &Scope) -> Result<Val, Ctl> {
        let o = Val::obj(Kind::Plain);
        for p in props { self.object_prop(&o, p, None, scope)?; }
        Ok(o)
    }
    /// One PropertyDefinitionEvaluation step (§13.2.5.5); `pre` = the value when already evaluated (resumable path).
    fn object_prop(&mut self, o: &Val, p: &A::Prop, pre: Option<Val>, scope: &Scope) -> Result<(), Ctl> {
        let Val::Obj(ob) = o else { return Ok(()) };
        match p.kind {
            A::PropKind::Spread => {
                let src = match pre { Some(v) => v, None => self.ev(&p.value, scope)? };
                if src.is_nullish() { return Ok(()); }
                if let Val::Str(s) = &src { for (i, c) in s.chars().enumerate() { let mut b = [0u8; 4]; Self::put(ob, Rc::from(i.to_string().as_str()), st(c.encode_utf8(&mut b))); } return Ok(()); }
                for k in self.own_keys(&src, true, true, false) { let x = self.get(&src, &k)?; Self::put(ob, k, x); }
            }
            A::PropKind::Get | A::PropKind::Set | A::PropKind::Method => {
                let k = self.key(&p.key, scope)?;
                let Expr::Func(f) = &p.value else { return Ok(()) };
                let nm = match self.syms.get(&*k) { Some(Val::Sym(s)) => format!("[{}]", s.desc.as_deref().unwrap_or("")), _ => k.to_string() };
                let fv = self.make_fn_x(f, scope, None, Some(o.clone()), Some(&nm));
                match p.kind { A::PropKind::Method => Self::put(ob, k, fv), A::PropKind::Get => Self::define_accessor(ob, &k, Some(fv), None, 0), _ => Self::define_accessor(ob, &k, None, Some(fv), 0) }
            }
            A::PropKind::Proto => { let v = match pre { Some(v) => v, None => self.ev(&p.value, scope)? }; match v { Val::Obj(x) => { ob.borrow_mut().proto = Some(x); } Val::Null => { ob.borrow_mut().flags |= F_NULLPROTO; } _ => {} } }
            A::PropKind::Init => {
                let k = self.key(&p.key, scope)?;
                let v = match pre { Some(v) => v, None => self.ev(&p.value, scope)? };
                if matches!(p.value, Expr::Func(_) | Expr::Class(_)) { self.name_fn(&v, &k); }
                Self::put(ob, k, v);
            }
        }
        Ok(())
    }

    pub fn ev(&mut self, n: &Expr, scope: &Scope) -> Result<Val, Ctl> {
        match n {
            Expr::Lit(l) => Ok(self.literal_t(l)),
            Expr::Ident(nm) => self.lookup(scope, nm),
            Expr::Slot(d, i, nm) => self.slot_get(scope, *d, *i, nm),
            Expr::This => Ok(self.lookup_opt(scope, "this").unwrap_or(Val::Undef)),
            Expr::Template(q, ex) => {
                let mut out = String::new();
                for (i, s) in q.iter().enumerate() { out += s; if let Some(e) = ex.get(i) { let v = self.ev(e, scope)?; out += &self.to_str(&v)?; } }
                Ok(st(&fix_pairs(out)))
            }
            Expr::Array(els, sp) => {
                let mut v = Vec::with_capacity(els.len());
                for (i, e) in els.iter().enumerate() { match e { None => v.push(Val::Undef), Some(e) => { let x = self.ev(e, scope)?; if sp[i] { v.extend(self.iterate(&x)?); } else { v.push(x); } } } }
                Ok(Val::array(v))
            }
            Expr::Object(props) => self.object_lit(props, scope),
            Expr::Func(f) => {
                if f.arrow { let this = self.lookup_opt(scope, "this"); return Ok(self.make_fn_t(f, scope, this)); }
                if f.name.is_empty() { return Ok(self.make_fn_t(f, scope, None)); }
                // a named function expression sees its own name (§15.2.5 InstantiateOrdinaryFunctionExpression: funcEnv)
                let fs = Self::child(scope);
                let v = self.make_fn_t(f, &fs, None);
                { let mut b = fs.borrow_mut(); b.put(&f.name, v.clone()); b.fixed = true; }
                Ok(v)
            }
            Expr::Class(c) => self.class_t(c, scope),
            Expr::TypeOfIdent(nm) => Ok(match self.lookup(scope, nm) { Ok(v) => st(v.type_of()), Err(_) => st("undefined") }),
            Expr::DeleteMember(o, k) => { let ov = self.ev(o, scope)?; let k = self.key(k, scope)?; if ov.is_nullish() { type_err!(self, "Cannot convert undefined or null to object"); } Ok(Val::Bool(self.delete(&ov, &k)?)) }
            Expr::Unary(op, a) => { let a = self.ev(a, scope)?; self.unary(*op, a) }
            Expr::Update { inc, prefix, target } => match &**target {
                Expr::Ident(n) => { let old = self.lookup(scope, n)?; let (old, nv) = self.upd(old, *inc)?; self.assign_var(scope, n, nv.clone())?; Ok(if *prefix { nv } else { old }) }
                Expr::Slot(d, i, n) => { let old = self.slot_get(scope, *d, *i, n)?; let (old, nv) = self.upd(old, *inc)?; self.slot_set(scope, *d, *i, n, nv.clone())?; Ok(if *prefix { nv } else { old }) }
                Expr::Member { obj, key, .. } => {
                    let o = self.ev(obj, scope)?;
                    let k = match &**key { A::Key::Static(n) => Val::Str(n.clone()), A::Key::Computed(e) => self.ev(e, scope)? };
                    let old = self.get_v(&o, &k)?;
                    let (old, nv) = self.upd(old, *inc)?;
                    self.set_v(&o, &k, nv.clone())?;
                    Ok(if *prefix { nv } else { old })
                }
                _ => throw!(self, "invalid update target"),
            },
            Expr::Binary(op, a, b) => { let a = self.ev(a, scope)?; let b = self.ev(b, scope)?; self.binop_t(*op, a, b) }
            Expr::Logical(op, a, b) => {
                let a = self.ev(a, scope)?;
                match op { A::Logic::And => if a.truthy() { self.ev(b, scope) } else { Ok(a) }, A::Logic::Or => if a.truthy() { Ok(a) } else { self.ev(b, scope) }, A::Logic::Nullish => if a.is_nullish() { self.ev(b, scope) } else { Ok(a) } }
            }
            Expr::Cond(t, c, a) => if self.ev(t, scope)?.truthy() { self.ev(c, scope) } else { self.ev(a, scope) },
            Expr::Assign(op, target, rhs) => {
                match &**target {
                    A::Target::Super(k) => {
                        let v = match op {
                            A::AssignOp::Set => self.ev(rhs, scope)?,
                            A::AssignOp::Bin(b) => { let cur = self.super_get(k, scope)?.0; let r = self.ev(rhs, scope)?; self.binop_t(*b, cur, r)? }
                            _ => { let cur = self.super_get(k, scope)?.0; let go = match op { A::AssignOp::AndSet => cur.truthy(), A::AssignOp::OrSet => !cur.truthy(), _ => cur.is_nullish() }; if !go { return Ok(cur); } self.ev(rhs, scope)? }
                        };
                        self.super_set(k, v.clone(), scope)?;
                        Ok(v)
                    }
                    A::Target::Member(o, k) => {
                        let ov = self.ev(o, scope)?;
                        let kv = match k { A::Key::Static(n) => Val::Str(n.clone()), A::Key::Computed(e) => self.ev(e, scope)? };
                        let v = match op {
                            A::AssignOp::Set => self.ev(rhs, scope)?,
                            A::AssignOp::AndSet | A::AssignOp::OrSet | A::AssignOp::NullishSet => {
                                let cur = self.get_v(&ov, &kv)?;
                                let go = match op { A::AssignOp::AndSet => cur.truthy(), A::AssignOp::OrSet => !cur.truthy(), _ => cur.is_nullish() };
                                if !go { return Ok(cur); }
                                self.ev(rhs, scope)?
                            }
                            A::AssignOp::Bin(b) => { let cur = self.get_v(&ov, &kv)?; let r = self.ev(rhs, scope)?; self.binop_t(*b, cur, r)? }
                        };
                        self.set_v(&ov, &kv, v.clone())?;
                        Ok(v)
                    }
                    A::Target::Slot(d, i, n) => {
                        let v = match op {
                            A::AssignOp::Set => { let v = self.ev(rhs, scope)?; if let Expr::Func(_) | Expr::Class(_) = &**rhs { self.name_fn(&v, n); } v }
                            A::AssignOp::AndSet | A::AssignOp::OrSet | A::AssignOp::NullishSet => {
                                let cur = self.slot_get(scope, *d, *i, n)?;
                                let go = match op { A::AssignOp::AndSet => cur.truthy(), A::AssignOp::OrSet => !cur.truthy(), _ => cur.is_nullish() };
                                if !go { return Ok(cur); }
                                self.ev(rhs, scope)?
                            }
                            A::AssignOp::Bin(b) => { let cur = self.slot_get(scope, *d, *i, n)?; let r = self.ev(rhs, scope)?; self.binop_t(*b, cur, r)? }
                        };
                        self.slot_set(scope, *d, *i, n, v.clone())?;
                        Ok(v)
                    }
                    A::Target::Pat(p) => {
                        let v = match op {
                            A::AssignOp::Set => { let v = self.ev(rhs, scope)?; if let (Pat::Ident(n), Expr::Func(_) | Expr::Class(_)) = (p, &**rhs) { self.name_fn(&v, n); } v }
                            A::AssignOp::AndSet | A::AssignOp::OrSet | A::AssignOp::NullishSet => {
                                let cur = match p { Pat::Ident(n) => self.lookup(scope, n)?, _ => Val::Undef };
                                let go = match op { A::AssignOp::AndSet => cur.truthy(), A::AssignOp::OrSet => !cur.truthy(), _ => cur.is_nullish() };
                                if !go { return Ok(cur); }
                                self.ev(rhs, scope)?
                            }
                            A::AssignOp::Bin(b) => { let cur = match p { Pat::Ident(n) => self.lookup(scope, n)?, _ => Val::Undef }; let r = self.ev(rhs, scope)?; self.binop_t(*b, cur, r)? }
                        };
                        self.assign_pat(p, v.clone(), scope)?;
                        Ok(v)
                    }
                }
            }
            Expr::Seq(v) => { let mut last = Val::Undef; for e in v { last = self.ev(e, scope)?; } Ok(last) }
            Expr::Chain(e) => Ok(self.chain(e, scope)?.unwrap_or(Val::Undef)),
            Expr::Member { obj, key, .. } => {
                let o = self.ev(obj, scope)?;
                if o.is_nullish() && blame_on() { eprintln!("jsi blame: nullish read of {} (.{}) in {}", expr_desc(obj), match &**key { A::Key::Static(k) => k.to_string(), _ => "[computed]".into() }, BS.with(|b| { let b = b.borrow(); b[b.len().saturating_sub(8)..].join(" < ") })); }
                match &**key { A::Key::Static(k) => self.get(&o, k), A::Key::Computed(e) => { let k = self.ev(e, scope)?; if o.is_nullish() { let ks = self.to_key(&k).unwrap_or_else(|_| Rc::from("?")); return self.get(&o, &ks); } self.get_v(&o, &k) } }
            }
            Expr::Call { .. } => Ok(self.call_t(n, scope)?.unwrap_or(Val::Undef)),
            Expr::New { callee, args, spread } => { let f = self.ev(callee, scope)?; let a = self.args_t(args, spread, scope)?; let plain_no_proto = matches!(&f, Val::Obj(o) if matches!(o.borrow().kind, Kind::Plain) && !o.borrow().props.iter().any(|(k, v)| &**k == "prototype" && matches!(v, Val::Obj(_))));
                if !matches!(f, Val::Obj(_)) || plain_no_proto { let s = match &**callee { Expr::Ident(n) | Expr::Slot(_, _, n) => n.to_string(), Expr::Member { key, .. } => match &**key { A::Key::Static(k) => k.to_string(), _ => "expression".into() }, _ => "expression".into() }; type_err!(self, "{} is not a constructor", s); }
                self.construct(&f, a) }
            Expr::Await(e) => { let v = self.ev(e, scope)?; self.await_val(v) }
            Expr::Yield(..) => Err(self.err("SyntaxError", "yield in an unsupported position")),
            Expr::Y(e) => self.ev(e, scope),
            Expr::SuperMember(k) => Ok(self.super_get(k, scope)?.0),
            Expr::SuperCall(args, spread) => { let a = self.args_t(args, spread, scope)?; self.super_call(a, scope) }
            Expr::NewTarget => Ok(self.lookup_opt(scope, "%nt").unwrap_or(Val::Undef)),
            Expr::ImportMeta => Ok(Val::plain(vec![("url", st(self.cur_module.last().map(|s| s.as_str()).unwrap_or("")))])),
            Expr::Tagged(tag, site) => {
                let (f, this) = match &**tag {
                    Expr::Member { obj, key, .. } => { let o = self.ev(obj, scope)?; let k = self.key(key, scope)?; (self.get(&o, &k)?, o) }
                    t => (self.ev(t, scope)?, Val::Undef),
                };
                let strings = self.template_object(site);
                let mut args = vec![strings];
                for e in &site.exprs { args.push(self.ev(e, scope)?); }
                if !f.is_callable() { type_err!(self, "tag is not a function"); }
                self.call(&f, this, args)
            }
            Expr::ImportCall(e) => {
                let s = self.ev(e, scope)?;
                let s = self.to_str(&s)?;
                Ok(match self.import(&s) { Ok(m) => crate::jsi_lib::resolved(m), Err(Ctl::Throw(x)) => crate::jsi_lib::rejected(x), Err(e) => return Err(e) })
            }
            Expr::Unsupported(k) => Err(self.err("SyntaxError", &format!("unsupported expression {k}"))),
        }
    }

    /// GetTemplateObject (§13.2.8.4): frozen cooked strings with a frozen `raw`, one per call site.
    fn template_object(&mut self, site: &Rc<A::TplSite>) -> Val {
        if let Some(v) = site.cache.borrow().as_ref() { return v.clone(); }
        let raw = Val::array(site.raw.iter().map(|r| Val::Str(r.clone())).collect());
        let s = Val::array(site.cooked.iter().map(|c| c.clone().map(Val::Str).unwrap_or(Val::Undef)).collect());
        if let (Val::Obj(so), Val::Obj(ro)) = (&s, &raw) { ro.borrow_mut().flags |= F_FROZEN; Self::define(so, "raw", raw.clone(), A_NOENUM | A_NOWRITE | A_NOCONF); so.borrow_mut().flags |= F_FROZEN; }
        *site.cache.borrow_mut() = Some(s.clone());
        s
    }

    /// `x++` / `x--` (§13.4.2.1): (old numeric value, new value).
    pub(crate) fn upd(&mut self, old: Val, inc: bool) -> Result<(Val, Val), Ctl> {
        if let Val::Num(o) = old { return Ok((Val::Num(o), Val::Num(if inc { o + 1.0 } else { o - 1.0 }))); }
        let old = self.to_numeric(&old)?;
        let one = crate::jsi_lib::Big::from_i64(1);
        let nv = match &old { Val::Big(b) => Val::Big(Rc::new(if inc { b.add(&one) } else { b.sub(&one) })), Val::Num(o) => Val::Num(if inc { o + 1.0 } else { o - 1.0 }), _ => Val::Num(f64::NAN) };
        Ok((old, nv))
    }
    /// ToNumeric (§7.1.3): BigInt stays BigInt.
    pub fn to_numeric(&mut self, v: &Val) -> Result<Val, Ctl> {
        match v { Val::Num(_) | Val::Big(_) => Ok(v.clone()), Val::Obj(_) => { let p = self.to_primitive(v, 1)?; self.to_numeric(&p) } _ => Ok(Val::Num(self.to_num(v)?)) }
    }

    /// `await v` outside a resumable body (module top level, parameter defaults): settle what can settle now.
    pub fn await_val(&mut self, v: Val) -> Result<Val, Ctl> {
        let p = match &v {
            Val::Obj(o) if matches!(o.borrow().kind, Kind::Promise(_)) => v.clone(),
            Val::Obj(_) => { let t = self.get(&v, "then")?; if !t.is_callable() { return Ok(v); } let p = new_promise(); self.resolve_promise(&p, v.clone()); p }
            _ => return Ok(v),
        };
        let mut guard = 0;
        loop {
            let (stt, x) = promise_state(&p);
            match stt { 1 => return Ok(x), 2 => { mark_handled(&p); return Err(Ctl::Throw(x)); } _ => {} }
            if self.jobs.is_empty() || guard > 1_000_000 { return Ok(Val::Undef); }
            guard += 1;
            self.run_one_job();
        }
    }

    /// Optional chains: None = short-circuited to undefined.
    fn chain(&mut self, n: &Expr, scope: &Scope) -> Result<Option<Val>, Ctl> {
        match n {
            Expr::Member { obj, key, optional } => {
                let o = match &**obj { Expr::Member { .. } | Expr::Call { .. } => match self.chain(obj, scope)? { Some(v) => v, None => return Ok(None) }, _ => self.ev(obj, scope)? };
                if *optional && o.is_nullish() { return Ok(None); }
                if o.is_nullish() && blame_on() { eprintln!("jsi blame: nullish read of {} (.{}) in {}", expr_desc(obj), match &**key { A::Key::Static(k) => k.to_string(), _ => "[computed]".into() }, BS.with(|b| { let b = b.borrow(); b[b.len().saturating_sub(8)..].join(" < ") })); }
                match &**key { A::Key::Static(k) => Ok(Some(self.get(&o, k)?)), A::Key::Computed(e) => { let k = self.ev(e, scope)?; Ok(Some(self.get_v(&o, &k)?)) } }
            }
            Expr::Call { .. } => self.call_t(n, scope),
            _ => Ok(Some(self.ev(n, scope)?)),
        }
    }

    fn call_t(&mut self, n: &Expr, scope: &Scope) -> Result<Option<Val>, Ctl> {
        let Expr::Call { callee, args, spread, optional } = n else { return Ok(None) };
        let (f, this) = match &**callee {
            Expr::Member { obj, key, optional: mopt } => {
                let o = match &**obj { Expr::Member { .. } | Expr::Call { .. } => match self.chain(obj, scope)? { Some(v) => v, None => return Ok(None) }, _ => self.ev(obj, scope)? };
                if o.is_nullish() && blame_on() { eprintln!("jsi blame: call of {}.{}(..) on nullish in {}", expr_desc(obj), match &**key { A::Key::Static(k) => k.to_string(), _ => "[computed]".into() }, BS.with(|b| { let b = b.borrow(); b[b.len().saturating_sub(8)..].join(" < ") })); }
                if *mopt && o.is_nullish() { return Ok(None); }
                let f = match &**key { A::Key::Static(k) => self.get(&o, k)?, A::Key::Computed(e) => { let k = self.ev(e, scope)?; self.get_v(&o, &k)? } };
                if !f.is_callable() {
                    if *optional && f.is_nullish() { return Ok(None); }
                    let k = match &**key { A::Key::Static(k) => self.shown_key(k), A::Key::Computed(_) => "expression".into() };
                    let on = match &**obj { Expr::Ident(n) | Expr::Slot(_, _, n) => format!("{n}."), Expr::This => "this.".into(), _ => String::new() };
                    type_err!(self, "{}{} is not a function", on, k);
                }
                (f, o)
            }
            Expr::SuperMember(k) => { let (f, this) = self.super_get(k, scope)?; if !f.is_callable() { type_err!(self, "(intermediate value).{} is not a function", match &**k { A::Key::Static(n) => n.to_string(), _ => "method".into() }); } (f, this) }
            Expr::Ident(name) if &**name == "eval" => {
                let f = self.lookup(scope, name)?;
                let direct = matches!((&f, &self.eval_fn), (Val::Obj(a), Some(Val::Obj(b))) if Rc::ptr_eq(a, b));
                if direct {
                    // direct eval (§19.2.1.1 PerformEval with direct = true): the caller's scope
                    let a = self.args_t(args, spread, scope)?;
                    return Ok(Some(match a.first() { Some(Val::Str(src)) => { let src = src.clone(); self.eval_src(&src, scope)? } Some(v) => v.clone(), None => Val::Undef }));
                }
                if !f.is_callable() { type_err!(self, "eval is not a function"); }
                (f, Val::Undef)
            }
            _ => {
                let f = self.ev(callee, scope)?;
                if *optional && f.is_nullish() { return Ok(None); }
                if !f.is_callable() { type_err!(self, "{} is not a function", match &**callee { Expr::Ident(n) | Expr::Slot(_, _, n) => n.to_string(), _ => "expression".into() }); }
                (f, Val::Undef)
            }
        };
        let a = self.args_t(args, spread, scope)?;
        Ok(Some(self.call(&f, this, a)?))
    }

    fn store_t(&mut self, target: &Expr, v: Val, scope: &Scope) -> Result<(), Ctl> {
        match target {
            Expr::Ident(n) => self.assign_var(scope, n, v),
            Expr::Slot(d, i, n) => self.slot_set(scope, *d, *i, n, v),
            Expr::Member { obj, key, .. } => { let o = self.ev(obj, scope)?; match &**key { A::Key::Static(k) => self.set(&o, k, v), A::Key::Computed(e) => { let k = self.ev(e, scope)?; self.set_v(&o, &k, v) } } }
            _ => throw!(self, "invalid assignment target"),
        }
    }

    pub(crate) fn binop_t(&mut self, op: A::Bin, a: Val, b: Val) -> Result<Val, Ctl> {
        use A::Bin::*;
        // fast paths for numbers (shared with the closure compiler, jsc::num_bin)
        if let (Val::Num(x), Val::Num(y)) = (&a, &b) { if let Some(r) = crate::jsc::num_bin(op, *x, *y) { return Ok(r); } }
        let s = match op { Add => "+", Sub => "-", Mul => "*", Div => "/", Mod => "%", Pow => "**", StrictEq => "===", StrictNe => "!==", Eq => "==", Ne => "!=", Lt => "<", Gt => ">", Le => "<=", Ge => ">=",
            BitAnd => "&", BitOr => "|", BitXor => "^", Shl => "<<", Shr => ">>", UShr => ">>>", In => "in", InstanceOf => "instanceof" };
        self.binop(s, a, b)
    }

    // ── J-based entry points (svrt, uiir): lower once per long-lived node, then run the typed tree ──
    fn lower_e(&mut self, j: &J) -> Rc<A::Expr> {
        let k = j as *const J as usize;
        if let Some(e) = self.cache_e.get(&k) { return e.clone(); }
        let e = Rc::new(A::expr(j)); self.cache_e.insert(k, e.clone()); e
    }
    fn lower_s(&mut self, j: &J) -> Rc<A::Stmt> {
        let k = j as *const J as usize;
        if let Some(e) = self.cache_s.get(&k) { return e.clone(); }
        let e = Rc::new(A::stmt(j)); self.cache_s.insert(k, e.clone()); e
    }
    fn lower_p(&mut self, j: &J) -> Rc<A::Pat> {
        let k = j as *const J as usize;
        if let Some(e) = self.cache_p.get(&k) { return e.clone(); }
        let e = Rc::new(A::pat(j)); self.cache_p.insert(k, e.clone()); e
    }
    pub fn eval(&mut self, n: &J, scope: &Scope) -> Result<Val, Ctl> { let e = self.lower_e(n); self.enter(); let r = self.ev(&e, scope); self.leave(); r }
    pub fn exec(&mut self, n: &J, scope: &Scope) -> Result<(), Ctl> { let s_ = self.lower_s(n); self.enter(); let r = self.ex(&s_, scope); self.leave(); r }
    pub fn bind(&mut self, p: &J, v: Val, scope: &Scope) -> Result<(), Ctl> { let pp = self.lower_p(p); self.enter(); let r = self.bind_pat(&pp, v, scope); self.leave(); r }
    pub fn store_pub(&mut self, target: &J, v: Val, scope: &Scope) -> Result<(), Ctl> { let e = self.lower_e(target); self.enter(); let r = self.store_t(&e, v, scope); self.leave(); r }
    pub fn hoist(&mut self, body: &[J], scope: &Scope) {
        let lowered: Vec<Rc<A::Stmt>> = body.iter().map(|j| self.lower_s(j)).collect();
        for st in lowered { self.hoist_t(std::slice::from_ref(&*st), scope); }
    }
    pub fn exec_list(&mut self, body: &[J], scope: &Scope) -> Result<(), Ctl> { self.enter(); let r = (|| { for st in body { self.exec(st, scope)?; } Ok(()) })(); self.leave(); r }

    pub fn binop(&mut self, op: &str, a: Val, b: Val) -> Result<Val, Ctl> {
        Ok(match op {
            "+" => {
                let (pa, pb) = (self.to_primitive(&a, 0)?, self.to_primitive(&b, 0)?);
                if matches!(pa, Val::Str(_)) || matches!(pb, Val::Str(_)) { let x = self.to_str(&pa)?; let y = self.to_str(&pb)?; if y.is_empty() { Val::Str(x) } else if x.is_empty() { Val::Str(y) } else { Val::Str(concat(&x, &y)) } }
                else if let (Val::Big(x), Val::Big(y)) = (&pa, &pb) { Val::Big(Rc::new(x.add(y))) }
                else if matches!(pa, Val::Big(_)) || matches!(pb, Val::Big(_)) { type_err!(self, "Cannot mix BigInt and other types, use explicit conversions") }
                else { Val::Num(self.to_num(&pa)? + self.to_num(&pb)?) }
            }
            "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^" | "<<" | ">>" | ">>>" => {
                let (x, y) = (self.to_numeric(&a)?, self.to_numeric(&b)?);
                match (&x, &y) {
                    (Val::Big(p), Val::Big(q)) => return crate::jsi_lib::big_op(self, op, p, q),
                    (Val::Big(_), _) | (_, Val::Big(_)) => type_err!(self, "Cannot mix BigInt and other types, use explicit conversions"),
                    _ => {}
                }
                let (x, y) = (self.to_num(&x)?, self.to_num(&y)?);
                Val::Num(match op {
                    "-" => x - y, "*" => x * y, "/" => x / y,
                    "%" => if y.is_infinite() && x.is_finite() { x } else { x % y },
                    "**" => if y.is_nan() || (x.abs() == 1.0 && y.is_infinite()) { f64::NAN } else { x.powf(y) },
                    "&" => (to_int32(x) & to_int32(y)) as f64, "|" => (to_int32(x) | to_int32(y)) as f64, "^" => (to_int32(x) ^ to_int32(y)) as f64,
                    "<<" => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64, ">>" => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
                    _ => (to_uint32(x) >> (to_uint32(y) & 31)) as f64,
                })
            }
            "===" => Val::Bool(Self::strict_eq(&a, &b)), "!==" => Val::Bool(!Self::strict_eq(&a, &b)),
            "==" => Val::Bool(self.loose_eq(&a, &b)?), "!=" => Val::Bool(!self.loose_eq(&a, &b)?),
            "<" | ">" | "<=" | ">=" => {
                let (pa, pb) = (self.to_primitive(&a, 1)?, self.to_primitive(&b, 1)?);
                if let (Val::Str(x), Val::Str(y)) = (&pa, &pb) {
                    let (x, y) = (utf16_of(x), utf16_of(y));
                    Val::Bool(match op { "<" => x < y, ">" => x > y, "<=" => x <= y, _ => x >= y })
                } else if matches!(pa, Val::Big(_)) || matches!(pb, Val::Big(_)) {
                    let c = crate::jsi_lib::big_cmp(self, &pa, &pb)?;
                    Val::Bool(match c { None => false, Some(o) => match op { "<" => o.is_lt(), ">" => o.is_gt(), "<=" => o.is_le(), _ => o.is_ge() } })
                } else {
                    let (x, y) = (self.to_num(&pa)?, self.to_num(&pb)?);
                    Val::Bool(match op { "<" => x < y, ">" => x > y, "<=" => x <= y, _ => x >= y })
                }
            }
            "in" => { if !matches!(b, Val::Obj(_)) { let k = self.to_str(&a).unwrap_or_else(|_| Rc::from("?")); let bs = self.to_str(&b).map(|x| x.to_string()).unwrap_or_default(); type_err!(self, "Cannot use 'in' operator to search for '{}' in {}", k, bs); } let k = self.to_key(&a)?; Val::Bool(self.has_prop(&b, &k)?) }
            "instanceof" => Val::Bool(self.instance_of(&a, &b)?),
            _ => throw!(self, "unsupported operator {op}"),
        })
    }

    pub(crate) fn unary(&mut self, op: A::Un, a: Val) -> Result<Val, Ctl> {
        Ok(match op { A::Un::Not => Val::Bool(!a.truthy()),
            A::Un::Neg => match &a { Val::Num(n) => Val::Num(-n), Val::Big(b) => Val::Big(Rc::new(b.neg())), _ => { let p = self.to_numeric(&a)?; match &p { Val::Big(b) => Val::Big(Rc::new(b.neg())), _ => Val::Num(-self.to_num(&p)?) } } },
            A::Un::Plus => Val::Num(self.to_num(&a)?),
            A::Un::BitNot => match self.to_numeric(&a)? { Val::Big(b) => Val::Big(Rc::new(b.neg().sub(&crate::jsi_lib::Big::from_i64(1)))), x => Val::Num(!to_int32(self.to_num(&x)?) as f64) },
            A::Un::TypeOf => st(a.type_of()), A::Un::Void | A::Un::Delete => Val::Undef })
    }

    // ── resumable evaluation (generator / async bodies; ECMA-262 §27.5 GeneratorResume, §27.7.5.3 Await) ──
    // A body holding a yield/await runs here. Only Y-marked nodes (the path to a suspension point) are walked resumably;
    // every other subtree runs on the fast evaluator. On Suspend each construct on the path saves its slot (stage, partial
    // values, scopes, open iterator, pending completion); resuming replays the path from those slots straight back to the
    // suspension point, which then answers with the value (or throw / return) the caller sent.
    fn take_slot(&mut self) -> Option<Slot> { let c = self.co.last_mut()?; if c.rpos < c.resume.len() { let s = std::mem::take(&mut c.resume[c.rpos]); c.rpos += 1; Some(s) } else { None } }
    fn save_slot(&mut self, s: Slot) { if let Some(c) = self.co.last_mut() { c.saving.push(s); } }
    fn take_sent(&mut self) -> Result<Val, Ctl> { match self.co.last_mut().and_then(|c| c.sent.take()) { Some(Resume::Next(v)) => Ok(v), Some(Resume::Throw(e)) => Err(Ctl::Throw(e)), Some(Resume::Return(v)) => Err(Ctl::Return(v)), None => Ok(Val::Undef) } }
    fn suspend(&mut self, v: Val, slot: Slot, is_await: bool) -> Ctl { if let Some(c) = self.co.last_mut() { c.out = v; c.awaiting = is_await; } self.save_slot(slot); Ctl::Suspend }

    fn ex_c(&mut self, s: &Stmt, scope: &Scope) -> Result<(), Ctl> { match s { Stmt::Y(i) => self.ex_r(i, scope, &[]), _ => self.ex(s, scope) } }
    fn ev_c(&mut self, e: &Expr, scope: &Scope) -> Result<Val, Ctl> { match e { Expr::Y(i) => self.ev_r(i, scope), _ => self.ev(e, scope) } }

    /// A statement list, resumably: the slot is the index of the statement holding the suspension point.
    fn stmts_r(&mut self, body: &[Stmt], scope: &Scope) -> Result<(), Ctl> {
        let start = match self.take_slot() { Some(s) => s.n, None => 0 };
        for i in start..body.len() { sus!(self, self.ex_c(&body[i], scope), Slot { n: i, ..Default::default() }); }
        Ok(())
    }

    fn ex_r(&mut self, n: &Stmt, scope: &Scope, labels: &[Rc<str>]) -> Result<(), Ctl> {
        self.steps += 1;
        if self.steps > self.limit { throw!(self, "step limit"); }
        match n {
            Stmt::Expr(e) => { self.ev_c(e, scope)?; Ok(()) }
            Stmt::Return(e) => { let v = match e { Some(e) => self.ev_c(e, scope)?, None => Val::Undef }; Err(Ctl::Return(v)) }
            Stmt::Throw(e) => { let v = self.ev_c(e, scope)?; Err(Ctl::Throw(v)) }
            Stmt::Var(kind, decls) => {
                let start = match self.take_slot() { Some(s) => s.n, None => 0 };
                for (i, (p, init)) in decls.iter().enumerate().skip(start) {
                    let v = match init { Some(e) => sus!(self, self.ev_c(e, scope), Slot { n: i, ..Default::default() }), None => Val::Undef };
                    if let (Pat::Ident(nm), Some(e)) = (p, init) { if is_anon_fn(e) { self.name_fn(&v, nm); } }
                    if matches!(kind, A::VarKind::Var) { if init.is_some() { self.assign_pat(p, v, scope)?; } } else { self.bind_pat(p, v, scope)?; }
                }
                Ok(())
            }
            Stmt::If(t, c, a) => {
                let st = match self.take_slot() { Some(s) => s.st, None => 0 };
                let br = if st == 0 { let v = sus!(self, self.ev_c(t, scope), Slot::st(0)); if v.truthy() { 1 } else { 2 } } else { st };
                if br == 1 { sus!(self, self.ex_c(c, scope), Slot::st(1)); } else if let Some(a) = a { sus!(self, self.ex_c(a, scope), Slot::st(2)); }
                Ok(())
            }
            Stmt::Block(b, scoped, _) => {
                let (start, sc) = match self.take_slot() { Some(s) => (s.n, s.sc.unwrap_or_else(|| scope.clone())), None => (0, if *scoped { let sc = Self::child(scope); self.hoist_fns(b, &sc); sc } else { scope.clone() }) };
                for i in start..b.len() { sus!(self, self.ex_c(&b[i], &sc), Slot { n: i, sc: Some(sc.clone()), ..Default::default() }); }
                Ok(())
            }
            Stmt::Labeled(l, b) => {
                let mut ls = labels.to_vec(); ls.push(l.clone());
                let r = match &**b {
                    Stmt::Y(inner) => self.ex_r(inner, scope, &ls),
                    other @ (Stmt::For { .. } | Stmt::ForOf { .. } | Stmt::While(..) | Stmt::DoWhile(..)) => self.ex_loop(other, scope, &ls),
                    other => self.ex(other, scope),
                };
                match r { Err(Ctl::Break(Some(x))) if ls.contains(&x) => Ok(()), r => r }
            }
            Stmt::While(t, b) => {
                let mut st = match self.take_slot() { Some(s) => s.st, None => 0 };
                loop {
                    if st == 0 { let v = sus!(self, self.ev_c(t, scope), Slot::st(0)); if !v.truthy() { break; } }
                    st = 0;
                    match self.ex_c(b, scope) { Ok(()) => {}, Err(Ctl::Suspend) => { self.save_slot(Slot::st(1)); return Err(Ctl::Suspend); } Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::DoWhile(b, t) => {
                let mut st = match self.take_slot() { Some(s) => s.st, None => 1 };
                loop {
                    if st == 1 { match self.ex_c(b, scope) { Ok(()) => {}, Err(Ctl::Suspend) => { self.save_slot(Slot::st(1)); return Err(Ctl::Suspend); } Err(e) if cont(&e, labels) => {}, Err(e) if brk(&e, labels) => break, Err(e) => return Err(e) } }
                    st = 1;
                    let v = sus!(self, self.ev_c(t, scope), Slot::st(0));
                    if !v.truthy() { break; }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::For { init, test, update, body, per_iter, .. } => {
                let (mut st, sc, mut itsc) = match self.take_slot() { Some(s) => (s.st, s.sc.unwrap_or_else(|| Self::child(scope)), s.sc2), None => (0, Self::child(scope), None) };
                if st == 0 { if let Some(i) = init { sus!(self, self.ex_c(i, &sc), Slot { st: 0, sc: Some(sc.clone()), ..Default::default() }); } st = 1; }
                loop {
                    if st == 1 { if let Some(t) = test { let v = sus!(self, self.ev_c(t, &sc), Slot { st: 1, sc: Some(sc.clone()), ..Default::default() }); if !v.truthy() { break; } } st = 2; }
                    if st == 2 {
                        let it = match itsc.take() { Some(x) => x, None => if *per_iter { let it = Self::child(&sc); { let s = sc.borrow(); it.borrow_mut().copy_bindings(&s); } it } else { sc.clone() } };
                        match self.ex_c(body, &it) {
                            Ok(()) => {}
                            Err(Ctl::Suspend) => { self.save_slot(Slot { st: 2, sc: Some(sc.clone()), sc2: Some(it), ..Default::default() }); return Err(Ctl::Suspend); }
                            Err(e) if cont(&e, labels) => {}
                            Err(e) if brk(&e, labels) => break,
                            Err(e) => return Err(e),
                        }
                        if *per_iter { let s = it.borrow(); sc.borrow_mut().copy_bindings(&s); }
                    }
                    if let Some(u) = update { sus!(self, self.ev_c(u, &sc), Slot { st: 3, sc: Some(sc.clone()), ..Default::default() }); }
                    st = 1;
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::ForOf { decl, pat, right, body, of, is_await, .. } => {
                // slot st: 0 right side; 1 in body (it, sc2); 2 awaiting a value (for await over a sync iterable); 3 awaiting next()
                let (mut st, iter0, mut itsc) = match self.take_slot() { Some(s) => (s.st, s.it, s.sc2), None => (0, None, None) };
                let mut iter = match iter0 {
                    Some(i) => i,
                    None => {
                        let src = sus!(self, self.ev_c(right, scope), Slot::st(0));
                        st = 1;
                        Box::new(if !*of { if src.is_nullish() { return Ok(()); } IterRec::List(self.for_in_keys(&src).into_iter().map(Val::Str).collect(), 0) } else if *is_await { self.async_iter_open(&src)? } else { self.iter_open(&src)? })
                    }
                };
                loop {
                    let isc = match itsc.take() {
                        Some(x) => x,
                        None => {
                            let x = match st {
                                2 => self.take_sent()?,
                                3 => { let res = self.take_sent()?; if !matches!(res, Val::Obj(_)) { type_err!(self, "Iterator result is not an object"); } if self.get(&res, "done")?.truthy() { break; } self.get(&res, "value")? }
                                _ => {
                                    if let IterRec::Async(it, next) = &*iter { let (it, next) = (it.clone(), next.clone()); let p = self.call(&next, it, vec![])?; return Err(self.suspend(p, Slot { st: 3, it: Some(iter), ..Default::default() }, true)); }
                                    match self.iter_step(&mut iter)? { None => break, Some(v) => { if *is_await { return Err(self.suspend(v, Slot { st: 2, it: Some(iter), ..Default::default() }, true)); } v } }
                                }
                            };
                            st = 1;
                            let isc = Self::child(scope);
                            if *decl == 2 { self.bind_pat(pat, x, &isc)?; } else { self.assign_pat(pat, x, scope)?; }
                            isc
                        }
                    };
                    match self.ex_c(body, &isc) {
                        Ok(()) => {}
                        Err(Ctl::Suspend) => { self.save_slot(Slot { st: 1, it: Some(iter), sc2: Some(isc), ..Default::default() }); return Err(Ctl::Suspend); }
                        Err(e) if cont(&e, labels) => {}
                        Err(e) if brk(&e, labels) => { self.iter_close(&iter)?; break; }
                        Err(Ctl::Throw(v)) => { let _ = self.iter_close(&iter); return Err(Ctl::Throw(v)); }
                        Err(e) => { self.iter_close(&iter)?; return Err(e); }
                    }
                    self.steps += 1; if self.steps > self.limit { throw!(self, "step limit"); }
                }
                Ok(())
            }
            Stmt::Try { block, param, handler, finalizer, .. } => {
                // slot st: 0 in block, 1 in handler, 2 in finalizer (ctl = the completion it re-raises)
                let (st0, rsc, pend) = match self.take_slot() { Some(s) => (s.st, s.sc, s.ctl), None => (0, None, None) };
                let mut st = st0;
                let mut r: Result<(), Ctl> = Ok(());
                let mut hsc: Option<Scope> = None;
                if st == 0 {
                    let bs = match rsc.clone() { Some(x) => x, None => { let b2 = Self::child(scope); self.hoist_fns(block, &b2); b2 } };
                    r = self.stmts_r(block, &bs);
                    if matches!(r, Err(Ctl::Suspend)) { self.save_slot(Slot { st: 0, sc: Some(bs), ..Default::default() }); return Err(Ctl::Suspend); }
                    st = 2;
                    if let (Err(Ctl::Throw(v)), Some(h)) = (&r, handler) { let hs = Self::child(scope); if let Some(p) = param { self.bind_pat(p, v.clone(), &hs)?; } self.hoist_fns(h, &hs); hsc = Some(hs); st = 1; }
                } else if st == 1 { hsc = rsc.clone(); }
                if st == 1 {
                    let hs = hsc.unwrap_or_else(|| Self::child(scope));
                    r = self.stmts_r(handler.as_deref().unwrap_or(&[]), &hs);
                    if matches!(r, Err(Ctl::Suspend)) { self.save_slot(Slot { st: 1, sc: Some(hs), ..Default::default() }); return Err(Ctl::Suspend); }
                }
                if let Some(f) = finalizer {
                    let (fs, r0) = if st0 == 2 { (rsc.unwrap_or_else(|| Self::child(scope)), match pend { Some(c) => Err(*c), None => Ok(()) }) } else { let fs = Self::child(scope); self.hoist_fns(f, &fs); (fs, r) };
                    return match self.stmts_r(f, &fs) {
                        Ok(()) => r0,
                        Err(Ctl::Suspend) => { self.save_slot(Slot { st: 2, sc: Some(fs), ctl: r0.err().map(Box::new), ..Default::default() }); Err(Ctl::Suspend) }
                        Err(e) => Err(e),
                    };
                }
                r
            }
            Stmt::Switch(d, cases, _) => {
                // slot st: 0 discriminant; 1 running case n's statements in the switch scope
                let (st, n0, rsc) = match self.take_slot() { Some(s) => (s.st, s.n, s.sc), None => (0, 0, None) };
                let (sc, start) = if st == 0 {
                    let dv = sus!(self, self.ev_c(d, scope), Slot::st(0));
                    let sc = Self::child(scope);
                    for (_, b) in cases { self.hoist_fns(b, &sc); }
                    let mut start = None;
                    for (i, (t, _)) in cases.iter().enumerate() { if let Some(t) = t { let tv = self.ev(t, &sc)?; if Self::strict_eq(&dv, &tv) { start = Some(i); break; } } }
                    if start.is_none() { start = cases.iter().position(|c| c.0.is_none()); }
                    match start { Some(i) => (sc, i), None => return Ok(()) }
                } else { (rsc.unwrap_or_else(|| Self::child(scope)), n0) };
                for ci in start..cases.len() {
                    match self.stmts_r(&cases[ci].1, &sc) { Ok(()) => {}, Err(Ctl::Suspend) => { self.save_slot(Slot { st: 1, n: ci, sc: Some(sc.clone()), ..Default::default() }); return Err(Ctl::Suspend); } Err(Ctl::Break(None)) => break, Err(e) => return Err(e) }
                }
                Ok(())
            }
            Stmt::Y(s) => self.ex_r(s, scope, labels),
            other => self.ex(other, scope),
        }
    }

    fn ev_r(&mut self, n: &Expr, scope: &Scope) -> Result<Val, Ctl> {
        match n {
            Expr::Y(e) => self.ev_r(e, scope),
            Expr::Await(e) => {
                let st = match self.take_slot() { Some(s) => s.st, None => 0 };
                if st == 1 { return self.take_sent(); }
                let v = sus!(self, self.ev_c(e, scope), Slot::st(0));
                Err(self.suspend(v, Slot::st(1), true))
            }
            Expr::Yield(arg, false) => {
                let st = match self.take_slot() { Some(s) => s.st, None => 0 };
                if st == 1 { return self.take_sent(); }
                let v = match arg { Some(e) => sus!(self, self.ev_c(e, scope), Slot::st(0)), None => Val::Undef };
                Err(self.suspend(v, Slot::st(1), false))
            }
            Expr::Yield(arg, true) => self.yield_star(arg.as_deref(), scope),
            Expr::Template(q, ex) => {
                let (start, mut acc) = match self.take_slot() { Some(s) => (s.n, match s.vals.into_iter().next() { Some(Val::Str(x)) => x.to_string(), _ => String::new() }), None => (0, q.first().map(|x| x.to_string()).unwrap_or_default()) };
                for i in start..ex.len() {
                    let v = sus!(self, self.ev_c(&ex[i], scope), Slot { n: i, vals: vec![st(&acc)], ..Default::default() });
                    acc += &self.to_str(&v)?;
                    if let Some(qq) = q.get(i + 1) { acc += qq; }
                }
                Ok(st(&fix_pairs(acc)))
            }
            Expr::Array(els, sp) => {
                let (start, mut v) = match self.take_slot() { Some(s) => (s.n, s.vals), None => (0, Vec::with_capacity(els.len())) };
                for i in start..els.len() {
                    match &els[i] { None => v.push(Val::Undef), Some(e) => {
                        let x = sus!(self, self.ev_c(e, scope), Slot { n: i, vals: v.clone(), ..Default::default() });
                        if sp[i] { v.extend(self.iterate(&x)?); } else { v.push(x); }
                    } }
                }
                Ok(Val::array(v))
            }
            Expr::Object(props) => {
                let (start, o) = match self.take_slot() { Some(s) => (s.n, s.vals.into_iter().next().unwrap_or_else(|| Val::obj(Kind::Plain))), None => (0, Val::obj(Kind::Plain)) };
                for i in start..props.len() {
                    let p = &props[i];
                    let pre = if matches!(p.value, Expr::Y(_)) && matches!(p.kind, A::PropKind::Init | A::PropKind::Spread | A::PropKind::Proto) { Some(sus!(self, self.ev_c(&p.value, scope), Slot { n: i, vals: vec![o.clone()], ..Default::default() })) } else { None };
                    self.object_prop(&o, p, pre, scope)?;
                }
                Ok(o)
            }
            Expr::Unary(op, a) => { let v = self.ev_c(a, scope)?; self.unary(*op, v) }
            Expr::Binary(op, a, b) => {
                let (st, l) = match self.take_slot() { Some(s) => (s.st, s.vals.into_iter().next()), None => (0, None) };
                let l = if st == 0 { sus!(self, self.ev_c(a, scope), Slot::st(0)) } else { l.unwrap_or(Val::Undef) };
                let r = sus!(self, self.ev_c(b, scope), Slot { st: 1, vals: vec![l.clone()], ..Default::default() });
                self.binop_t(*op, l, r)
            }
            Expr::Logical(op, a, b) => {
                let st = match self.take_slot() { Some(s) => s.st, None => 0 };
                if st == 0 {
                    let l = sus!(self, self.ev_c(a, scope), Slot::st(0));
                    let go = match op { A::Logic::And => l.truthy(), A::Logic::Or => !l.truthy(), A::Logic::Nullish => l.is_nullish() };
                    if !go { return Ok(l); }
                }
                Ok(sus!(self, self.ev_c(b, scope), Slot::st(1)))
            }
            Expr::Cond(t, c, a) => {
                let st = match self.take_slot() { Some(s) => s.st, None => 0 };
                let br = if st == 0 { let v = sus!(self, self.ev_c(t, scope), Slot::st(0)); if v.truthy() { 1 } else { 2 } } else { st };
                if br == 1 { Ok(sus!(self, self.ev_c(c, scope), Slot::st(1))) } else { Ok(sus!(self, self.ev_c(a, scope), Slot::st(2))) }
            }
            Expr::Seq(v) => {
                let start = match self.take_slot() { Some(s) => s.n, None => 0 };
                let mut last = Val::Undef;
                for i in start..v.len() { last = sus!(self, self.ev_c(&v[i], scope), Slot { n: i, ..Default::default() }); }
                Ok(last)
            }
            Expr::Chain(e) => self.ev_r(e, scope),
            Expr::Member { obj, key, optional } => {
                let (st, ov) = match self.take_slot() { Some(s) => (s.st, s.vals.into_iter().next()), None => (0, None) };
                let o = if st == 0 { sus!(self, self.ev_c(obj, scope), Slot::st(0)) } else { ov.unwrap_or(Val::Undef) };
                if *optional && o.is_nullish() { return Ok(Val::Undef); }
                match &**key { A::Key::Static(k) => self.get(&o, k), A::Key::Computed(e) => { let k = sus!(self, self.ev_c(e, scope), Slot { st: 1, vals: vec![o.clone()], ..Default::default() }); self.get_v(&o, &k) } }
            }
            Expr::Call { callee, args, spread, .. } | Expr::New { callee, args, spread } => {
                let is_new = matches!(n, Expr::New { .. });
                let optional = if let Expr::Call { optional, .. } = n { *optional } else { false };
                let (st, n0, mut vals) = match self.take_slot() { Some(s) => (s.st, s.n, s.vals), None => (0, 0, Vec::new()) };
                if st == 0 {
                    let inner = match &**callee { Expr::Y(i) => &**i, c => c };
                    let (f, this) = match inner {
                        Expr::Member { obj, key, optional: mo } if !is_new => {
                            let o = sus!(self, self.ev_c(obj, scope), Slot::st(0));
                            if *mo && o.is_nullish() { return Ok(Val::Undef); }
                            let f = match &**key { A::Key::Static(k) => self.get(&o, k)?, A::Key::Computed(e) => { let k = self.ev(e, scope)?; self.get_v(&o, &k)? } };
                            (f, o)
                        }
                        Expr::SuperMember(k) if !is_new => self.super_get(k, scope)?,
                        _ => (sus!(self, self.ev_c(callee, scope), Slot::st(0)), Val::Undef),
                    };
                    if optional && f.is_nullish() { return Ok(Val::Undef); }
                    if !is_new && !f.is_callable() { type_err!(self, "{} is not a function", match inner { Expr::Ident(n) => n.to_string(), Expr::Member { key, .. } => match &**key { A::Key::Static(k) => k.to_string(), _ => "expression".into() }, _ => "expression".into() }); }
                    vals = vec![f, this];
                }
                for i in n0..args.len() {
                    let v = sus!(self, self.ev_c(&args[i], scope), Slot { st: 1, n: i, vals: vals.clone(), ..Default::default() });
                    if spread[i] { vals.extend(self.iterate(&v)?); } else { vals.push(v); }
                }
                let mut it = vals.into_iter();
                let f = it.next().unwrap_or(Val::Undef); let this = it.next().unwrap_or(Val::Undef);
                if is_new { self.construct(&f, it.collect()) } else { self.call(&f, this, it.collect()) }
            }
            Expr::Assign(op, target, rhs) => {
                // slot st: 0 the reference (object, key, current value); 1 the right side
                let (st, mut vals) = match self.take_slot() { Some(s) => (s.st, s.vals), None => (0, Vec::new()) };
                if matches!(&**target, A::Target::Super(_)) { return self.ev(n, scope); }
                if st == 0 {
                    if let A::Target::Member(o, k) = &**target {
                        let ov = sus!(self, self.ev_c(o, scope), Slot::st(0));
                        let kv = match k { A::Key::Static(n) => Val::Str(n.clone()), A::Key::Computed(e) => self.ev(e, scope)? };
                        vals = vec![ov, kv];
                    }
                    if !matches!(op, A::AssignOp::Set) {
                        let cur = match &**target { A::Target::Member(..) => self.get_v(&vals[0], &vals[1])?, A::Target::Pat(Pat::Ident(n)) => self.lookup(scope, n)?, _ => Val::Undef };
                        let go = match op { A::AssignOp::AndSet => cur.truthy(), A::AssignOp::OrSet => !cur.truthy(), A::AssignOp::NullishSet => cur.is_nullish(), _ => true };
                        if !go { return Ok(cur); }
                        vals.push(cur);
                    }
                }
                let r = sus!(self, self.ev_c(rhs, scope), Slot { st: 1, vals: vals.clone(), ..Default::default() });
                let v = match op { A::AssignOp::Bin(b) => { let cur = vals.last().cloned().unwrap_or(Val::Undef); self.binop_t(*b, cur, r)? } _ => r };
                match &**target { A::Target::Member(..) => self.set_v(&vals[0], &vals[1], v.clone())?, A::Target::Pat(p) => self.assign_pat(p, v.clone(), scope)?, A::Target::Super(k) => self.super_set(k, v.clone(), scope)?, A::Target::Slot(d, i, n) => self.slot_set(scope, *d, *i, n, v.clone())? }
                Ok(v)
            }
            other => self.ev(other, scope),
        }
    }

    /// `yield* iterable` (§15.5.5): next / throw / return are forwarded to the inner iterator until it is done.
    fn yield_star(&mut self, arg: Option<&Expr>, scope: &Scope) -> Result<Val, Ctl> {
        let (st, vals) = match self.take_slot() { Some(s) => (s.st, s.vals), None => (0, vec![]) };
        let (iter, next, received) = if st == 0 {
            let src = match arg { Some(e) => sus!(self, self.ev_c(e, scope), Slot::st(0)), None => Val::Undef };
            let f = self.get(&src, K_ITER)?;
            if !f.is_callable() { type_err!(self, "yield* operand is not iterable"); }
            let it = self.call(&f, src, vec![])?;
            let next = self.get(&it, "next")?;
            (it, next, Resume::Next(Val::Undef))
        } else {
            let sent = self.co.last_mut().and_then(|c| c.sent.take()).unwrap_or(Resume::Next(Val::Undef));
            (vals.first().cloned().unwrap_or(Val::Undef), vals.get(1).cloned().unwrap_or(Val::Undef), sent)
        };
        let res = match received {
            Resume::Next(v) => self.call(&next, iter.clone(), vec![v])?,
            Resume::Throw(e) => {
                let t = self.get(&iter, "throw")?;
                if t.is_callable() { self.call(&t, iter.clone(), vec![e])? } else { let r = self.get(&iter, "return")?; if r.is_callable() { self.call(&r, iter.clone(), vec![])?; } type_err!(self, "The iterator does not provide a 'throw' method"); }
            }
            Resume::Return(v) => {
                let r = self.get(&iter, "return")?;
                if !r.is_callable() { return Err(Ctl::Return(v)); }
                let res = self.call(&r, iter.clone(), vec![v])?;
                if !matches!(res, Val::Obj(_)) { type_err!(self, "iterator result is not an object"); }
                if self.get(&res, "done")?.truthy() { let v = self.get(&res, "value")?; return Err(Ctl::Return(v)); }
                res
            }
        };
        if !matches!(res, Val::Obj(_)) { type_err!(self, "iterator result is not an object"); }
        if self.get(&res, "done")?.truthy() { return self.get(&res, "value"); }
        let v = self.get(&res, "value")?;
        Err(self.suspend(v, Slot { st: 1, vals: vec![iter, next], ..Default::default() }, false))
    }

    /// GeneratorResume / GeneratorResumeAbrupt (§27.5.3.3–4): run the body to its next suspension, its end, or a throw.
    /// Returns (value, done).
    pub fn gen_resume(&mut self, g: &Rc<RefCell<Coro>>, r: Resume) -> Result<(Val, bool), Ctl> {
        let state = g.borrow().state;
        match state {
            CO_RUN => type_err!(self, "Generator is already running"),
            CO_DONE => return match r { Resume::Next(_) => Ok((Val::Undef, true)), Resume::Return(v) => Ok((v, true)), Resume::Throw(e) => Err(Ctl::Throw(e)) },
            CO_START => match r { Resume::Return(v) => { g.borrow_mut().state = CO_DONE; return Ok((v, true)); } Resume::Throw(e) => { g.borrow_mut().state = CO_DONE; return Err(Ctl::Throw(e)); } _ => {} },
            _ => {}
        }
        self.stack_check()?;
        let (clo, scope, frames) = { let mut b = g.borrow_mut(); b.state = CO_RUN; (b.clo.clone(), b.scope.clone(), std::mem::take(&mut b.frames)) };
        let fresh = state == CO_START;
        self.co.push(CoCtx { resume: frames, rpos: 0, saving: Vec::new(), sent: if fresh { None } else { Some(r) }, out: Val::Undef, awaiting: false });
        let func = clo.func.clone();
        let res = match &func.b().body {
            Body::Block(b) => { if fresh { self.hoist_fns(b, &scope); } self.stmts_r(b, &scope).map(|_| Val::Undef) }
            Body::Expr(e) => self.ev_c(e, &scope),
        };
        let ctx = self.co.pop().unwrap();
        let mut b = g.borrow_mut();
        b.awaiting = ctx.awaiting;
        match res {
            Err(Ctl::Suspend) => { let mut fr = ctx.saving; fr.reverse(); b.frames = fr; b.state = CO_SUSP; Ok((ctx.out, false)) }
            Ok(v) => { b.state = CO_DONE; Ok((v, true)) }
            Err(Ctl::Return(v)) => { b.state = CO_DONE; Ok((v, true)) }
            Err(Ctl::Throw(e)) => { b.state = CO_DONE; Err(Ctl::Throw(e)) }
            Err(_) => { b.state = CO_DONE; Ok((Val::Undef, true)) }
        }
    }

    /// An async function call (§27.7.5.1 AsyncFunctionStart): runs synchronously to the first await.
    fn async_start(&mut self, c: Rc<Closure>, scope: Scope) -> Val {
        let p = new_promise();
        let g = Rc::new(RefCell::new(Coro { clo: c, scope, frames: Vec::new(), state: CO_START, kind: 1, promise: Some(p.clone()), queue: VecDeque::new(), busy: false, awaiting: false }));
        self.async_step(&g, Resume::Next(Val::Undef));
        p
    }
    /// One leg of an async function (§27.7.5.3 Await): to the next await (then wait on that promise), or settle the result.
    fn async_step(&mut self, g: &Rc<RefCell<Coro>>, r: Resume) {
        let p = g.borrow().promise.clone().unwrap_or(Val::Undef);
        match self.gen_resume(g, r) {
            Ok((v, false)) => self.await_then(g, v, false),
            Ok((v, true)) => self.resolve_promise(&p, v),
            Err(Ctl::Throw(e)) => self.settle(&p, false, e),
            Err(_) => self.resolve_promise(&p, Val::Undef),
        }
    }
    /// Subscribe a suspended coroutine to the awaited value: it resumes from a microtask once that settles.
    fn await_then(&mut self, g: &Rc<RefCell<Coro>>, v: Val, agen: bool) {
        let awaited = self.promise_resolve(v);
        let (g1, g2) = (g.clone(), g.clone());
        let first = |a: Vec<Val>| a.into_iter().next().unwrap_or(Val::Undef);
        let (onf, onr) = if agen {
            (Val::native("", move |it, _t, a| { it.agen_step(&g1, Resume::Next(first(a))); Ok(Val::Undef) }), Val::native("", move |it, _t, a| { it.agen_step(&g2, Resume::Throw(first(a))); Ok(Val::Undef) }))
        } else {
            (Val::native("", move |it, _t, a| { it.async_step(&g1, Resume::Next(first(a))); Ok(Val::Undef) }), Val::native("", move |it, _t, a| { it.async_step(&g2, Resume::Throw(first(a))); Ok(Val::Undef) }))
        };
        self.promise_react(&awaited, onf, onr, None);
    }
    /// AsyncGenerator next / throw / return (§27.6.3.2 AsyncGeneratorEnqueue): requests queue and settle in order.
    pub fn agen_request(&mut self, g: &Rc<RefCell<Coro>>, r: Resume) -> Val {
        let p = new_promise();
        let start = { let mut b = g.borrow_mut(); b.queue.push_back((r.clone(), p.clone())); !b.busy && b.queue.len() == 1 };
        if start { self.agen_step(g, r); }
        p
    }
    fn agen_step(&mut self, g: &Rc<RefCell<Coro>>, r: Resume) {
        g.borrow_mut().busy = true;
        let res = self.gen_resume(g, r);
        let awaiting = g.borrow().awaiting;
        match res {
            Ok((v, _)) if awaiting => self.await_then(g, v, true),
            // falling off the end settles at once (§27.6.3.2 AsyncGeneratorStart step 4)
            Ok((v, true)) => { let ir = Self::iter_result(v, true); self.agen_settle(g, Ok(ir)) }
            // `yield v` is AsyncGeneratorYield(? Await(v)) (§15.5.5, §27.6.3.8): the value is awaited before the request
            // settles; a rejection is thrown back in at the yield
            Ok((v, done)) => {
                let pv = self.promise_resolve(v);
                let (g1, g2) = (g.clone(), g.clone());
                let first = |a: Vec<Val>| a.into_iter().next().unwrap_or(Val::Undef);
                let onf = Val::native("", move |it, _t, a| { let ir = Self::iter_result(first(a), done); it.agen_settle(&g1, Ok(ir)); Ok(Val::Undef) });
                let onr = Val::native("", move |it, _t, a| { if done { it.agen_settle(&g2, Err(first(a))); } else { it.agen_step(&g2, Resume::Throw(first(a))); } Ok(Val::Undef) });
                self.promise_react(&pv, onf, onr, None);
            }
            Err(Ctl::Throw(e)) => self.agen_settle(g, Err(e)),
            Err(_) => { let ir = Self::iter_result(Val::Undef, true); self.agen_settle(g, Ok(ir)) }
        }
    }
    /// AsyncGeneratorCompleteStep (§27.6.3.4): settle the oldest request, then serve the next one.
    fn agen_settle(&mut self, g: &Rc<RefCell<Coro>>, r: Result<Val, Val>) {
        let req = g.borrow_mut().queue.pop_front();
        if let Some((_, p)) = req { match r { Ok(v) => self.resolve_promise(&p, v), Err(e) => self.settle(&p, false, e) } }
        g.borrow_mut().busy = false;
        let next = g.borrow().queue.front().map(|x| x.0.clone());
        if let Some(nr) = next { self.agen_step(g, nr); }
    }
    /// GetIterator(obj, async) (§7.4.3): @@asyncIterator first, else the sync iterator (its values are awaited).
    fn async_iter_open(&mut self, v: &Val) -> Result<IterRec, Ctl> {
        let f = if matches!(v, Val::Obj(_)) { self.get(v, K_ASYNC_ITER)? } else { Val::Undef };
        if f.is_callable() { let it = self.call(&f, v.clone(), vec![])?; let next = self.get(&it, "next")?; return Ok(IterRec::Async(it, next)); }
        self.iter_open(v)
    }

    // ── promises and the job queue (ECMA-262 §27.2, §9.5) ──
    /// FulfillPromise / RejectPromise (§27.2.1.4, §27.2.1.7): settle once, then queue every reaction as a microtask.
    pub fn settle(&mut self, p: &Val, ok: bool, v: Val) {
        let reacts = {
            let Val::Obj(o) = p else { return };
            let mut b = o.borrow_mut();
            let Kind::Promise(s) = &mut b.kind else { return };
            if s.st != 0 { return; }
            s.st = if ok { 1 } else { 2 }; s.v = v.clone();
            if !ok && !s.handled { self.unhandled.push(p.clone()); }
            std::mem::take(&mut s.reacts)
        };
        for (onf, onr, d) in reacts { self.jobs.push_back(Job::React { handler: if ok { onf } else { onr }, arg: v.clone(), derived: d, ok }); }
    }
    /// Promise resolve functions (§27.2.1.3.2): a thenable is adopted through a NewPromiseResolveThenableJob.
    pub fn resolve_promise(&mut self, p: &Val, v: Val) {
        if let Val::Obj(vo) = &v {
            if let Val::Obj(po) = p { if Rc::ptr_eq(vo, po) { let e = self.error("TypeError", "Chaining cycle detected for promise #<Promise>"); self.settle(p, false, e); return; } }
            match self.get(&v, "then") {
                Err(Ctl::Throw(e)) => { self.settle(p, false, e); return; }
                Ok(t) if t.is_callable() => { self.jobs.push_back(Job::Thenable { p: p.clone(), obj: v, then: t }); return; }
                _ => {}
            }
        }
        self.settle(p, true, v);
    }
    /// CreateResolvingFunctions (§27.2.1.3): one-shot resolve / reject for `p`.
    pub fn resolving_functions(&mut self, p: &Val) -> (Val, Val) {
        let done = Rc::new(std::cell::Cell::new(false));
        let (p1, d1, p2, d2) = (p.clone(), done.clone(), p.clone(), done);
        (Val::native("resolve", move |it, _t, a| { if !d1.replace(true) { it.resolve_promise(&p1, a.into_iter().next().unwrap_or(Val::Undef)); } Ok(Val::Undef) }),
         Val::native("reject", move |it, _t, a| { if !d2.replace(true) { it.settle(&p2, false, a.into_iter().next().unwrap_or(Val::Undef)); } Ok(Val::Undef) }))
    }
    /// PerformPromiseThen (§27.2.5.4.1); `derived` = the promise `then` returns (None for internal awaits).
    pub fn promise_react(&mut self, p: &Val, onf: Val, onr: Val, derived: Option<Val>) {
        let Val::Obj(o) = p else { return };
        let mut b = o.borrow_mut();
        let Kind::Promise(s) = &mut b.kind else { return };
        s.handled = true;
        match s.st {
            0 => s.reacts.push((onf, onr, derived)),
            1 => { let v = s.v.clone(); drop(b); self.jobs.push_back(Job::React { handler: onf, arg: v, derived, ok: true }); }
            _ => { let v = s.v.clone(); drop(b); self.jobs.push_back(Job::React { handler: onr, arg: v, derived, ok: false }); }
        }
    }
    pub fn promise_then(&mut self, p: &Val, onf: Val, onr: Val) -> Val { let d = new_promise(); self.promise_react(p, onf, onr, Some(d.clone())); d }
    /// Promise.prototype.then (§27.2.5.4): the derived promise comes from SpeciesConstructor(promise, %Promise%); a foreign
    /// constructor gets a NewPromiseCapability whose resolve / reject the reaction calls.
    pub fn promise_then_species(&mut self, p: &Val, onf: Val, onr: Val) -> Result<Val, Ctl> {
        let c = self.get(p, "constructor")?;
        let builtin = |v: &Val| matches!(v, Val::Obj(o) if matches!(&o.borrow().kind, Kind::Native(_, "Promise")));
        if matches!(c, Val::Undef) || builtin(&c) { return Ok(self.promise_then(p, onf, onr)); }
        if !matches!(c, Val::Obj(_)) { type_err!(self, "The .constructor property is not an object"); }
        let sp = self.get(&c, K_SPECIES)?;
        if sp.is_nullish() || builtin(&sp) { return Ok(self.promise_then(p, onf, onr)); }
        let slot = Rc::new(RefCell::new((Val::Undef, Val::Undef)));
        let s2 = slot.clone();
        let exec = Val::native("", move |_it, _t, a| { let mut b = s2.borrow_mut(); b.0 = a.first().cloned().unwrap_or(Val::Undef); b.1 = a.get(1).cloned().unwrap_or(Val::Undef); Ok(Val::Undef) });
        let derived = self.construct(&sp, vec![exec])?;
        let (res, rej) = slot.borrow().clone();
        let cap = Val::plain(vec![("\u{0}cap_promise", derived.clone()), ("\u{0}cap_resolve", res), ("\u{0}cap_reject", rej)]);
        self.promise_react(p, onf, onr, Some(cap));
        Ok(derived)
    }
    /// PromiseResolve(%Promise%, x) (§27.2.4.7.1): a promise stays itself, anything else is wrapped (thenables adopted).
    pub fn promise_resolve(&mut self, v: Val) -> Val { if let Val::Obj(o) = &v { if matches!(o.borrow().kind, Kind::Promise(_)) { return v; } } let p = new_promise(); self.resolve_promise(&p, v); p }

    fn run_one_job(&mut self) {
        let Some(j) = self.jobs.pop_front() else { return };
        self.steps += 1;
        match j {
            Job::React { handler, arg, derived, ok } => {
                let r = if handler.is_callable() { self.call(&handler, Val::Undef, vec![arg]) } else if ok { Ok(arg) } else { Err(Ctl::Throw(arg)) };
                if let Some(d) = derived {
                    let is_cap = matches!(&d, Val::Obj(o) if matches!(o.borrow().kind, Kind::Plain));
                    if is_cap {
                        let (ok, v) = match r { Ok(v) => (true, v), Err(Ctl::Throw(e)) => (false, e), Err(_) => (true, Val::Undef) };
                        let f = self.get(&d, if ok { "\u{0}cap_resolve" } else { "\u{0}cap_reject" }).unwrap_or(Val::Undef);
                        if f.is_callable() { let _ = self.call(&f, Val::Undef, vec![v]); }
                    } else { match r { Ok(v) => self.resolve_promise(&d, v), Err(Ctl::Throw(e)) => self.settle(&d, false, e), Err(_) => self.resolve_promise(&d, Val::Undef) } }
                }
            }
            Job::Thenable { p, obj, then } => { let (res, rej) = self.resolving_functions(&p); if let Err(Ctl::Throw(e)) = self.call(&then, obj, vec![res, rej.clone()]) { let _ = self.call(&rej, Val::Undef, vec![e]); } }
            Job::Call(f, args) => { let _ = self.call(&f, Val::Undef, args); }
        }
    }
    /// A microtask checkpoint (HTML §8.1.7.3): run jobs until the queue is empty or the step budget is spent.
    /// Not re-entrant: a job's own callbacks returning to depth 0 do not start a nested checkpoint.
    pub fn run_jobs(&mut self) {
        if self.in_checkpoint { return; }
        self.in_checkpoint = true;
        while !self.jobs.is_empty() { if self.steps > self.limit { self.rejection_log.push("microtask queue dropped: the step budget ran out".into()); self.jobs.clear(); break; } self.run_one_job(); }
        self.in_checkpoint = false;
        // HTML §8.1.5.7 unhandled promise rejections: a rejected promise nobody reacted to by the end of the checkpoint
        for p in std::mem::take(&mut self.unhandled) {
            let (st, v) = promise_state(&p);
            let handled = matches!(&p, Val::Obj(o) if matches!(&o.borrow().kind, Kind::Promise(s) if s.handled));
            if st == 2 && !handled { let t = self.err_text(&v); self.rejection_log.push(format!("Uncaught (in promise) {t}")); mark_handled(&p); }
        }
    }

    /// Resolve and run a module once; returns its exports object. Host modules answer first, then the page's loader
    /// (`module_loader`), then IR files inside module_dir as `<stem>.ir.json` (the stem of the specifier's last path segment).
    pub fn import(&mut self, spec: &str) -> Result<Val, Ctl> {
        if let Some(m) = crate::jsi_lib::host_module(self, spec) { return Ok(m); }
        if let Some(loader) = self.module_loader.clone() {
            let referrer = self.cur_module.last().cloned().unwrap_or_default();
            if let Some((key, prog)) = loader(spec, &referrer) {
                if let Some(m) = self.modules.get(&key) { return Ok(m.clone()); }
                return self.module_body(&key, &prog);
            }
            if self.module_dir.is_none() { throw!(self, "Cannot find module '{spec}'"); }
        }
        let stem = spec.rsplit('/').next().unwrap_or(spec).trim_end_matches(".js").trim_end_matches(".mjs").trim_end_matches(".svelte").to_string();
        if spec.ends_with(".svelte") { return Ok(Val::plain(vec![("default", st(&format!("\u{1}comp:{stem}")))])); }
        if let Some(m) = self.modules.get(&stem) { return Ok(m.clone()); }
        let dir = match &self.module_dir { Some(d) => d.clone(), None => throw!(self, "cannot import {spec}: no module directory") };
        let path = dir.join(format!("{stem}.ir.json"));
        let prog: J = match std::fs::read(&path).ok().and_then(|b| ir_json(&b)) { Some(p) => p, None => throw!(self, "module not found: {spec} ({})", path.display()) };
        self.module_body(&stem, &prog)
    }
    /// Evaluate one module body under `key` (§16.2.1.5.3 ModuleEvaluation, without live bindings): its exports object.
    fn module_body(&mut self, key: &str, prog: &J) -> Result<Val, Ctl> {
        let exports = Val::obj(Kind::Plain);
        self.modules.insert(key.to_string(), exports.clone()); // cycles see the partial exports, as in ES
        let scope = Self::child(&self.global.clone());
        self.exports.push(exports.clone()); self.cur_module.push(key.to_string());
        let body = A::module_program(&prog["body"]);
        self.enter();
        self.hoist_program(&body, &scope);
        let r = self.exec_block(&body, &scope);
        self.exports.pop(); self.cur_module.pop();
        self.leave();
        match r { Ok(()) | Err(Ctl::Return(_)) => Ok(exports), Err(e) => Err(e) }
    }
    /// Run a Program as an ES module under `key` (imports resolve through `module_loader` relative to it).
    /// Returns the uncaught exception text, if any; microtasks run after it.
    pub fn run_module(&mut self, key: &str, prog: &J) -> Option<String> {
        let r = self.module_body(key, prog);
        let out = match r { Ok(_) => None, Err(Ctl::Throw(v)) => Some(self.err_text(&v)), Err(_) => Some("illegal break/continue".into()) };
        self.run_jobs();
        out
    }

    /// PerformEval (§19.2.1.1) on source text: `var`s and functions join `scope`, lexical declarations stay inside; the
    /// result is the completion value of the last expression statement.
    pub fn eval_src(&mut self, src: &str, scope: &Scope) -> Result<Val, Ctl> {
        let prog = crate::jsparse::parse(src, false).map_err(|e| self.err("SyntaxError", &e))?;
        let body = A::program(&prog["body"]);
        let mut vars = Vec::new(); A::collect_vars(&body, &mut vars);
        { let mut sb = scope.borrow_mut(); for v in vars { if !sb.has(&v) { sb.put(&v, Val::Undef); } } }
        self.hoist_fns(&body, scope);
        let sc = Self::child(scope);
        let mut last = Val::Undef;
        for stmt in &body { match stmt { Stmt::Expr(e) => last = self.ev(e, &sc)?, other => self.ex(other, &sc)? } }
        Ok(last)
    }
    /// CreateDynamicFunction (§20.2.1.1.1): `Function(p1, …, body)` compiled in the global scope; kind = "function",
    /// "async function" or "function*".
    pub fn dynamic_function(&mut self, args: &[Val], kind: &str) -> Result<Val, Ctl> {
        let mut parts = Vec::new(); for a in args { parts.push(self.to_str(a)?.to_string()); }
        let body = parts.pop().unwrap_or_default();
        let src = format!("({kind} anonymous({}\n) {{\n{body}\n}})", parts.join(","));
        let prog = crate::jsparse::parse(&src, false).map_err(|e| self.err("SyntaxError", &e))?;
        let e = A::expr(&prog["body"][0]["expression"]);
        let g = self.global.clone();
        self.ev(&e, &g)
    }

    /// HTML §8.6 timer initialisation: schedule `f` after `delay` ms of virtual time (repeating every `delay` ms when
    /// `repeat`); returns the handle (index + 1) clearTimeout takes.
    pub fn add_timer(&mut self, f: Val, delay: f64, repeat: bool, args: Vec<Val>) -> usize {
        let d = if delay.is_nan() || delay < 0.0 { 0.0 } else { delay };
        self.timers.push((f, self.timer_now + d, repeat, args));
        if repeat { self.timer_meta.insert(self.timers.len() - 1, (d, 0)); }
        self.timers.len()
    }
    /// Timers still scheduled.
    pub fn pending_timers(&self) -> usize { self.timers.iter().filter(|x| x.0.is_callable()).count() }
    /// Advance the virtual clock by `ms`: fire every timer that comes due, in due order (ties in creation order), with a
    /// microtask checkpoint after each callback (HTML §8.1.7.3). Intervals re-arm at their period (clamped to ≥ 4 ms once
    /// nested deeper than 5, §8.6 step 5). Returns the callbacks run.
    /// OMR_PROF=1: the timers still waiting (due time on the page's virtual clock, repeat, which function)
    pub fn timers_report(&self) {
        if !prof_on() { return; }
        eprintln!("timers waiting: {} (virtual now {:.0} ms)", self.timers.iter().filter(|x| x.0.is_callable()).count(), self.timer_now);
        for (f, due, rep, _) in self.timers.iter().filter(|x| x.0.is_callable()).take(40) {
            let d = match f { Val::Obj(o) => match &o.borrow().kind { Kind::Func(c) => format!("{}{}", if c.func.name.is_empty() { "<anon>".to_string() } else { c.func.name.to_string() }, c.func.lazy_addr().map(|a| { let t = crate::jscomp::fetch(a)["body"].to_string(); format!(" @{a} {}", t.chars().take(160).collect::<String>()) }).unwrap_or_default()), Kind::Native(_, n) => format!("native:{n}"), _ => "?".into() }, _ => "?".into() };
            eprintln!("  timer due {:9.0} ms {} {}", due, if *rep { "every" } else { "once " }, d);
        }
    }
    pub fn run_timers(&mut self, ms: f64) -> usize {
        crate::jsi_date::vclock_advance(ms);
        self.run_jobs();
        let mut ran = 0;
        loop {
            if self.steps > self.limit || ran > 1_000_000 { break; }
            let next = (0..self.timers.len()).filter(|&i| self.timers[i].0.is_callable() && self.timers[i].1 <= ms)
                .min_by(|&a, &b| self.timers[a].1.partial_cmp(&self.timers[b].1).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
            let Some(i) = next else { break };
            let t = self.timers[i].1.max(self.timer_now);
            self.timer_now = t;
            let (f, due, repeat, args) = self.timers[i].clone();
            if repeat {
                let (period, fired) = self.timer_meta.get(&i).copied().unwrap_or((due.max(0.0), 0));
                let p = if fired >= 5 { period.max(4.0) } else { period.max(1.0) };
                self.timers[i].1 = t + p;
                self.timer_meta.insert(i, (period, fired + 1));
            } else { self.timers[i].0 = Val::Undef; }
            let _ = self.call(&f, Val::Undef, args); ran += 1;
            self.run_jobs();
        }
        // the clock moved by ms: what remains keeps its delay relative to the new "now"
        for x in self.timers.iter_mut() { x.1 = (x.1 - ms).max(0.0); }
        self.timer_now = 0.0;
        ran
    }

    fn err_text(&mut self, v: &Val) -> String { self.to_str(v).map(|x| x.to_string()).unwrap_or_else(|_| "exception".into()) }

    /// Run a whole program (estree Program) as a classic script, then a microtask checkpoint. Returns the uncaught
    /// exception text, if any.
    pub fn fnstat_report(&self) { if fnstat_on() { let (t, c) = fn_stats(); eprintln!("fnstat: {t} functions made, {} lowered, {c} called ({:.1} %)", FN_FORCED.with(|x| x.get()), 100.0 * c as f64 / t.max(1) as f64); } }
    pub fn run_program(&mut self, prog: &J) -> Option<String> {
        let g = self.global.clone();
        let body = A::program(&prog["body"]);
        if self.depth == 0 { let x = 0u8; self.stack_base = &x as *const u8 as usize; }
        self.depth += 1;
        self.hoist_program(&body, &g);
        let r = self.exec_block(&body, &g);
        self.depth = self.depth.saturating_sub(1);
        let out = match r {
            Ok(()) | Err(Ctl::Return(_)) => None,
            Err(Ctl::Throw(v)) => Some(self.err_text(&v)),
            Err(_) => Some("illegal break/continue".into()),
        };
        self.run_jobs();
        out
    }
}

pub fn new_promise() -> Val { Val::obj(Kind::Promise(Box::new(PState { st: 0, v: Val::Undef, reacts: Vec::new(), handled: false }))) }
/// (state, value): 0 pending, 1 fulfilled, 2 rejected; a non-promise reads as fulfilled with itself.
pub fn promise_state(p: &Val) -> (u8, Val) { if let Val::Obj(o) = p { if let Kind::Promise(s) = &o.borrow().kind { return (s.st, s.v.clone()); } } (1, p.clone()) }
pub fn mark_handled(p: &Val) { if let Val::Obj(o) = p { if let Kind::Promise(s) = &mut o.borrow_mut().kind { s.handled = true; } } }
/// str_at over the cached unit view.
pub fn str_at_rc(x: &Rc<str>, i: usize) -> Val {
    match units(x) { None => if i < x.len() { st(&x[i..i + 1]) } else { Val::Undef }, Some(u) => u.get(i).map(|c| st(&str_of16(&[*c]))).unwrap_or(Val::Undef) }
}
/// The UTF-16 code unit at index i, as a one-unit string (§22.1.3.1 charAt semantics; undefined past the end).
pub fn str_at(x: &str, i: usize) -> Val {
    if x.is_ascii() { return if i < x.len() { st(&x[i..i + 1]) } else { Val::Undef }; }
    let u = utf16_of(x);
    u.get(i).map(|c| st(&str_of16(&[*c]))).unwrap_or(Val::Undef)
}

/// Parse estree IR JSON. JSON.stringify writes a lone surrogate as `\udXXX`, which serde_json refuses; such escapes are
/// re-spelled as the private-use carrier this engine uses for lone surrogates (see LONE) and the text parsed again.
pub fn ir_json(b: &[u8]) -> Option<J> {
    if let Ok(j) = serde_json::from_slice(b) { return Some(j); }
    let hex = |s: &[u8]| -> Option<u32> { u32::from_str_radix(std::str::from_utf8(s).ok()?, 16).ok() };
    let mut out = Vec::with_capacity(b.len()); let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            if b[i + 1] == b'u' && i + 6 <= b.len() {
                if let Some(u) = hex(&b[i + 2..i + 6]) {
                    if (0xD800..0xDC00).contains(&u) && i + 12 <= b.len() && b[i + 6] == b'\\' && b[i + 7] == b'u' && hex(&b[i + 8..i + 12]).map_or(false, |l| (0xDC00..0xE000).contains(&l)) { out.extend_from_slice(&b[i..i + 12]); i += 12; continue; }
                    if (0xD800..0xE000).contains(&u) { let mut t = [0u8; 4]; out.extend_from_slice(char::from_u32(LONE + u).unwrap().encode_utf8(&mut t).as_bytes()); i += 6; continue; }
                }
            }
            out.extend_from_slice(&b[i..i + 2]); i += 2; continue;
        }
        out.push(b[i]); i += 1;
    }
    serde_json::from_slice(&out).ok()
}

/// `omni-render js <program.ir.json>`: run it (then its microtasks and timers, as node's event loop would); console
/// output on stdout; an uncaught exception exits 1. Runs on a large-stack thread so deep recursion reports RangeError.
pub fn cli(path: &str) {
    let path = path.to_string();
    let h = std::thread::Builder::new().stack_size(512 << 20).spawn(move || {
        let prog: J = ir_json(&std::fs::read(&path).expect("ir file")).expect("ir json");
        let mut it = Interp::new();
        it.stack_max = 480 << 20;
        let r = it.run_program(&prog);
        if r.is_none() { it.run_timers(600_000.0); }
        print!("{}", it.out);
        crate::jscomp::forget_store();
        if let Some(e) = r { eprintln!("Uncaught {e}"); std::process::exit(1); }
    }).expect("interpreter thread");
    if h.join().is_err() { std::process::exit(101); }
}

/// `omni-render js-mod <module-dir> <module> [export…]`: import a translated module (fetch → broker client), read each named
/// export — a store's current value via subscribe — and print it as JSON; then the requests the module made.
pub fn mod_cli(args: &[String]) {
    let mut it = Interp::new();
    crate::jsi_lib::install_fetch(&mut it);
    it.module_dir = Some(std::path::PathBuf::from(&args[0]));
    let t = std::time::Instant::now();
    let m = match it.import(&args[1]) { Ok(m) => m, Err(Ctl::Throw(e)) => { let x = it.to_str(&e).map(|x| x.to_string()).unwrap_or_default(); eprintln!("Uncaught {x}"); std::process::exit(1); } Err(_) => std::process::exit(1) };
    it.run_jobs();
    let names: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { it.keys(&m).iter().map(|k| k.to_string()).collect() };
    for n in names {
        let v = it.get(&m, &n).unwrap_or(Val::Undef);
        let is_store = matches!(&v, Val::Obj(_)) && it.get(&v, "subscribe").map_or(false, |f| f.is_callable());
        let shown = if is_store { crate::jsi_lib::store_value(&mut it, &v).unwrap_or(Val::Undef) } else { v };
        let j = crate::jsi_lib::stringify_val(&mut it, &shown);
        println!("{n}{}: {}", if is_store { " (store)" } else { "" }, j.chars().take(400).collect::<String>());
    }
    eprintln!("js-mod {}: {:.1} ms · requests {:?} · timers pending {}", args[1], t.elapsed().as_secs_f64() * 1e3, it.fetch_log, it.timers.iter().filter(|x| x.0.is_callable()).count());
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prog(src: &str, module: bool) -> J { crate::jsparse::parse(src, module).expect("parse") }
    #[test]
    fn module_loader_graph_and_dynamic_import() {
        let mut it = Interp::new();
        it.module_loader = Some(Rc::new(|spec: &str, referrer: &str| {
            let key = if spec.starts_with("./") { format!("{}{}", referrer.rsplit_once('/').map(|x| x.0).unwrap_or(""), &spec[1..]) } else { spec.to_string() };
            let src = match key.as_str() { "/app/a.js" => "import { b } from './b.js'; export const a = 'a' + b; export default 7;", "/app/b.js" => "export const b = 'B' + import.meta.url;", _ => return None };
            Some((key.clone(), crate::jsparse::parse(src, true).ok()?))
        }));
        let r = it.run_module("/app/main.js", &prog("import d, { a } from './a.js'; console.log(a, d); import('./b.js').then(m => console.log('dyn', m.b));", true));
        assert_eq!(r, None, "{}", it.out);
        assert_eq!(it.out, "aB/app/b.js 7\ndyn B/app/b.js\n");
    }
    #[test]
    fn microtasks_before_timers_and_await_after_timer() {
        let mut it = Interp::new();
        let r = it.run_program(&prog("setTimeout(() => console.log('t'), 0); Promise.resolve().then(() => console.log('m')); (async () => { console.log('v', await new Promise(r => setTimeout(() => r(5), 10))); })(); console.log('s');", false));
        assert_eq!(r, None);
        assert_eq!(it.out, "s\nm\n");
        it.run_timers(5.0);
        assert_eq!(it.out, "s\nm\nt\n");
        it.run_timers(10.0);
        assert_eq!(it.out, "s\nm\nt\nv 5\n");
    }
    #[test]
    fn interval_keeps_its_period() {
        let mut it = Interp::new();
        it.run_program(&prog("globalThis.n = 0; setInterval(() => n++, 500);", false));
        for _ in 0..10 { it.run_timers(100.0); }
        let g = it.global.clone();
        assert!(matches!(it.lookup_opt(&g, "n"), Some(Val::Num(x)) if x == 2.0));
    }
}

/// A short source-like description of an expression (OMR_BLAME: which expression was null/undefined).
fn expr_desc(e: &Expr) -> String {
    match e {
        Expr::Ident(n) | Expr::Slot(_, _, n) => n.to_string(),
        Expr::Member { obj, key, .. } => format!("{}.{}", expr_desc(obj), match &**key { A::Key::Static(k) => k.to_string(), _ => "[..]".into() }),
        Expr::Call { callee, .. } => format!("{}(..)", expr_desc(callee)),
        Expr::This => "this".into(),
        _ => "<expr>".into(),
    }
}

fn fnstat_on() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var_os("OMR_FNSTAT").is_some()) }
/// OMR_FNSTAT=1 diagnostic: (functions lowered, distinct functions ever called) — the share of a bundle that actually fires.
pub fn fn_stats() -> (usize, usize) { (FN_TOTAL.with(|c| c.get()), FN_CALLED.with(|h| h.borrow().len())) }
pub fn count_fn() { if fnstat_on() { FN_TOTAL.with(|c| c.set(c.get() + 1)); } }

thread_local! { static FN_FORCED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub fn count_forced() { FN_FORCED.with(|c| c.set(c.get() + 1)); }

pub fn prof_on() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var_os("OMR_PROF").is_some()) }
thread_local! { static PROF: std::cell::RefCell<(HashMap<String, (f64, u64, u64)>, Vec<(f64, u64)>)> = std::cell::RefCell::new((HashMap::new(), Vec::new())); }
pub fn prof_enter() { PROF.with(|p| p.borrow_mut().1.push((0.0, 0))); }
pub fn prof_leave(name: &str, start: std::time::Instant, steps: u64) {
    let el = start.elapsed().as_secs_f64();
    PROF.with(|p| { let mut p = p.borrow_mut(); let (child, cs) = p.1.pop().unwrap_or((0.0, 0)); if let Some(t) = p.1.last_mut() { t.0 += el; t.1 += steps; }
        let e = p.0.entry(name.to_string()).or_insert((0.0, 0, 0)); e.0 += (el - child).max(0.0); e.1 += 1; e.2 += steps.saturating_sub(cs); });
}
/// OMR_PROF=1: self time and call count per function name (closures and natives), top 30 — where the interpreter's time goes.
pub fn prof_report() {
    if !prof_on() { return; }
    PROF.with(|p| { let p = p.borrow(); let mut v: Vec<(&String, &(f64, u64, u64))> = p.0.iter().collect(); v.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap());
        let tot: f64 = v.iter().map(|x| x.1.0).sum(); eprintln!("prof: {:.2} s self time over {} names", tot, v.len());
        for (n, (t, c, st)) in v.iter().take(30) { eprintln!("prof {:7.3} s {:9} calls {:6.2} us/call {:9} steps {:6.2} us/step {}", t, c, t / *c as f64 * 1e6, st, t / (*st).max(1) as f64 * 1e6, n);
            if let Some(a) = n.strip_prefix("<anon@").and_then(|x| x.strip_suffix('>')).and_then(|x| x.parse::<usize>().ok()) { let j = crate::jscomp::fetch(a); let t = j["body"].to_string(); eprintln!("       body: {}", t.chars().take(420).collect::<String>()); } } });
}
