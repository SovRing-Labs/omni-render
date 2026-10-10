// SPDX-License-Identifier: LicenseRef-PolyForm-Small-Business-1.0.0 OR LicenseRef-PolyForm-Noncommercial-1.0.0
//! CODE-ABC (author 10-06, readback confirmed; plan `an unpublished plan`): the doctor's code ALPHABET.
//! Equations are the letters, recurring equation groups the words, programs the sentences.
//! A1 (this file, `lower`): Rust functions → name-free SSA terms over typed primitives, stored in an e-graph. Names are
//!   erased to input slots; unknown calls become opaque nodes, each unique, so an opaque node never equals anything.
//! A2 (`EGraph` + `RULES`): equality saturation, so `x*2`, `x+x` and `x<<1` are one e-class.
//!   Clean-room from Willsey et al., "egg: Fast and Extensible Equality Saturation", POPL 2021: hashcons, union-find,
//!   congruence rebuild, rewrite rules as data, node and iteration limits.
//! Patent US10133657B2 design-around: binding is by equation structure / e-class, never by name text or per-position similarity.
use super::{lex, Tok};
use std::collections::{HashMap, HashSet};

pub type Id = u32;

/// The alphabet. `Var` = an input slot (a name, erased). `Acc(d, k)` / `Item(d)` = loop accumulator / element placeholders,
/// canonical by nesting depth so two folds written in different apps can meet. `Opq(u)` = opaque, unique per site.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Op {
    Num(i64), Flt(u64), Str, Var(u32), Acc(u8, u8), Item(u8), Opq(u32),
    Add, Sub, Mul, Div, Rem, Neg, Min, Max, Abs, Clamp, Pow, Sqrt, Floor, Ceil, Round,
    Lt, Le, Gt, Ge, Eq, Ne, And, Or, Not, Sel, Shl, Shr, BAnd, BOr, BXor,
    Idx, Store, Get(u8), Tup, List, Fill, Range, Map, Filter, Zip, Sum, MaxAll, MinAll, Len, Fold,
}

impl Op {
    pub fn opaque(&self) -> bool { matches!(self, Op::Opq(_)) }
    /// Symbol used by the rule language and by name-free signatures.
    pub fn sym(&self) -> String {
        match self {
            Op::Num(v) => v.to_string(), Op::Flt(b) => format!("{}f", f64::from_bits(*b)), Op::Str => "str".into(), Op::Var(_) => "v".into(),
            Op::Acc(d, k) => format!("acc{d}.{k}"), Op::Item(d) => format!("item{d}"), Op::Opq(_) => "opq".into(), Op::Get(k) => format!("get{k}"),
            o => SYMS.iter().find(|(_, x)| x == o).map(|(s, _)| s.to_string()).unwrap_or_else(|| format!("{o:?}")),
        }
    }
}

const SYMS: &[(&str, Op)] = &[
    ("+", Op::Add), ("-", Op::Sub), ("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem), ("neg", Op::Neg), ("min", Op::Min), ("max", Op::Max),
    ("abs", Op::Abs), ("clamp", Op::Clamp), ("pow", Op::Pow), ("sqrt", Op::Sqrt), ("floor", Op::Floor), ("ceil", Op::Ceil), ("round", Op::Round),
    ("<", Op::Lt), ("<=", Op::Le), (">", Op::Gt), (">=", Op::Ge), ("==", Op::Eq), ("!=", Op::Ne), ("and", Op::And), ("or", Op::Or),
    ("not", Op::Not), ("sel", Op::Sel), ("<<", Op::Shl), (">>", Op::Shr), ("&", Op::BAnd), ("|", Op::BOr), ("^", Op::BXor), ("idx", Op::Idx),
    ("store", Op::Store), ("tup", Op::Tup), ("list", Op::List), ("fill", Op::Fill), ("range", Op::Range), ("map", Op::Map), ("filter", Op::Filter),
    ("zip", Op::Zip), ("sum", Op::Sum), ("maxall", Op::MaxAll), ("minall", Op::MinAll), ("len", Op::Len), ("fold", Op::Fold),
];

pub type Node = (Op, Vec<Id>);

// ── e-graph (A2) ──────────────────────────────────────────────────────────────────────────────────────────────────────────────
#[derive(Default)]
pub struct EGraph { pub uf: Vec<Id>, pub classes: Vec<Vec<Node>>, memo: HashMap<Node, Id>, dirty: bool }

impl EGraph {
    pub fn find(&self, mut a: Id) -> Id { while self.uf[a as usize] != a { a = self.uf[a as usize]; } a }
    fn find_c(&mut self, a: Id) -> Id { let r = self.find(a); let mut x = a; while self.uf[x as usize] != r { let n = self.uf[x as usize]; self.uf[x as usize] = r; x = n; } r }
    fn canon(&mut self, n: &Node) -> Node { (n.0.clone(), n.1.iter().map(|&c| self.find_c(c)).collect()) }
    pub fn add(&mut self, op: Op, kids: Vec<Id>) -> Id {
        let n = self.canon(&(op, kids));
        if let Some(&id) = self.memo.get(&n) { return self.find_c(id); }
        let id = self.uf.len() as Id; self.uf.push(id); self.classes.push(vec![n.clone()]); self.memo.insert(n, id); id
    }
    pub fn union(&mut self, a: Id, b: Id) -> bool {
        let (a, b) = (self.find_c(a), self.find_c(b)); if a == b { return false; }
        let (keep, drop) = if self.classes[a as usize].len() >= self.classes[b as usize].len() { (a, b) } else { (b, a) };
        self.uf[drop as usize] = keep; let moved = std::mem::take(&mut self.classes[drop as usize]); self.classes[keep as usize].extend(moved);
        self.dirty = true; true
    }
    /// Congruence closure by full re-canonicalisation until a fixpoint (egg's deferred rebuild, simplest form).
    pub fn rebuild(&mut self) {
        loop {
            let mut memo: HashMap<Node, Id> = HashMap::with_capacity(self.memo.len());
            let mut merges = Vec::new();
            for c in 0..self.classes.len() as Id {
                if self.find(c) != c { continue; }
                let nodes = std::mem::take(&mut self.classes[c as usize]);
                let mut seen = HashSet::new(); let mut keep = Vec::with_capacity(nodes.len());
                for n in nodes { let n = self.canon(&n); if seen.insert(n.clone()) {
                    if let Some(&o) = memo.get(&n) { if self.find(o) != c { merges.push((o, c)); } } else { memo.insert(n.clone(), c); }
                    keep.push(n); } }
                self.classes[c as usize] = keep;
            }
            self.memo = memo;
            let mut any = false; for (a, b) in merges { any |= self.union(a, b); }
            if !any { break; }
        }
        self.dirty = false;
    }
    pub fn n_nodes(&self) -> usize { self.classes.iter().map(|c| c.len()).sum() }
    /// Smallest term of a class (AST size), with opaque counted heavy; memoised bottom-up by fixpoint.
    pub fn extract(&self) -> HashMap<Id, (u32, Node)> {
        let mut best: HashMap<Id, (u32, Node)> = HashMap::new();
        loop {
            let mut ch = false;
            for c in 0..self.classes.len() as Id {
                if self.find(c) != c { continue; }
                for n in &self.classes[c as usize] {
                    let mut cost: u32 = if n.0.opaque() { 50 } else { 1 };
                    let mut ok = true;
                    for &k in &n.1 { match best.get(&self.find(k)) { Some((x, _)) => cost = cost.saturating_add(*x), None => { ok = false; break } } }
                    if !ok { continue; }
                    let better = match best.get(&c) { None => true, Some((x, m)) => cost < *x || (cost == *x && n < m) };
                    if better { best.insert(c, (cost, n.clone())); ch = true; }
                }
            }
            if !ch { break; }
        }
        best
    }
}

const COMM: &[Op] = &[Op::Add, Op::Mul, Op::Min, Op::Max, Op::Eq, Op::Ne, Op::And, Op::Or, Op::BAnd, Op::BOr, Op::BXor];

/// Name-free signature of a class's best term (A3): input slots and element accesses `idx(leaf, item)` are α-leaves renamed in
/// first-occurrence order (v0, v1, …) AFTER commutative operands are sorted by their leaf-blind shape — so `a + b·c` and
/// `c·b + a` from two apps give one string. Opaque → "opq". `size` = operator count.
pub fn signature(g: &EGraph, best: &HashMap<Id, (u32, Node)>, id: Id, depth: usize) -> (String, u32) {
    fn is_leafy(g: &EGraph, best: &HashMap<Id, (u32, Node)>, id: Id) -> bool {
        let Some((_, n)) = best.get(&g.find(id)) else { return true };
        // an opaque result is an unknown input of the equation
        match n.0 { Op::Var(_) | Op::Acc(..) | Op::Opq(_) => true, Op::Idx => n.1.len() == 2 && is_leafy(g, best, n.1[0]) && matches!(best.get(&g.find(n.1[1])).map(|x| &x.1 .0), Some(Op::Item(_))), _ => false }
    }
    // shape: leaves → "v", used only to order commutative operands
    fn shape(g: &EGraph, best: &HashMap<Id, (u32, Node)>, id: Id, d: usize) -> String {
        if is_leafy(g, best, id) { return "v".into(); }
        let Some((_, n)) = best.get(&g.find(id)) else { return "?".into() };
        if n.1.is_empty() || d == 0 { return if n.1.is_empty() { n.0.sym() } else { "…".into() }; }
        let mut ks: Vec<String> = n.1.iter().map(|&k| shape(g, best, k, d - 1)).collect();
        if COMM.contains(&n.0) { ks.sort(); }
        format!("({} {})", n.0.sym(), ks.join(" "))
    }
    fn go(g: &EGraph, best: &HashMap<Id, (u32, Node)>, id: Id, d: usize, ren: &mut HashMap<Id, usize>, size: &mut u32) -> String {
        let c = g.find(id);
        if is_leafy(g, best, c) { let k = ren.len(); let k = *ren.entry(c).or_insert(k); return format!("v{k}"); }
        let Some((_, n)) = best.get(&c) else { return "?".into() };
        if n.1.is_empty() { return n.0.sym(); }
        if d == 0 { let k = ren.len(); let k = *ren.entry(c).or_insert(k); return format!("v{k}"); } // cut-off = an input
        *size += 1;
        let mut kids: Vec<Id> = n.1.clone();
        if COMM.contains(&n.0) { kids.sort_by_key(|&k| shape(g, best, k, d - 1)); }
        let parts: Vec<String> = kids.iter().map(|&k| go(g, best, k, d - 1, ren, size)).collect();
        format!("({} {})", n.0.sym(), parts.join(" "))
    }
    let mut size = 0; let s = go(g, best, id, depth, &mut HashMap::new(), &mut size); (s, size)
}

/// A4 anti-unification (Plotkin 1970 / Reynolds 1970, least general generalisation) of two classes' best terms: equal operators
/// are kept, every disagreeing pair becomes a hole `?k` — the same pair always the same hole, so `a + (b − a)` and `w + (l − w)`
/// keep their shared variable. Commutative operands try both orders. Returns (pattern, operators kept).
pub fn lgg(g: &EGraph, best: &HashMap<Id, (u32, Node)>, a: Id, b: Id, depth: usize) -> (String, u32) {
    fn leafy(g: &EGraph, best: &HashMap<Id, (u32, Node)>, c: Id) -> bool {
        match best.get(&g.find(c)).map(|x| &x.1) { None => true, Some(n) => matches!(n.0, Op::Var(_) | Op::Acc(..) | Op::Opq(_))
            || (n.0 == Op::Idx && n.1.len() == 2 && leafy(g, best, n.1[0]) && matches!(best.get(&g.find(n.1[1])).map(|x| &x.1 .0), Some(Op::Item(_)))) }
    }
    fn go(g: &EGraph, best: &HashMap<Id, (u32, Node)>, a: Id, b: Id, d: usize, holes: &mut HashMap<(Id, Id), usize>) -> (String, u32) {
        let (a, b) = (g.find(a), g.find(b));
        let hole = |holes: &mut HashMap<(Id, Id), usize>| { let k = holes.len(); let k = *holes.entry((a, b)).or_insert(k); (format!("?{k}"), 0) };
        if d == 0 || leafy(g, best, a) || leafy(g, best, b) { return hole(holes); }
        let (Some((_, x)), Some((_, y))) = (best.get(&a), best.get(&b)) else { return hole(holes) };
        if x.0 != y.0 || x.1.len() != y.1.len() { return hole(holes); }
        if x.1.is_empty() { return (x.0.sym(), 0); }
        let try_order = |ys: &[Id], holes: &mut HashMap<(Id, Id), usize>| { let mut h = holes.clone(); let mut n = 1; let mut parts = Vec::new();
            for (&p, &q) in x.1.iter().zip(ys) { let (s, k) = go(g, best, p, q, d - 1, &mut h); n += k; parts.push(s); } (format!("({} {})", x.0.sym(), parts.join(" ")), n, h) };
        let (s1, n1, h1) = try_order(&y.1, holes);
        if COMM.contains(&x.0) && y.1.len() == 2 { let r: Vec<Id> = vec![y.1[1], y.1[0]]; let (s2, n2, h2) = try_order(&r, holes); if n2 > n1 { *holes = h2; return (s2, n2); } }
        *holes = h1; (s1, n1)
    }
    go(g, best, a, b, depth, &mut HashMap::new())
}

// ── resonance + sieve (author 10-06, readback confirmed): find the words without comparing every pair ──────────────────────────────
/// 1,024-bit structural hypervector of a class's best term: a node = majority of its operator vector and its children's vectors,
/// each child rotated by its position (commutative operands share one rotation, so order does not matter); α-leaves (inputs,
/// opaque results, element accesses) share one LEAF vector — so shape, not names, sets the vector (HDC term encoding; clean-room).
pub type Hv = [u64; 16];
fn seed_hv(mut x: u64) -> Hv { let mut v = [0u64; 16]; for w in v.iter_mut() { x = x.wrapping_add(0x9E37_79B9_7F4A_7C15); let mut z = x; z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB); *w = z ^ (z >> 31); } v }
fn rot(v: &Hv, k: usize) -> Hv { // rotate the 1,024-bit vector left by 67·k bits (word shift + bit shift)
    let k = (k * 67) % 1024; if k == 0 { return *v; } let (w, b) = (k / 64, k % 64); let mut o = [0u64; 16];
    for i in 0..16 { let lo = v[i]; let j = (i + w) % 16; if b == 0 { o[j] |= lo; } else { o[j] |= lo << b; o[(j + 1) % 16] |= lo >> (64 - b); } } o }
fn majority(vs: &[Hv]) -> Hv { // bitwise majority (odd count guaranteed by the caller), counted per word
    let n = vs.len(); let mut o = [0u64; 16];
    for w in 0..16 { let mut out = 0u64; for bit in 0..64 { let c = vs.iter().filter(|v| v[w] >> bit & 1 == 1).count(); if 2 * c > n { out |= 1 << bit; } } o[w] = out; } o }
pub fn hamming(a: &Hv, b: &Hv) -> u32 { a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum() }
pub fn class_hvs(g: &EGraph, best: &HashMap<Id, (u32, Node)>, classes: &[Id], depth: usize) -> HashMap<Id, Hv> { class_hvs_lane(g, best, classes, depth, 0) }
/// one 1,024-bit lane of the structural vector; independent lanes (different seeds) concatenate into wider vectors
pub fn class_hvs_lane(g: &EGraph, best: &HashMap<Id, (u32, Node)>, classes: &[Id], depth: usize, lane: u64) -> HashMap<Id, Hv> {
    fn h(g: &EGraph, best: &HashMap<Id, (u32, Node)>, c: Id, d: usize, memo: &mut HashMap<(Id, usize), Hv>, leaf: &Hv, lane: u64) -> Hv {
        let c = g.find(c);
        if let Some(v) = memo.get(&(c, d)) { return *v; }
        let v = match best.get(&c) {
            None => *leaf,
            Some((_, n)) => {
                let leafy = matches!(n.0, Op::Var(_) | Op::Acc(..) | Op::Opq(_)) || (n.0 == Op::Idx && n.1.len() == 2 && matches!(best.get(&g.find(n.1[1])).map(|x| &x.1 .0), Some(Op::Item(_))));
                if leafy || d == 0 { *leaf } else {
                    let sym = n.0.sym(); let opv = seed_hv(sym.bytes().fold(0xcbf2_9ce4_8422_2325u64 ^ lane.wrapping_mul(0xD6E8_FEB8_6659_FD93), |a, b| (a ^ b as u64).wrapping_mul(0x100_0000_01b3)));
                    if n.1.is_empty() { opv } else {
                        let comm = COMM.contains(&n.0);
                        let mut parts = vec![opv];
                        for (k, &ch) in n.1.iter().enumerate() { let cv = h(g, best, ch, d - 1, memo, leaf, lane); parts.push(rot(&cv, if comm { 1 } else { k + 1 })); }
                        if parts.len() % 2 == 0 { parts.push(opv); }
                        majority(&parts)
                    }
                }
            }
        };
        memo.insert((c, d), v); v
    }
    let leaf = seed_hv(0x1EAF ^ lane.wrapping_mul(0x9E37_79B9)); let mut memo = HashMap::new();
    classes.iter().map(|&c| (g.find(c), h(g, best, c, depth, &mut memo, &leaf, lane))).collect()
}

// ── A5 step 1: search by function (author 10-06, readback confirmed) ─────────────────────────────────────────────────────────────
/// Values of the alphabet's integer + sequence semantics (Rust wrapping arithmetic; division by 0, opaque, out-of-range = Bot).
#[derive(Clone, Debug, PartialEq)]
pub enum Val { I(i64), L(Vec<Val>), T(Vec<Val>), Bot }
impl Val {
    fn i(&self) -> Option<i64> { if let Val::I(x) = self { Some(*x) } else { None } }
    pub fn from_json(j: &serde_json::Value) -> Val { match j { serde_json::Value::Number(n) => n.as_i64().map(Val::I).unwrap_or(Val::Bot), serde_json::Value::Bool(b) => Val::I(*b as i64),
        serde_json::Value::Array(a) => Val::L(a.iter().map(Val::from_json).collect()), _ => Val::Bot } }
}
/// tuples and lists compare as sequences (a need's JSON array matches either)
fn norm(v: Val) -> Val { match v { Val::T(x) | Val::L(x) => Val::L(x.into_iter().map(norm).collect()), o => o } }
pub struct Ev<'g> { g: &'g EGraph, best: &'g HashMap<Id, (u32, Node)>, pub leaf: HashMap<Id, Val>, items: Vec<Val>, accs: Vec<Val>, steps: u32 }
impl<'g> Ev<'g> {
    pub fn new(g: &'g EGraph, best: &'g HashMap<Id, (u32, Node)>) -> Self { Ev { g, best, leaf: HashMap::new(), items: vec![Val::Bot; 8], accs: vec![Val::Bot; 8], steps: 0 } }
    /// iterate a sequence: a range yields its values, anything else yields its indexes (the lowering binds elements as idx(seq, item))
    fn walk(&mut self, seq: Id) -> Option<(Vec<Val>, bool)> {
        let is_range = self.best.get(&self.g.find(seq)).is_some_and(|x| x.1 .0 == Op::Range);
        match self.eval(seq) { Val::L(v) => { if is_range { Some((v, true)) } else { let n = v.len(); Some(((0..n as i64).map(Val::I).collect(), false)) } } _ => None }
    }
    pub fn eval(&mut self, c: Id) -> Val {
        self.steps += 1; if self.steps > 200_000 { return Val::Bot; }
        let c = self.g.find(c);
        if let Some(v) = self.leaf.get(&c) { return v.clone(); }
        let Some((_, n)) = self.best.get(&c) else { return Val::Bot };
        let (op, k) = (n.0.clone(), n.1.clone());
        let i2 = |s: &mut Self, f: &dyn Fn(i64, i64) -> Option<i64>| -> Val { match (s.eval(k[0]).i(), s.eval(k[1]).i()) { (Some(a), Some(b)) => f(a, b).map(Val::I).unwrap_or(Val::Bot), _ => Val::Bot } };
        match op {
            Op::Num(v) => Val::I(v), Op::Flt(b) => { let f = f64::from_bits(b); if f.fract() == 0.0 { Val::I(f as i64) } else { Val::Bot } }
            Op::Item(d) => self.items[d as usize % 8].clone(), Op::Acc(d, _) => self.accs[d as usize % 8].clone(),
            Op::Str | Op::Var(_) | Op::Opq(_) => Val::Bot,
            Op::Add => i2(self, &|a, b| Some(a.wrapping_add(b))), Op::Sub => i2(self, &|a, b| Some(a.wrapping_sub(b))), Op::Mul => i2(self, &|a, b| Some(a.wrapping_mul(b))),
            Op::Div => i2(self, &|a, b| a.checked_div(b)), Op::Rem => i2(self, &|a, b| a.checked_rem(b)),
            Op::Min => i2(self, &|a, b| Some(a.min(b))), Op::Max => i2(self, &|a, b| Some(a.max(b))),
            Op::Lt => i2(self, &|a, b| Some((a < b) as i64)), Op::Le => i2(self, &|a, b| Some((a <= b) as i64)), Op::Gt => i2(self, &|a, b| Some((a > b) as i64)),
            Op::Ge => i2(self, &|a, b| Some((a >= b) as i64)), Op::Eq => i2(self, &|a, b| Some((a == b) as i64)), Op::Ne => i2(self, &|a, b| Some((a != b) as i64)),
            Op::And => i2(self, &|a, b| Some((a != 0 && b != 0) as i64)), Op::Or => i2(self, &|a, b| Some((a != 0 || b != 0) as i64)),
            Op::Shl => i2(self, &|a, b| Some(a.wrapping_shl(b as u32 & 63))), Op::Shr => i2(self, &|a, b| Some(a.wrapping_shr(b as u32 & 63))),
            Op::BAnd => i2(self, &|a, b| Some(a & b)), Op::BOr => i2(self, &|a, b| Some(a | b)), Op::BXor => i2(self, &|a, b| Some(a ^ b)),
            Op::Pow => i2(self, &|a, b| if (0..63).contains(&b) { Some(a.wrapping_pow(b as u32)) } else { None }),
            Op::Neg => self.eval(k[0]).i().map(|a| Val::I(a.wrapping_neg())).unwrap_or(Val::Bot),
            Op::Abs => self.eval(k[0]).i().map(|a| Val::I(a.wrapping_abs())).unwrap_or(Val::Bot),
            Op::Not => self.eval(k[0]).i().map(|a| Val::I((a == 0) as i64)).unwrap_or(Val::Bot),
            Op::Sqrt | Op::Floor | Op::Ceil | Op::Round => match op { Op::Sqrt => Val::Bot, _ => self.eval(k[0]) },
            Op::Clamp => match (self.eval(k[0]).i(), self.eval(k[1]).i(), self.eval(k[2]).i()) { (Some(x), Some(lo), Some(hi)) if lo <= hi => Val::I(x.max(lo).min(hi)), _ => Val::Bot },
            Op::Sel => match self.eval(k[0]).i() { Some(0) => self.eval(k[2]), Some(_) => self.eval(k[1]), None => Val::Bot },
            Op::Tup => Val::T(k.iter().map(|&x| self.eval(x)).collect()), Op::List => Val::L(k.iter().map(|&x| self.eval(x)).collect()),
            Op::Get(j) => match self.eval(k[0]) { Val::T(v) | Val::L(v) => v.get(j as usize).cloned().unwrap_or(Val::Bot), _ => Val::Bot },
            Op::Fill => match (self.eval(k[0]), self.eval(k[1]).i()) { (x, Some(n)) if (0..=256).contains(&n) => Val::L(vec![x; n as usize]), _ => Val::Bot },
            Op::Range => match (self.eval(k[0]).i(), self.eval(k[1]).i()) { (Some(a), Some(b)) if b - a <= 256 => Val::L((a..b.max(a)).map(Val::I).collect()), _ => Val::Bot },
            Op::Idx => match (self.eval(k[0]), self.eval(k[1]).i()) { (Val::L(v), Some(i)) if i >= 0 => v.get(i as usize).cloned().unwrap_or(Val::Bot), _ => Val::Bot },
            Op::Store => match (self.eval(k[0]), self.eval(k[1]).i()) { (Val::L(mut v), Some(i)) if i >= 0 && (i as usize) < v.len() => { v[i as usize] = self.eval(k[2]); Val::L(v) } _ => Val::Bot },
            Op::Len => match self.eval(k[0]) { Val::L(v) => Val::I(v.len() as i64), _ => Val::Bot },
            Op::Sum | Op::MaxAll | Op::MinAll => match self.eval(k[0]) { Val::L(v) => { let xs: Option<Vec<i64>> = v.iter().map(|x| x.i()).collect();
                match (xs, &op) { (Some(xs), Op::Sum) => Val::I(xs.iter().fold(0i64, |a, b| a.wrapping_add(*b))), (Some(xs), Op::MaxAll) => xs.iter().max().map(|x| Val::I(*x)).unwrap_or(Val::Bot),
                    (Some(xs), _) => xs.iter().min().map(|x| Val::I(*x)).unwrap_or(Val::Bot), _ => Val::Bot } } _ => Val::Bot },
            Op::Zip => match (self.eval(k[0]), self.eval(k[1])) { (Val::L(a), Val::L(b)) => Val::L(a.into_iter().zip(b).map(|(x, y)| Val::T(vec![x, y])).collect()), _ => Val::Bot },
            Op::Map | Op::Filter | Op::Fold => {
                let (seq, body) = if op == Op::Fold { (k[0], k[2]) } else { (k[0], k[1]) };
                let d = self.item_depth(body); let Some((xs, is_range)) = self.walk(seq) else { return Val::Bot };
                let src = if is_range { None } else { match self.eval(seq) { Val::L(v) => Some(v), _ => None } };
                let saved = (self.items[d].clone(), self.accs[d].clone());
                let out = match op {
                    Op::Fold => { let mut acc = self.eval(k[1]); for x in xs { self.items[d] = x; self.accs[d] = acc; acc = self.eval(body); if acc == Val::Bot { break; } } acc }
                    Op::Map => { let mut v = Vec::new(); for x in xs { self.items[d] = x; v.push(self.eval(body)); } Val::L(v) }
                    _ => { let mut v = Vec::new(); for (j, x) in xs.into_iter().enumerate() { self.items[d] = x.clone(); if self.eval(body).i().is_some_and(|t| t != 0) { v.push(match &src { Some(s) => s[j].clone(), None => x }); } } Val::L(v) }
                };
                self.items[d] = saved.0; self.accs[d] = saved.1; out
            }
        }
    }
    /// the loop depth a body's Item / Acc placeholders use (first found; 1 when none)
    fn item_depth(&self, body: Id) -> usize {
        let mut st = vec![body]; let mut seen = HashSet::new();
        while let Some(c) = st.pop() { let c = self.g.find(c); if !seen.insert(c) || seen.len() > 400 { continue; }
            if let Some((_, n)) = self.best.get(&c) { match n.0 { Op::Item(d) | Op::Acc(d, _) => return d as usize % 8, _ => st.extend(n.1.iter().copied()) } } }
        1
    }
}

/// Inputs of a class's best term for evaluation: input slots and opaque results (first-occurrence order, commutative operands
/// sorted by shape as in `signature`), each typed List when it is the sequence operand of idx / map / filter / sum / len / zip / fold.
pub fn eval_inputs(g: &EGraph, best: &HashMap<Id, (u32, Node)>, c: Id, depth: usize) -> Vec<(Id, bool)> {
    fn go(g: &EGraph, best: &HashMap<Id, (u32, Node)>, c: Id, d: usize, as_list: bool, out: &mut Vec<(Id, bool)>, seen: &mut HashSet<Id>) {
        let c = g.find(c);
        let Some((_, n)) = best.get(&c) else { return };
        if matches!(n.0, Op::Var(_) | Op::Opq(_)) { if let Some(p) = out.iter_mut().find(|x| x.0 == c) { p.1 |= as_list; } else { out.push((c, as_list)); } return; }
        if d == 0 || !seen.insert(c) { return; }
        let seq0 = matches!(n.0, Op::Idx | Op::Map | Op::Filter | Op::Sum | Op::Len | Op::Zip | Op::Fold | Op::MaxAll | Op::MinAll | Op::Store);
        for (j, &k) in n.1.iter().enumerate() { go(g, best, k, d - 1, seq0 && j == 0 || (n.0 == Op::Zip), out, seen); }
    }
    let mut out = Vec::new(); go(g, best, c, depth, false, &mut out, &mut HashSet::new()); out
}

// ── A5 step 3: build what it needs — bottom-up enumeration, smallest first, observational-equivalence pruning ──────────────
// Clean-room from the published method (bottom-up enumerative synthesis with observational equivalence: Udupa et al. TRANSIT,
// PLDI 2013; Albarghouthi et al. "Recursive program synthesis", CAV 2013). Our alphabet's primitives; terms are kept only if
// their outputs on the examples are new, so equal-behaving terms are never grown twice.
#[derive(Clone, Debug)]
pub enum E { In(usize), K(i64), It, It2, S(u8), Un(Op, Box<E>), Bin(Op, Box<E>, Box<E>), Sel(Box<E>, Box<E>, Box<E>), Red(Op, Box<E>), Map(Box<E>, Box<E>), Filt(Box<E>, Box<E>),
    Map2(Box<E>, Box<E>, Box<E>), Loop2(Box<(E, E)>, Box<E>, Box<(E, E)>, u8) }
fn bin_i(op: &Op, a: i64, b: i64) -> Option<i64> { Some(match op { Op::Add => a.wrapping_add(b), Op::Sub => a.wrapping_sub(b), Op::Mul => a.wrapping_mul(b), Op::Div => a.checked_div(b)?,
    Op::Rem => a.checked_rem(b)?, Op::Min => a.min(b), Op::Max => a.max(b), Op::Lt => (a < b) as i64, Op::Le => (a <= b) as i64, Op::Eq => (a == b) as i64, Op::Ne => (a != b) as i64,
    Op::Gt => (a > b) as i64, Op::And => (a != 0 && b != 0) as i64, Op::Or => (a != 0 || b != 0) as i64, _ => return None }) }
/// evaluation context: lambda elements (`x`, `y` of map / map2) and a loop's two-slot state
#[derive(Clone, Copy, Default)]
pub struct Cx { it: Option<i64>, it2: Option<i64>, s: Option<(i64, i64)> }
/// evaluate a synthesised term on one example (`it` = the lambda element when inside map / filter)
pub fn ev_e(e: &E, args: &[Val], it: Option<i64>) -> Val { ev_x(e, args, Cx { it, ..Cx::default() }) }
pub fn ev_x(e: &E, args: &[Val], c: Cx) -> Val {
    match e {
        E::In(k) => args[*k].clone(), E::K(v) => Val::I(*v), E::It => c.it.map(Val::I).unwrap_or(Val::Bot), E::It2 => c.it2.map(Val::I).unwrap_or(Val::Bot),
        E::S(k) => c.s.map(|s| Val::I(if *k == 0 { s.0 } else { s.1 })).unwrap_or(Val::Bot),
        E::Un(op, a) => match ev_x(a, args, c).i() { Some(x) => Val::I(match op { Op::Neg => x.wrapping_neg(), Op::Abs => x.wrapping_abs(), _ => (x == 0) as i64 }), None => Val::Bot },
        E::Bin(op, a, b) => match (ev_x(a, args, c).i(), ev_x(b, args, c).i()) { (Some(x), Some(y)) => bin_i(op, x, y).map(Val::I).unwrap_or(Val::Bot), _ => Val::Bot },
        E::Sel(q, a, b) => match ev_x(q, args, c).i() { Some(0) => ev_x(b, args, c), Some(_) => ev_x(a, args, c), None => Val::Bot },
        E::Red(op, a) => match ev_x(a, args, c) { Val::L(v) => { let xs: Option<Vec<i64>> = v.iter().map(|x| x.i()).collect(); match (xs, op) {
            (Some(x), Op::Sum) => Val::I(x.iter().fold(0i64, |p, q| p.wrapping_add(*q))), (Some(x), Op::Len) => Val::I(x.len() as i64),
            (Some(x), Op::MaxAll) => x.iter().max().map(|m| Val::I(*m)).unwrap_or(Val::Bot), (Some(x), _) => x.iter().min().map(|m| Val::I(*m)).unwrap_or(Val::Bot), _ => Val::Bot } } _ => Val::Bot },
        E::Map(l, f) | E::Filt(l, f) => match ev_x(l, args, c) { Val::L(v) => { let mut out = Vec::new();
            for x in v { let Some(xi) = x.i() else { return Val::Bot }; let y = ev_x(f, args, Cx { it: Some(xi), ..c });
                match e { E::Map(..) => out.push(y), _ => { if y.i().is_some_and(|t| t != 0) { out.push(Val::I(xi)); } else if y == Val::Bot { return Val::Bot; } } } }
            Val::L(out) } _ => Val::Bot },
        E::Map2(l1, l2, f) => match (ev_x(l1, args, c), ev_x(l2, args, c)) { (Val::L(a), Val::L(b)) => { let mut out = Vec::new();
            for (x, y) in a.iter().zip(&b) { let (Some(x), Some(y)) = (x.i(), y.i()) else { return Val::Bot }; out.push(ev_x(f, args, Cx { it: Some(x), it2: Some(y), ..c })); }
            Val::L(out) } _ => Val::Bot },
        E::Loop2(init, cond, step, ret) => {
            let (Some(a), Some(b)) = (ev_x(&init.0, args, c).i(), ev_x(&init.1, args, c).i()) else { return Val::Bot };
            let mut s = (a, b);
            for _ in 0..LOOP_CAP { let cc = Cx { s: Some(s), ..c };
                match ev_x(cond, args, cc).i() { Some(0) => return Val::I(if *ret == 0 { s.0 } else { s.1 }), None => return Val::Bot, _ => {} }
                let (Some(t0), Some(t1)) = (ev_x(&step.0, args, cc).i(), ev_x(&step.1, args, cc).i()) else { return Val::Bot }; s = (t0, t1); }
            Val::Bot
        }
    }
}
const LOOP_CAP: usize = 64;
fn size_e(e: &E) -> usize { match e { E::In(_) | E::K(_) | E::It | E::It2 | E::S(_) => 1, E::Un(_, a) | E::Red(_, a) => 1 + size_e(a), E::Bin(_, a, b) | E::Map(a, b) | E::Filt(a, b) => 1 + size_e(a) + size_e(b),
    E::Sel(q, a, b) | E::Map2(q, a, b) => 1 + size_e(q) + size_e(a) + size_e(b), E::Loop2(i, c, s, _) => 1 + size_e(&i.0) + size_e(&i.1) + size_e(c) + size_e(&s.0) + size_e(&s.1) } }
/// Rust text of a synthesised term (inputs a0, a1 …: i64 or &[i64]; booleans as 0 / 1 i64)
pub fn rust_e(e: &E) -> String {
    match e {
        E::In(k) => format!("a{k}"), E::K(v) => format!("({v}i64)"), E::It => "x".into(), E::It2 => "y".into(), E::S(k) => format!("s{k}"),
        E::Un(Op::Neg, a) => format!("(-{})", rust_e(a)), E::Un(Op::Abs, a) => format!("{}.abs()", rust_e(a)), E::Un(_, a) => format!("(({} == 0) as i64)", rust_e(a)),
        E::Bin(op, a, b) => { let (x, y) = (rust_e(a), rust_e(b)); match op {
            Op::Add => format!("({x} + {y})"), Op::Sub => format!("({x} - {y})"), Op::Mul => format!("({x} * {y})"), Op::Div => format!("({x} / {y})"), Op::Rem => format!("({x} % {y})"),
            Op::Min => format!("{x}.min({y})"), Op::Max => format!("{x}.max({y})"), Op::Lt => format!("(({x} < {y}) as i64)"), Op::Le => format!("(({x} <= {y}) as i64)"), Op::Gt => format!("(({x} > {y}) as i64)"),
            Op::Eq => format!("(({x} == {y}) as i64)"), Op::Ne => format!("(({x} != {y}) as i64)"), Op::And => format!("(({x} != 0 && {y} != 0) as i64)"), _ => format!("(({x} != 0 || {y} != 0) as i64)") } }
        E::Sel(q, a, b) => format!("(if {} != 0 {{ {} }} else {{ {} }})", rust_e(q), rust_e(a), rust_e(b)),
        E::Red(op, a) => { let x = rust_e(a); match op { Op::Sum => format!("{x}.iter().sum::<i64>()"), Op::Len => format!("({x}.len() as i64)"), Op::MaxAll => format!("*{x}.iter().max().unwrap()"), _ => format!("*{x}.iter().min().unwrap()") } }
        E::Map(l, f) => format!("{}.iter().map(|&x| {}).collect::<Vec<i64>>()", rust_e(l), rust_e(f)),
        E::Filt(l, f) => format!("{}.iter().copied().filter(|&x| {} != 0).collect::<Vec<i64>>()", rust_e(l), rust_e(f)),
        E::Map2(a, b, f) => format!("{}.iter().zip({}.iter()).map(|(&x, &y)| {}).collect::<Vec<i64>>()", rust_e(a), rust_e(b), rust_e(f)),
        E::Loop2(i, c, s, r) => format!("{{ let (mut s0, mut s1): (i64, i64) = ({}, {}); let mut n = 0; while {} != 0 {{ n += 1; if n > {LOOP_CAP} {{ panic!(\"loop cap\") }} let t: (i64, i64) = ({}, {}); s0 = t.0; s1 = t.1; }} s{r} }}",
            rust_e(&i.0), rust_e(&i.1), rust_e(c), rust_e(&s.0), rust_e(&s.1)),
    }
}
/// A learned word as a synthesis component: an anti-unifier pattern over integer primitives, holes `?k` → arguments.
#[derive(Clone, Debug)]
pub enum Tm { H(usize), K(i64), U(Op, Box<Tm>), B(Op, Box<Tm>, Box<Tm>) }
pub fn parse_word(s: &str) -> Option<(Tm, usize)> {
    let toks: Vec<String> = s.replace('(', " ( ").replace(')', " ) ").split_whitespace().map(|x| x.to_string()).collect();
    fn go(t: &[String], i: &mut usize, holes: &mut usize) -> Option<Tm> {
        let x = t.get(*i)?.clone(); *i += 1;
        if x == "(" { let op = t.get(*i)?.clone(); *i += 1; let mut kids = Vec::new(); while t.get(*i)? != ")" { kids.push(go(t, i, holes)?); } *i += 1;
            let o = SYMS.iter().find(|(y, _)| *y == op)?.1.clone();
            return match (o, kids.len()) {
                (o @ (Op::Neg | Op::Abs | Op::Not), 1) => Some(Tm::U(o, Box::new(kids.remove(0)))),
                (o @ (Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Min | Op::Max | Op::Lt | Op::Le | Op::Eq | Op::Ne | Op::And | Op::Or), 2) => { let b = kids.pop()?; let a = kids.pop()?; Some(Tm::B(o, Box::new(a), Box::new(b))) }
                (Op::Gt, 2) => { let b = kids.pop()?; let a = kids.pop()?; Some(Tm::B(Op::Lt, Box::new(b), Box::new(a))) }
                (Op::Ge, 2) => { let b = kids.pop()?; let a = kids.pop()?; Some(Tm::B(Op::Le, Box::new(b), Box::new(a))) }
                (Op::Clamp, 3) => { let hi = kids.pop()?; let lo = kids.pop()?; let x = kids.pop()?; Some(Tm::B(Op::Min, Box::new(Tm::B(Op::Max, Box::new(x), Box::new(lo))), Box::new(hi))) }
                _ => None };
        }
        if let Some(h) = x.strip_prefix('?') { let k: usize = h.parse().ok()?; *holes = (*holes).max(k + 1); return Some(Tm::H(k)); }
        x.parse::<i64>().ok().map(Tm::K)
    }
    let (mut i, mut h) = (0, 0); let t = go(&toks, &mut i, &mut h)?; if i != toks.len() || h == 0 { return None; } Some((t, h))
}
fn inst_word(t: &Tm, args: &[E]) -> E { match t { Tm::H(k) => args[*k].clone(), Tm::K(v) => E::K(*v), Tm::U(o, a) => E::Un(o.clone(), Box::new(inst_word(a, args))), Tm::B(o, a, b) => E::Bin(o.clone(), Box::new(inst_word(a, args)), Box::new(inst_word(b, args))) } }

/// smallest term reproducing every example (≤ `max_size`), or None. `words` = learned patterns usable as one-step components
/// (cost 1 + their arguments): the library makes the search shorter — author 10-06 "build what it needs" from the words.
thread_local! { pub static BU_WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
pub fn synth(types: &[bool], ex: &[(Vec<Val>, Val)], max_size: usize, cap: usize) -> Option<E> { synth_w(types, ex, max_size, cap, &[]) }
pub fn synth_w(types: &[bool], ex: &[(Vec<Val>, Val)], max_size: usize, cap: usize, words: &[(Tm, usize)]) -> Option<E> {
    let goal: Vec<Val> = ex.iter().map(|(_, o)| norm(o.clone())).collect();
    let out_is_list = matches!(goal.first(), Some(Val::L(_)));
    let key = |vs: &Vec<Val>| format!("{vs:?}");
    // lambda bodies (functions of the element and the int inputs), size ≤ 3, deduplicated on sample elements
    let mut lam: Vec<E> = vec![E::It]; for (k, t) in types.iter().enumerate() { if !t { lam.push(E::In(k)); } } for c in [0, 1, 2] { lam.push(E::K(c)); }
    let atoms = lam.clone();
    for op in [Op::Add, Op::Sub, Op::Mul, Op::Lt, Op::Eq, Op::Ne, Op::Rem, Op::Max, Op::Min] { for a in &atoms { for b in &atoms { if matches!((a, b), (E::It, _) | (_, E::It)) { lam.push(E::Bin(op.clone(), Box::new(a.clone()), Box::new(b.clone()))); } } } }
    lam.push(E::Un(Op::Abs, Box::new(E::It))); lam.push(E::Un(Op::Neg, Box::new(E::It)));
    let mut bank: Vec<Vec<(E, Vec<Val>)>> = vec![Vec::new(); max_size + 1];
    let mut seen: HashSet<String> = HashSet::new(); let mut n = 0usize;
    let mut push = |e: E, bank: &mut Vec<Vec<(E, Vec<Val>)>>, seen: &mut HashSet<String>| -> Option<E> {
        BU_WORK.with(|w| w.set(w.get() + 1));
        let vs: Vec<Val> = ex.iter().map(|(a, _)| norm(ev_e(&e, a, None))).collect();
        if vs.iter().any(|v| *v == Val::Bot) { return None; }
        if vs == goal { return Some(e); }
        let s = size_e(&e); if s <= max_size && seen.insert(key(&vs)) { bank[s].push((e, vs)); } None
    };
    for (k, _) in types.iter().enumerate() { if let Some(e) = push(E::In(k), &mut bank, &mut seen) { return Some(e); } }
    for c in [0, 1, 2, -1] { if let Some(e) = push(E::K(c), &mut bank, &mut seen) { return Some(e); } }
    let bins = [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Rem, Op::Min, Op::Max, Op::Lt, Op::Le, Op::Eq, Op::Ne, Op::And, Op::Or];
    for size in 2..=max_size {
        let mut news: Vec<E> = Vec::new();
        for (e, vs) in &bank[size - 1] {
            let isl = matches!(vs.first(), Some(Val::L(_)));
            if isl { for op in [Op::Sum, Op::Len, Op::MaxAll, Op::MinAll] { news.push(E::Red(op, Box::new(e.clone()))); } }
            else { for op in [Op::Neg, Op::Abs, Op::Not] { news.push(E::Un(op, Box::new(e.clone()))); } }
        }
        for ls in 1..size - 1 { let rs = size - 1 - ls;
            for (a, va) in &bank[ls] { for (b, vb) in &bank[rs] {
                let (la, lb) = (matches!(va.first(), Some(Val::L(_))), matches!(vb.first(), Some(Val::L(_))));
                if !la && !lb { for op in &bins { news.push(E::Bin(op.clone(), Box::new(a.clone()), Box::new(b.clone()))); } }
            } } }
        // learned words: holes filled from size-1 terms (inputs, constants), cost 1 + holes
        for (w, h) in words { if 1 + h != size { continue; }
            let leaves: Vec<E> = bank[1].iter().filter(|(_, v)| !matches!(v.first(), Some(Val::L(_)))).map(|(e, _)| e.clone()).collect();
            let mut idx = vec![0usize; *h];
            'w: loop { let args: Vec<E> = idx.iter().map(|&i| leaves[i].clone()).collect(); news.push(inst_word(w, &args));
                let mut j = 0; loop { if j == *h { break 'w; } idx[j] += 1; if idx[j] < leaves.len() { break; } idx[j] = 0; j += 1; } } }
        // map / filter over a list term with a lambda body (body size counts as its size)
        for ls in 1..size { for (l, vl) in &bank[ls] { if !matches!(vl.first(), Some(Val::L(_))) { continue; }
            for f in lam.iter().filter(|f| size_e(f) + ls + 1 == size) { news.push(E::Map(Box::new(l.clone()), Box::new(f.clone()))); news.push(E::Filt(Box::new(l.clone()), Box::new(f.clone()))); } } }
        for e in news { n += 1; if n > cap { return None; } if let Some(hit) = push(e, &mut bank, &mut seen) { return Some(hit); } }
        let _ = out_is_list;
    }
    None
}
// ── FF-SYNTH-0 (author 10-06 "build"): frame-first synthesis — the PLM-0 result applied to code ──────────────────────────────────
// Lock both ends — the goal (the need's outputs) and the givens (its inputs) — and work backward: each operator is inverted on the
// examples (a witness function) to give its operand a sub-goal (an exact value or an interval per example), and every sub-goal is
// answered from a bank of small terms indexed by behaviour (observational equivalence). Field: top-down, example-propagating
// synthesis — FlashMeta / PROSE (Polozov & Gulwani OOPSLA 2015, witness functions), λ² (Feser et al. PLDI 2015), Myth; bidirectional
// search with a bottom-up bank. Loop skeletons (sketches, Solar-Lezama 2008) are tried when no straight-line term fits.
const INF: i64 = i64::MAX / 4;
type Rg = (i64, i64);
pub struct FfStats { pub work: u64, pub bank: usize, pub how: String }
struct Bank { terms: Vec<(E, Vec<i64>, usize)>, exact: HashMap<Vec<i64>, usize>, by_ex: Vec<Vec<(i64, usize)>> }
impl Bank {
    fn fits(v: &[i64], g: &[Rg]) -> bool { v.iter().zip(g).all(|(x, (lo, hi))| *x >= *lo && *x <= *hi) }
    /// the smallest term whose outputs lie inside every example's interval (narrowed by the tightest example's sorted index)
    fn find(&self, g: &[Rg], work: &mut u64) -> Option<usize> {
        if g.iter().all(|(lo, hi)| lo == hi) { *work += 1; let k: Vec<i64> = g.iter().map(|r| r.0).collect(); return self.exact.get(&k).copied(); }
        let (ei, _) = g.iter().enumerate().min_by_key(|(_, (lo, hi))| hi.saturating_sub(*lo))?;
        let ix = &self.by_ex[ei]; let (lo, hi) = g[ei];
        let a = ix.partition_point(|x| x.0 < lo); let b = ix.partition_point(|x| x.0 <= hi);
        let mut best: Option<usize> = None;
        for &(_, t) in &ix[a..b] { *work += 1; if Bank::fits(&self.terms[t].1, g) && best.is_none_or(|b0| self.terms[t].2 < self.terms[b0].2) { best = Some(t); } }
        best
    }
}
/// Period mining (math index #10, cycle coincidence; #1 CRT): inputs that give the same output and differ by multiples of m
/// are one residue class mod m — so the gcd of their differences is a candidate period. Mined again inside every residue class
/// found (nested periods: 4 → 100 → 400 for leap years). Returns predicates (input k, modulus m, residue r) meaning x_k % m == r.
pub fn mine_periods(types: &[bool], ex: &[(Vec<Val>, Val)]) -> Vec<(usize, i64, i64)> {
    fn gcd(a: i64, b: i64) -> i64 { let (mut a, mut b) = (a.abs(), b.abs()); while b != 0 { let t = a % b; a = b; b = t; } a }
    let outs: Vec<Option<i64>> = ex.iter().map(|(_, o)| o.i()).collect();
    let mut found: Vec<(usize, i64, i64)> = Vec::new();
    for (k, t) in types.iter().enumerate() { if *t { continue; }
        let xs: Vec<Option<i64>> = ex.iter().map(|(a, _)| a[k].i()).collect();
        // each population carries the period that defined it (root: 1)
        let mut pops: Vec<(Vec<usize>, i64)> = vec![((0..ex.len()).collect(), 1)];
        for _round in 0..4 {
            let mut next = Vec::new();
            for (pop, parent) in &pops {
                let mut labels: Vec<i64> = pop.iter().filter_map(|&i| outs[i]).collect(); labels.sort(); labels.dedup();
                for &v in &labels {
                    let mem: Vec<i64> = pop.iter().filter(|&&i| outs[i] == Some(v)).filter_map(|&i| xs[i]).collect();
                    if mem.len() < 2 { continue; }
                    let g = mem.iter().fold(0, |g, x| gcd(g, x - mem[0]));
                    if !(2..=10_000_000).contains(&g) || g == *parent { continue; }
                    let r = mem[0] % g;
                    if !found.contains(&(k, g, r)) { found.push((k, g, r));
                        next.push((pop.iter().copied().filter(|&i| xs[i].is_some_and(|x| x % g == r)).collect::<Vec<usize>>(), g)); }
                }
                // period refinement (math index #7, mixed radix): in a mixed class, a multiple of the parent period that puts one
                // label's members in one residue and every other member outside it separates them exactly — even from one member
                if labels.len() >= 2 && *parent >= 2 {
                    'f: for f in 2..=12i64 { let m = parent * f;
                        for &v in &labels {
                            let mem: Vec<i64> = pop.iter().filter(|&&i| outs[i] == Some(v)).filter_map(|&i| xs[i]).collect();
                            let oth: Vec<i64> = pop.iter().filter(|&&i| outs[i] != Some(v)).filter_map(|&i| xs[i]).collect();
                            let Some(&x0) = mem.first() else { continue }; let r = x0 % m;
                            if mem.iter().all(|x| x % m == r) && oth.iter().all(|x| x % m != r) {
                                if !found.contains(&(k, m, r)) { found.push((k, m, r)); next.push((pop.iter().copied().filter(|&i| xs[i].is_some_and(|x| x % m == r)).collect(), m)); }
                                break 'f;
                            } } }
                }
            }
            if next.is_empty() || found.len() > 24 { break; }
            pops = next;
        }
    }
    found
}

/// Constant mining (oracle runs, author 10-06): layout laws carry constants the alphabet does not hold (5em = 320 LayoutUnits per px of
/// font size, 1/64 px units, UA paddings). Mined exactly from the examples: per input an affine law out = m·x + c that fits EVERY
/// example (slope from two points, checked on all), the gcd of the outputs, and the outputs themselves when there are ≤ 3 values.
pub fn mine_constants(types: &[bool], ex: &[(Vec<Val>, Val)]) -> (Vec<i64>, Vec<(usize, i64, i64)>) {
    fn gcd(a: i64, b: i64) -> i64 { let (mut a, mut b) = (a.abs(), b.abs()); while b != 0 { let t = a % b; a = b; b = t; } a }
    let outs: Vec<i64> = ex.iter().filter_map(|(_, o)| o.i()).collect();
    let mut ks: Vec<i64> = Vec::new(); let mut affine = Vec::new();
    if outs.len() != ex.len() || outs.is_empty() { return (ks, affine); }
    let g = outs.iter().fold(0, |g, &o| gcd(g, o)); if g > 2 { ks.push(g); }
    let mut d: Vec<i64> = outs.clone(); d.sort(); d.dedup(); if d.len() <= 3 { ks.extend(d.iter().copied()); }
    for (k, t) in types.iter().enumerate() { if *t { continue; }
        let xs: Vec<Option<i64>> = ex.iter().map(|(a, _)| a[k].i()).collect();
        let Some(i) = (0..ex.len()).find(|&i| xs[i].is_some()) else { continue };
        let Some(j) = (0..ex.len()).find(|&j| xs[j].is_some() && xs[j] != xs[i]) else { continue };
        let (x0, x1, y0, y1) = (xs[i].unwrap(), xs[j].unwrap(), outs[i], outs[j]);
        if (y1 - y0) % (x1 - x0) != 0 { continue; }
        let m = (y1 - y0) / (x1 - x0); let c = y0 - m * x0;
        if (0..ex.len()).all(|q| xs[q].is_some_and(|x| m.checked_mul(x).and_then(|v| v.checked_add(c)) == Some(outs[q]))) { affine.push((k, m, c)); ks.push(m); ks.push(c); }
    }
    ks.sort(); ks.dedup(); ks.retain(|&v| !matches!(v, -1 | 0 | 1 | 2));
    (ks, affine)
}

/// Floor-rational laws (oracle runs: LayoutUnit / pixel quantisation): out = g·(⌊p·x/q⌋ + t) for one input x ≥ 0, where g = gcd of
/// the outputs. Every observation bounds the slope: ⌊s·x_j⌋ − ⌊s·x_i⌋ = Y_j − Y_i ⇒ s ∈ ((ΔY − 1)/Δx, (ΔY + 1)/Δx); the intersection
/// over all pairs is an interval, and the SIMPLEST fraction p/q inside it (Stern–Brocot walk = Euclid / continued fractions, math
/// index #8) is the law; t must then be one constant. Textarea: 22 px + ⌊cols · 634/79⌋ px (8.0253 px per column).
pub fn mine_floor_rational(types: &[bool], ex: &[(Vec<Val>, Val)]) -> Vec<(usize, i64, i64, i64, i64)> {
    fn gcd(a: i64, b: i64) -> i64 { let (mut a, mut b) = (a.abs(), b.abs()); while b != 0 { let t = a % b; a = b; b = t; } a }
    let mut out = Vec::new();
    let outs: Option<Vec<i64>> = ex.iter().map(|(_, o)| o.i()).collect(); let Some(outs) = outs else { return out };
    let g = outs.iter().fold(0, |g, &o| gcd(g, o)).max(1);
    for (k, t) in types.iter().enumerate() { if *t { continue; }
        let xs: Option<Vec<i64>> = ex.iter().map(|(a, _)| a[k].i()).collect(); let Some(xs) = xs else { continue };
        if xs.iter().any(|&x| x < 0) { continue; }
        let ys: Vec<i64> = outs.iter().map(|&o| o / g).collect();
        // slope interval as fractions (num, den): lo < s < hi
        let (mut lo, mut hi) = ((-1i128 << 60, 1i128), (1i128 << 60, 1i128));
        for i in 0..xs.len() { for j in 0..xs.len() { let dx = xs[j] - xs[i]; if dx <= 0 { continue; }
            let dy = (ys[j] - ys[i]) as i128; let dx = dx as i128;
            if (dy - 1) * lo.1 > lo.0 * dx { lo = (dy - 1, dx); }
            if (dy + 1) * hi.1 < hi.0 * dx { hi = (dy + 1, dx); } } }
        if lo.0 * hi.1 >= hi.0 * lo.1 { continue; }
        // simplest-first: every fraction p/q in (lo, hi) by increasing denominator (the Stern–Brocot order up to q ≤ 2,000); the
        // first whose intercept t = y − ⌊p·x/q⌋ is ONE constant on every example is the law
        if hi.0 <= 0 { continue; }
        'q: for q in 2..=2000i128 {
            let p_lo = (lo.0 * q).div_euclid(lo.1) + 1; let p_hi = { let v = hi.0 * q; let r = v.div_euclid(hi.1); if v.rem_euclid(hi.1) == 0 { r - 1 } else { r } };
            let mut p = p_lo.max(1);
            while p <= p_hi {
                if gcd(p as i64, q as i64) == 1 {
                    let t0 = ys[0] - ((p * xs[0] as i128) / q) as i64;
                    if xs.iter().zip(&ys).all(|(&x, &y)| y - ((p * x as i128) / q) as i64 == t0) { out.push((k, g, p as i64, q as i64, t0)); break 'q; }
                }
                p += 1;
            }
        }
    }
    out
}

/// bottom-up bank of integer-valued terms up to `max_size` (inputs, constants, unary / binary ops, reductions over lists, map /
/// filter / map2 lambdas), one per behaviour
fn ff_bank(types: &[bool], ex: &[(Vec<Val>, Val)], max_size: usize, cap: usize, work: &mut u64) -> Bank {
    let n = ex.len();
    let ev = |e: &E| -> Option<Vec<i64>> { ex.iter().map(|(a, _)| ev_e(e, a, None).i()).collect() };
    let mut terms: Vec<(E, Vec<i64>, usize)> = Vec::new(); let mut exact: HashMap<Vec<i64>, usize> = HashMap::new();
    let mut lists: Vec<(E, usize)> = Vec::new();
    let mut add = |e: E, sz: usize, terms: &mut Vec<(E, Vec<i64>, usize)>, exact: &mut HashMap<Vec<i64>, usize>, work: &mut u64| { *work += 1;
        if let Some(v) = ev(&e) { if !exact.contains_key(&v) { exact.insert(v.clone(), terms.len()); terms.push((e, v, sz)); } } };
    for (k, t) in types.iter().enumerate() { if *t { lists.push((E::In(k), 1)); } else { add(E::In(k), 1, &mut terms, &mut exact, work); } }
    for c in [0, 1, 2, -1] { add(E::K(c), 1, &mut terms, &mut exact, work); }
    // mined constants and exact affine laws (cost 1 / 2: atoms, like learned words)
    let (mks, aff) = mine_constants(types, ex);
    for c in mks { add(E::K(c), 1, &mut terms, &mut exact, work); }
    for (k, m, c) in aff { add(E::Bin(Op::Add, Box::new(E::Bin(Op::Mul, Box::new(E::In(k)), Box::new(E::K(m)))), Box::new(E::K(c))), 2, &mut terms, &mut exact, work); }
    for (k, g, p, q, t) in mine_floor_rational(types, ex) {
        let fl = E::Bin(Op::Div, Box::new(E::Bin(Op::Mul, Box::new(E::In(k)), Box::new(E::K(p)))), Box::new(E::K(q)));
        add(E::Bin(Op::Mul, Box::new(E::K(g)), Box::new(E::Bin(Op::Add, Box::new(fl), Box::new(E::K(t))))), 2, &mut terms, &mut exact, work);
    }
    // mined periods: the modulus as a constant, the residue test as a cheap atom (cost 2, like a learned word)
    for (k, m, r) in mine_periods(types, ex) {
        add(E::K(m), 1, &mut terms, &mut exact, work);
        add(E::Bin(Op::Eq, Box::new(E::Bin(Op::Rem, Box::new(E::In(k)), Box::new(E::K(m)))), Box::new(E::K(r))), 2, &mut terms, &mut exact, work);
    }
    // lambda bodies over the element(s), size ≤ 3
    let ints: Vec<usize> = types.iter().enumerate().filter(|(_, t)| !**t).map(|(k, _)| k).collect();
    let mut atoms: Vec<E> = vec![E::It, E::K(0), E::K(1)]; for &k in &ints { atoms.push(E::In(k)); }
    let mut lam: Vec<E> = atoms.clone(); let mut lam2: Vec<E> = vec![E::Bin(Op::Mul, Box::new(E::It), Box::new(E::It2)), E::Bin(Op::Add, Box::new(E::It), Box::new(E::It2)),
        E::Bin(Op::Sub, Box::new(E::It), Box::new(E::It2)), E::Bin(Op::Max, Box::new(E::It), Box::new(E::It2)), E::Bin(Op::Min, Box::new(E::It), Box::new(E::It2))];
    for op in [Op::Add, Op::Sub, Op::Mul, Op::Lt, Op::Gt, Op::Eq, Op::Ne, Op::Rem, Op::Max, Op::Min] { for a in &atoms { for b in &atoms { if matches!(a, E::It) || matches!(b, E::It) { lam.push(E::Bin(op.clone(), Box::new(a.clone()), Box::new(b.clone()))); } } } }
    let _ = &mut lam2;
    for (l, _) in lists.clone() {
        for op in [Op::Sum, Op::Len, Op::MaxAll, Op::MinAll] { add(E::Red(op, Box::new(l.clone())), 2, &mut terms, &mut exact, work); }
        for f in &lam { for op in [Op::Sum, Op::Len, Op::MaxAll, Op::MinAll] {
            add(E::Red(op.clone(), Box::new(E::Map(Box::new(l.clone()), Box::new(f.clone())))), 3 + size_e(f), &mut terms, &mut exact, work);
            add(E::Red(op, Box::new(E::Filt(Box::new(l.clone()), Box::new(f.clone())))), 3 + size_e(f), &mut terms, &mut exact, work); } }
    }
    for (a, _) in &lists { for (b, _) in &lists { for f in &lam2 { for op in [Op::Sum, Op::MaxAll, Op::MinAll] {
        add(E::Red(op, Box::new(E::Map2(Box::new(a.clone()), Box::new(b.clone()), Box::new(f.clone())))), 4, &mut terms, &mut exact, work); } } } }
    // grow by size: unary + binary over smaller terms
    let bins = [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Rem, Op::Min, Op::Max, Op::Lt, Op::Le, Op::Eq, Op::Ne];
    for size in 2..=max_size {
        let snapshot: Vec<(E, usize)> = terms.iter().map(|(e, _, z)| (e.clone(), *z)).collect();
        for (e, z) in &snapshot { if z + 1 == size { for op in [Op::Neg, Op::Abs] { add(E::Un(op, Box::new(e.clone())), size, &mut terms, &mut exact, work); } } }
        for (a, za) in &snapshot { for (b, zb) in &snapshot { if za + zb + 1 != size { continue; }
            for op in &bins { add(E::Bin(op.clone(), Box::new(a.clone()), Box::new(b.clone())), size, &mut terms, &mut exact, work); if terms.len() > cap { break; } } } }
        if terms.len() > cap { break; }
    }
    terms.sort_by_key(|t| t.2);
    let exact: HashMap<Vec<i64>, usize> = { let mut m = HashMap::new(); for (i, t) in terms.iter().enumerate() { m.entry(t.1.clone()).or_insert(i); } m };
    let by_ex: Vec<Vec<(i64, usize)>> = (0..n).map(|k| { let mut v: Vec<(i64, usize)> = terms.iter().enumerate().map(|(i, t)| (t.1[k], i)).collect(); v.sort(); v }).collect();
    Bank { terms, exact, by_ex }
}
fn div_range(o: i64, b: i64) -> Option<Rg> {   // all n with trunc(n / b) == o (Rust integer division)
    if b == 0 { return None; }
    if b < 0 { return div_range(o.checked_neg()?, b.checked_neg()?); }   // trunc(n / −c) = −trunc(n / c)
    let base = o.checked_mul(b)?;
    Some(if o > 0 { (base, base + b - 1) } else if o < 0 { (base - b + 1, base) } else { (-(b - 1), b - 1) })
}
/// the frame-first solver: goal intervals per example → a term, recursing through operator witnesses up to `depth`
fn ff_solve(bank: &Bank, g: &[Rg], depth: usize, picks: usize, work: &mut u64) -> Option<E> { ff_solve_o(bank, g, depth, picks, work, &[0, 1, 2, 3, 4, 5]) }
/// witness blocks tried in `order`: 0 Add · 1 Sub · 2 Mul · 3 Div · 4 Max/Min · 5 Sel
fn ff_solve_o(bank: &Bank, g: &[Rg], depth: usize, picks: usize, work: &mut u64, order: &[u8]) -> Option<E> {
    if let Some(t) = bank.find(g, work) { return Some(bank.terms[t].0.clone()); }
    if depth == 0 || *work > 60_000_000 { return None; }
    let pick: Vec<usize> = (0..bank.terms.len().min(picks)).collect();
    let n = g.len();
    let sub = |f: &dyn Fn(usize) -> Option<Rg>| -> Option<Vec<Rg>> { (0..n).map(f).collect() };
    let exact = g.iter().all(|(lo, hi)| lo == hi);
    for &blk in order { match blk {
    // Add: a + b ∈ g  ⇒  b ∈ g − a
    0 => for &i in &pick { let a = &bank.terms[i]; *work += 1;
        if let Some(gb) = sub(&|k| Some((g[k].0.saturating_sub(a.1[k]), g[k].1.saturating_sub(a.1[k])))) {
            if let Some(b) = ff_solve(bank, &gb, depth - 1, picks, work) { return Some(E::Bin(Op::Add, Box::new(a.0.clone()), Box::new(b))); } } }
    // Sub: a − b ∈ g  ⇒  b ∈ a − g ;  and  a ∈ g + b
    1 => for &i in &pick { let a = &bank.terms[i]; *work += 1;
        if let Some(gb) = sub(&|k| Some((a.1[k].saturating_sub(g[k].1), a.1[k].saturating_sub(g[k].0)))) {
            if let Some(b) = ff_solve(bank, &gb, depth - 1, picks, work) { return Some(E::Bin(Op::Sub, Box::new(a.0.clone()), Box::new(b))); } }
        let b = a; if let Some(ga) = sub(&|k| Some((g[k].0.saturating_add(b.1[k]), g[k].1.saturating_add(b.1[k])))) {
            if let Some(x) = ff_solve(bank, &ga, depth - 1, picks, work) { return Some(E::Bin(Op::Sub, Box::new(x), Box::new(b.0.clone()))); } } }
    // Mul: a · b ∈ g, a ≠ 0  ⇒  b ∈ [⌈lo/a⌉, ⌊hi/a⌋]
    2 => for &i in &pick { let a = &bank.terms[i]; *work += 1; if a.1.iter().any(|x| *x == 0) { continue; }
        let gb = sub(&|k| { let (lo, hi, d) = (g[k].0 as i128, g[k].1 as i128, a.1[k] as i128); let (x, y) = if d > 0 { (lo, hi) } else { (hi, lo) };
            let c = |p: i128, q: i128| -> i128 { let r = p.div_euclid(q); if p.rem_euclid(q) != 0 { r + 1 } else { r } };
            let (l, h) = (c(x, d), (y).div_euclid(d)); if l > h { None } else { Some((l.clamp(-INF as i128, INF as i128) as i64, h.clamp(-INF as i128, INF as i128) as i64)) } });
        if let Some(gb) = gb { if let Some(b) = ff_solve(bank, &gb, depth - 1, picks, work) { return Some(E::Bin(Op::Mul, Box::new(a.0.clone()), Box::new(b))); } } }
    // Div: n / b ∈ g (exact g), b ≠ 0  ⇒  n ∈ the truncation interval
    3 => if exact { for &i in &pick { let b = &bank.terms[i]; *work += 1; if b.1.iter().any(|x| *x == 0) { continue; }
        if let Some(gn) = sub(&|k| div_range(g[k].0, b.1[k])) { if let Some(x) = ff_solve(bank, &gn, depth - 1, picks, work) { return Some(E::Bin(Op::Div, Box::new(x), Box::new(b.0.clone()))); } } } }
    // Max / Min with one side from the bank
    4 => for &i in &pick { let a = &bank.terms[i]; *work += 1;
        if let Some(gb) = sub(&|k| if a.1[k] > g[k].1 { None } else if a.1[k] >= g[k].0 { Some((-INF, g[k].1)) } else { Some(g[k]) }) {
            if gb.iter().zip(g).any(|(x, y)| x == y) || true { if let Some(b) = ff_solve(bank, &gb, depth - 1, picks, work) { return Some(E::Bin(Op::Max, Box::new(a.0.clone()), Box::new(b))); } } }
        if let Some(gb) = sub(&|k| if a.1[k] < g[k].0 { None } else if a.1[k] <= g[k].1 { Some((g[k].0, INF)) } else { Some(g[k]) }) {
            if let Some(b) = ff_solve(bank, &gb, depth - 1, picks, work) { return Some(E::Bin(Op::Min, Box::new(a.0.clone()), Box::new(b))); } } }
    // Sel: a 0/1 condition from the bank splits the examples; each branch gets the goal on its own examples
    _ => for &i in &pick { let c = &bank.terms[i]; *work += 1;
        if !c.1.iter().all(|x| *x == 0 || *x == 1) || c.1.iter().all(|x| *x == c.1[0]) { continue; }
        let ga: Vec<Rg> = (0..n).map(|k| if c.1[k] == 1 { g[k] } else { (-INF, INF) }).collect();
        let gb: Vec<Rg> = (0..n).map(|k| if c.1[k] == 0 { g[k] } else { (-INF, INF) }).collect();
        if let Some(a) = ff_solve(bank, &ga, depth - 1, picks, work) { if let Some(b) = ff_solve(bank, &gb, depth - 1, picks, work) { return Some(E::Sel(Box::new(c.0.clone()), Box::new(a), Box::new(b))); } } }
    } }
    None
}
/// loop sketch: (s0, s1) = (in_i, in_j); while cond(s) { s = step(s) }; return s0 or s1 — holes filled from small state terms
fn ff_loop(types: &[bool], ex: &[(Vec<Val>, Val)], work: &mut u64) -> Option<E> {
    let ints: Vec<usize> = types.iter().enumerate().filter(|(_, t)| !**t).map(|(k, _)| k).collect();
    if ints.len() < 2 { return None; }
    let mut st: Vec<E> = vec![E::S(0), E::S(1), E::K(0), E::K(1)];
    for op in [Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Rem, Op::Min, Op::Max] { for a in [E::S(0), E::S(1)] { for b in [E::S(0), E::S(1), E::K(1), E::K(2)] {
        if matches!((&a, &b), (E::S(x), E::S(y)) if x == y) { continue; } st.push(E::Bin(op.clone(), Box::new(a.clone()), Box::new(b))); } } }
    let mut conds: Vec<E> = Vec::new();
    for op in [Op::Ne, Op::Gt, Op::Lt] { for a in [E::S(0), E::S(1)] { for b in [E::K(0), E::K(1), E::S(0), E::S(1)] { if matches!((&a, &b), (E::S(x), E::S(y)) if x == y) { continue; } conds.push(E::Bin(op.clone(), Box::new(a.clone()), Box::new(b))); } } }
    let want: Vec<Val> = ex.iter().map(|(_, o)| norm(o.clone())).collect();
    for &i in &ints { for &j in &ints { if i == j { continue; }
        for c in &conds { for t0 in &st { for t1 in &st { for r in [0u8, 1] {
            *work += 1;
            let e = E::Loop2(Box::new((E::In(i), E::In(j))), Box::new(c.clone()), Box::new((t0.clone(), t1.clone())), r);
            if ex.iter().zip(&want).all(|((a, _), w)| norm(ev_e(&e, a, None)) == *w) { return Some(e); }
        } } } } } }
    None
}
/// FF-SYNTH entry: integer goals by witnesses (depth ≤ 2 over a size-≤ 4 bank), list goals by an element-wise map, then loops
pub fn ff_synth(types: &[bool], ex: &[(Vec<Val>, Val)], st: &mut FfStats) -> Option<E> {
    let outs: Vec<Val> = ex.iter().map(|(_, o)| norm(o.clone())).collect();
    if let Some(Val::L(_)) = outs.first() {
        // element-wise: out[j] = f(xs[j]) for some list input xs of the same length
        for (k, t) in types.iter().enumerate() { if !*t { continue; }
            if !ex.iter().zip(&outs).all(|((a, _), o)| matches!((&a[k], o), (Val::L(x), Val::L(y)) if x.len() == y.len())) { continue; }
            let mut ex2: Vec<(Vec<Val>, Val)> = Vec::new();
            for ((a, _), o) in ex.iter().zip(&outs) { if let (Val::L(xs), Val::L(ys)) = (&a[k], o) { for (x, y) in xs.iter().zip(ys) { let mut a2 = a.clone(); a2.push(x.clone()); ex2.push((a2, y.clone())); } } }
            let mut t2 = types.to_vec(); t2.push(false);
            let mut sub = FfStats { work: 0, bank: 0, how: String::new() };
            if let Some(body) = ff_synth(&t2, &ex2, &mut sub) { st.work += sub.work; st.how = "map".into();
                let body = subst_in(&body, t2.len() - 1); return Some(E::Map(Box::new(E::In(k)), Box::new(body))); }
            st.work += sub.work;
        }
        return None;
    }
    let goal: Option<Vec<Rg>> = outs.iter().map(|o| o.i().map(|v| (v, v))).collect();
    let goal = goal?;
    if std::env::var("OMR_FF_DEBUG").is_ok() { eprintln!("    periods {:?} · train x/y {:?}", mine_periods(types, ex), ex.iter().map(|(a, o)| (a.first().and_then(|v| v.i()), o.i())).collect::<Vec<_>>()); }
    let bank = ff_bank(types, ex, 4, 400_000, &mut st.work); st.bank = bank.terms.len();
    // Occam: at the first depth that answers, try every witness order and keep the smallest term (fewer overfits for CEGIS to undo)
    const ORDERS: [[u8; 6]; 4] = [[0, 1, 2, 3, 4, 5], [3, 2, 0, 1, 4, 5], [2, 3, 1, 0, 4, 5], [4, 0, 1, 2, 3, 5]];
    for depth in 0..=3 { let picks = if depth == 3 { 60 } else { 160 };
        let mut best: Option<E> = None;
        for o in ORDERS.iter() { if let Some(e) = ff_solve_o(&bank, &goal, depth, picks, &mut st.work, o) {
            // the assembled term must reproduce the goal itself (a witness is only as good as its inverse)
            let ok = ex.iter().zip(&goal).all(|((a, _), (lo, _))| ev_e(&e, a, None).i() == Some(*lo));
            if ok && best.as_ref().is_none_or(|b| size_e(&e) < size_e(b)) { best = Some(e); } } if depth == 0 { break; } }
        if let Some(e) = best { st.how = format!("witness depth {depth}"); return Some(e); } }
    if let Some(e) = ff_loop(types, ex, &mut st.work) { st.how = "loop sketch".into(); return Some(e); }
    None
}
/// the element input appended for a map body becomes the lambda element `x`
fn subst_in(e: &E, k: usize) -> E {
    let r = |x: &E| Box::new(subst_in(x, k));
    match e { E::In(j) if *j == k => E::It, E::Un(o, a) => E::Un(o.clone(), r(a)), E::Bin(o, a, b) => E::Bin(o.clone(), r(a), r(b)), E::Sel(q, a, b) => E::Sel(r(q), r(a), r(b)),
        E::Red(o, a) => E::Red(o.clone(), r(a)), E::Map(a, b) => E::Map(r(a), b.clone()), E::Filt(a, b) => E::Filt(r(a), b.clone()), o => o.clone() }
}

/// Independent verifier: the synthesised term printed as Rust, compiled by rustc and run on every example.
pub fn verify_rust(e: &E, types: &[bool], ex: &[(Vec<Val>, Val)], dir: &str, name: &str) -> Result<(), String> {
    let lit = |v: &Val| -> String { match norm(v.clone()) { Val::I(x) => format!("{x}i64"), Val::L(xs) => format!("vec![{}]", xs.iter().map(|x| format!("{}i64", x.i().unwrap_or(0))).collect::<Vec<_>>().join(", ")), _ => "0".into() } };
    let out_list = matches!(norm(ex[0].1.clone()), Val::L(_));
    let params: Vec<String> = types.iter().enumerate().map(|(k, t)| format!("a{k}: {}", if *t { "&[i64]" } else { "i64" })).collect();
    let mut src = format!("#[allow(unused_parens, unused_variables, unused_mut, unused_assignments, unused_braces)]\nfn f({}) -> {} {{ {} }}\nfn main() {{\n", params.join(", "), if out_list { "Vec<i64>" } else { "i64" }, rust_e(e));
    for (args, want) in ex { let a: Vec<String> = args.iter().zip(types).map(|(v, t)| if *t { format!("&{}", lit(v)) } else { lit(v) }).collect(); src.push_str(&format!("    assert_eq!(f({}), {});\n", a.join(", "), lit(want))); }
    src.push_str("    println!(\"ok\");\n}\n");
    let rs = format!("{dir}/{name}.rs"); let bin = format!("{dir}/{name}.bin"); std::fs::create_dir_all(dir).ok(); std::fs::write(&rs, &src).map_err(|e| e.to_string())?;
    let c = std::process::Command::new("rustc").args(["-O", "--edition", "2021", "-o", &bin, &rs]).output().map_err(|e| e.to_string())?;
    if !c.status.success() { return Err(format!("rustc: {}", String::from_utf8_lossy(&c.stderr).lines().find(|l| l.starts_with("error")).unwrap_or("error"))); }
    let r = std::process::Command::new(&bin).output().map_err(|e| e.to_string())?;
    if r.status.success() { Ok(()) } else { Err("assert failed".into()) }
}

/// Every class reachable from a function's outputs (its sub-terms), through each class's best node.
pub fn reachable(g: &EGraph, best: &HashMap<Id, (u32, Node)>, roots: &[Id]) -> Vec<Id> {
    let mut seen = HashSet::new(); let mut st: Vec<Id> = roots.iter().map(|&r| g.find(r)).collect(); let mut out = Vec::new();
    while let Some(c) = st.pop() { let c = g.find(c); if !seen.insert(c) { continue; } out.push(c); if let Some((_, n)) = best.get(&c) { for &k in &n.1 { st.push(k); } } }
    out
}

// ── rewrite rules as data (A2) ────────────────────────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub enum Pat { V(String), N(Op, Vec<Pat>) }

fn parse_pat(s: &str) -> Pat {
    let toks: Vec<String> = s.replace('(', " ( ").replace(')', " ) ").split_whitespace().map(|x| x.to_string()).collect();
    fn go(t: &[String], i: &mut usize) -> Pat {
        let x = t[*i].clone(); *i += 1;
        if x == "(" {
            let op = sym_op(&t[*i]); *i += 1; let mut kids = Vec::new();
            while t[*i] != ")" { kids.push(go(t, i)); } *i += 1; Pat::N(op, kids)
        } else if let Some(v) = x.strip_prefix('?') { Pat::V(v.into()) } else { Pat::N(sym_op(&x), vec![]) }
    }
    let mut i = 0; go(&toks, &mut i)
}
fn sym_op(s: &str) -> Op {
    if let Ok(v) = s.parse::<i64>() { return Op::Num(v); }
    SYMS.iter().find(|(x, _)| *x == s).map(|(_, o)| o.clone()).unwrap_or_else(|| panic!("rule symbol {s}"))
}

/// Integer-sound identities (Rust wrapping arithmetic keeps them: x·2 = x+x = x<<1 in every width). Each line `lhs => rhs`;
/// `<=>` adds both directions. Kept small on purpose: distribution and division rules blow up or need side conditions.
pub const RULES: &str = "
(+ ?a ?b) => (+ ?b ?a)
(* ?a ?b) => (* ?b ?a)
(min ?a ?b) => (min ?b ?a)
(max ?a ?b) => (max ?b ?a)
(== ?a ?b) => (== ?b ?a)
(!= ?a ?b) => (!= ?b ?a)
(and ?a ?b) => (and ?b ?a)
(or ?a ?b) => (or ?b ?a)
(+ (+ ?a ?b) ?c) <=> (+ ?a (+ ?b ?c))
(* (* ?a ?b) ?c) <=> (* ?a (* ?b ?c))
(min (min ?a ?b) ?c) <=> (min ?a (min ?b ?c))
(max (max ?a ?b) ?c) <=> (max ?a (max ?b ?c))
(* ?a 2) <=> (+ ?a ?a)
(<< ?a 1) <=> (* ?a 2)
(+ ?a 0) => ?a
(* ?a 1) => ?a
(* ?a 0) => 0
(- ?a 0) => ?a
(- ?a ?a) => 0
(- ?a ?b) <=> (+ ?a (neg ?b))
(+ ?a (neg ?a)) => 0
(neg (neg ?a)) => ?a
(neg (+ ?a ?b)) <=> (+ (neg ?a) (neg ?b))
(min ?a ?a) => ?a
(max ?a ?a) => ?a
(clamp ?x ?lo ?hi) <=> (min (max ?x ?lo) ?hi)
(sel ?c ?a ?a) => ?a
(> ?a ?b) <=> (< ?b ?a)
(>= ?a ?b) <=> (<= ?b ?a)
(not (< ?a ?b)) <=> (<= ?b ?a)
(not (not ?a)) => ?a
(sel (not ?c) ?a ?b) => (sel ?c ?b ?a)
(max ?a ?b) <=> (sel (< ?a ?b) ?b ?a)
(min ?a ?b) <=> (sel (< ?a ?b) ?a ?b)
";

/// Expansive identities (distribution): sound, but with assoc + comm they grow the graph exponentially — opt-in (`--expand`).
pub const RULES_EXPAND: &str = "
(* ?a (+ ?b ?c)) => (+ (* ?a ?b) (* ?a ?c))
(+ (* ?a ?b) (* ?a ?c)) => (* ?a (+ ?b ?c))
";

pub fn rules() -> Vec<(String, Pat, Pat)> { rules_from(RULES) }
pub fn rules_from(text: &str) -> Vec<(String, Pat, Pat)> {
    let mut out = Vec::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (both, (a, b)) = if let Some(x) = l.split_once("<=>") { (true, x) } else { (false, l.split_once("=>").expect("rule")) };
        let (pa, pb) = (parse_pat(a.trim()), parse_pat(b.trim()));
        out.push((l.to_string(), pa.clone(), pb.clone()));
        if both { out.push((format!("{l} (rev)"), pb, pa)); }
    }
    out
}

fn ematch(g: &EGraph, p: &Pat, c: Id, sub: HashMap<String, Id>, out: &mut Vec<HashMap<String, Id>>) {
    let c = g.find(c);
    match p {
        Pat::V(v) => { match sub.get(v) { Some(&x) if g.find(x) != c => {}, Some(_) => out.push(sub), None => { let mut s = sub; s.insert(v.clone(), c); out.push(s); } } }
        Pat::N(op, kids) => {
            for n in &g.classes[c as usize] {
                if n.0 != *op || n.1.len() != kids.len() { continue; }
                let mut subs = vec![sub.clone()];
                for (k, pk) in n.1.iter().zip(kids) { let mut nx = Vec::new(); for s in subs { ematch(g, pk, *k, s, &mut nx); } subs = nx; if subs.is_empty() { break; } }
                out.extend(subs);
            }
        }
    }
}
fn inst(g: &mut EGraph, p: &Pat, s: &HashMap<String, Id>) -> Id {
    match p { Pat::V(v) => s[v], Pat::N(op, kids) => { let ks = kids.iter().map(|k| inst(g, k, s)).collect(); g.add(op.clone(), ks) } }
}

/// Equality saturation: apply every rule everywhere, rebuild, repeat until nothing changes or a limit is hit.
/// Returns (iterations, saturated).
pub fn saturate(g: &mut EGraph, rs: &[(String, Pat, Pat)], max_iter: usize, max_nodes: usize) -> (usize, bool) {
    for it in 0..max_iter {
        let mut todo: Vec<(Id, usize, HashMap<String, Id>)> = Vec::new();
        for c in 0..g.classes.len() as Id {
            if g.find(c) != c { continue; }
            for (ri, (_, l, _)) in rs.iter().enumerate() { let mut m = Vec::new(); ematch(g, l, c, HashMap::new(), &mut m); for s in m { todo.push((c, ri, s)); } }
        }
        let mut changed = false;
        for (c, ri, s) in todo { if g.n_nodes() > max_nodes { g.rebuild(); return (it + 1, false); } let r = inst(g, &rs[ri].2, &s); changed |= g.union(c, r); }
        g.rebuild();
        if !changed { return (it + 1, true); }
    }
    (max_iter, false)
}

// ── A1: Rust subset → equations ───────────────────────────────────────────────────────────────────────────────────────────────
/// One lowered function: its outputs as (role, class) — `ret`, each written outer name (role = name kind, never the name itself
/// in signatures), each side effect — plus counters for the alphabet coverage measure.
pub struct FnEq { pub name: String, pub line: u32, pub p: usize, pub e: usize, pub outs: Vec<(String, Id)>, pub nodes: u32, pub opaque: u32, pub reasons: Vec<String> }

struct Lw<'a> {
    t: &'a [Tok], i: usize, end: usize, g: &'a mut EGraph,
    env: HashMap<String, Id>, declared: HashSet<String>, written: Vec<String>, effects: Vec<Id>, ret: Vec<Id>,
    slots: &'a mut u32, opq: &'a mut u32, inputs: HashMap<String, Id>, nodes: u32, opaque: u32, reasons: Vec<String>, depth: u8, accs: Vec<HashMap<String, u8>>,
}

const IDENTITY_METHODS: &[&str] = &["iter", "iter_mut", "into_iter", "copied", "cloned", "collect", "clone", "to_vec", "to_owned", "as_ref", "as_mut",
    "borrow", "borrow_mut", "unwrap", "expect", "unwrap_or_default", "as_str", "into", "to_string", "as_slice", "rev", "get_mut", "deref"];

impl<'a> Lw<'a> {
    fn peek(&self) -> &str { if self.i < self.end { &self.t[self.i].s } else { "" } }
    fn peek_at(&self, k: usize) -> &str { if self.i + k < self.end { &self.t[self.i + k].s } else { "" } }
    fn eat(&mut self) -> String { let s = self.peek().to_string(); if self.i < self.end { self.i += 1; } s }
    fn want(&mut self, x: &str) -> bool { if self.peek() == x { self.i += 1; true } else { false } }
    fn mk(&mut self, op: Op, kids: Vec<Id>) -> Id { self.nodes += 1; self.g.add(op, kids) }
    fn opq(&mut self, kids: Vec<Id>, why: &str) -> Id { *self.opq += 1; self.nodes += 1; self.opaque += 1; if self.reasons.len() < 64 { self.reasons.push(why.into()); } let u = *self.opq; self.g.add(Op::Opq(u), kids) }
    /// skip a balanced group starting at an opening bracket
    fn skip_group(&mut self) { let mut d = 0i32; while self.i < self.end { let s = self.eat(); match s.as_str() { "(" | "[" | "{" => d += 1, ")" | "]" | "}" => { d -= 1; if d <= 0 { return; } } _ => {} } } }
    /// skip to the end of a statement (`;` at depth 0, or a closing block brace) — returns false at block end
    fn skip_stmt(&mut self) { let mut d = 0i32; while self.i < self.end { match self.peek() { "(" | "[" | "{" => d += 1, ")" | "]" => d -= 1, "}" => { if d == 0 { return; } d -= 1; if d == 0 { self.i += 1; if self.peek() != "else" && self.peek() != "." { return; } continue; } } ";" if d == 0 => { self.i += 1; return; } _ => {} } self.i += 1; } }
    fn var(&mut self, name: &str) -> Id {
        if let Some(&id) = self.env.get(name) { return id; }
        // one slot per input name for the whole function, whichever branch touches it first
        let id = match self.inputs.get(name) { Some(&x) => x, None => { *self.slots += 1; let s = *self.slots; let x = self.mk(Op::Var(s), vec![]); self.inputs.insert(name.to_string(), x); x } };
        self.env.insert(name.to_string(), id); id
    }
    fn skip_type(&mut self) { // after `as` / `:` in a let: a path with optional generics / refs / tuples
        let mut d = 0i32;
        loop { let s = self.peek().to_string(); if s.is_empty() { return; }
            match s.as_str() { "<" | "(" | "[" => d += 1, ">" | ")" | "]" if d > 0 => d -= 1, ">>" if d > 1 => d -= 2, "&" | "mut" | "::" | "'" | "dyn" | "," if d > 0 || s != "," => {}, _ => { let t = &self.t[self.i]; if d == 0 && !(t.k == 0 || s == "Self" || s == "self" || s == "crate") { return; } } }
            self.i += 1;
            if d == 0 { let n = self.peek(); if n != "::" && n != "<" && !(self.t.get(self.i.wrapping_sub(1)).is_some_and(|t| t.s == "::" || t.s == "&" || t.s == "mut" || t.s == "dyn" || t.s == "'")) { return; } }
        }
    }

    // expressions: precedence climbing (Rust order)
    fn expr(&mut self, ns: bool) -> Id {
        if matches!(self.peek(), ".." | "..=") { self.eat(); let z = self.mk(Op::Num(0), vec![]); let hi = self.bin(0, ns); return self.mk(Op::Range, vec![z, hi]); }
        let a = self.bin(0, ns);
        if matches!(self.peek(), ".." | "..=") {
            let incl = self.eat() == "..=";
            if matches!(self.peek(), ")" | "]" | "}" | "," | ";" | "{" | "") { let inf = self.opq(vec![], "open range"); return self.mk(Op::Range, vec![a, inf]); }
            let mut b = self.bin(0, ns);
            if incl { let one = self.mk(Op::Num(1), vec![]); b = self.mk(Op::Add, vec![b, one]); }
            return self.mk(Op::Range, vec![a, b]);
        }
        a
    }
    fn bin(&mut self, lvl: usize, ns: bool) -> Id {
        const P: &[&[&str]] = &[&["||"], &["&&"], &["==", "!=", "<", ">", "<=", ">="], &["|"], &["^"], &["&"], &["<<", ">>"], &["+", "-"], &["*", "/", "%"]];
        if lvl == P.len() { return self.cast(ns); }
        let mut l = self.bin(lvl + 1, ns);
        while P[lvl].contains(&self.peek()) {
            let op = self.eat(); let r = self.bin(lvl + 1, ns);
            let o = match op.as_str() { "||" => Op::Or, "&&" => Op::And, "==" => Op::Eq, "!=" => Op::Ne, "<" => Op::Lt, ">" => Op::Gt, "<=" => Op::Le, ">=" => Op::Ge,
                "|" => Op::BOr, "^" => Op::BXor, "&" => Op::BAnd, "<<" => Op::Shl, ">>" => Op::Shr, "+" => Op::Add, "-" => Op::Sub, "*" => Op::Mul, "/" => Op::Div, _ => Op::Rem };
            l = self.mk(o, vec![l, r]);
        }
        l
    }
    fn cast(&mut self, ns: bool) -> Id { let mut e = self.unary(ns); while self.peek() == "as" { self.eat(); self.skip_type(); e = self.postfix(e, ns); } e }
    fn unary(&mut self, ns: bool) -> Id {
        match self.peek() {
            "-" => { self.eat(); let x = self.unary(ns); self.mk(Op::Neg, vec![x]) }
            "!" => { self.eat(); let x = self.unary(ns); self.mk(Op::Not, vec![x]) }
            "&" | "*" | "&&" => { self.eat(); self.want("mut"); self.unary(ns) }
            _ => { let p = self.primary(ns); self.postfix(p, ns) }
        }
    }
    fn args(&mut self) -> Vec<Id> {
        self.eat(); let mut a = Vec::new();
        while self.i < self.end && self.peek() != ")" { let x = self.i; a.push(self.expr(false)); if !self.want(",") && self.peek() != ")" { self.skip_to_close(); break; } if self.i == x { self.i += 1; } }
        self.want(")"); a
    }
    fn skip_to_close(&mut self) { let mut d = 0i32; while self.i < self.end { match self.peek() { "(" | "[" | "{" => d += 1, ")" | "]" | "}" => { if d == 0 { return; } d -= 1; } _ => {} } self.i += 1; } }
    fn closure(&mut self, recv: Option<Id>) -> Id {
        // |pat| body  — the element(s) bind to Item(depth); a zip receiver binds a 2-tuple pattern to get0/get1 of the item
        self.want("move"); let mut pats: Vec<String> = Vec::new();
        if self.want("||") {} else { self.want("|"); while self.i < self.end && self.peek() != "|" { let t = self.eat(); if self.t[self.i - 1].k == 0 { pats.push(t); } } self.want("|"); }
        self.depth = self.depth.saturating_add(1);
        let item = self.mk(Op::Item(self.depth), vec![]);
        let saved: Vec<(String, Option<Id>)> = pats.iter().map(|p| (p.clone(), self.env.get(p).copied())).collect();
        // element binding: over a range the pattern IS the index; over a sequence s it is idx(s, index); over zip(a, b) a
        // 2-pattern binds idx(a, ·), idx(b, ·) — so `mn.iter().zip(&mx).map(|(a, b)| …)` and `for k in 0..n { … mn[k] … mx[k] }` meet
        let (is_range, zip) = match recv { Some(r) => { let c = &self.g.classes[self.g.find(r) as usize];
            (c.iter().any(|n| n.0 == Op::Range), c.iter().find(|n| n.0 == Op::Zip).map(|n| n.1.clone())) } None => (true, None) };
        if pats.len() == 1 {
            let v = if is_range { item } else { let r = recv.unwrap(); self.mk(Op::Idx, vec![r, item]) };
            self.env.insert(pats[0].clone(), v);
        } else if let (Some(z), 2) = (zip, pats.len()) {
            for (k, p) in pats.iter().enumerate() { let v = self.mk(Op::Idx, vec![z[k], item]); self.env.insert(p.clone(), v); }
        } else { for (k, p) in pats.iter().enumerate() { let g = self.mk(Op::Get(k as u8), vec![item]); self.env.insert(p.clone(), g); } }
        let body = if self.peek() == "{" { self.block_value() } else { self.expr(false) };
        for (p, old) in saved { match old { Some(v) => { self.env.insert(p, v); } None => { self.env.remove(&p); } } }
        self.depth -= 1; body
    }
    fn postfix(&mut self, mut e: Id, ns: bool) -> Id {
        loop {
            match self.peek() {
                "?" => { self.eat(); }
                "[" => { self.eat(); let i = self.expr(false); self.want("]"); e = self.mk(Op::Idx, vec![e, i]); }
                "." => {
                    self.eat(); let name = self.eat();
                    if let Ok(k) = name.parse::<u8>() { e = self.mk(Op::Get(k), vec![e]); continue; }
                    if name == "await" { continue; }
                    if self.peek() == "::" { self.eat(); if self.peek() == "<" { let mut d = 0; loop { let s = self.eat(); if s == "<" { d += 1; } else if s == ">" { d -= 1; if d == 0 { break; } } else if s == ">>" { d -= 2; if d <= 0 { break; } } else if s.is_empty() { break; } } } }
                    if self.peek() != "(" { e = self.field(e, &name); continue; }
                    // method call
                    if matches!(name.as_str(), "map" | "filter" | "any" | "all" | "for_each" | "flat_map" | "filter_map" | "fold" | "max_by_key" | "min_by_key" | "position" | "find" | "take_while" | "skip_while") {
                        self.eat(); // (
                        let mut init = None;
                        if name == "fold" { init = Some(self.expr(false)); self.want(","); }
                        let lam = if matches!(self.peek(), "|" | "||" | "move") { self.closure(Some(e)) } else { let x = self.expr(false); x };
                        while self.i < self.end && self.peek() != ")" { self.i += 1; } self.want(")");
                        e = match name.as_str() {
                            "map" => self.mk(Op::Map, vec![e, lam]),
                            "filter" => self.mk(Op::Filter, vec![e, lam]),
                            "fold" => { let z = init.unwrap(); self.mk(Op::Fold, vec![e, z, lam]) }
                            _ => { let w = name.clone(); self.opq(vec![e, lam], &format!("iter.{w}")) }
                        };
                        continue;
                    }
                    let a = self.args();
                    e = match (name.as_str(), a.len()) {
                        (m, 0) if IDENTITY_METHODS.contains(&m) => e,
                        ("unwrap_or" | "unwrap_or_else" | "or", 1) => e,
                        ("max", 1) => self.mk(Op::Max, vec![e, a[0]]), ("min", 1) => self.mk(Op::Min, vec![e, a[0]]),
                        ("max", 0) => self.mk(Op::MaxAll, vec![e]), ("min", 0) => self.mk(Op::MinAll, vec![e]),
                        ("abs", 0) => self.mk(Op::Abs, vec![e]), ("clamp", 2) => self.mk(Op::Clamp, vec![e, a[0], a[1]]),
                        ("sum", 0) => self.mk(Op::Sum, vec![e]), ("len" | "count", 0) => self.mk(Op::Len, vec![e]),
                        ("zip", 1) => self.mk(Op::Zip, vec![e, a[0]]),
                        ("pow" | "powi" | "powf", 1) => self.mk(Op::Pow, vec![e, a[0]]), ("sqrt", 0) => self.mk(Op::Sqrt, vec![e]),
                        ("floor", 0) => self.mk(Op::Floor, vec![e]), ("ceil", 0) => self.mk(Op::Ceil, vec![e]), ("round", 0) => self.mk(Op::Round, vec![e]),
                        ("saturating_sub", 1) => { let d = self.mk(Op::Sub, vec![e, a[0]]); let z = self.mk(Op::Num(0), vec![]); self.mk(Op::Max, vec![d, z]) }
                        ("wrapping_add" | "saturating_add" | "checked_add", 1) => self.mk(Op::Add, vec![e, a[0]]),
                        ("wrapping_sub" | "checked_sub", 1) => self.mk(Op::Sub, vec![e, a[0]]), ("wrapping_mul" | "checked_mul", 1) => self.mk(Op::Mul, vec![e, a[0]]),
                        ("signum", 0) => { let z = self.mk(Op::Num(0), vec![]); let gt = self.mk(Op::Gt, vec![e, z]); let lt = self.mk(Op::Lt, vec![e, z]); self.mk(Op::Sub, vec![gt, lt]) }
                        ("term", 3) => a[2], // Layout::term(n, key, v) = v weighed by its knob: the value is v
                        _ => { let mut k = vec![e]; k.extend(a); self.opq(k, &format!(".{name}()")) }
                    };
                    let _ = ns;
                }
                _ => return e,
            }
        }
    }
    /// field read: on a named path it is one input name (`s.margin` = one slot); on a computed value it is opaque
    fn field(&mut self, e: Id, name: &str) -> Id {
        if let Some(path) = self.path_of(e) { let p = format!("{path}.{name}"); return self.var(&p); }
        self.opq(vec![e], "field of value")
    }
    fn path_of(&self, e: Id) -> Option<String> { self.env.iter().find(|(_, v)| **v == e).map(|(k, _)| k.clone()).filter(|k| !k.is_empty()) }
    fn primary(&mut self, ns: bool) -> Id {
        let t = self.peek().to_string();
        if t.is_empty() { return self.opq(vec![], "eof"); }
        let k = self.t[self.i].k;
        match t.as_str() {
            "(" => {
                self.eat(); if self.want(")") { return self.mk(Op::Tup, vec![]); }
                let a = self.expr(false);
                if self.want(",") { let mut v = vec![a]; while self.i < self.end && self.peek() != ")" { v.push(self.expr(false)); if !self.want(",") { break; } } self.want(")"); return self.mk(Op::Tup, v); }
                self.want(")"); return a;
            }
            "[" => {
                self.eat(); if self.want("]") { return self.mk(Op::List, vec![]); }
                let a = self.expr(false);
                if self.want(";") { let n = self.expr(false); self.want("]"); return self.mk(Op::Fill, vec![a, n]); }
                let mut v = vec![a]; while self.want(",") { if self.peek() == "]" { break; } v.push(self.expr(false)); } self.want("]"); return self.mk(Op::List, v);
            }
            "if" => return self.if_expr(),
            "match" => { self.eat(); let s = self.expr(true); if self.peek() == "{" { self.skip_group(); } return self.opq(vec![s], "match"); }
            "{" | "unsafe" => { self.want("unsafe"); return self.block_value(); }
            "loop" | "while" | "for" => { let x = self.stmt_loop(); return x; }
            "|" | "||" | "move" => return self.closure(None),
            "return" => { self.eat(); if !matches!(self.peek(), ";" | "}" | "") { let v = self.expr(false); self.ret.push(v); } return self.opq(vec![], "return-expr"); }
            "break" | "continue" => { self.eat(); return self.opq(vec![], "break"); }
            "true" => { self.eat(); return self.mk(Op::Num(1), vec![]); }
            "false" => { self.eat(); return self.mk(Op::Num(0), vec![]); }
            _ => {}
        }
        if k == 3 { self.eat(); return self.mk(Op::Str, vec![]); }
        if k == 2 {
            self.eat(); let raw: String = t.chars().filter(|c| *c != '_').collect();
            let num = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize", "isize", "f32", "f64"].iter().fold(raw.clone(), |s, suf| s.strip_suffix(suf).map(|x| x.to_string()).unwrap_or(s));
            if let Some(h) = num.strip_prefix("0x") { if let Ok(v) = i64::from_str_radix(h, 16) { return self.mk(Op::Num(v), vec![]); } }
            if let Ok(v) = num.parse::<i64>() { return self.mk(Op::Num(v), vec![]); }
            if let Ok(f) = num.parse::<f64>() { if f.fract() == 0.0 && f.abs() < 1e15 { return self.mk(Op::Num(f as i64), vec![]); } return self.mk(Op::Flt(f.to_bits()), vec![]); }
            return self.opq(vec![], "number");
        }
        if k == 0 || matches!(t.as_str(), "self" | "Self" | "crate" | "super" | "Some" | "Ok" | "Err" | "None") {
            // path a::b::c, macro, call, struct literal
            let mut path = self.eat();
            while self.peek() == "::" { self.eat(); if self.peek() == "<" { self.skip_type(); continue; } path.push_str("::"); path.push_str(&self.eat()); }
            if self.peek() == "!" && matches!(self.peek_at(1), "(" | "[" | "{") { self.eat(); let m = path.clone(); self.skip_group(); return if m == "vec" { self.mk(Op::List, vec![]) } else { self.opq(vec![], &format!("{m}!")) }; }
            if self.peek() == "(" {
                let a = self.args(); let last = path.rsplit("::").next().unwrap_or("").to_string();
                return match (last.as_str(), a.len()) {
                    ("Some" | "Ok" | "Box" | "Rc" | "Cow" | "Borrowed" | "Owned", 1) => a[0],
                    ("max", 2) => self.mk(Op::Max, a), ("min", 2) => self.mk(Op::Min, a),
                    _ => { let l = last.clone(); self.opq(a, &format!("call {l}")) }
                };
            }
            if !ns && self.peek() == "{" && path.chars().next().is_some_and(|c| c.is_ascii_uppercase()) { self.skip_group(); return self.opq(vec![], "struct literal"); }
            if path == "None" { return self.mk(Op::Opq(0), vec![]); }
            return self.var(&path);
        }
        self.eat(); self.opq(vec![], &format!("token {t}"))
    }
    fn cond(&mut self) -> Id {
        if self.want("let") { // if let PAT = e : the condition is opaque; pattern names bind to projections of e
            let st = self.i; while self.i < self.end && self.peek() != "=" { self.i += 1; }
            let names: Vec<String> = self.t[st..self.i].iter().filter(|t| t.k == 0 && t.s.chars().next().is_some_and(|c| c.is_ascii_lowercase())).map(|t| t.s.clone()).collect();
            self.want("="); let e = self.expr(true);
            for (k, n) in names.iter().enumerate() { let g = if names.len() == 1 { e } else { self.mk(Op::Get(k as u8), vec![e]) }; self.env.insert(n.clone(), g); self.declared.insert(n.clone()); }
            return self.opq(vec![e], "if let");
        }
        self.expr(true)
    }
    fn if_expr(&mut self) -> Id {
        self.eat(); let c = self.cond();
        let base = self.env.clone();
        let a = if self.peek() == "{" { self.block_value() } else { self.opq(vec![], "if body") };
        let env_a = std::mem::replace(&mut self.env, base.clone());
        let b = if self.want("else") { if self.peek() == "if" { self.if_expr() } else { self.block_value() } } else { self.mk(Op::Tup, vec![]) };
        let env_b = std::mem::take(&mut self.env);
        self.env = self.merge(c, &base, env_a, env_b);
        self.mk(Op::Sel, vec![c, a, b])
    }
    /// φ: a name written on either branch becomes sel(c, then, else)
    fn merge(&mut self, c: Id, base: &HashMap<String, Id>, a: HashMap<String, Id>, b: HashMap<String, Id>) -> HashMap<String, Id> {
        let mut out = base.clone();
        let keys: HashSet<String> = a.keys().chain(b.keys()).cloned().collect();
        for k in keys {
            let (x, y) = (a.get(&k).copied(), b.get(&k).copied());
            let o = base.get(&k).copied().or_else(|| if self.declared.contains(&k) { None } else { self.inputs.get(&k).copied() });
            match (x, y) {
                (Some(x), Some(y)) if x == y => { out.insert(k, x); }
                (Some(x), Some(y)) => { if o.is_some() || self.declared.contains(&k) { let s = self.mk(Op::Sel, vec![c, x, y]); out.insert(k, s); } }
                (Some(x), None) | (None, Some(x)) => { if let Some(o) = o { let (t, f) = if a.contains_key(&k) { (x, o) } else { (o, x) }; let s = self.mk(Op::Sel, vec![c, t, f]); out.insert(k, s); } }
                _ => {}
            }
        }
        out
    }
    /// names assigned anywhere inside the next balanced `{…}` group (pre-scan, so loop accumulators are known before the body)
    fn assigned_in_group(&self) -> Vec<String> {
        let mut out = Vec::new(); let mut j = self.i; let mut d = 0i32;
        while j < self.end { let s = &self.t[j].s; if s == "{" { d += 1; } if s == "}" { d -= 1; if d == 0 { break; } }
            if matches!(s.as_str(), "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "|=" | "&=" | "^=" | "<<=" | ">>=") {
                // walk back over a path a.b.c / a[i] to its root
                let mut k = j; let mut path = String::new(); let mut br = 0;
                while k > self.i { k -= 1; let t = &self.t[k].s; if t == "]" { br += 1; continue; } if t == "[" { br -= 1; continue; } if br > 0 { continue; }
                    if self.t[k].k == 0 || t == "self" { path = if path.is_empty() { t.clone() } else { format!("{t}.{path}") }; if k > 0 && self.t[k - 1].s == "." { k -= 1; continue; } break; }
                    if t == "*" { continue; } break; }
                if !path.is_empty() && !out.contains(&path) && !(k > 0 && self.t[k - 1].s == "let") { out.push(path); }
            }
            j += 1; }
        out
    }
    fn stmt_loop(&mut self) -> Id {
        let kw = self.eat();
        if kw == "for" {
            // for PAT in RANGE { body } → each written name v becomes fold(range, v₀, body(acc, item))
            let st = self.i; while self.i < self.end && self.peek() != "in" { self.i += 1; }
            let pats: Vec<String> = self.t[st..self.i].iter().filter(|t| t.k == 0).map(|t| t.s.clone()).collect();
            self.want("in"); let range = self.expr(true);
            if self.peek() != "{" { return self.opq(vec![range], "for without body"); }
            let acc_names = self.assigned_in_group();
            self.depth = self.depth.saturating_add(1); let d = self.depth;
            let item = self.mk(Op::Item(d), vec![]);
            let base = self.env.clone();
            let is_range = self.g.classes[self.g.find(range) as usize].iter().any(|n| n.0 == Op::Range);
            if pats.len() == 1 { let v = if is_range { item } else { self.mk(Op::Idx, vec![range, item]) }; self.env.insert(pats[0].clone(), v); }
            else { for (k, p) in pats.iter().enumerate() { let g = self.mk(Op::Get(k as u8), vec![item]); self.env.insert(p.clone(), g); } }
            for p in &pats { self.declared.insert(p.clone()); }
            let mut ord = HashMap::new();
            for (k, n) in acc_names.iter().enumerate() { let a = self.mk(Op::Acc(d, k as u8), vec![]); ord.insert(n.clone(), k as u8); self.env.insert(n.clone(), a); }
            self.accs.push(ord);
            let _ = self.block_value();
            let after = std::mem::replace(&mut self.env, base.clone());
            self.accs.pop(); self.depth -= 1;
            for n in &acc_names {
                let init = match base.get(n) { Some(&x) => x, None => self.var(n) };
                let body = after.get(n).copied().unwrap_or(init);
                let f = self.mk(Op::Fold, vec![range, init, body]); self.env.insert(n.clone(), f);
                if !self.declared.contains(n) && !self.written.contains(n) { self.written.push(n.clone()); }
            }
            return self.mk(Op::Tup, vec![]);
        }
        // while / loop: written names become opaque functions of their old values
        if kw == "while" { let _ = self.cond(); }
        if self.peek() != "{" { return self.opq(vec![], "loop"); }
        let names = self.assigned_in_group(); self.skip_group();
        for n in names { let o = match self.env.get(&n) { Some(&x) => x, None => self.var(&n) }; let v = self.opq(vec![o], &format!("{kw} body")); self.env.insert(n.clone(), v); if !self.declared.contains(&n) && !self.written.contains(&n) { self.written.push(n); } }
        self.opq(vec![], kw.as_str())
    }
    /// `{ stmt* tail? }` → the tail's class (or unit); statements update `env`
    fn block_value(&mut self) -> Id {
        if !self.want("{") { return self.opq(vec![], "block"); }
        let mut val = self.mk(Op::Tup, vec![]);
        while self.i < self.end && self.peek() != "}" {
            let before = self.i;
            if let Some(v) = self.stmt() { val = v; } else { val = self.mk(Op::Tup, vec![]); }
            if self.i == before { self.i += 1; }
        }
        self.want("}"); val
    }
    /// one statement; Some(value) when it is a tail expression (no `;`)
    fn stmt(&mut self) -> Option<Id> {
        match self.peek() {
            ";" => { self.eat(); return None; }
            "let" => {
                self.eat(); self.want("mut");
                let st = self.i; let mut d = 0i32;
                while self.i < self.end { let s = self.peek(); if d == 0 && (s == "=" || s == ";" || s == ":") { break; } if matches!(s, "(" | "[") { d += 1; } if matches!(s, ")" | "]") { d -= 1; } self.i += 1; }
                let pat: Vec<String> = self.t[st..self.i].iter().filter(|t| t.k == 0 && t.s != "mut" && t.s != "ref").map(|t| t.s.clone()).collect();
                if self.want(":") { while self.i < self.end && !matches!(self.peek(), "=" | ";") { self.i += 1; } }
                if self.want("=") {
                    let v = self.expr(false);
                    if self.want("else") { self.skip_group(); }
                    if pat.len() == 1 { self.env.insert(pat[0].clone(), v); } else { for (k, p) in pat.iter().enumerate() { let g = self.mk(Op::Get(k as u8), vec![v]); self.env.insert(p.clone(), g); } }
                }
                for p in pat { self.declared.insert(p); }
                self.want(";"); return None;
            }
            "if" => { let v = self.if_expr(); if self.want(";") { return None; } return if self.peek() == "}" { Some(v) } else { None }; }
            "for" | "while" | "loop" => { self.stmt_loop(); self.want(";"); return None; }
            "return" => { self.eat(); if !matches!(self.peek(), ";" | "}") { let v = self.expr(false); self.ret.push(v); } self.want(";"); return None; }
            "fn" | "struct" | "enum" | "impl" | "use" | "const" | "static" | "type" | "mod" | "#" => { self.skip_stmt(); return None; }
            "'" => { self.eat(); self.want(":"); return None; } // loop label
            _ => {}
        }
        // assignment or expression statement
        let st = self.i;
        let lhs_path = self.lvalue_path();
        if let Some((path, index)) = lhs_path {
            let op = self.peek().to_string();
            if matches!(op.as_str(), "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "|=" | "&=" | "^=" | "<<=" | ">>=") {
                self.eat(); let rhs = self.expr(false);
                let old = match self.env.get(&path) { Some(&x) => x, None => self.var(&path) };
                let cur = match index { Some(ix) => self.mk(Op::Idx, vec![old, ix]), None => old };
                let val = match op.as_str() { "=" => rhs, _ => {
                    let o = match &op[..op.len() - 1] { "+" => Op::Add, "-" => Op::Sub, "*" => Op::Mul, "/" => Op::Div, "%" => Op::Rem, "|" => Op::BOr, "&" => Op::BAnd, "^" => Op::BXor, "<<" => Op::Shl, _ => Op::Shr };
                    self.mk(o, vec![cur, rhs]) } };
                let newv = match index { Some(ix) => self.mk(Op::Store, vec![old, ix, val]), None => val };
                self.env.insert(path.clone(), newv);
                if !self.declared.contains(&path) && !self.written.contains(&path) && !path.is_empty() { self.written.push(path); }
                self.want(";"); return None;
            }
            self.i = st;
        }
        let v = self.expr(false);
        if self.want(";") { if self.g.classes[self.g.find(v) as usize].iter().any(|n| n.0.opaque()) { self.effects.push(v); } return None; }
        if self.peek() == "}" { return Some(v); }
        // unparsed rest of a statement
        let r = format!("stmt @{}", self.peek()); self.opq(vec![], &r); self.skip_stmt(); None
    }
    /// `a`, `a.b.c`, `*a`, `a[i]`, `self.x[i]` followed by an assignment operator → (path, index); restores nothing (caller rewinds)
    fn lvalue_path(&mut self) -> Option<(String, Option<Id>)> {
        let st = self.i; self.want("*");
        if !(self.i < self.end && (self.t[self.i].k == 0 || self.peek() == "self")) { self.i = st; return None; }
        let mut path = self.eat();
        while self.peek() == "." && self.i + 1 < self.end && (self.t[self.i + 1].k == 0 || self.t[self.i + 1].k == 2) && self.peek_at(2) != "(" { self.eat(); path = format!("{path}.{}", self.eat()); }
        let mut idx = None;
        if self.peek() == "[" { let save = self.i; let probe_ok = { let mut d = 0; let mut j = self.i; while j < self.end { let s = &self.t[j].s; if s == "[" { d += 1; } if s == "]" { d -= 1; if d == 0 { break; } } j += 1; } j + 1 < self.end && self.t[j + 1].s.ends_with('=') && !matches!(self.t[j + 1].s.as_str(), "==" | "<=" | ">=" | "!=") };
            if probe_ok { self.eat(); idx = Some(self.expr(false)); self.want("]"); } else { self.i = save; } }
        if self.peek().ends_with('=') && !matches!(self.peek(), "==" | "<=" | ">=" | "!=" | "=>") { return Some((path, idx)); }
        self.i = st; None
    }
}

/// Lower every `fn` in a Rust source into the shared e-graph. `slots` / `opq` are global counters so two functions never share an
/// input slot or an opaque node by accident.
pub fn lower_file(src: &str, g: &mut EGraph, slots: &mut u32, opq: &mut u32) -> Vec<FnEq> {
    let t = lex(src); let mut out = Vec::new(); let mut i = 0;
    while i < t.len() {
        if t[i].s != "fn" || i + 1 >= t.len() || t[i + 1].k != 0 { i += 1; continue; }
        let name = t[i + 1].s.clone(); let line = t[i].line;
        // find the body brace at depth 0 after the parameter list (skip `-> T where …`)
        let mut j = i + 2; let mut d = 0i32; let mut body = None;
        while j < t.len() { let s = &t[j].s; if s == "(" || s == "[" || s == "<" { d += 1; } if s == ")" || s == "]" || s == ">" { d -= 1; } if s == ">>" { d -= 2; }
            if d <= 0 && s == ";" { break; } if d <= 0 && s == "{" { body = Some(j); break; } j += 1; }
        let Some(b) = body else { i = j + 1; continue };
        let mut k = b; let mut dd = 0i32; while k < t.len() { if t[k].s == "{" { dd += 1; } if t[k].s == "}" { dd -= 1; if dd == 0 { break; } } k += 1; }
        let end = (k + 1).min(t.len());
        let mut lw = Lw { t: &t, i: b, end, g, env: HashMap::new(), declared: HashSet::new(), written: Vec::new(), effects: Vec::new(), ret: Vec::new(),
            slots, opq, inputs: HashMap::new(), nodes: 0, opaque: 0, reasons: Vec::new(), depth: 0, accs: Vec::new() };
        let v = lw.block_value();
        let mut outs: Vec<(String, Id)> = Vec::new();
        for r in &lw.ret { outs.push(("ret".into(), *r)); }
        if !lw.g.classes[lw.g.find(v) as usize].iter().any(|n| n.0 == Op::Tup && n.1.is_empty()) { outs.push(("ret".into(), v)); }
        for w in lw.written.clone() { if let Some(&x) = lw.env.get(&w) { outs.push((if w.starts_with("self.") { "field".into() } else { "out".into() }, x)); } }
        for e in lw.effects.clone() { outs.push(("effect".into(), e)); }
        out.push(FnEq { name, line, p: t[i].p, e: t[k.min(t.len() - 1)].e, outs, nodes: lw.nodes, opaque: lw.opaque, reasons: lw.reasons });
        i = b + 1; // nested fns are visited too (their bodies were skipped as items inside the outer body)
    }
    out
}

fn arg(a: &[String], k: &str) -> Option<String> { a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned() }

/// `omni-render core --abc lower --src F…` (A1 coverage) · `--abc probe` (A2 unit checks) · see an unpublished plan
pub fn cli(a: &[String]) {
    let sub = a.iter().skip_while(|x| *x != "--abc").nth(1).cloned().unwrap_or_default();
    match sub.as_str() {
        "lower" => {
            let srcs: Vec<String> = a.windows(2).filter(|w| w[0] == "--src").map(|w| w[1].clone()).collect();
            let thr: f64 = arg(a, "--opaque-max").and_then(|v| v.parse().ok()).unwrap_or(0.20);
            let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32);
            let t0 = std::time::Instant::now();
            let mut why: HashMap<String, usize> = HashMap::new();
            let (mut nf, mut good, mut nodes, mut opaque) = (0usize, 0usize, 0u64, 0u64);
            let mut rows = Vec::new();
            for s in &srcs {
                let Ok(src) = std::fs::read_to_string(s) else { eprintln!("abc: cannot read {s}"); continue };
                let fs = lower_file(&src, &mut g, &mut slots, &mut opq);
                for f in &fs {
                    nf += 1; nodes += f.nodes as u64; opaque += f.opaque as u64;
                    let share = if f.nodes == 0 { 1.0 } else { f.opaque as f64 / f.nodes as f64 };
                    if share <= thr && f.nodes > 0 { good += 1; }
                    for r in &f.reasons { let key: String = r.split_whitespace().take(2).collect::<Vec<_>>().join(" "); *why.entry(key).or_default() += 1; }
                    rows.push(serde_json::json!({"src": s, "fn": f.name, "line": f.line, "nodes": f.nodes, "opaque": f.opaque, "outs": f.outs.len()}));
                }
            }
            if let Some(o) = arg(a, "--out") { let txt: String = rows.iter().map(|r| r.to_string() + "\n").collect(); std::fs::write(&o, txt).ok(); }
            let mut w: Vec<_> = why.into_iter().collect(); w.sort_by(|x, y| y.1.cmp(&x.1));
            let j = serde_json::json!({"fns": nf, "lowered_ok": good, "share_ok": good as f64 / nf.max(1) as f64, "opaque_max": thr, "nodes": nodes, "opaque": opaque,
                "eclasses": g.classes.len(), "enodes": g.n_nodes(), "secs": t0.elapsed().as_secs_f64(), "top_opaque": w.iter().take(25).map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>()});
            println!("{j}");
            eprintln!("abc lower: {nf} fns · {good} with ≤ {:.0}% opaque nodes ({:.1}%) · {nodes} nodes, {opaque} opaque · {} e-nodes · {:.2}s", thr * 100.0, 100.0 * good as f64 / nf.max(1) as f64, g.n_nodes(), t0.elapsed().as_secs_f64());
        }
        "probe" => probe(),
        "recognise" | "recognize" => recognise(a),
        "words" => words(a),
        "patient" => patient(a),
        "shape" => super::shape::run(a),
        "t25" => super::shape::t25(a),
        "t25live" => super::shape::t25live(a),
        "t25fire" => super::shape::t25fire(a),
        "t25map" => super::shape::t25map(a),
        "t25solve" => super::fix::solve(a),
        "pristine" => super::shape::pristine(a),
        "anneal" => super::shape::anneal(a),
        "simd" => super::shape::simd(a),
        "lines" => super::shape::lines(a),
        "inject" => super::shape::inject(a),
        "need" => need(a),
        "tri" => tri(a),
        "dump" => { // every function's outputs as name-free signatures (debug view of the lowering)
            let src = arg(a, "--src").expect("--src F"); let only = arg(a, "--fn");
            let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32);
            let fs = lower_file(&std::fs::read_to_string(&src).expect("read"), &mut g, &mut slots, &mut opq);
            let best = g.extract();
            for f in fs.iter().filter(|f| only.as_ref().is_none_or(|o| *o == f.name)) {
                eprintln!("fn {} L{} bytes {}..{} · {} nodes, {} opaque · reasons {:?}", f.name, f.line, f.p, f.e, f.nodes, f.opaque, f.reasons.iter().take(8).collect::<Vec<_>>());
                for (role, id) in &f.outs { eprintln!("   {role}: {}", signature(&g, &best, *id, 14).0.chars().take(500).collect::<String>()); }
            }
        }
        _ => { eprintln!("usage: omni-render core --abc lower --src F [--src F…] [--opaque-max 0.2] [--out rows.jsonl] | --abc probe"); std::process::exit(2); }
    }
}

/// A3: lower → saturate → for every function, the α-signatures of its sub-terms (size ≥ `--min-size`); a signature shared by
/// two or more functions = the same equation used in two places. `--pair F:G` reports what two named functions share.
pub fn recognise(a: &[String]) {
    let srcs: Vec<String> = a.windows(2).filter(|w| w[0] == "--src").map(|w| w[1].clone()).collect();
    let min_size: u32 = arg(a, "--min-size").and_then(|v| v.parse().ok()).unwrap_or(4);
    let depth: usize = arg(a, "--depth").and_then(|v| v.parse().ok()).unwrap_or(8);
    let iters: usize = arg(a, "--iters").and_then(|v| v.parse().ok()).unwrap_or(3);
    let max_nodes: usize = arg(a, "--max-nodes").and_then(|v| v.parse().ok()).unwrap_or(300_000);
    let t0 = std::time::Instant::now();
    let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32); let mut fns: Vec<(String, FnEq)> = Vec::new();
    for s in &srcs { let Ok(src) = std::fs::read_to_string(s) else { continue }; for f in lower_file(&src, &mut g, &mut slots, &mut opq) { fns.push((s.clone(), f)); } }
    let n0 = g.n_nodes();
    let mut rs = rules(); if a.iter().any(|x| x == "--expand") { rs.extend(rules_from(RULES_EXPAND)); }
    let (it, sat) = if iters > 0 { saturate(&mut g, &rs, iters, max_nodes) } else { (0, true) };
    let best = g.extract();
    let mut by_sig: HashMap<String, Vec<usize>> = HashMap::new(); let mut sizes: HashMap<String, u32> = HashMap::new();
    for (fi, (_, f)) in fns.iter().enumerate() {
        let roots: Vec<Id> = f.outs.iter().map(|o| o.1).collect();
        let mut mine = HashSet::new();
        for c in reachable(&g, &best, &roots) {
            let (sig, size) = signature(&g, &best, c, depth);
            if size < min_size { continue; }
            if mine.insert(sig.clone()) { by_sig.entry(sig.clone()).or_default().push(fi); sizes.insert(sig, size); }
        }
    }
    let mut shared: Vec<(&String, &Vec<usize>)> = by_sig.iter().filter(|(_, v)| v.len() >= 2).collect();
    shared.sort_by(|x, y| (sizes[y.0] as usize * y.1.len()).cmp(&(sizes[x.0] as usize * x.1.len())).then(x.0.cmp(y.0)));
    let rows: Vec<serde_json::Value> = shared.iter().map(|(s, v)| serde_json::json!({"sig": s, "size": sizes[*s], "fns": v.iter().map(|&i| format!("{}:{}@L{}", fns[i].0.rsplit('/').next().unwrap_or(""), fns[i].1.name, fns[i].1.line)).collect::<Vec<_>>()})).collect();
    if let Some(o) = arg(a, "--out") { std::fs::write(&o, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
    for r in rows.iter().take(arg(a, "--top").and_then(|v| v.parse().ok()).unwrap_or(15)) { eprintln!("  {}", r.to_string().chars().take(400).collect::<String>()); }
    if let Some(pair) = arg(a, "--pair") {
        let (x, y) = pair.split_once(':').expect("--pair F:G");
        let fx: Vec<usize> = fns.iter().enumerate().filter(|(_, f)| f.1.name == x).map(|(i, _)| i).collect();
        let fy: Vec<usize> = fns.iter().enumerate().filter(|(_, f)| f.1.name == y).map(|(i, _)| i).collect();
        let mut both: Vec<(&String, u32)> = by_sig.iter().filter(|(_, v)| v.iter().any(|i| fx.contains(i)) && v.iter().any(|i| fy.contains(i))).map(|(s, _)| (s, sizes[s])).collect();
        both.sort_by(|p, q| q.1.cmp(&p.1));
        eprintln!("pair {x} ↔ {y}: {} shared signatures (size ≥ {min_size})", both.len());
        for (s, z) in both.iter().take(10) { eprintln!("    size {z}: {}", s.chars().take(300).collect::<String>()); }
        // A4: best anti-unifiers over every sub-term pair of the two functions (exact, pairwise — the sift shortlists at corpus scale)
        let subs = |ix: &[usize]| -> Vec<Id> { let mut v = Vec::new(); for &i in ix { let roots: Vec<Id> = fns[i].1.outs.iter().map(|o| o.1).collect(); v.extend(reachable(&g, &best, &roots)); } v };
        let (sx, sy) = (subs(&fx), subs(&fy));
        let mut gens: Vec<(u32, String)> = Vec::new(); let mut seen = HashSet::new();
        for &p in &sx { for &q in &sy { let (pat, n) = lgg(&g, &best, p, q, depth); if n >= min_size && seen.insert(pat.clone()) { gens.push((n, pat)); } } }
        gens.sort_by(|p, q| q.0.cmp(&p.0).then(p.1.len().cmp(&q.1.len())));
        eprintln!("pair {x} ↔ {y}: {} anti-unifiers with ≥ {min_size} shared operators ({} × {} sub-terms)", gens.len(), sx.len(), sy.len());
        for (n, pat) in gens.iter().take(8) { eprintln!("    lgg {n} ops: {}", pat.chars().take(300).collect::<String>()); }
    }
    println!("{}", serde_json::json!({"fns": fns.len(), "enodes_lowered": n0, "enodes_saturated": g.n_nodes(), "iters": it, "saturated": sat,
        "signatures": by_sig.len(), "shared": shared.len(), "shared_fns": shared.iter().flat_map(|(_, v)| v.iter()).collect::<HashSet<_>>().len(), "secs": t0.elapsed().as_secs_f64()}));
}

/// A4 at scale: words = anti-unifiers shared by sub-terms of DIFFERENT functions. The sift (Hamming nearness of structural HVs)
/// and the sieve (LSH bands: each band of `--bits` HV bits is a residue address; only same-bucket classes meet) shortlist the
/// pairs; the exact LGG decides. `--exhaustive` runs every cross-function pair instead (the recall reference).
pub fn words(a: &[String]) {
    let srcs: Vec<String> = a.windows(2).filter(|w| w[0] == "--src").map(|w| w[1].clone()).collect();
    let min_ops: u32 = arg(a, "--min-size").and_then(|v| v.parse().ok()).unwrap_or(5);
    let depth: usize = arg(a, "--depth").and_then(|v| v.parse().ok()).unwrap_or(8);
    let bands: usize = arg(a, "--bands").and_then(|v| v.parse().ok()).unwrap_or(32); // 32×12: 89 % weighted recall, top-30 30/30 vs exhaustive (layout.rs)
    let bits: usize = arg(a, "--bits").and_then(|v| v.parse().ok()).unwrap_or(12);
    let cap: usize = arg(a, "--bucket-cap").and_then(|v| v.parse().ok()).unwrap_or(64);
    let maxd: u32 = arg(a, "--max-hamming").and_then(|v| v.parse().ok()).unwrap_or(300);
    let exhaustive = a.iter().any(|x| x == "--exhaustive");
    let t0 = std::time::Instant::now();
    let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32); let mut fns: Vec<(String, FnEq)> = Vec::new();
    for s in &srcs { let Ok(src) = std::fs::read_to_string(s) else { continue }; for f in lower_file(&src, &mut g, &mut slots, &mut opq) { fns.push((s.clone(), f)); } }
    let best = g.extract();
    // candidate classes: sub-terms with ≥ min_ops operators (signature size), tagged by owning function
    let mut owner: HashMap<Id, Vec<usize>> = HashMap::new();
    for (fi, (_, f)) in fns.iter().enumerate() { let roots: Vec<Id> = f.outs.iter().map(|o| o.1).collect();
        for c in reachable(&g, &best, &roots) { if signature(&g, &best, c, depth).1 >= min_ops { let e = owner.entry(c).or_default(); if !e.contains(&fi) { e.push(fi); } } } }
    let cls: Vec<Id> = { let mut v: Vec<Id> = owner.keys().copied().collect(); v.sort(); v };
    let t_lower = t0.elapsed().as_secs_f64();
    let hv = class_hvs(&g, &best, &cls, depth);
    let t_hv = t0.elapsed().as_secs_f64() - t_lower;
    let cross = |p: Id, q: Id| owner[&p].iter().any(|x| !owner[&q].contains(x)) || owner[&q].iter().any(|x| !owner[&p].contains(x));
    let mut pairs: HashSet<(Id, Id)> = HashSet::new();
    if exhaustive { for i in 0..cls.len() { for j in i + 1..cls.len() { if cross(cls[i], cls[j]) { pairs.insert((cls[i], cls[j])); } } } }
    else {
        for b in 0..bands {
            let mut buckets: HashMap<u64, Vec<Id>> = HashMap::new();
            for &c in &cls { let v = &hv[&c]; let mut key = b as u64; for k in 0..bits { let i = (b * bits + k) % 1024; key = key << 1 | (v[i / 64] >> (i % 64) & 1); } buckets.entry(key).or_default().push(c); }
            for (_, mut m) in buckets { if m.len() < 2 { continue; } m.truncate(cap);
                for i in 0..m.len() { for j in i + 1..m.len() { let (p, q) = (m[i].min(m[j]), m[i].max(m[j])); if cross(p, q) && hamming(&hv[&p], &hv[&q]) <= maxd { pairs.insert((p, q)); } } } }
        }
    }
    let t_sieve = t0.elapsed().as_secs_f64() - t_lower - t_hv;
    let mut lex: HashMap<String, (u32, HashSet<usize>)> = HashMap::new();
    for &(p, q) in &pairs { let (pat, n) = lgg(&g, &best, p, q, depth); if n < min_ops { continue; }
        let e = lex.entry(pat).or_insert((n, HashSet::new())); e.1.extend(owner[&p].iter().copied()); e.1.extend(owner[&q].iter().copied()); }
    let names: HashMap<String, Vec<String>> = lex.iter().map(|(k, (_, f))| { let mut v: Vec<String> = f.iter().map(|&i| format!("{}:{}", fns[i].0.trim_start_matches("./"), fns[i].1.name)).collect(); v.sort(); (k.clone(), v) }).collect();
    let mut ws: Vec<(String, u32, usize)> = lex.into_iter().map(|(k, (n, f))| (k, n, f.len())).collect();
    ws.sort_by(|x, y| ((y.1 as usize) * y.2).cmp(&((x.1 as usize) * x.2)).then(x.0.cmp(&y.0)));
    let secs = t0.elapsed().as_secs_f64();
    if let Some(o) = arg(a, "--out") { std::fs::write(&o, ws.iter().map(|(p, n, f)| serde_json::json!({"word": p, "ops": n, "fns": f, "where": names[p]}).to_string() + "\n").collect::<String>()).ok(); }
    for (p, n, f) in ws.iter().take(arg(a, "--top").and_then(|v| v.parse().ok()).unwrap_or(12)) { eprintln!("  {n:3} ops × {f:3} fns  {}", p.chars().take(220).collect::<String>()); }
    let j = serde_json::json!({"mode": if exhaustive { "exhaustive" } else { "sift+sieve" }, "fns": fns.len(), "classes": cls.len(), "pairs_checked": pairs.len(), "words": ws.len(),
        "secs": secs, "secs_lower": t_lower, "secs_hv": t_hv, "secs_sieve": t_sieve, "secs_lgg": secs - t_lower - t_hv - t_sieve, "bands": bands, "bits": bits});
    println!("{j}");
}

/// A5 step 1 — search by function: each need = {"need", "args": ["int"|"list"…], "examples": [[[args…], out]…]}. Every library
/// sub-term with the same input count and types is evaluated on the examples under every argument order; a term that reproduces
/// every example answers the need (reported with its function and byte span). Names are never read.
pub fn need(a: &[String]) {
    let srcs: Vec<String> = a.windows(2).filter(|w| w[0] == "--src").map(|w| w[1].clone()).collect();
    let needs_path = arg(a, "--needs").expect("--needs F.jsonl");
    let depth: usize = arg(a, "--depth").and_then(|v| v.parse().ok()).unwrap_or(10);
    let t0 = std::time::Instant::now();
    let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32); let mut fns: Vec<(String, FnEq)> = Vec::new();
    for s in &srcs { let Ok(src) = std::fs::read_to_string(s) else { continue }; for f in lower_file(&src, &mut g, &mut slots, &mut opq) { fns.push((s.clone(), f)); } }
    let t_lower = t0.elapsed().as_secs_f64();
    let best = g.extract();
    let t_extract = t0.elapsed().as_secs_f64() - t_lower;
    // library: every sub-term of every function, with its owner and typed inputs (≤ 4 inputs, ≥ 1 operator)
    let mut lib: Vec<(Id, usize, Vec<(Id, bool)>)> = Vec::new(); let mut seen = HashSet::new();
    for (fi, (_, f)) in fns.iter().enumerate() { let roots: Vec<Id> = f.outs.iter().map(|o| o.1).collect();
        for c in reachable(&g, &best, &roots) { if !seen.insert(c) { continue; } let Some((_, n)) = best.get(&c) else { continue };
            if n.1.is_empty() || n.0.opaque() { continue; } let ins = eval_inputs(&g, &best, c, depth); if ins.is_empty() || ins.len() > 4 { continue; } lib.push((c, fi, ins)); } }
    let t_lib = t0.elapsed().as_secs_f64() - t_lower - t_extract;
    let (mut t_search, mut t_build, mut t_verify) = (0f64, 0f64, 0f64);
    let mut rows = Vec::new(); let (mut answered, mut total) = (0, 0);
    // learned words (from `--abc words --out`) usable as synthesis components
    let wlib: Vec<(Tm, usize)> = arg(a, "--words").map(|p| std::fs::read_to_string(p).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|j| parse_word(j["word"].as_str()?)).filter(|(_, h)| *h <= 4).collect()).unwrap_or_default();
    if !wlib.is_empty() { eprintln!("abc need: {} integer words usable as components", wlib.len()); }
    for l in std::fs::read_to_string(&needs_path).expect("needs").lines().filter(|l| !l.trim().is_empty()) {
        let nd: serde_json::Value = serde_json::from_str(l).expect("need json"); total += 1;
        let types: Vec<bool> = nd["args"].as_array().unwrap().iter().map(|t| t.as_str() == Some("list")).collect();
        let ex: Vec<(Vec<Val>, Val)> = nd["examples"].as_array().unwrap().iter().map(|e| (e[0].as_array().unwrap().iter().map(Val::from_json).collect(), Val::from_json(&e[1]))).collect();
        let hold: Vec<(Vec<Val>, Val)> = nd["holdout"].as_array().map(|h| h.iter().map(|e| (e[0].as_array().unwrap().iter().map(Val::from_json).collect(), Val::from_json(&e[1]))).collect()).unwrap_or_default();
        let mut hits: Vec<serde_json::Value> = Vec::new(); let ts0 = std::time::Instant::now();
        for (c, fi, ins) in &lib {
            if ins.len() != types.len() { continue; }
            // argument orders: permutations of the need's args onto the term's inputs, types must agree
            let n = ins.len(); let mut perm: Vec<usize> = (0..n).collect(); let mut found = None;
            loop {
                if perm.iter().enumerate().all(|(i, &p)| ins[i].1 == types[p]) {
                    let mut ok = true;
                    for (args, want) in ex.iter().chain(hold.iter()) { let mut ev = Ev::new(&g, &best); for (i, &p) in perm.iter().enumerate() { ev.leaf.insert(ins[i].0, args[p].clone()); } if norm(ev.eval(*c)) != norm(want.clone()) { ok = false; break; } }
                    if ok { found = Some(perm.clone()); break; }
                }
                // next permutation
                let mut i = n.saturating_sub(1); while i > 0 && perm[i - 1] >= perm[i] { i -= 1; } if i == 0 { break; }
                let mut j = n - 1; while perm[j] <= perm[i - 1] { j -= 1; } perm.swap(i - 1, j); perm[i..].reverse();
            }
            if let Some(p) = found { let f = &fns[*fi]; hits.push(serde_json::json!({"fn": format!("{}:{}", f.0.trim_start_matches("./"), f.1.name), "line": f.1.line, "term": signature(&g, &best, *c, 8).0.chars().take(160).collect::<String>(), "arg_order": p})); if hits.len() >= 5 { break; } }
        }
        let search_secs = ts0.elapsed().as_secs_f64(); t_search += search_secs;
        let mut built = serde_json::Value::Null;
        if (hits.is_empty() || a.iter().any(|x| x == "--build-all")) && a.iter().any(|x| x == "--build" || x == "--build-all") {
            // step 3: nothing whole fits — compose it, smallest first, then verify independently (rustc on the examples)
            let ts = std::time::Instant::now();
            // CEGIS (Solar-Lezama 2008): a candidate that fails a held-out example gets that example as a counterexample, search again
            let mut train = ex.clone(); let mut rounds = 0; let mut cand = None; let (mut ffw, mut buw, mut ffhow) = (0u64, 0u64, String::new());
            while rounds < 16 { rounds += 1;
                let ff = a.iter().any(|x| x == "--ff");
                let found = if ff { let mut st = FfStats { work: 0, bank: 0, how: String::new() }; let r = ff_synth(&types, &train, &mut st); ffw += st.work; ffhow = st.how.clone(); r }
                    else { BU_WORK.with(|w| w.set(0)); let r = synth_w(&types, &train, arg(a, "--max-size").and_then(|v| v.parse().ok()).unwrap_or(7), 3_000_000, &wlib); buw += BU_WORK.with(|w| w.get()); r };
                if std::env::var("OMR_FF_DEBUG").is_ok() { eprintln!("  round {rounds}: train {} → {}", train.len(), found.as_ref().map(rust_e).unwrap_or("none".into())); }
                let Some(e) = found else { break };
                match hold.iter().find(|(args, want)| norm(ev_e(&e, args, None)) != norm(want.clone())) { Some(cx) => train.push(cx.clone()), None => { cand = Some(e); break; } } }
            if let Some(e) = cand {
                let name = nd["need"].as_str().unwrap_or("need").replace(|c: char| !c.is_ascii_alphanumeric(), "_");
                // the verifier sees the held-out examples too (the builder never did)
                let mut all = ex.clone(); all.extend(hold.iter().cloned());
                let synth_secs = ts.elapsed().as_secs_f64(); let tv = std::time::Instant::now();
                let v = verify_rust(&e, &types, &all, &arg(a, "--build-dir").unwrap_or("/tmp/omr-abc-build".into()), &name);
                let verify_secs = tv.elapsed().as_secs_f64(); t_build += synth_secs; t_verify += verify_secs;
                // final check on oracle answers the builder never saw (neither as examples nor as counterexamples)
                let val: Vec<(Vec<Val>, Val)> = nd["validate"].as_array().map(|h| h.iter().map(|e| (e[0].as_array().unwrap().iter().map(Val::from_json).collect(), Val::from_json(&e[1]))).collect()).unwrap_or_default();
                let validate = if val.is_empty() { serde_json::Value::Null } else {
                    let ok = val.iter().filter(|(a, w)| norm(ev_e(&e, a, None)) == norm(w.clone())).count();
                    let rv = verify_rust(&e, &types, &val, &arg(a, "--build-dir").unwrap_or("/tmp/omr-abc-build".into()), &format!("{name}-validate"));
                    serde_json::json!({"agree": ok, "of": val.len(), "rustc": rv.is_ok()}) };
                built = serde_json::json!({"rust": rust_e(&e), "size": size_e(&e), "verified": v.is_ok(), "verify": v.err(), "synth_secs": synth_secs, "verify_secs": verify_secs, "validate": validate, "cegis_rounds": rounds, "work": if ffw > 0 { ffw } else { buw }, "how": ffhow, "secs": ts.elapsed().as_secs_f64()});
            } else { t_build += ts.elapsed().as_secs_f64(); built = serde_json::json!({"rust": null, "cegis_rounds": rounds, "work": if ffw > 0 { ffw } else { buw }, "secs": ts.elapsed().as_secs_f64()}); }
        }
        if !hits.is_empty() { answered += 1; } else if built["verified"] == true { answered += 1; }
        eprintln!("{} {:24} {} hit(s){}", if !hits.is_empty() { "HIT " } else if built["verified"] == true { "BUILT" } else { "MISS" }, nd["need"].as_str().unwrap_or("?"), hits.len(), hits.first().map(|h| format!(" — {} L{}: {}", h["fn"].as_str().unwrap(), h["line"], h["term"].as_str().unwrap())).unwrap_or_default().chars().take(200).collect::<String>());
        if !built.is_null() { eprintln!("      build: {}", built.to_string().chars().take(220).collect::<String>()); }
        rows.push(serde_json::json!({"need": nd["need"], "hits": hits, "built": built, "search_secs": search_secs}));
    }
    if let Some(o) = arg(a, "--out") { std::fs::write(&o, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
    println!("{}", serde_json::json!({"needs": total, "answered": answered, "library_terms": lib.len(), "fns": fns.len(), "secs": t0.elapsed().as_secs_f64(),
        "phase_secs": {"lower_library": t_lower, "extract": t_extract, "collect_library": t_lib, "search_all_needs": t_search, "build_all_needs": t_build, "rustc_verify_all": t_verify}}));
}

// ── TRI-CODE-0 (author 10-06, readback confirmed): Omnitri triangles + polygons for code ─────────────────────────────────────────
// A triangle = one equation as a bound subject–relation–object fact: S = its inputs' structural vectors (ordered by position,
// commutative operands sorted), R = the operator, O = its output's structural vector; T = S ⊕ ρ⁵(R) ⊕ ρ⁹(O). A function's polygon
// = the superposition (bipolar int accumulator) of its triangles. Query = a few triangles (pieces / needs) → every polygon is
// scored by its inner product with them (normalised by √triangles); the exact steps then decide (R1/R4). 8 lanes × 1,024 bits.
// Behaviour face: each equation's outputs on fixed probe inputs (value fingerprint), indexed exactly and as a vector.
const TRI_LANES: usize = 8;
const PROBE_I: [[i64; 4]; 8] = [[3, 7, -2, 11], [0, 5, 9, -4], [12, -6, 1, 8], [-9, 2, 14, 3], [6, 6, -1, 0], [21, -13, 4, 7], [1, 0, -8, 15], [-5, 17, 6, 2]];
/// canonical probe value k for argument position `pos` of a need / an equation (the same table is written in the needs generator)
pub fn probe_val(k: usize, pos: usize, list: bool) -> Val {
    if !list { return Val::I(PROBE_I[k][pos % 4]); }
    let n = 1 + (k * 3 + pos * 5) % 6; Val::L((0..n).map(|j| Val::I(((k * 7 + pos * 11 + j * 13) % 23) as i64 - 9)).collect())
}
fn xor_hv(a: &Hv, b: &Hv) -> Hv { let mut o = [0u64; 16]; for i in 0..16 { o[i] = a[i] ^ b[i]; } o }
fn op_lane_hv(op: &Op, lane: u64) -> Hv { seed_hv(op.sym().bytes().fold(0xcbf2_9ce4_8422_2325u64 ^ lane.wrapping_mul(0xD6E8_FEB8_6659_FD93) ^ 0x7121, |a, b| (a ^ b as u64).wrapping_mul(0x100_0000_01b3))) }

pub fn tri(a: &[String]) {
    let srcs: Vec<String> = a.windows(2).filter(|w| w[0] == "--src").map(|w| w[1].clone()).collect();
    let sdepth: usize = arg(a, "--shape-depth").and_then(|v| v.parse().ok()).unwrap_or(3);
    let nq: usize = arg(a, "--pieces").and_then(|v| v.parse().ok()).unwrap_or(3);
    let ntest: usize = arg(a, "--tests").and_then(|v| v.parse().ok()).unwrap_or(300);
    let noise: usize = arg(a, "--noise").and_then(|v| v.parse().ok()).unwrap_or(0); // pieces replaced by a piece from another function
    let t0 = std::time::Instant::now();
    let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32); let mut fns: Vec<(String, FnEq)> = Vec::new();
    for s in &srcs { let Ok(src) = std::fs::read_to_string(s) else { continue }; for f in lower_file(&src, &mut g, &mut slots, &mut opq) { fns.push((s.clone(), f)); } }
    let best = g.extract();
    // classes of every function (its equations) — non-leaf, non-opaque operator nodes
    let fcls: Vec<Vec<Id>> = fns.iter().map(|(_, f)| { let roots: Vec<Id> = f.outs.iter().map(|o| o.1).collect();
        reachable(&g, &best, &roots).into_iter().filter(|c| best.get(c).is_some_and(|(_, n)| !n.1.is_empty() && !n.0.opaque())).collect() }).collect();
    let all: Vec<Id> = { let mut v: Vec<Id> = fcls.iter().flatten().copied().collect::<HashSet<_>>().into_iter().collect(); v.sort(); v };
    let mut kids: HashSet<Id> = HashSet::new(); for &c in &all { for &k in &best[&c].1 .1 { kids.insert(g.find(k)); } }
    let shape_cls: Vec<Id> = all.iter().copied().chain(kids.iter().copied()).collect::<HashSet<_>>().into_iter().collect();
    let lanes: Vec<HashMap<Id, Hv>> = (0..TRI_LANES).map(|l| class_hvs_lane(&g, &best, &shape_cls, sdepth, l as u64 + 1)).collect();
    let tri_of = |c: Id| -> Vec<Hv> {
        let n = &best[&c].1; let mut ks: Vec<Id> = n.1.iter().map(|&k| g.find(k)).collect();
        (0..TRI_LANES).map(|l| { let hv = &lanes[l];
            if COMM.contains(&n.0) { ks.sort_by_key(|k| hv[k]); }
            let mut s_ = [0u64; 16]; for (j, k) in ks.iter().enumerate() { s_ = xor_hv(&s_, &rot(&hv[k], 3 * j)); }
            xor_hv(&xor_hv(&s_, &rot(&op_lane_hv(&n.0, l as u64), 5)), &rot(&hv[&g.find(c)], 9)) }).collect()
    };
    let tris: HashMap<Id, Vec<Hv>> = all.iter().map(|&c| (c, tri_of(c))).collect();
    // polygons: bipolar accumulators, one per function
    let d = TRI_LANES * 1024;
    let polys: Vec<Vec<i16>> = fcls.iter().map(|cs| { let mut p = vec![0i16; d];
        for c in cs { for (l, t) in tris[c].iter().enumerate() { for b in 0..1024 { p[l * 1024 + b] += if t[b / 64] >> (b % 64) & 1 == 1 { 1 } else { -1 }; } } } p }).collect();
    let t_build = t0.elapsed().as_secs_f64();
    // ── test 1: pieces → the whole (recall of the source function) vs exact signatures (A3)
    let sigs: Vec<HashSet<String>> = fcls.iter().map(|cs| cs.iter().map(|&c| signature(&g, &best, c, 8).0).collect()).collect();
    let mut rng = 0x5EED_u64; let mut next = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let cand: Vec<usize> = (0..fns.len()).filter(|&i| fcls[i].len() >= 2 * nq).collect();
    let (mut r1, mut r5, mut e1, mut e5, mut nt) = (0f64, 0f64, 0f64, 0f64, 0usize); let mut qsecs = 0f64;
    for t in 0..ntest.min(cand.len()) {
        let fi = cand[(t * 7919) % cand.len()]; let cs = &fcls[fi];
        let mut q: Vec<Id> = Vec::new(); while q.len() < nq { let c = cs[(next() as usize) % cs.len()]; if !q.contains(&c) { q.push(c); } }
        for k in 0..noise.min(nq) { loop { let o = cand[(next() as usize) % cand.len()]; if o == fi { continue; } let c = fcls[o][(next() as usize) % fcls[o].len()]; if !cs.contains(&c) { q[k] = c; break; } } }
        let tq = std::time::Instant::now();
        let qb: Vec<Vec<i16>> = q.iter().map(|c| { let mut v = vec![0i16; d]; for (l, t) in tris[c].iter().enumerate() { for b in 0..1024 { v[l * 1024 + b] = if t[b / 64] >> (b % 64) & 1 == 1 { 1 } else { -1 }; } } v }).collect();
        let mut scores: Vec<(f64, usize)> = polys.iter().enumerate().map(|(j, p)| { let mut s = 0i64; for v in &qb { for b in 0..d { s += p[b] as i64 * v[b] as i64; } } (s as f64 / (fcls[j].len().max(1) as f64).sqrt(), j) }).collect();
        scores.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
        qsecs += tq.elapsed().as_secs_f64();
        let rank = scores.iter().position(|x| x.1 == fi).unwrap();
        // ties: functions with the same score as the target count against it
        let ahead = scores.iter().filter(|x| x.0 > scores[rank].0).count(); let tied = scores.iter().filter(|x| x.0 == scores[rank].0).count();
        let credit = |k: usize| -> f64 { if ahead >= k { 0.0 } else { ((k - ahead) as f64 / tied as f64).min(1.0) } };
        r1 += credit(1); r5 += credit(5);
        let qs: Vec<String> = q.iter().map(|&c| signature(&g, &best, c, 8).0).collect();
        // exact baseline, tolerant form: rank functions by how many query signatures they contain (ties count against the target)
        let votes: Vec<usize> = (0..fns.len()).map(|j| qs.iter().filter(|x| sigs[j].contains(*x)).count()).collect();
        let vt = votes[fi]; let ahead_e = votes.iter().filter(|&&v| v > vt).count(); let tied_e = votes.iter().filter(|&&v| v == vt).count();
        let ce = |k: usize| -> f64 { if ahead_e >= k { 0.0 } else { ((k - ahead_e) as f64 / tied_e as f64).min(1.0) } };
        e1 += ce(1); e5 += ce(5); nt += 1;
    }
    eprintln!("tri test 1 (pieces → whole): {nt} functions, {nq} random triangles each — polygon recall@1 {:.3} @5 {:.3} · exact signatures (A3) @1 {:.3} @5 {:.3} · {:.2} ms/query over {} polygons",
        r1 / nt as f64, r5 / nt as f64, e1 / nt as f64, e5 / nt as f64, 1000.0 * qsecs / nt.max(1) as f64, fns.len());
    // ── test 2: behaviour face — needs stated on the canonical probes ("probe_out"), exact index + vector shortlist
    let mut by_fp: HashMap<String, Vec<(Id, usize)>> = HashMap::new();
    let owner: HashMap<Id, usize> = fcls.iter().enumerate().flat_map(|(i, cs)| cs.iter().map(move |&c| (c, i))).collect();
    let tb = std::time::Instant::now(); let mut nfp = 0usize;
    for &c in &all {
        let ins = eval_inputs(&g, &best, c, 10); if ins.is_empty() || ins.len() > 4 { continue; }
        let n = ins.len(); let mut perm: Vec<usize> = (0..n).collect();
        loop {
            // need argument j feeds equation input perm[j]
            let types: String = perm.iter().map(|&i| if ins[i].1 { 'L' } else { 'I' }).collect();
            let outs: Vec<Val> = (0..8).map(|k| { let mut ev = Ev::new(&g, &best); for (j, &i) in perm.iter().enumerate() { ev.leaf.insert(ins[i].0, probe_val(k, j, ins[i].1)); } norm(ev.eval(c)) }).collect();
            if outs.iter().filter(|v| **v == Val::Bot).count() < 3 { by_fp.entry(format!("{types}|{outs:?}")).or_default().push((c, perm.len())); nfp += 1; }
            let mut i = n.saturating_sub(1); while i > 0 && perm[i - 1] >= perm[i] { i -= 1; } if i == 0 { break; }
            let mut j = n - 1; while perm[j] <= perm[i - 1] { j -= 1; } perm.swap(i - 1, j); perm[i..].reverse();
        }
    }
    let t_fp = tb.elapsed().as_secs_f64();
    if let Some(np) = arg(a, "--needs") {
        let (mut hit, mut tot) = (0, 0); let tq = std::time::Instant::now();
        for l in std::fs::read_to_string(np).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()) {
            let nd: serde_json::Value = serde_json::from_str(l).unwrap(); let Some(po) = nd["probe_out"].as_array() else { continue }; tot += 1;
            let types: String = nd["args"].as_array().unwrap().iter().map(|t| if t.as_str() == Some("list") { 'L' } else { 'I' }).collect();
            let outs: Vec<Val> = po.iter().map(|v| norm(Val::from_json(v))).collect();
            let hits = by_fp.get(&format!("{types}|{outs:?}")).cloned().unwrap_or_default();
            if !hits.is_empty() { hit += 1; }
            let first = hits.first().map(|(c, _)| { let fi = owner[c]; format!("{}:{} — {}", fns[fi].0.trim_start_matches("./"), fns[fi].1.name, signature(&g, &best, *c, 6).0.chars().take(110).collect::<String>()) }).unwrap_or_default();
            eprintln!("  {} {:18} {:3} equation(s) {first}", if hits.is_empty() { "MISS" } else { "HIT " }, nd["need"].as_str().unwrap_or("?"), hits.len());
        }
        eprintln!("tri test 2 (behaviour face): {hit}/{tot} needs found by fingerprint · {:.3} ms total query time · index {nfp} fingerprints built in {:.2}s", 1000.0 * tq.elapsed().as_secs_f64(), t_fp);
    }
    println!("{}", serde_json::json!({"fns": fns.len(), "equations": all.len(), "lanes": TRI_LANES, "shape_depth": sdepth, "build_secs": t_build, "tests": nt,
        "recall1": r1 / nt.max(1) as f64, "recall5": r5 / nt.max(1) as f64, "exact_recall1": e1 / nt.max(1) as f64, "exact_recall5": e5 / nt.max(1) as f64, "ms_per_query": 1000.0 * qsecs / nt.max(1) as f64, "fingerprints": nfp}));
}

/// PATIENT run (author 10-06: "run it on the actual thing you're debugging"): every failing ORIGIN box of the renderer (wrong while its
/// parent agrees) on the replay pages, grouped into families by failure signature. Per family the need is: inputs = our value on
/// the wrong axis + the layout terms the engine recorded for that box (term ledger, 1/64 px), output = Chrome's value, goal = the
/// agreement tolerance (±max(2 px, 5 %)) as an interval — the frame-first witnesses take intervals natively. Fit on 70 % of the
/// family's boxes, measured on the held-out 30 %. `omni-render core corpus --abc patient [--min 6] [--top 12]`
pub fn patient(a: &[String]) {
    use std::collections::BTreeMap;
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let min_n: usize = arg(a, "--min").and_then(|v| v.parse().ok()).unwrap_or(6);
    let top: usize = arg(a, "--top").and_then(|v| v.parse().ok()).unwrap_or(12);
    let prog = arg(a, "--program").unwrap_or_else(|| "results/omr.program.json".into());
    let t0 = std::time::Instant::now();
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, _hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    // (family sig) → boxes: (page, node, ours axis value px, chrome value px, terms key → value)
    let mut fam: BTreeMap<String, Vec<(String, u32, f32, f32, BTreeMap<&'static str, i64>)>> = BTreeMap::new();
    for p in &replay {
        crate::layout::ledger_start();
        let Some(f) = super::super::score_full(p) else { crate::layout::ledger_take(); continue };
        let led = crate::layout::ledger_take();
        let mut terms: HashMap<u32, BTreeMap<&'static str, i64>> = HashMap::new();
        for (n, k, v) in led { *terms.entry(n).or_default().entry(k).or_default() += v as i64; }
        let cc = super::super::cached(p);
        for o in f.origins {
            let chrome = o.ours - o.err;
            let tagc = cc.as_ref().map(|c| format!("{}.{}", c.l.dom.tag(o.n), c.l.dom.attr(o.n, "class").unwrap_or("").split_whitespace().next().unwrap_or(""))).unwrap_or_default();
            fam.entry(o.sig.clone()).or_default().push((format!("{}#{tagc}", p.file_name().unwrap().to_string_lossy()), o.n, o.ours, chrome, terms.remove(&o.n).unwrap_or_default()));
        }
    }
    let t_collect = t0.elapsed().as_secs_f64();
    let nboxes: usize = fam.values().map(|v| v.len()).sum();
    eprintln!("patient: {} replay pages · {nboxes} failing origin boxes in {} families · collected in {t_collect:.1}s", replay.len(), fam.len());
    let mut fams: Vec<(String, Vec<_>)> = fam.into_iter().filter(|(_, v)| v.len() >= min_n).collect();
    fams.sort_by(|x, y| y.1.len().cmp(&x.1.len()));
    let mut rows = Vec::new();
    for (sig, boxes) in fams.into_iter().take(top) {
        let ts = std::time::Instant::now();
        // inputs: our value + the 3 ledger keys present on most boxes of the family (missing = 0)
        let mut cnt: HashMap<&'static str, usize> = HashMap::new();
        for b in &boxes { for k in b.4.keys() { *cnt.entry(*k).or_default() += 1; } }
        // choose the 3 ledger keys (present on ≥ half the boxes) most correlated with the miss (Chrome − ours): the terms that explain it
        let miss: Vec<f64> = boxes.iter().map(|b| ((b.3 - b.2) * 64.0) as f64).collect();
        let corr = |k: &str| -> f64 { let xs: Vec<f64> = boxes.iter().map(|b| *b.4.get(k).unwrap_or(&0) as f64).collect(); let n = xs.len() as f64;
            let (mx, my) = (xs.iter().sum::<f64>() / n, miss.iter().sum::<f64>() / n);
            let (sxy, sxx, syy) = xs.iter().zip(&miss).fold((0.0, 0.0, 0.0), |a, (x, y)| (a.0 + (x - mx) * (y - my), a.1 + (x - mx).powi(2), a.2 + (y - my).powi(2)));
            if sxx == 0.0 || syy == 0.0 { 0.0 } else { (sxy / (sxx * syy).sqrt()).abs() } };
        let mut keys: Vec<(&'static str, f64)> = cnt.into_iter().filter(|(_, c)| 2 * c >= boxes.len()).map(|(k, _)| (k, corr(k))).collect();
        keys.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap().then(x.0.cmp(y.0)));
        let keys: Vec<&'static str> = keys.into_iter().take(3).map(|k| k.0).collect();
        let names: Vec<String> = std::iter::once("ours".to_string()).chain(keys.iter().map(|k| k.to_string())).collect();
        let mk = |b: &(String, u32, f32, f32, BTreeMap<&'static str, i64>)| -> (Vec<Val>, Rg) {
            let mut v = vec![Val::I((b.2 * 64.0).round() as i64)]; for k in &keys { v.push(Val::I(*b.4.get(k).unwrap_or(&0))); }
            let c = b.3 * 64.0; let tol = (2.0f32.max(b.3.abs() * 0.05) * 64.0).floor();
            (v, ((c - tol).ceil() as i64, (c + tol).floor() as i64))
        };
        let all: Vec<(Vec<Val>, Rg)> = boxes.iter().map(mk).collect();
        let (train, hold): (Vec<_>, Vec<_>) = all.iter().cloned().enumerate().partition(|(i, _)| i % 10 < 7);
        let train: Vec<(Vec<Val>, Rg)> = train.into_iter().map(|x| x.1).collect(); let hold: Vec<(Vec<Val>, Rg)> = hold.into_iter().map(|x| x.1).collect();
        let types = vec![false; names.len()];
        let ex: Vec<(Vec<Val>, Val)> = train.iter().map(|(v, g)| (v.clone(), Val::I(g.0))).collect();
        let goal: Vec<Rg> = train.iter().map(|x| x.1).collect();
        let mut work = 0u64;
        let bank = ff_bank(&types, &ex, 3, 200_000, &mut work);
        let mut found = None;
        for depth in 0..=2 { if let Some(e) = ff_solve(&bank, &goal, depth, 120, &mut work) {
            if train.iter().all(|(v, g)| ev_e(&e, v, None).i().is_some_and(|x| x >= g.0 && x <= g.1)) { found = Some((e, depth)); break; } } }
        let fit = |e: &E, set: &[(Vec<Val>, Rg)]| set.iter().filter(|(v, g)| ev_e(e, v, None).i().is_some_and(|x| x >= g.0 && x <= g.1)).count();
        let (eq, depth, ht) = match &found { Some((e, d)) => { let mut t = rust_e(e); for (i, n) in names.iter().enumerate().rev() { t = t.replace(&format!("a{i}"), n); } (t, *d as i64, fit(e, &hold)) } None => ("—".into(), -1, 0) };
        let secs = ts.elapsed().as_secs_f64();
        let mean_err: f32 = boxes.iter().map(|b| (b.2 - b.3).abs()).sum::<f32>() / boxes.len() as f32;
        eprintln!("  {:34} n {:4} mean |err| {:6.1}px · inputs [{}] · {} · held-out {}/{} agree · {:.2}s, work {}", sig, boxes.len(), mean_err, names.join(", "),
            if found.is_some() { format!("depth {depth}: {}", eq.chars().take(90).collect::<String>()) } else { "no equation".into() }, ht, hold.len(), secs, work);
        rows.push(serde_json::json!({"family": sig, "boxes": boxes.len(), "mean_err_px": mean_err, "inputs": names, "equation": eq, "depth": depth,
            "heldout_agree": ht, "heldout": hold.len(), "secs": secs, "work": work, "box_list": boxes.iter().map(|b| (b.0.clone(), b.1, b.2, b.3)).collect::<Vec<_>>(), "pages": boxes.iter().map(|b| b.0.clone()).collect::<HashSet<_>>().len()}));
    }
    if let Some(o) = arg(a, "--out") { std::fs::write(&o, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
    println!("{}", serde_json::json!({"origin_boxes": nboxes, "families_run": rows.len(), "collect_secs": t_collect, "total_secs": t0.elapsed().as_secs_f64()}));
}

/// A2 unit checks: the three spellings of 2x meet; clamp meets min∘max; (a+b)−b meets a.
pub fn probe() {
    let rs = rules();
    let mut ok = true;
    let cases: &[(&str, &str, &str)] = &[
        ("x*2 ≡ x+x ≡ x<<1", "fn f(x: i32) -> i32 { x * 2 }\nfn g(x: i32) -> i32 { x + x }\nfn h(x: i32) -> i32 { x << 1 }", "same"),
        ("clamp ≡ min∘max", "fn f(x: i32, a: i32, b: i32) -> i32 { x.clamp(a, b) }\nfn g(x: i32, a: i32, b: i32) -> i32 { x.max(a).min(b) }", "same"),
        ("(a+b)−b ≡ a", "fn f(a: i32, b: i32) -> i32 { (a + b) - b }\nfn g(a: i32, b: i32) -> i32 { a }", "same"),
        ("a−b ≢ b−a", "fn f(a: i32, b: i32) -> i32 { a - b }\nfn g(a: i32, b: i32) -> i32 { b - a }", "diff"),
    ];
    for (name, src, want) in cases {
        // the same input names across the test fns are the SAME symbols (one shared slot table per case)
        let mut g = EGraph::default(); let (mut slots, mut opq) = (0u32, 0u32);
        let shared: String = src.lines().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        let fs = lower_shared(&shared, &mut g, &mut slots, &mut opq);
        let (it, sat) = saturate(&mut g, &rs, 12, 20_000);
        let roots: Vec<Id> = fs.iter().map(|f| g.find(f.outs.iter().find(|o| o.0 == "ret").map(|o| o.1).unwrap_or(0))).collect();
        let same = roots.windows(2).all(|w| w[0] == w[1]);
        let pass = (*want == "same") == same; ok &= pass;
        eprintln!("{} {name}: {} fns → classes {:?} after {it} iterations (saturated {sat}, {} e-nodes)", if pass { "PASS" } else { "FAIL" }, fs.len(), roots, g.n_nodes());
    }
    eprintln!("abc probe: {}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

/// Lower several fns that share their input names (a test fixture: parameter `x` in f and g is one symbol).
fn lower_shared(src: &str, g: &mut EGraph, slots: &mut u32, opq: &mut u32) -> Vec<FnEq> {
    let t = lex(src); let mut out = Vec::new(); let mut env: HashMap<String, Id> = HashMap::new(); let mut i = 0;
    while i < t.len() {
        if t[i].s != "fn" { i += 1; continue; }
        let mut b = i; while b < t.len() && t[b].s != "{" { b += 1; }
        let mut k = b; let mut d = 0; while k < t.len() { if t[k].s == "{" { d += 1; } if t[k].s == "}" { d -= 1; if d == 0 { break; } } k += 1; }
        let mut lw = Lw { t: &t, i: b, end: (k + 1).min(t.len()), g, env: env.clone(), declared: HashSet::new(), written: Vec::new(), effects: Vec::new(), ret: Vec::new(),
            slots, opq, inputs: HashMap::new(), nodes: 0, opaque: 0, reasons: Vec::new(), depth: 0, accs: Vec::new() };
        let v = lw.block_value();
        for (n, id) in lw.env.iter() { if !env.contains_key(n) && !lw.declared.contains(n) { env.insert(n.clone(), *id); } }
        out.push(FnEq { name: t[i + 1].s.clone(), line: t[i].line, p: 0, e: 0, outs: vec![("ret".into(), v)], nodes: lw.nodes, opaque: lw.opaque, reasons: vec![] });
        i = k + 1;
    }
    out
}

