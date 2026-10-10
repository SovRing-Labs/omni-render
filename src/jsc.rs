// SPDX-License-Identifier: MIT OR Apache-2.0
//! JS-COMPILE-0 C3: closure compilation of hot function bodies (Feeley & Lapalme 1987 "Using closures for code
//! generation"; Würthinger et al. 2013 self-specialising AST interpreters — ideas only, clean-room).
//!
//! A function body that has run `OMR_JIT_N` times (default 2; `OMR_JIT=0` turns this off) is turned once into nested
//! Rust closures: each node's dispatch is decided at compile time, and the common shapes (number literals, slot reads,
//! numeric binary operators, slot / member assignment, updates, static member reads, plain and method calls, if /
//! block / for / while / return / let) get fused fast paths. Every other node compiles to a call back into the
//! tree-walker (`Interp::ev` / `Interp::ex`) on that same node, so there is one semantics: "compiled == interpreted"
//! is the C7 law (omniglyph js laws with `OMR_JIT_N=1` against the interpreter).
use crate::jsi::{to_int32, Ctl, Interp, Scope, Val};
use crate::jsi_ast::{AssignOp, Bin, Body, Expr, FuncBody, Key, Lit, Logic, Pat, Stmt, Target, Un, VarKind};
use std::rc::Rc;

pub type CE = Box<dyn Fn(&mut Interp, &Scope) -> Result<Val, Ctl>>;
pub type CS = Box<dyn Fn(&mut Interp, &Scope) -> Result<(), Ctl>>;

/// A compiled body, kept beside the nodes it was made from (`FuncBody.code`).
pub struct Code(pub CE);
impl std::fmt::Debug for Code { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Code") } }

/// Nodes are owned by the FuncBody that also owns the compiled code: they never move or change once lowered and
/// resolved, so a fallback closure may keep a pointer to its node for as long as the code lives.
#[derive(Clone, Copy)]
struct P<T>(*const T);
impl<T> P<T> { #[inline] fn get(&self) -> &T { unsafe { &*self.0 } } }

fn jit_n() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| if std::env::var("OMR_JIT").map_or(false, |v| v == "0") { u32::MAX } else { std::env::var("OMR_JIT_N").ok().and_then(|x| x.parse().ok()).unwrap_or(2) })
}

/// The compiled form of `fb`, made on its `OMR_JIT_N`-th call; None while it is still cold (or the JIT is off).
#[inline]
pub fn code_for(fb: &FuncBody) -> Option<&Code> {
    if let Some(c) = fb.code.get() { return Some(c); }
    let n = fb.calls.get() + 1; fb.calls.set(n);
    if fb.resumable || (n < jit_n() && !(fb.has_loop && jit_n() != u32::MAX)) { return None; }
    Some(fb.code.get_or_init(|| Code(body(&fb.body))))
}

fn body(b: &Body) -> CE {
    match b {
        Body::Expr(e) => expr(e),
        Body::Block(b) => {
            let bp = P(b as *const Vec<Stmt>);
            let cs: Vec<CS> = b.iter().map(stmt).collect();
            Box::new(move |it, sc| {
                it.hoist_fns(bp.get(), sc);
                for s in &cs { match s(it, sc) { Ok(()) => {} Err(Ctl::Return(v)) => return Ok(v), Err(e) => return Err(e) } }
                Ok(Val::Undef)
            })
        }
    }
}

/// C4 — a per-site property cache (Hölzle, Chambers & Ungar 1991 inline caches; clean-room): the index of the key among
/// the receiver's OWN properties the last time this site read it. A hit needs the same key at that index and no
/// accessor on the object (`F_ACC`); own properties are searched first for every object kind (`Interp::get_obj`), keys
/// are unique in what the engine builds, additions append and removals keep order — so the key at index i is the
/// property. A site whose reads keep missing (prototype methods, changing objects) stops trying. OMR_ICCHECK=1
/// cross-checks every hit against the full [[Get]].
#[derive(Default)]
pub struct Ic { idx: std::cell::Cell<u32>, miss: std::cell::Cell<u8> }
const IC_NONE: u32 = u32::MAX;
fn iccheck_on() -> bool { static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new(); *ON.get_or_init(|| std::env::var_os("OMR_ICCHECK").is_some()) }
impl Ic {
    #[inline]
    pub fn get(&self, o: &Val, k: &str) -> Option<Val> {
        let i = self.idx.get(); if i == IC_NONE { return None; }
        let Val::Obj(r) = o else { return None };
        let b = r.borrow();
        if b.flags & crate::jsi::F_ACC != 0 { return None; }
        match b.props.get(i as usize) { Some((pk, v)) if &**pk == k => Some(v.clone()), _ => None }
    }
    /// after a full [[Get]]: remember where the key sits among the own properties (or count a miss)
    #[inline]
    pub fn fill(&self, o: &Val, k: &str) {
        if self.miss.get() >= 8 { self.idx.set(IC_NONE); return; }
        let Val::Obj(r) = o else { self.miss.set(self.miss.get() + 1); return };
        let b = r.borrow();
        match b.props.iter().position(|(pk, _)| &**pk == k) {
            Some(i) if b.flags & crate::jsi::F_ACC == 0 => self.idx.set(i as u32),
            _ => { self.idx.set(IC_NONE); self.miss.set(self.miss.get() + 1); }
        }
    }
}

fn fb_e(e: &Expr) -> CE { let p = P(e as *const Expr); Box::new(move |it, sc| it.ev(p.get(), sc)) }
fn fb_s(s: &Stmt) -> CS { let p = P(s as *const Stmt); Box::new(move |it, sc| it.ex(p.get(), sc)) }

/// the statement step count and limit `Interp::ex` keeps
#[inline(always)]
fn tick(it: &mut Interp) -> Result<(), Ctl> { it.steps += 1; if it.steps > it.limit { return Err(Ctl::Throw(it.error("Error", "step limit"))); } Ok(()) }

fn is_anon_fn(e: &Expr) -> bool { match e { Expr::Func(f) => f.name.is_empty(), Expr::Class(c) => c.name.is_empty(), _ => false } }

/// Number-only results of binary operators (the fast paths of `Interp::binop_t`); None = take the general path.
#[inline(always)]
pub fn num_bin(op: Bin, x: f64, y: f64) -> Option<Val> {
    use Bin::*;
    Some(match op {
        Add => Val::Num(x + y), Sub => Val::Num(x - y), Mul => Val::Num(x * y), Div => Val::Num(x / y),
        Mod => Val::Num(js_mod(x, y)),
        BitAnd => Val::Num((to_int32(x) & to_int32(y)) as f64), BitOr => Val::Num((to_int32(x) | to_int32(y)) as f64),
        BitXor => Val::Num((to_int32(x) ^ to_int32(y)) as f64),
        Shl => Val::Num(to_int32(x).wrapping_shl(to_int32(y) as u32 & 31) as f64),
        Shr => Val::Num((to_int32(x) >> (to_int32(y) as u32 & 31)) as f64),
        UShr => Val::Num(((to_int32(x) as u32) >> (to_int32(y) as u32 & 31)) as f64),
        Lt => Val::Bool(x < y), Gt => Val::Bool(x > y), Le => Val::Bool(x <= y), Ge => Val::Bool(x >= y),
        StrictEq | Eq => Val::Bool(x == y), StrictNe | Ne => Val::Bool(x != y),
        _ => return None,
    })
}

/// `x % y` (§6.1.6.1.6 Number::remainder). Integers below 2^53 take an exact integer remainder (the libm fmod call was
/// a visible share of loop time); the sign follows the dividend, so a zero result from a negative dividend is -0.
#[inline(always)]
pub fn js_mod(x: f64, y: f64) -> f64 {
    const LIM: f64 = 9007199254740992.0;
    if x.fract() == 0.0 && y.fract() == 0.0 && x.abs() < LIM && y.abs() < LIM && y != 0.0 {
        let r = (x as i64 % y as i64) as f64;
        return if r == 0.0 && x.is_sign_negative() { -0.0 } else { r };
    }
    if y.is_infinite() && x.is_finite() { x } else { x % y }
}

fn binary(op: Bin, a: &Expr, b: &Expr) -> CE {
    if matches!(op, Bin::In | Bin::InstanceOf | Bin::Pow) { let (ca, cb) = (expr(a), expr(b)); return Box::new(move |it, sc| { let x = ca(it, sc)?; let y = cb(it, sc)?; it.binop_t(op, x, y) }); }
    let ca = expr(a);
    if let Expr::Lit(Lit::Num(k)) = b {
        let k = *k;
        return Box::new(move |it, sc| { let x = ca(it, sc)?; if let Val::Num(x) = x { if let Some(r) = num_bin(op, x, k) { return Ok(r); } } it.binop_t(op, x, Val::Num(k)) });
    }
    let cb = expr(b);
    Box::new(move |it, sc| {
        let x = ca(it, sc)?; let y = cb(it, sc)?;
        if let (Val::Num(p), Val::Num(q)) = (&x, &y) { if let Some(r) = num_bin(op, *p, *q) { return Ok(r); } }
        it.binop_t(op, x, y)
    })
}

fn call(e: &Expr) -> CE {
    let Expr::Call { callee, args, spread, optional } = e else { return fb_e(e) };
    if *optional || spread.iter().any(|s| *s) { return fb_e(e); }
    let ca: Vec<CE> = args.iter().map(expr).collect();
    let n = ca.len();
    match &**callee {
        // a method call: `this` = the object (call_t's Member branch; outside an optional chain no `?.` sits below)
        Expr::Member { obj, key, optional: false } if !has_optional(obj) => {
            let Key::Static(k) = &**key else { return fb_e(e) };
            let (co, k, op) = (expr(obj), k.clone(), P(&**obj as *const Expr));
            Box::new(move |it, sc| {
                let o = co(it, sc)?;
                if o.is_nullish() { it.blame_call(op.get(), &k); }
                let f = it.get(&o, &k)?;
                if !f.is_callable() { return Err(it.not_a_method(op.get(), &k)); }
                let mut a = Vec::with_capacity(n); for c in &ca { a.push(c(it, sc)?); }
                it.call(&f, o, a)
            })
        }
        Expr::Member { .. } | Expr::SuperMember(_) => fb_e(e),
        Expr::Ident(name) if &**name == "eval" => fb_e(e),
        other => {
            let (cf, name) = (expr(other), match other { Expr::Ident(n) | Expr::Slot(_, _, n) => n.to_string(), _ => "expression".into() });
            Box::new(move |it, sc| {
                let f = cf(it, sc)?;
                if !f.is_callable() { return Err(Ctl::Throw(it.error("TypeError", &format!("{name} is not a function")))); }
                let mut a = Vec::with_capacity(n); for c in &ca { a.push(c(it, sc)?); }
                it.call(&f, Val::Undef, a)
            })
        }
    }
}
fn has_optional(e: &Expr) -> bool {
    match e { Expr::Member { obj, optional, .. } => *optional || has_optional(obj), Expr::Call { callee, optional, .. } => *optional || has_optional(callee), _ => false }
}

pub fn expr(e: &Expr) -> CE {
    match e {
        Expr::Lit(Lit::Num(n)) => { let n = *n; Box::new(move |_, _| Ok(Val::Num(n))) }
        Expr::Lit(Lit::Bool(b)) => { let b = *b; Box::new(move |_, _| Ok(Val::Bool(b))) }
        Expr::Lit(Lit::Undef) => Box::new(|_, _| Ok(Val::Undef)),
        Expr::Lit(Lit::Null) => Box::new(|_, _| Ok(Val::Null)),
        Expr::Lit(Lit::Str(s)) => { let s = s.clone(); Box::new(move |_, _| Ok(Val::Str(s.clone()))) }
        Expr::Slot(d, i, n) => { let (d, i, n) = (*d, *i, n.clone()); Box::new(move |it, sc| it.slot_get(sc, d, i, &n)) }
        Expr::Binary(op, a, b) => binary(*op, a, b),
        Expr::Logical(op, a, b) => {
            let (ca, cb) = (expr(a), expr(b));
            match op {
                Logic::And => Box::new(move |it, sc| { let x = ca(it, sc)?; if x.truthy() { cb(it, sc) } else { Ok(x) } }),
                Logic::Or => Box::new(move |it, sc| { let x = ca(it, sc)?; if x.truthy() { Ok(x) } else { cb(it, sc) } }),
                Logic::Nullish => Box::new(move |it, sc| { let x = ca(it, sc)?; if x.is_nullish() { cb(it, sc) } else { Ok(x) } }),
            }
        }
        Expr::Cond(t, c, a) => { let (ct, cc, ca) = (expr(t), expr(c), expr(a)); Box::new(move |it, sc| if ct(it, sc)?.truthy() { cc(it, sc) } else { ca(it, sc) }) }
        Expr::Unary(Un::Not, a) => { let ca = expr(a); Box::new(move |it, sc| Ok(Val::Bool(!ca(it, sc)?.truthy()))) }
        Expr::Unary(op, a) if !matches!(op, Un::Delete) => { let (ca, op) = (expr(a), *op); Box::new(move |it, sc| { let v = ca(it, sc)?; it.unary(op, v) }) }
        Expr::Seq(v) => { let cs: Vec<CE> = v.iter().map(expr).collect(); Box::new(move |it, sc| { let mut last = Val::Undef; for c in &cs { last = c(it, sc)?; } Ok(last) }) }
        Expr::Member { obj, key, .. } => {
            let (co, op) = (expr(obj), P(&**obj as *const Expr));
            match &**key {
                Key::Static(k) => {
                    let (k, ic) = (k.clone(), Ic::default());
                    Box::new(move |it, sc| {
                        let o = co(it, sc)?;
                        if let Some(v) = ic.get(&o, &k) {
                            if iccheck_on() { let w = it.get(&o, &k)?; if !Interp::same_value_zero(&v, &w) { panic!("ICCHECK mismatch: .{k} cache gave {} [[Get]] gave {}", v.type_of(), w.type_of()); } }
                            return Ok(v);
                        }
                        if o.is_nullish() { it.blame_read(op.get(), &k); }
                        let v = it.get(&o, &k)?; ic.fill(&o, &k); Ok(v)
                    })
                }
                Key::Computed(ke) => {
                    let ck = expr(ke);
                    Box::new(move |it, sc| {
                        let o = co(it, sc)?;
                        if o.is_nullish() { it.blame_read(op.get(), "[computed]"); }
                        let k = ck(it, sc)?;
                        if o.is_nullish() { let ks = it.to_key(&k).unwrap_or_else(|_| Rc::from("?")); return it.get(&o, &ks); }
                        it.get_v(&o, &k)
                    })
                }
            }
        }
        Expr::Call { .. } => call(e),
        Expr::Update { inc, prefix, target } => match &**target {
            Expr::Slot(d, i, n) => {
                let (d, i, n, inc, prefix) = (*d, *i, n.clone(), *inc, *prefix);
                Box::new(move |it, sc| {
                    let old = it.slot_get(sc, d, i, &n)?;
                    let (old, nv) = if let Val::Num(o) = old { (Val::Num(o), Val::Num(if inc { o + 1.0 } else { o - 1.0 })) } else { it.upd(old, inc)? };
                    it.slot_set(sc, d, i, &n, nv.clone())?;
                    Ok(if prefix { nv } else { old })
                })
            }
            _ => fb_e(e),
        },
        Expr::Assign(op, target, rhs) => match (&**target, op) {
            (Target::Slot(d, i, n), AssignOp::Set) => {
                let (d, i, n, cr, named) = (*d, *i, n.clone(), expr(rhs), matches!(&**rhs, Expr::Func(_) | Expr::Class(_)));
                Box::new(move |it, sc| { let v = cr(it, sc)?; if named { it.name_fn(&v, &n); } it.slot_set(sc, d, i, &n, v.clone())?; Ok(v) })
            }
            (Target::Slot(d, i, n), AssignOp::Bin(b)) => {
                let (d, i, n, cr, b) = (*d, *i, n.clone(), expr(rhs), *b);
                Box::new(move |it, sc| {
                    let cur = it.slot_get(sc, d, i, &n)?; let r = cr(it, sc)?;
                    let v = match (&cur, &r) { (Val::Num(x), Val::Num(y)) => match num_bin(b, *x, *y) { Some(v) => v, None => it.binop_t(b, cur, r)? }, _ => it.binop_t(b, cur, r)? };
                    it.slot_set(sc, d, i, &n, v.clone())?; Ok(v)
                })
            }
            (Target::Member(o, Key::Static(k)), AssignOp::Set) => {
                let (co, kv, cr) = (expr(o), Val::Str(k.clone()), expr(rhs));
                Box::new(move |it, sc| { let ov = co(it, sc)?; let v = cr(it, sc)?; it.set_v(&ov, &kv, v.clone())?; Ok(v) })
            }
            (Target::Member(o, Key::Static(k)), AssignOp::Bin(b)) => {
                let (co, kv, cr, b) = (expr(o), Val::Str(k.clone()), expr(rhs), *b);
                Box::new(move |it, sc| {
                    let ov = co(it, sc)?; let cur = it.get_v(&ov, &kv)?; let r = cr(it, sc)?;
                    let v = match (&cur, &r) { (Val::Num(x), Val::Num(y)) => match num_bin(b, *x, *y) { Some(v) => v, None => it.binop_t(b, cur, r)? }, _ => it.binop_t(b, cur, r)? };
                    it.set_v(&ov, &kv, v.clone())?; Ok(v)
                })
            }
            (Target::Member(o, Key::Computed(ke)), AssignOp::Set) => {
                let (co, ck, cr) = (expr(o), expr(ke), expr(rhs));
                Box::new(move |it, sc| { let ov = co(it, sc)?; let kv = ck(it, sc)?; let v = cr(it, sc)?; it.set_v(&ov, &kv, v.clone())?; Ok(v) })
            }
            _ => fb_e(e),
        },
        _ => fb_e(e),
    }
}

/// the statements of a list, in order
fn seq(b: &[Stmt]) -> Vec<CS> { b.iter().map(stmt).collect() }

pub fn stmt(s: &Stmt) -> CS {
    match s {
        Stmt::Expr(e) => { let c = expr(e); Box::new(move |it, sc| { tick(it)?; c(it, sc)?; Ok(()) }) }
        Stmt::Return(e) => match e {
            Some(e) => { let c = expr(e); Box::new(move |it, sc| { tick(it)?; Err(Ctl::Return(c(it, sc)?)) }) }
            None => Box::new(|it, _| { tick(it)?; Err(Ctl::Return(Val::Undef)) }),
        },
        Stmt::If(t, c, a) => {
            let (ct, cc, ca) = (expr(t), stmt(c), a.as_ref().map(|a| stmt(a)));
            Box::new(move |it, sc| { tick(it)?; if ct(it, sc)?.truthy() { cc(it, sc) } else if let Some(a) = &ca { a(it, sc) } else { Ok(()) } })
        }
        Stmt::Block(b, false, _) => { let cs = seq(b); Box::new(move |it, sc| { tick(it)?; for c in &cs { c(it, sc)?; } Ok(()) }) }
        Stmt::Block(b, true, lay) => {
            let (cs, bp, lay) = (seq(b), P(b as *const Vec<Stmt>), lay.clone());
            Box::new(move |it, sc| { tick(it)?; let s2 = Interp::child_l(sc, &lay); it.hoist_fns(bp.get(), &s2); for c in &cs { c(it, &s2)?; } Ok(()) })
        }
        Stmt::Var(kind, decls) if decls.iter().all(|(p, i)| matches!(p, Pat::Ident(_)) && i.as_ref().map_or(true, |i| !is_anon_fn(i))) => {
            let ds: Vec<(Rc<str>, Option<CE>)> = decls.iter().map(|(p, i)| (match p { Pat::Ident(n) => n.clone(), _ => unreachable!() }, i.as_ref().map(expr))).collect();
            if matches!(kind, VarKind::Var) {
                Box::new(move |it, sc| { tick(it)?; for (n, i) in &ds { if let Some(i) = i { let v = i(it, sc)?; it.assign_var(sc, n, v)?; } } Ok(()) })
            } else {
                Box::new(move |it, sc| { tick(it)?; for (n, i) in &ds { let v = match i { Some(i) => i(it, sc)?, None => Val::Undef }; sc.borrow_mut().put(n, v); } Ok(()) })
            }
        }
        Stmt::For { .. } | Stmt::While(..) | Stmt::DoWhile(..) => { let l = lp(s, Rc::from(vec![])); Box::new(move |it, sc| { tick(it)?; l(it, sc) }) }
        Stmt::ForOf { is_await: false, .. } => { let l = lp(s, Rc::from(vec![])); Box::new(move |it, sc| { tick(it)?; l(it, sc) }) }
        Stmt::Labeled(..) => {
            // LabelledEvaluation (§14.13): the label set reaches the loop; a break to one of the labels ends here
            let mut labels = Vec::new(); let mut body = s;
            while let Stmt::Labeled(l, b) = body { labels.push(l.clone()); body = b; }
            let labels: Rc<[Rc<str>]> = Rc::from(labels);
            let inner: CS = match body { Stmt::For { .. } | Stmt::While(..) | Stmt::DoWhile(..) | Stmt::ForOf { is_await: false, .. } => lp(body, labels.clone()), _ => stmt(body) };
            Box::new(move |it, sc| { tick(it)?; match inner(it, sc) { Err(Ctl::Break(Some(x))) if labels.contains(&x) => Ok(()), r => r } })
        }
        Stmt::Throw(e) => { let c = expr(e); Box::new(move |it, sc| { tick(it)?; let v = c(it, sc)?; Err(Ctl::Throw(v)) }) }
        Stmt::Break(l) => { let l = l.clone(); Box::new(move |it, _| { tick(it)?; Err(Ctl::Break(l.clone())) }) }
        Stmt::Continue(l) => { let l = l.clone(); Box::new(move |it, _| { tick(it)?; Err(Ctl::Continue(l.clone())) }) }
        Stmt::Empty | Stmt::Func(..) => Box::new(|it, _| tick(it)),
        Stmt::Try { block, param, handler, finalizer, lays } => {
            let (cb, bp, l0) = (seq(block), P(block as *const Vec<Stmt>), lays[0].clone());
            let ch = handler.as_ref().map(|h| (seq(h), P(h as *const Vec<Stmt>), lays[1].clone(), param.as_ref().map(|p| P(p as *const Pat))));
            let cf = finalizer.as_ref().map(|f| (seq(f), P(f as *const Vec<Stmt>), lays[2].clone()));
            Box::new(move |it, sc| {
                tick(it)?;
                let s0 = Interp::child_l(sc, &l0); it.hoist_fns(bp.get(), &s0);
                let r = run(&cb, it, &s0);
                let r = match (r, &ch) {
                    (Err(Ctl::Throw(v)), Some((hc, hp, l1, pp))) => {
                        let hs = Interp::child_l(sc, l1);
                        if let Some(pp) = pp { it.bind_pat(pp.get(), v, &hs)?; }
                        it.hoist_fns(hp.get(), &hs); run(hc, it, &hs)
                    }
                    (r, _) => r,
                };
                if let Some((fc, fp, l2)) = &cf { let fs = Interp::child_l(sc, l2); it.hoist_fns(fp.get(), &fs); run(fc, it, &fs)?; }
                r
            })
        }
        Stmt::Switch(d, cases, lay) => {
            let (cd, lay) = (expr(d), lay.clone());
            let cs: Vec<(Option<CE>, Vec<CS>, P<Vec<Stmt>>)> = cases.iter().map(|(t, b)| (t.as_ref().map(expr), seq(b), P(b as *const Vec<Stmt>))).collect();
            let default = cases.iter().position(|c| c.0.is_none());
            Box::new(move |it, sc| {
                tick(it)?;
                let dv = cd(it, sc)?;
                let s2 = Interp::child_l(sc, &lay);
                for (_, _, bp) in &cs { it.hoist_fns(bp.get(), &s2); }
                let mut start = None;
                for (i, (t, _, _)) in cs.iter().enumerate() { if let Some(t) = t { let tv = t(it, &s2)?; if Interp::strict_eq(&dv, &tv) { start = Some(i); break; } } }
                if start.is_none() { start = default; }
                if let Some(i0) = start { for (_, b, _) in &cs[i0..] { match run(b, it, &s2) { Ok(()) => {} Err(Ctl::Break(None)) => break, Err(e) => return Err(e) } } }
                Ok(())
            })
        }
        _ => fb_s(s),
    }
}

fn run(cs: &[CS], it: &mut Interp, sc: &Scope) -> Result<(), Ctl> { for c in cs { c(it, sc)?; } Ok(()) }
#[inline] fn brk(e: &Ctl, labels: &[Rc<str>]) -> bool { match e { Ctl::Break(None) => true, Ctl::Break(Some(l)) => labels.iter().any(|x| x == l), _ => false } }
#[inline] fn cont(e: &Ctl, labels: &[Rc<str>]) -> bool { match e { Ctl::Continue(None) => true, Ctl::Continue(Some(l)) => labels.iter().any(|x| x == l), _ => false } }

/// A loop with its label set (Interp::ex_loop; the caller has already counted the loop statement's own step).
fn lp(s: &Stmt, labels: Rc<[Rc<str>]>) -> CS {
    match s {
        Stmt::For { init, test, update, body, per_iter, lay } => {
            let (ci, ct, cu, cb, lay, per_iter) = (init.as_ref().map(|i| stmt(i)), test.as_ref().map(expr), update.as_ref().map(expr), stmt(body), lay.clone(), *per_iter);
            Box::new(move |it, sc| {
                let s2 = Interp::child_l(sc, &lay);
                if let Some(i) = &ci { i(it, &s2)?; }
                loop {
                    if let Some(t) = &ct { if !t(it, &s2)?.truthy() { break; } }
                    if per_iter {
                        // closures in the body capture this iteration's bindings (§14.7.4.4 CreatePerIterationEnvironment)
                        let i2 = Interp::child(&s2);
                        { let b = s2.borrow(); i2.borrow_mut().copy_bindings(&b); }
                        match cb(it, &i2) { Ok(()) => {} Err(e) if cont(&e, &labels) => {} Err(e) if brk(&e, &labels) => break, Err(e) => return Err(e) }
                        { let b = i2.borrow(); s2.borrow_mut().copy_bindings(&b); }
                    } else {
                        match cb(it, &s2) { Ok(()) => {} Err(e) if cont(&e, &labels) => {} Err(e) if brk(&e, &labels) => break, Err(e) => return Err(e) }
                    }
                    if let Some(u) = &cu { u(it, &s2)?; }
                    tick(it)?;
                }
                Ok(())
            })
        }
        Stmt::While(t, b) => {
            let (ct, cb) = (expr(t), stmt(b));
            Box::new(move |it, sc| {
                while ct(it, sc)?.truthy() {
                    match cb(it, sc) { Ok(()) => {} Err(e) if cont(&e, &labels) => {} Err(e) if brk(&e, &labels) => break, Err(e) => return Err(e) }
                    tick(it)?;
                }
                Ok(())
            })
        }
        Stmt::DoWhile(b, t) => {
            let (ct, cb) = (expr(t), stmt(b));
            Box::new(move |it, sc| {
                loop {
                    match cb(it, sc) { Ok(()) => {} Err(e) if cont(&e, &labels) => {} Err(e) if brk(&e, &labels) => break, Err(e) => return Err(e) }
                    if !ct(it, sc)?.truthy() { break; }
                    tick(it)?;
                }
                Ok(())
            })
        }
        Stmt::ForOf { decl, pat, right, body, of, is_await: false, lay } => {
            let (cr, cb, pp, lay, decl, of) = (expr(right), stmt(body), P(pat as *const Pat), lay.clone(), *decl, *of);
            Box::new(move |it, sc| {
                let src = cr(it, sc)?;
                if !of {
                    if src.is_nullish() { return Ok(()); }
                    for k in it.for_in_keys(&src) {
                        let i2 = Interp::child_l(sc, &lay);
                        if decl == 2 { it.bind_pat(pp.get(), Val::Str(k), &i2)?; } else { it.assign_pat(pp.get(), Val::Str(k), sc)?; }
                        match cb(it, &i2) { Ok(()) => {} Err(e) if cont(&e, &labels) => {} Err(e) if brk(&e, &labels) => break, Err(e) => return Err(e) }
                    }
                    return Ok(());
                }
                let mut iter = it.iter_open(&src)?;
                loop {
                    let x = match it.iter_step(&mut iter)? { Some(x) => x, None => break };
                    let i2 = Interp::child_l(sc, &lay);
                    if decl == 2 { it.bind_pat(pp.get(), x, &i2)?; } else { it.assign_pat(pp.get(), x, sc)?; }
                    match cb(it, &i2) {
                        Ok(()) => {} Err(e) if cont(&e, &labels) => {}
                        Err(e) if brk(&e, &labels) => { it.iter_close(&iter)?; break; }
                        Err(Ctl::Throw(v)) => { let _ = it.iter_close(&iter); return Err(Ctl::Throw(v)); }
                        Err(e) => { it.iter_close(&iter)?; return Err(e); }
                    }
                    tick(it)?;
                }
                Ok(())
            })
        }
        other => stmt(other),
    }
}
