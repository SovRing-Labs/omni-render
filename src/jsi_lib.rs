// SPDX-License-Identifier: MIT OR Apache-2.0
//! jsi standard library: the built-in constructors, prototypes and globals, written to the ECMAScript rules
//! (toFixed rounds exact ties up, Math.round is floor(x + 0.5), parseInt reads the longest valid prefix, sort is stable,
//! JSON keeps key order) and, for the web globals (URL, TextEncoder, atob, structuredClone), the WHATWG / HTML rules.
//! Each rule is cemented against node through Omniglyph laws (language `omniir`).
use crate::jsi::{is_new, len16, num_str, st, str_num, Ctl, Interp, Kind, MapData, ORef, Val, A_NOCONF, A_NOENUM, A_NOWRITE, F_FROZEN, F_NOEXT, F_SEALED, IterSt, TArr, Resume, K_ITER, K_ASYNC_ITER, K_TAG, K_TO_PRIM, K_HAS_INSTANCE, WELL_KNOWN};
use std::cmp::Ordering;
use std::rc::Rc;

macro_rules! nat { ($name:expr, |$it:ident, $this:ident, $args:ident| $body:expr) => { Val::native($name, move |$it: &mut Interp, $this: Val, $args: Vec<Val>| -> Result<Val, Ctl> { $body }) }; }
fn arg(a: &[Val], i: usize) -> Val { a.get(i).cloned().unwrap_or(Val::Undef) }
fn throw(it: &Interp, name: &str, msg: &str) -> Ctl { Ctl::Throw(it.error(name, msg)) }

fn this_rc(it: &mut Interp, this: &Val) -> Result<Rc<str>, Ctl> { match this { Val::Str(s) => Ok(s.clone()), Val::Undef | Val::Null => Err(throw(it, "TypeError", "String.prototype method called on null or undefined")), v => it.to_str(v) } }
/// A string's UTF-16 view for the index-based methods: (units or None for ASCII, length in units).
struct U { s: Rc<str>, u: Option<Rc<Vec<u16>>> }
impl U {
    fn of(it: &mut Interp, this: &Val) -> Result<U, Ctl> { let s = this_rc(it, this)?; let u = crate::jsi::units(&s); Ok(U { s, u }) }
    fn len(&self) -> usize { self.u.as_ref().map_or(self.s.len(), |v| v.len()) }
    fn at(&self, i: usize) -> u16 { match &self.u { None => self.s.as_bytes()[i] as u16, Some(v) => v[i] } }
    fn sub(&self, b: usize, e: usize) -> Val { if b >= e { return st(""); } match &self.u { None => st(&self.s[b..e]), Some(v) => st(&from16(&v[b..e])) } }
    fn units(&self) -> std::borrow::Cow<'_, [u16]> { match &self.u { None => std::borrow::Cow::Owned(self.s.bytes().map(|b| b as u16).collect()), Some(v) => std::borrow::Cow::Borrowed(&v[..]) } }
    /// first index ≥ from where `n` occurs
    fn find(&self, n: &str, from: usize) -> Option<usize> {
        if self.u.is_none() && n.is_ascii() { return if from > self.s.len() { None } else { self.s[from..].find(n).map(|i| i + from) }; }
        let h = self.units(); let n = utf16(n);
        if n.len() > h.len() { return None; }
        (from.min(h.len())..=h.len() - n.len()).find(|&i| h[i..i + n.len()] == n[..])
    }
}
fn this_str(it: &mut Interp, this: &Val) -> Result<String, Ctl> { if this.is_nullish() { return Err(throw(it, "TypeError", "String.prototype method called on null or undefined")); } Ok(it.to_str(this)?.to_string()) }
fn utf16(x: &str) -> Vec<u16> { crate::jsi::utf16_of(x) }
fn from16(v: &[u16]) -> String { crate::jsi::str_of16(v) }
fn rel_index(n: f64, len: usize) -> usize { let len = len as f64; let n = if n.is_nan() { 0.0 } else { n.trunc() }; (if n < 0.0 { (len + n).max(0.0) } else { n.min(len) }) as usize }
fn rel_arg(it: &mut Interp, a: &[Val], i: usize, len: usize, dflt: usize) -> Result<usize, Ctl> { match a.get(i) { None | Some(Val::Undef) => Ok(dflt), Some(v) => Ok(rel_index(it.to_num(v)?, len)) } }
fn obj_of(v: &Val) -> Option<&ORef> { v.as_obj() }
fn def(o: &ORef, k: &str, v: Val) { Interp::define(o, k, v, A_NOENUM); }

fn arr_of(this: &Val) -> Vec<Val> { if let Val::Obj(o) = this { if let Kind::Array(a) = &o.borrow().kind { return a.clone(); } } Vec::new() }
fn with_arr<R>(this: &Val, f: impl FnOnce(&mut Vec<Val>) -> R) -> Option<R> { if let Val::Obj(o) = this { if let Kind::Array(a) = &mut o.borrow_mut().kind { return Some(f(a)); } } None }
/// The elements of an array, typed array, string or array-like (§7.3.18 LengthOfArrayLike + Get per index).
pub fn list_of(it: &mut Interp, this: &Val) -> Result<Vec<Val>, Ctl> {
    match this {
        Val::Obj(o) => { let b = o.borrow(); match &b.kind { Kind::Array(a) => return Ok(a.clone()), Kind::Typed(t) => return Ok((0..t.len).map(|i| typed_get(t, i)).collect()), _ => {} } }
        Val::Str(s) => return Ok(utf16(s).iter().map(|c| st(&from16(&[*c]))).collect()),
        Val::Undef | Val::Null => return Err(throw(it, "TypeError", "Array.prototype method called on null or undefined")),
        _ => return Ok(vec![]),
    }
    let n = it.get(this, "length")?;
    let n = it.to_num(&n)?;
    let n = if n.is_nan() || n <= 0.0 { 0 } else { n.min(1e8) as usize };
    let mut v = Vec::with_capacity(n.min(1 << 16));
    for i in 0..n { v.push(it.get(this, &i.to_string())?); }
    Ok(v)
}
fn frozen_arr(it: &Interp, this: &Val) -> Result<(), Ctl> { if let Val::Obj(o) = this { if o.borrow().flags & (F_FROZEN | F_SEALED | F_NOEXT) != 0 { return Err(throw(it, "TypeError", "Cannot add property, object is not extensible")); } } Ok(()) }

// ── BigInt (ECMA-262 §6.1.6.2): sign + magnitude, base 2^32 little-endian, no leading zero limbs ──
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct Big { pub neg: bool, pub mag: Vec<u32> }
impl Big {
    fn norm(mut self) -> Big { while self.mag.last() == Some(&0) { self.mag.pop(); } if self.mag.is_empty() { self.neg = false; } self }
    pub fn from_u128(mut u: u128, neg: bool) -> Big { let mut mag = Vec::new(); while u > 0 { mag.push(u as u32); u >>= 32; } Big { neg, mag }.norm() }
    pub fn from_i64(v: i64) -> Big { Big::from_u128(v.unsigned_abs() as u128, v < 0) }
    /// NumberToBigInt (§21.2.1.1.1) for an integral double, exactly.
    pub fn from_f64(x: f64) -> Option<Big> {
        if !x.is_finite() || x.fract() != 0.0 { return None; }
        let bits = x.to_bits(); let exp = ((bits >> 52) & 0x7ff) as i64;
        let man = if exp == 0 { (bits & 0xf_ffff_ffff_ffff) << 1 } else { (bits & 0xf_ffff_ffff_ffff) | (1 << 52) };
        let e = exp - 1075;
        let b = Big::from_u128(man as u128, false);
        let b = if e >= 0 { b.shl_mag(e as usize) } else { b.shr_mag((-e) as usize) };
        Some(Big { neg: x < 0.0, mag: b.mag }.norm())
    }
    pub fn is_zero(&self) -> bool { self.mag.is_empty() }
    pub fn neg(&self) -> Big { Big { neg: !self.neg, mag: self.mag.clone() }.norm() }
    fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering { if a.len() != b.len() { return a.len().cmp(&b.len()); } for i in (0..a.len()).rev() { if a[i] != b[i] { return a[i].cmp(&b[i]); } } Ordering::Equal }
    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> { let mut r = Vec::with_capacity(a.len().max(b.len()) + 1); let mut c = 0u64; for i in 0..a.len().max(b.len()) { let s = *a.get(i).unwrap_or(&0) as u64 + *b.get(i).unwrap_or(&0) as u64 + c; r.push(s as u32); c = s >> 32; } if c > 0 { r.push(c as u32); } r }
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> { let mut r = Vec::with_capacity(a.len()); let mut br = 0i64; for i in 0..a.len() { let mut d = a[i] as i64 - *b.get(i).unwrap_or(&0) as i64 - br; if d < 0 { d += 1 << 32; br = 1; } else { br = 0; } r.push(d as u32); } r }
    pub fn add(&self, o: &Big) -> Big {
        if self.neg == o.neg { return Big { neg: self.neg, mag: Self::add_mag(&self.mag, &o.mag) }.norm(); }
        match Self::cmp_mag(&self.mag, &o.mag) { Ordering::Less => Big { neg: o.neg, mag: Self::sub_mag(&o.mag, &self.mag) }.norm(), _ => Big { neg: self.neg, mag: Self::sub_mag(&self.mag, &o.mag) }.norm() }
    }
    pub fn sub(&self, o: &Big) -> Big { self.add(&o.neg()) }
    pub fn mul(&self, o: &Big) -> Big {
        if self.is_zero() || o.is_zero() { return Big::default(); }
        let mut r = vec![0u32; self.mag.len() + o.mag.len()];
        for (i, &x) in self.mag.iter().enumerate() { let mut c = 0u64; for (j, &y) in o.mag.iter().enumerate() { let t = r[i + j] as u64 + x as u64 * y as u64 + c; r[i + j] = t as u32; c = t >> 32; } let mut k = i + o.mag.len(); while c > 0 { let t = r[k] as u64 + c; r[k] = t as u32; c = t >> 32; k += 1; } }
        Big { neg: self.neg != o.neg, mag: r }.norm()
    }
    fn divrem_small(mag: &[u32], d: u32) -> (Vec<u32>, u32) { let mut q = vec![0u32; mag.len()]; let mut r = 0u64; for i in (0..mag.len()).rev() { let cur = (r << 32) | mag[i] as u64; q[i] = (cur / d as u64) as u32; r = cur % d as u64; } (q, r as u32) }
    fn bits(&self) -> usize { match self.mag.last() { None => 0, Some(t) => (self.mag.len() - 1) * 32 + (32 - t.leading_zeros() as usize) } }
    fn bit(&self, i: usize) -> bool { self.mag.get(i / 32).map_or(false, |w| (w >> (i % 32)) & 1 == 1) }
    pub fn shl_mag(&self, n: usize) -> Big { if self.is_zero() { return self.clone(); } let (w, b) = (n / 32, n % 32); let mut r = vec![0u32; w]; let mut c = 0u32; for &x in &self.mag { r.push(if b == 0 { x } else { (x << b) | c }); c = if b == 0 { 0 } else { x >> (32 - b) }; } if c > 0 { r.push(c); } Big { neg: self.neg, mag: r }.norm() }
    pub fn shr_mag(&self, n: usize) -> Big { let (w, b) = (n / 32, n % 32); if w >= self.mag.len() { return Big::default(); } let m = &self.mag[w..]; let mut r = Vec::with_capacity(m.len()); for i in 0..m.len() { let lo = m[i] >> b; let hi = if b == 0 { 0 } else { m.get(i + 1).map_or(0, |x| x << (32 - b)) }; r.push(lo | hi); } Big { neg: self.neg, mag: r }.norm() }
    /// Truncating division (§6.1.6.2.5–6): quotient rounds toward zero, the remainder takes the dividend's sign.
    pub fn divrem(&self, o: &Big) -> Option<(Big, Big)> {
        if o.is_zero() { return None; }
        if Self::cmp_mag(&self.mag, &o.mag) == Ordering::Less { return Some((Big::default(), self.clone())); }
        let (q, r) = if o.mag.len() == 1 { let (q, r) = Self::divrem_small(&self.mag, o.mag[0]); (q, vec![r]) } else {
            // binary long division on magnitudes
            let a = Big { neg: false, mag: self.mag.clone() }; let d = Big { neg: false, mag: o.mag.clone() };
            let mut q = vec![0u32; self.mag.len()]; let mut r = Big::default();
            for i in (0..a.bits()).rev() { r = r.shl_mag(1); if a.bit(i) { r = r.add(&Big::from_i64(1)); } if Self::cmp_mag(&r.mag, &d.mag) != Ordering::Less { r = Big { neg: false, mag: Self::sub_mag(&r.mag, &d.mag) }.norm(); q[i / 32] |= 1 << (i % 32); } }
            (q, r.mag)
        };
        Some((Big { neg: self.neg != o.neg, mag: q }.norm(), Big { neg: self.neg, mag: r }.norm()))
    }
    pub fn pow(&self, mut e: u64) -> Big { let mut base = self.clone(); let mut r = Big::from_i64(1); while e > 0 { if e & 1 == 1 { r = r.mul(&base); } e >>= 1; if e > 0 { base = base.mul(&base); } } r }
    pub fn cmp(&self, o: &Big) -> Ordering {
        match (self.neg, o.neg) { (false, true) => Ordering::Greater, (true, false) => Ordering::Less, (false, false) => Self::cmp_mag(&self.mag, &o.mag), (true, true) => Self::cmp_mag(&o.mag, &self.mag) }
    }
    pub fn to_f64(&self) -> f64 { let mut x = 0f64; for &w in self.mag.iter().rev() { x = x * 4294967296.0 + w as f64; } if self.neg { -x } else { x } }
    pub fn to_i128_wrapping(&self) -> i128 { let mut u: u128 = 0; for (i, &w) in self.mag.iter().enumerate().take(4) { u |= (w as u128) << (32 * i); } let v = u as i128; if self.neg { v.wrapping_neg() } else { v } }
    pub fn to_string_radix(&self, r: u32) -> String {
        if self.is_zero() { return "0".into(); }
        let mut digits = Vec::new(); let mut m = self.mag.clone();
        while !m.is_empty() { let (q, rem) = Self::divrem_small(&m, r); digits.push(std::char::from_digit(rem, r).unwrap()); m = q; while m.last() == Some(&0) { m.pop(); } }
        if self.neg { digits.push('-'); }
        digits.iter().rev().collect()
    }
    /// StringToBigInt (§7.1.14): whitespace-trimmed decimal with sign, or 0x / 0o / 0b without one; "" is 0n.
    pub fn parse(s: &str) -> Option<Big> {
        let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if t.is_empty() { return Some(Big::default()); }
        let (radix, body, neg) = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { (16, h, false) } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) { (8, o, false) } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) { (2, b, false) }
            else if let Some(r) = t.strip_prefix('-') { (10, r, true) } else { (10, t.strip_prefix('+').unwrap_or(t), false) };
        if body.is_empty() || !body.chars().all(|c| c.is_digit(radix)) { return None; }
        let mut v = Big::default(); let rb = Big::from_i64(radix as i64);
        for c in body.chars() { v = v.mul(&rb).add(&Big::from_i64(c.to_digit(radix).unwrap() as i64)); }
        Some(Big { neg, mag: v.mag }.norm())
    }
    /// Two's-complement limbs (sign-extended to n limbs) for the bitwise operators (§6.1.6.2.17–20).
    fn twos(&self, n: usize) -> Vec<u32> { let mut v = self.mag.clone(); v.resize(n, 0); if self.neg { for w in v.iter_mut() { *w = !*w; } let mut i = 0; while i < n { let (s, o) = v[i].overflowing_add(1); v[i] = s; if !o { break; } i += 1; } } v }
    fn from_twos(mut v: Vec<u32>) -> Big { let neg = v.last().map_or(false, |w| w >> 31 == 1); if neg { for w in v.iter_mut() { *w = !*w; } let mut i = 0; while i < v.len() { let (s, o) = v[i].overflowing_add(1); v[i] = s; if !o { break; } i += 1; } } Big { neg, mag: v }.norm() }
    pub fn bitop(&self, o: &Big, op: char) -> Big { let n = self.mag.len().max(o.mag.len()) + 1; let (a, b) = (self.twos(n), o.twos(n)); Self::from_twos(a.iter().zip(&b).map(|(x, y)| match op { '&' => x & y, '|' => x | y, _ => x ^ y }).collect()) }
    /// BigInt.asIntN / asUintN (§21.2.2.1–2).
    pub fn as_n(&self, bits: usize, signed: bool) -> Big {
        if bits == 0 { return Big::default(); }
        let n = bits / 32 + 2;
        let mut v = self.twos(n); let (w, b) = (bits / 32, bits % 32);
        for (i, x) in v.iter_mut().enumerate() { if i > w || (i == w && b == 0) { *x = 0; } else if i == w { *x &= (1u32 << b) - 1; } }
        let r = Big { neg: false, mag: v }.norm();
        if signed && r.bit(bits - 1) { r.sub(&Big::from_i64(1).shl_mag(bits)) } else { r }
    }
}
impl std::fmt::Display for Big { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.to_string_radix(10)) } }

/// BigInt arithmetic (§6.1.6.2): both operands BigInt.
pub fn big_op(it: &mut Interp, op: &str, p: &Big, q: &Big) -> Result<Val, Ctl> {
    let r = match op {
        "-" => p.sub(q), "*" => p.mul(q),
        "/" => match p.divrem(q) { Some((d, _)) => d, None => return Err(throw(it, "RangeError", "Division by zero")) },
        "%" => match p.divrem(q) { Some((_, r)) => r, None => return Err(throw(it, "RangeError", "Division by zero")) },
        "**" => { if q.neg { return Err(throw(it, "RangeError", "Exponent must be non-negative")); } let e = q.to_f64(); if e * (p.bits().max(1) as f64) > 1e7 { return Err(throw(it, "RangeError", "Maximum BigInt size exceeded")); } p.pow(e as u64) }
        "&" | "|" | "^" => p.bitop(q, op.chars().next().unwrap()),
        "<<" | ">>" => {
            let n = q.to_f64(); let left = (op == "<<") == (n >= 0.0); let n = n.abs();
            if n > 1e7 { return Err(throw(it, "RangeError", "Maximum BigInt size exceeded")); }
            if left { p.shl_mag(n as usize) } else if !p.neg { p.shr_mag(n as usize) } else { Big { neg: false, mag: p.mag.clone() }.sub(&Big::from_i64(1)).shr_mag(n as usize).add(&Big::from_i64(1)).neg() }
        }
        _ => return Err(throw(it, "TypeError", "BigInts have no unsigned right shift, use >> instead")),
    };
    Ok(Val::Big(Rc::new(r)))
}
/// IsLessThan with a BigInt on either side (§7.2.13): None = undefined (NaN / unparseable string).
pub fn big_cmp(it: &mut Interp, a: &Val, b: &Val) -> Result<Option<Ordering>, Ctl> {
    let side = |it: &mut Interp, v: &Val| -> Result<Option<Result<Big, f64>>, Ctl> {
        Ok(match v { Val::Big(x) => Some(Ok((**x).clone())), Val::Str(s) => Big::parse(s).map(Ok), other => { let n = it.to_num(other)?; if n.is_nan() { None } else { Some(Err(n)) } } })
    };
    let (Some(x), Some(y)) = (side(it, a)?, side(it, b)?) else { return Ok(None) };
    Ok(Some(match (x, y) {
        (Ok(p), Ok(q)) => p.cmp(&q),
        (Ok(p), Err(n)) => cmp_big_num(&p, n),
        (Err(n), Ok(q)) => cmp_big_num(&q, n).reverse(),
        (Err(m), Err(n)) => m.partial_cmp(&n).unwrap_or(Ordering::Equal),
    }))
}
fn cmp_big_num(p: &Big, n: f64) -> Ordering {
    if n == f64::INFINITY { return Ordering::Less; } if n == f64::NEG_INFINITY { return Ordering::Greater; }
    let f = n.floor();
    match p.cmp(&Big::from_f64(f).unwrap_or_default()) { Ordering::Equal => if n > f { Ordering::Less } else { Ordering::Equal }, o => o }
}

// ── String.prototype (§22.1.3) ──
pub const STRING_METHODS: &[&str] = &["toUpperCase", "toLocaleUpperCase", "toLowerCase", "toLocaleLowerCase", "trim", "trimStart", "trimLeft", "trimEnd", "trimRight", "toString", "valueOf", "normalize",
    "includes", "startsWith", "endsWith", "indexOf", "lastIndexOf", "slice", "substring", "substr", "charAt", "charCodeAt", "codePointAt", "at", "repeat", "padStart", "padEnd", "concat", "split",
    "replace", "replaceAll", "localeCompare", "match", "matchAll", "search", "isWellFormed", "toWellFormed"];
pub fn string_method(key: &str, _x: Rc<str>) -> Option<Val> {
    Some(match key {
        "toUpperCase" | "toLocaleUpperCase" => nat!("toUpperCase", |it, this, _a| Ok(st(&this_str(it, &this)?.to_uppercase()))),
        "toLowerCase" | "toLocaleLowerCase" => nat!("toLowerCase", |it, this, _a| Ok(st(&this_str(it, &this)?.to_lowercase()))),
        "trim" => nat!("trim", |it, this, _a| Ok(st(this_str(it, &this)?.trim_matches(crate::jsi::es_ws)))),
        "trimStart" | "trimLeft" => nat!("trimStart", |it, this, _a| Ok(st(this_str(it, &this)?.trim_start_matches(crate::jsi::es_ws)))),
        "trimEnd" | "trimRight" => nat!("trimEnd", |it, this, _a| Ok(st(this_str(it, &this)?.trim_end_matches(crate::jsi::es_ws)))),
        "toString" | "valueOf" => nat!("toString", |it, this, _a| { if let Val::Obj(o) = &this { if !matches!(&o.borrow().kind, Kind::Prim(Val::Str(_))) { return Err(throw(it, "TypeError", "String.prototype.toString requires that 'this' be a String")); } } Ok(st(&this_str(it, &this)?)) }),
        "normalize" => nat!("normalize", |it, this, _a| Ok(st(&this_str(it, &this)?))),
        "isWellFormed" => nat!("isWellFormed", |it, this, _a| { let s = this_str(it, &this)?; Ok(Val::Bool(!s.chars().any(|c| (0xF0000 + 0xD800..=0xF0000 + 0xDFFF).contains(&(c as u32))))) }),
        "toWellFormed" => nat!("toWellFormed", |it, this, _a| { let s = this_str(it, &this)?; Ok(st(&s.chars().map(|c| if (0xF0000 + 0xD800..=0xF0000 + 0xDFFF).contains(&(c as u32)) { '\u{fffd}' } else { c }).collect::<String>())) }),
        "includes" => nat!("includes", |it, this, a| { let s = U::of(it, &this)?; if is_regexp(it, &arg(&a, 0))? { return Err(throw(it, "TypeError", "First argument to String.prototype.includes must not be a regular expression")); } let n = it.to_str(&arg(&a, 0))?; let p = rel_arg(it, &a, 1, s.len(), 0)?.min(s.len()); Ok(Val::Bool(s.find(&n, p).is_some())) }),
        "startsWith" => nat!("startsWith", |it, this, a| { let s = U::of(it, &this)?; if is_regexp(it, &arg(&a, 0))? { return Err(throw(it, "TypeError", "First argument to String.prototype.startsWith must not be a regular expression")); } let n = utf16(&it.to_str(&arg(&a, 0))?); let p = if matches!(arg(&a, 1), Val::Undef) { 0 } else { let f = it.to_num(&arg(&a, 1))?; if f.is_nan() { 0 } else { (f.max(0.0) as usize).min(s.len()) } }; Ok(Val::Bool(p + n.len() <= s.len() && (0..n.len()).all(|i| s.at(p + i) == n[i]))) }),
        "endsWith" => nat!("endsWith", |it, this, a| { let s = U::of(it, &this)?; if is_regexp(it, &arg(&a, 0))? { return Err(throw(it, "TypeError", "First argument to String.prototype.endsWith must not be a regular expression")); } let n = utf16(&it.to_str(&arg(&a, 0))?); let e = if matches!(arg(&a, 1), Val::Undef) { s.len() } else { let f = it.to_num(&arg(&a, 1))?; if f.is_nan() { 0 } else { (f.max(0.0) as usize).min(s.len()) } }; Ok(Val::Bool(n.len() <= e && (0..n.len()).all(|i| s.at(e - n.len() + i) == n[i]))) }),
        "indexOf" => nat!("indexOf", |it, this, a| { let s = U::of(it, &this)?; let n = it.to_str(&arg(&a, 0))?; let from = if matches!(arg(&a, 1), Val::Undef) { 0 } else { let f = it.to_num(&arg(&a, 1))?; if f.is_nan() { 0 } else { f.max(0.0).min(s.len() as f64) as usize } }; Ok(Val::Num(match s.find(&n, from) { Some(i) => i as f64, None => if n.is_empty() { s.len().min(from) as f64 } else { -1.0 } })) }),
        "lastIndexOf" => nat!("lastIndexOf", |it, this, a| { let s = U::of(it, &this)?; let h = s.units(); let n = utf16(&it.to_str(&arg(&a, 0))?); let from = match arg(&a, 1) { Val::Undef => h.len(), v => { let f = it.to_num(&v)?; if f.is_nan() { h.len() } else { f.max(0.0) as usize } } }; Ok(Val::Num(if n.len() > h.len() { -1.0 } else { (0..=(h.len() - n.len()).min(from)).rev().find(|&i| h[i..i + n.len()] == n[..]).map(|i| i as f64).unwrap_or(-1.0) })) }),
        "slice" => nat!("slice", |it, this, a| { let s = U::of(it, &this)?; let b = rel_arg(it, &a, 0, s.len(), 0)?; let e = rel_arg(it, &a, 1, s.len(), s.len())?; Ok(s.sub(b, e)) }),
        "substring" => nat!("substring", |it, this, a| { let s = U::of(it, &this)?; let n = s.len(); let cl = |v: f64| if v.is_nan() { 0 } else { v.max(0.0).min(n as f64) as usize }; let b = cl(it.to_num(&arg(&a, 0))?); let e = if matches!(arg(&a, 1), Val::Undef) { n } else { cl(it.to_num(&arg(&a, 1))?) }; let (b, e) = (b.min(e), b.max(e)); Ok(s.sub(b, e)) }),
        "substr" => nat!("substr", |it, this, a| { let s = U::of(it, &this)?; let b = rel_arg(it, &a, 0, s.len(), 0)?; let l = if matches!(arg(&a, 1), Val::Undef) { s.len() } else { let l = it.to_num(&arg(&a, 1))?; if l.is_nan() { 0 } else { l.max(0.0).min(s.len() as f64) as usize } }; Ok(s.sub(b, (b + l).min(s.len()))) }),
        "charAt" => nat!("charAt", |it, this, a| { let s = U::of(it, &this)?; let i = it.to_num(&arg(&a, 0))?; let i = if i.is_nan() { 0.0 } else { i.trunc() }; Ok(if i >= 0.0 && (i as usize) < s.len() { s.sub(i as usize, i as usize + 1) } else { st("") }) }),
        "charCodeAt" => nat!("charCodeAt", |it, this, a| { let s = U::of(it, &this)?; let i = match arg(&a, 0) { Val::Num(n) => n, Val::Undef => 0.0, v => it.to_num(&v)? }; let i = if i.is_nan() { 0.0 } else { i.trunc() }; Ok(Val::Num(if i >= 0.0 && (i as usize) < s.len() { s.at(i as usize) as f64 } else { f64::NAN })) }),
        "codePointAt" => nat!("codePointAt", |it, this, a| { let s = U::of(it, &this)?; let i = it.to_num(&arg(&a, 0))?; let i = if i.is_nan() { 0.0 } else { i.trunc() }; if i < 0.0 || i as usize >= s.len() { return Ok(Val::Undef); } let i = i as usize; let c = s.at(i); if (0xD800..0xDC00).contains(&c) && i + 1 < s.len() { let d = s.at(i + 1); if (0xDC00..0xE000).contains(&d) { return Ok(Val::Num((0x10000 + ((c as u32 - 0xD800) << 10) + (d as u32 - 0xDC00)) as f64)); } } Ok(Val::Num(c as f64)) }),
        "at" => nat!("at", |it, this, a| { let s = U::of(it, &this)?; let i = it.to_num(&arg(&a, 0))?; let i = if i.is_nan() { 0.0 } else { i.trunc() }; let i = if i < 0.0 { s.len() as f64 + i } else { i }; Ok(if i >= 0.0 && (i as usize) < s.len() { s.sub(i as usize, i as usize + 1) } else { Val::Undef }) }),
        "repeat" => nat!("repeat", |it, this, a| { let s = this_str(it, &this)?; let n = it.to_num(&arg(&a, 0))?; let n = if n.is_nan() { 0.0 } else { n }; if n < 0.0 || n.is_infinite() || (s.len() as f64 * n.trunc()) > 5e8 { return Err(throw(it, "RangeError", &format!("Invalid count value: {}", num_str(n)))); } Ok(st(&s.repeat(n as usize))) }),
        "padStart" | "padEnd" => { let start = key == "padStart"; nat!("pad", |it, this, a| { let s = utf16(&this_str(it, &this)?); let n = it.to_num(&arg(&a, 0))?; let n = if n.is_nan() { 0 } else { n.max(0.0).min(5e8) as usize }; let f = if matches!(arg(&a, 1), Val::Undef) { vec![32u16] } else { utf16(&it.to_str(&arg(&a, 1))?) }; if n <= s.len() || f.is_empty() { return Ok(st(&from16(&s))); } let pad: Vec<u16> = f.iter().copied().cycle().take(n - s.len()).collect(); Ok(st(&from16(&if start { [pad, s].concat() } else { [s, pad].concat() }))) }) }
        "concat" => nat!("concat", |it, this, a| { let mut s = this_str(it, &this)?; for x in &a { s += &it.to_str(x)?; } Ok(st(&crate::jsi::fix_pairs(s))) }),
        "split" => nat!("split", |it, this, a| {
            if this.is_nullish() { return Err(throw(it, "TypeError", "String.prototype.split called on null or undefined")); }
            // §22.1.3.23 step 2: a separator object with @@split decides (RegExp.prototype[@@split] for regexps)
            if let Some(m) = sym_method(it, &arg(&a, 0), "\u{0}@@split")? { let sv = Val::Str(this_rc(it, &this)?); return it.call(&m, arg(&a, 0), vec![sv, arg(&a, 1)]); }
            let s = this_str(it, &this)?;
            let lim = if matches!(arg(&a, 1), Val::Undef) { usize::MAX } else { crate::jsi::to_uint32(it.to_num(&arg(&a, 1))?) as usize };
            if lim == 0 { return Ok(Val::array(vec![])); }
            let sep0 = arg(&a, 0);
            // a RegExp separator splits on matches (captures interleaved, per spec), not on its source text
            let is_re = matches!(&sep0, Val::Obj(o) if matches!(o.borrow().kind, Kind::RegExp(..)));
            let all: Vec<Val> = if is_re {
                let v = crate::jsi_re::split(it, &s, &sep0)?;
                it.iterate(&v)?
            } else {
                match sep0 {
                    Val::Undef => vec![st(&s)],
                    sep => { let sep = it.to_str(&sep)?; if sep.is_empty() { utf16(&s).iter().map(|c| st(&from16(&[*c]))).collect() } else { s.split(&*sep).map(st).collect() } }
                }
            };
            Ok(Val::array(all.into_iter().take(lim).collect()))
        }),
        "replace" | "replaceAll" => { let all = key == "replaceAll"; nat!("replace", |it, this, a| {
            if this.is_nullish() { return Err(throw(it, "TypeError", "String.prototype.replace called on null or undefined")); }
            let pat = arg(&a, 0);
            if all && is_regexp(it, &pat)? { let f = it.get(&pat, "flags")?; if !it.to_str(&f)?.contains('g') { return Err(throw(it, "TypeError", "replaceAll must be called with a global RegExp")); } }
            if let Some(m) = sym_method(it, &pat, "\u{0}@@replace")? { let sv = Val::Str(this_rc(it, &this)?); return it.call(&m, pat, vec![sv, arg(&a, 1)]); }
            let s = this_str(it, &this)?;
            if let Val::Obj(o) = &pat { if let Kind::RegExp(_, f) = &o.borrow().kind { if all && !f.contains('g') { return Err(throw(it, "TypeError", "replaceAll must be called with a global RegExp")); } } }
            if let Val::Obj(o) = &pat { if matches!(o.borrow().kind, Kind::RegExp(..)) { return crate::jsi_re::replace(it, &s, &pat, &arg(&a, 1)); } }
            let p = it.to_str(&pat)?.to_string();
            let rep = arg(&a, 1);
            let rep_s = if rep.is_callable() { None } else { Some(it.to_str(&rep)?.to_string()) };
            let mut out = String::new(); let mut rest = s.as_str(); let mut pos = 0usize;
            loop {
                match rest.find(&p) {
                    Some(i) => {
                        out += &rest[..i];
                        let at = pos + i;
                        let r = match &rep_s { None => { let v = it.call(&rep, Val::Undef, vec![st(&p), Val::Num(len16(&s[..at]) as f64), st(&s)])?; it.to_str(&v)?.to_string() } Some(t) => expand_dollars(t, &p, &s[..at], &s[at + p.len()..]) };
                        out += &r;
                        let adv = i + p.len().max(if p.is_empty() { rest[i..].chars().next().map_or(1, |c| c.len_utf8()) } else { 0 });
                        if p.is_empty() { out += &rest[i..adv.min(rest.len())]; }
                        pos += adv; rest = &rest[adv.min(rest.len())..];
                        if !all || rest.is_empty() && !p.is_empty() { out += rest; break; }
                        if rest.is_empty() { if p.is_empty() { let r = match &rep_s { None => { let v = it.call(&rep, Val::Undef, vec![st(&p), Val::Num(len16(&s) as f64), st(&s)])?; it.to_str(&v)?.to_string() } Some(t) => expand_dollars(t, &p, &s, "") }; out += &r; } break; }
                    }
                    None => { out += rest; break; }
                }
            }
            Ok(st(&out))
        }) }
        "localeCompare" => nat!("localeCompare", |it, this, a| { let x = this_str(it, &this)?; let y = it.to_str(&arg(&a, 0))?.to_string(); Ok(Val::Num(match locale_cmp(&x, &y) { Ordering::Less => -1.0, Ordering::Equal => 0.0, _ => 1.0 })) }),
        // §22.1.3.13 / .14 / .22: delegate to @@match / @@matchAll / @@search; a non-regexp argument becomes RegExp(arg)
        "match" | "matchAll" | "search" => { let k = key.to_string(); nat!("match", |it, this, a| {
            if this.is_nullish() { return Err(throw(it, "TypeError", "String.prototype.match called on null or undefined")); }
            let sym = format!("\u{0}@@{k}");
            let r0 = arg(&a, 0);
            if k == "matchAll" && is_regexp(it, &r0)? { let f = it.get(&r0, "flags")?; if !it.to_str(&f)?.contains('g') { return Err(throw(it, "TypeError", "String.prototype.matchAll called with a non-global RegExp argument")); } }
            if let Some(m) = sym_method(it, &r0, &sym)? { let sv = Val::Str(this_rc(it, &this)?); return it.call(&m, r0, vec![sv]); }
            let sv = Val::Str(this_rc(it, &this)?);
            let rx = regexp_create(it, &r0, if k == "matchAll" { "g" } else { "" })?;
            let m = it.get(&rx, &sym)?;
            it.call(&m, rx, vec![sv])
        }) }
        _ => return None,
    })
}
/// GetSubstitution (§22.1.3.19.1) for a string pattern: $$ $& $` $'.
fn expand_dollars(t: &str, matched: &str, before: &str, after: &str) -> String {
    if !t.contains('$') { return t.to_string(); }
    let c: Vec<char> = t.chars().collect(); let mut o = String::new(); let mut j = 0;
    while j < c.len() { if c[j] == '$' && j + 1 < c.len() { match c[j + 1] { '$' => { o.push('$'); j += 2; continue; } '&' => { o += matched; j += 2; continue; } '`' => { o += before; j += 2; continue; } '\'' => { o += after; j += 2; continue; } _ => {} } } o.push(c[j]); j += 1; }
    o
}
/// Collation for localeCompare / Intl.Collator: case- and accent-insensitive first, then by case (lower first), as ICU's root order.
pub fn locale_cmp(x: &str, y: &str) -> Ordering {
    let fold = |s: &str| -> Vec<char> { s.chars().map(|c| strip_accent(c).to_lowercase().next().unwrap_or(c)).collect() };
    match fold(x).cmp(&fold(y)) { Ordering::Equal => { let lx: Vec<bool> = x.chars().map(|c| c.is_uppercase()).collect(); let ly: Vec<bool> = y.chars().map(|c| c.is_uppercase()).collect(); match lx.cmp(&ly) { Ordering::Equal => x.cmp(y), o => o } } o => o }
}
fn strip_accent(c: char) -> char {
    match c { 'à'..='å' => 'a', 'À'..='Å' => 'A', 'è'..='ë' => 'e', 'È'..='Ë' => 'E', 'ì'..='ï' => 'i', 'Ì'..='Ï' => 'I', 'ò'..='ö' => 'o', 'Ò'..='Ö' => 'O', 'ù'..='ü' => 'u', 'Ù'..='Ü' => 'U', 'ç' => 'c', 'Ç' => 'C', 'ñ' => 'n', 'Ñ' => 'N', 'ý' | 'ÿ' => 'y', _ => c }
}

/// The exact decimal digits of |x| (finite, non-zero) and its exponent: |x| = d0.d1d2… × 10^e.
fn exact_digits(x: f64) -> (Vec<u8>, i32) {
    let s = format!("{:.1100e}", x.abs());
    let (m, e) = s.split_once('e').unwrap();
    let mut d: Vec<u8> = m.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect();
    while d.len() > 1 && d.last() == Some(&0) { d.pop(); }
    (d, e.parse().unwrap_or(0))
}
/// Round exact digits to n significant digits, ties away from zero (the value is exact, so ties are real ties); returns (digits, exponent bump).
fn round_sig(d: &[u8], n: usize) -> (Vec<u8>, i32) {
    let mut v: Vec<u8> = d.iter().take(n).cloned().collect(); while v.len() < n { v.push(0); }
    if d.get(n).map_or(false, |&x| x >= 5) {
        let mut i = n;
        loop { if i == 0 { v.insert(0, 1); v.pop(); return (v, 1); } i -= 1; if v[i] == 9 { v[i] = 0; } else { v[i] += 1; break; } }
    }
    (v, 0)
}
fn digits_str(d: &[u8]) -> String { d.iter().map(|x| (b'0' + x) as char).collect() }
/// Number.prototype.toExponential (§21.1.3.2); f = None → as many digits as needed to round-trip.
pub fn to_exponential(x: f64, f: Option<usize>) -> String {
    if !x.is_finite() { return num_str(x); }
    let sign = if x < 0.0 { "-" } else { "" };
    if x == 0.0 { let fd = f.unwrap_or(0); return format!("0{}e+0", if fd > 0 { format!(".{}", "0".repeat(fd)) } else { String::new() }); }
    let (d, e) = match f {
        Some(fd) => { let (d, e0) = exact_digits(x); let (r, bump) = round_sig(&d, fd + 1); (r, e0 + bump) }
        None => { let s = format!("{:e}", x.abs()); let (m, e) = s.split_once('e').unwrap(); (m.bytes().filter(|b| b.is_ascii_digit()).map(|b| b - b'0').collect(), e.parse().unwrap_or(0)) }
    };
    let ds = digits_str(&d);
    let mant = if ds.len() > 1 { format!("{}.{}", &ds[..1], &ds[1..]) } else { ds };
    format!("{sign}{mant}e{}{}", if e >= 0 { "+" } else { "-" }, e.abs())
}
/// Number.prototype.toPrecision (§21.1.3.5).
pub fn to_precision(x: f64, p: usize) -> String {
    if !x.is_finite() { return num_str(x); }
    let sign = if x < 0.0 { "-" } else { "" };
    if x == 0.0 { return if p > 1 { format!("0.{}", "0".repeat(p - 1)) } else { "0".into() }; }
    let (d, e0) = exact_digits(x); let (r, bump) = round_sig(&d, p); let e = e0 + bump;
    let ds = digits_str(&r);
    if e < -6 || e >= p as i32 { let mant = if p > 1 { format!("{}.{}", &ds[..1], &ds[1..]) } else { ds }; return format!("{sign}{mant}e{}{}", if e >= 0 { "+" } else { "-" }, e.abs()); }
    if e >= 0 { let ip = &ds[..(e as usize + 1)]; let fp = &ds[(e as usize + 1)..]; if fp.is_empty() { format!("{sign}{ip}") } else { format!("{sign}{ip}.{fp}") } }
    else { format!("{sign}0.{}{}", "0".repeat((-e - 1) as usize), ds) }
}
/// Number.prototype.toString(radix) for fractions (§21.1.3.6): digits until the value is exact (≤ 52 binary digits of fraction).
fn radix_str(x: f64, r: u32) -> String {
    let neg = x < 0.0; let a = x.abs();
    let mut ip = a.trunc(); let mut fp = a - ip;
    let mut ids = Vec::new();
    if ip == 0.0 { ids.push('0'); }
    while ip >= 1.0 { let d = (ip % r as f64) as u32; ids.push(std::char::from_digit(d, r).unwrap()); ip = (ip / r as f64).trunc(); }
    let mut s: String = ids.iter().rev().collect();
    if fp > 0.0 { s.push('.'); let mut n = 0; while fp > 0.0 && n < 52 { fp *= r as f64; let d = fp.trunc() as u32; s.push(std::char::from_digit(d.min(r - 1), r).unwrap()); fp -= d as f64; n += 1; } }
    if neg { format!("-{s}") } else { s }
}

/// Intl.NumberFormat / toLocaleString for en-US (ECMA-402 §15.5): grouping, fraction digit bounds, percent and currency styles.
pub struct NumFmt { pub min_frac: usize, pub max_frac: usize, pub grouping: bool, pub style: u8, pub currency: String, pub min_int: usize, pub sign_display: u8, pub compact: bool }
impl Default for NumFmt { fn default() -> Self { NumFmt { min_frac: 0, max_frac: 3, grouping: true, style: 0, currency: String::new(), min_int: 1, sign_display: 0, compact: false } } }
pub fn num_fmt_opts(it: &mut Interp, o: &Val) -> Result<NumFmt, Ctl> {
    let mut f = NumFmt::default();
    if !matches!(o, Val::Obj(_)) { return Ok(f); }
    let gs = |it: &mut Interp, k: &str| -> Result<Option<String>, Ctl> { let v = it.get(o, k)?; Ok(if v.is_nullish() { None } else { Some(it.to_str(&v)?.to_string()) }) };
    let gn = |it: &mut Interp, k: &str| -> Result<Option<usize>, Ctl> { let v = it.get(o, k)?; Ok(if v.is_nullish() { None } else { Some(it.to_num(&v)?.clamp(0.0, 100.0) as usize) }) };
    match gs(it, "style")?.as_deref() { Some("percent") => { f.style = 1; f.max_frac = 0; } Some("currency") => { f.style = 2; f.currency = gs(it, "currency")?.unwrap_or_else(|| "USD".into()).to_uppercase(); let d = if matches!(f.currency.as_str(), "JPY" | "KRW") { 0 } else { 2 }; f.min_frac = d; f.max_frac = d; } Some("unit") | Some("decimal") | None | Some(_) => {} }
    if let Some(n) = gn(it, "minimumFractionDigits")? { f.min_frac = n; if f.max_frac < n { f.max_frac = n; } }
    if let Some(n) = gn(it, "maximumFractionDigits")? { f.max_frac = n.max(f.min_frac); }
    if let Some(n) = gn(it, "minimumIntegerDigits")? { f.min_int = n.max(1); }
    if let Some(n) = gn(it, "maximumSignificantDigits")? { let _ = n; }
    let ug = it.get(o, "useGrouping")?; if matches!(ug, Val::Bool(false)) { f.grouping = false; }
    match gs(it, "signDisplay")?.as_deref() { Some("always") => f.sign_display = 1, Some("never") => f.sign_display = 2, Some("exceptZero") => f.sign_display = 3, _ => {} }
    if gs(it, "notation")?.as_deref() == Some("compact") { f.compact = true; }
    Ok(f)
}
pub fn format_number(x: f64, f: &NumFmt) -> String {
    if x.is_nan() { return "NaN".into(); }
    let neg = x < 0.0 || (x == 0.0 && x.is_sign_negative() && f.sign_display == 0 && false);
    let mut v = x.abs();
    if f.style == 1 { v *= 100.0; }
    if v.is_infinite() { return format!("{}∞", if neg { "-" } else { "" }); }
    let mut suffix = String::new();
    let (min_frac, max_frac) = if f.compact { let units = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")]; let mut mf = (f.min_frac, f.max_frac); for (u, s) in units { if v >= u { v /= u; suffix = s.into(); mf = (0, if v < 100.0 { 1 } else { 0 }); break; } } if suffix.is_empty() { mf = (0, 0); } mf } else { (f.min_frac, f.max_frac) };
    let s = to_fixed(v, max_frac);
    let (ip, fp) = match s.split_once('.') { Some((a, b)) => (a.to_string(), b.to_string()), None => (s.clone(), String::new()) };
    let mut fp = fp; while fp.len() > min_frac && fp.ends_with('0') { fp.pop(); }
    let ip = if ip.len() < f.min_int { format!("{}{ip}", "0".repeat(f.min_int - ip.len())) } else { ip };
    let ip = if f.grouping { let b = ip.as_bytes(); let mut o = String::new(); for (i, c) in b.iter().enumerate() { if i > 0 && (b.len() - i) % 3 == 0 { o.push(','); } o.push(*c as char); } o } else { ip };
    let num = if fp.is_empty() { ip } else { format!("{ip}.{fp}") };
    let is_zero = v == 0.0;
    let sign = match f.sign_display { 1 => if neg { "-" } else { "+" }, 2 => "", 3 => if is_zero { "" } else if neg { "-" } else { "+" }, _ => if neg && !is_zero { "-" } else { "" } };
    match f.style { 1 => format!("{sign}{num}{suffix}%"), 2 => { let sym = match f.currency.as_str() { "USD" => "$", "EUR" => "€", "GBP" => "£", "JPY" => "¥", "INR" => "₹", "CAD" => "CA$", "AUD" => "A$", c => return format!("{sign}{c} {num}{suffix}") }; format!("{sign}{sym}{num}{suffix}") } _ => format!("{sign}{num}{suffix}") }
}

// ── Number.prototype (§21.1.3) ──
pub const NUMBER_METHODS: &[&str] = &["toFixed", "toString", "valueOf", "toLocaleString", "toPrecision", "toExponential"];
fn this_num(it: &mut Interp, this: &Val) -> Result<f64, Ctl> {
    match this { Val::Num(n) => Ok(*n), Val::Obj(o) => match &o.borrow().kind { Kind::Prim(Val::Num(n)) => Ok(*n), _ => Err(throw(it, "TypeError", "Number.prototype method called on an incompatible receiver")) }, _ => Err(throw(it, "TypeError", "Number.prototype method called on an incompatible receiver")) }
}
pub fn number_method(key: &str, _n: f64) -> Option<Val> {
    Some(match key {
        "toFixed" => nat!("toFixed", |it, this, a| { let x = this_num(it, &this)?; let d = if matches!(arg(&a, 0), Val::Undef) { 0.0 } else { it.to_num(&arg(&a, 0))?.trunc() }; let d = if d.is_nan() { 0.0 } else { d }; if !(0.0..=100.0).contains(&d) { return Err(throw(it, "RangeError", "toFixed() digits argument must be between 0 and 100")); } Ok(st(&to_fixed(x, d as usize))) }),
        "toString" => nat!("toString", |it, this, a| {
            let x = this_num(it, &this)?;
            let r = if matches!(arg(&a, 0), Val::Undef) { 10.0 } else { it.to_num(&arg(&a, 0))?.trunc() };
            if !(2.0..=36.0).contains(&r) { return Err(throw(it, "RangeError", "toString() radix must be between 2 and 36")); }
            let r = r as u32;
            if r == 10 || !x.is_finite() { return Ok(st(&num_str(x))); }
            Ok(st(&radix_str(x, r)))
        }),
        "valueOf" => nat!("valueOf", |it, this, _a| Ok(Val::Num(this_num(it, &this)?))),
        "toLocaleString" => nat!("toLocaleString", |it, this, a| { let x = this_num(it, &this)?; let f = num_fmt_opts(it, &arg(&a, 1))?; Ok(st(&format_number(x, &f))) }),
        "toPrecision" => nat!("toPrecision", |it, this, a| { let x = this_num(it, &this)?; if matches!(arg(&a, 0), Val::Undef) { return Ok(st(&num_str(x))); } let p = it.to_num(&arg(&a, 0))?.trunc(); if !x.is_finite() { return Ok(st(&num_str(x))); } if !(1.0..=100.0).contains(&p) { return Err(throw(it, "RangeError", "toPrecision() argument must be between 1 and 100")); } Ok(st(&to_precision(x, p as usize))) }),
        "toExponential" => nat!("toExponential", |it, this, a| { let x = this_num(it, &this)?; let f = if matches!(arg(&a, 0), Val::Undef) { None } else { Some(it.to_num(&arg(&a, 0))?.trunc()) }; if !x.is_finite() { return Ok(st(&num_str(x))); } if let Some(f) = f { if !(0.0..=100.0).contains(&f) { return Err(throw(it, "RangeError", "toExponential() argument must be between 0 and 100")); } } Ok(st(&to_exponential(x, f.map(|f| f as usize)))) }),
        _ => return None,
    })
}

fn cmp_default(it: &mut Interp, a: &Val, b: &Val) -> Result<Ordering, Ctl> {
    // undefined sorts last; otherwise by UTF-16 string order
    Ok(match (a, b) { (Val::Undef, Val::Undef) => Ordering::Equal, (Val::Undef, _) => Ordering::Greater, (_, Val::Undef) => Ordering::Less,
        (Val::Num(x), Val::Num(y)) if x.fract() == 0.0 && y.fract() == 0.0 && *x >= 0.0 && *y >= 0.0 && *x < 1e15 && *y < 1e15 => num_str(*x).cmp(&num_str(*y)),
        _ => { let (x, y) = (utf16(&it.to_str(a)?), utf16(&it.to_str(b)?)); x.cmp(&y) } })
}
/// SortCompare (§23.1.3.30.2) as a stable merge sort with a fallible comparator.
pub fn sort_vals(it: &mut Interp, vals: Vec<Val>, f: &Val) -> Result<Vec<Val>, Ctl> {
    let mut err = None;
    let mut idx: Vec<usize> = (0..vals.len()).collect();
    idx.sort_by(|&i, &j| {
        if err.is_some() { return Ordering::Equal; }
        let (x, y) = (&vals[i], &vals[j]);
        let r = if f.is_callable() && !matches!(x, Val::Undef) && !matches!(y, Val::Undef) { it.call(f, Val::Undef, vec![x.clone(), y.clone()]).and_then(|r| it.to_num(&r)).map(|n| if n < 0.0 { Ordering::Less } else if n > 0.0 { Ordering::Greater } else { Ordering::Equal }) } else { cmp_default(it, x, y) };
        match r { Ok(o) => o, Err(e) => { err = Some(e); Ordering::Equal } }
    });
    if let Some(e) = err { return Err(e); }
    Ok(idx.into_iter().map(|i| vals[i].clone()).collect())
}
fn flatten(it: &mut Interp, v: Vec<Val>, depth: f64, out: &mut Vec<Val>) -> Result<(), Ctl> {
    for x in v { let is_arr = matches!(&x, Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_))); if is_arr && depth >= 1.0 { let inner = arr_of(&x); flatten(it, inner, depth - 1.0, out)?; } else { out.push(x); } }
    Ok(())
}
fn new_iter(st_: IterSt) -> Val { Val::obj(Kind::Iter(Box::new(st_))) }
/// ArraySpeciesCreate (§10.4.2.3) + filling: a plain array unless `this` is an array whose constructor's @@species is
/// another constructor (subclasses of Array, core-js's detection).
/// Array(n) / a length gap leaves holes. Arrays hold plain values here, so a flagged array (F_HOLES) reads its `undefined`
/// elements as holes in the iteration methods (ECMA-262 §23.1.3: map/forEach/filter/some/every skip absent indices) —
/// the `Array(3).concat(...).map(f)` idiom (d3-scale-chromatic) depends on it.
fn holey(v: &Val) -> bool { matches!(v, Val::Obj(o) if o.borrow().flags & crate::jsi::F_HOLES != 0) }
fn mark_holey(v: Val) -> Val { if let Val::Obj(o) = &v { o.borrow_mut().flags |= crate::jsi::F_HOLES; } v }
fn species_wrap(it: &mut Interp, this: &Val, items: Vec<Val>) -> Result<Val, Ctl> {
    let is_array = |v: &Val| matches!(v, Val::Obj(o) if matches!(&o.borrow().kind, Kind::Native(_, "Array")));
    if !matches!(this, Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_))) { return Ok(Val::array(items)); }
    let c = it.get(this, "constructor")?;
    let c = match &c {
        Val::Undef => return Ok(Val::array(items)),
        Val::Obj(_) if is_array(&c) => return Ok(Val::array(items)),
        Val::Obj(_) => { let sp = it.get(&c, crate::jsi::K_SPECIES)?; if sp.is_nullish() || is_array(&sp) { return Ok(Val::array(items)); } sp }
        _ => return Err(throw(it, "TypeError", "object.constructor[Symbol.species] is not a constructor")),
    };
    let n = items.len();
    let o = it.construct(&c, vec![Val::Num(0.0)])?;
    for (i, x) in items.into_iter().enumerate() { if let Val::Obj(oo) = &o { Interp::define(oo, &i.to_string(), x, 0); } }
    it.set(&o, "length", Val::Num(n as f64))?;
    Ok(o)
}

// ── Array.prototype (§23.1.3) ──
pub const ARRAY_METHODS: &[&str] = &["push", "pop", "shift", "unshift", "slice", "splice", "concat", "join", "reverse", "indexOf", "lastIndexOf", "includes", "at", "map", "filter", "forEach",
    "some", "every", "find", "findIndex", "findLast", "findLastIndex", "reduce", "reduceRight", "sort", "flat", "flatMap", "fill", "keys", "values", "entries", "toString", "toLocaleString",
    "copyWithin", "toSorted", "toReversed", "toSpliced", "with"];
pub fn array_method(key: &str, _v: Val) -> Option<Val> {
    Some(match key {
        "push" => nat!("push", |it, this, a| { if !matches!(&this, Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_))) { let n = it.get(&this, "length")?; let mut n = it.to_num(&n)?.max(0.0) as usize; for x in a { it.set(&this, &n.to_string(), x)?; n += 1; } it.set(&this, "length", Val::Num(n as f64))?; return Ok(Val::Num(n as f64)); } frozen_arr(it, &this)?; Ok(Val::Num(with_arr(&this, |v| { v.extend(a); v.len() }).unwrap_or(0) as f64)) }),
        "pop" => nat!("pop", |it, this, _a| { if !arr_of(&this).is_empty() { frozen_arr(it, &this)?; } Ok(with_arr(&this, |v| v.pop()).flatten().unwrap_or(Val::Undef)) }),
        "shift" => nat!("shift", |it, this, _a| { if !arr_of(&this).is_empty() { frozen_arr(it, &this)?; } Ok(with_arr(&this, |v| if v.is_empty() { None } else { Some(v.remove(0)) }).flatten().unwrap_or(Val::Undef)) }),
        "unshift" => nat!("unshift", |it, this, a| { frozen_arr(it, &this)?; Ok(Val::Num(with_arr(&this, |v| { v.splice(0..0, a); v.len() }).unwrap_or(0) as f64)) }),
        "slice" => nat!("slice", |it, this, a| { let v = list_of(it, &this)?; let b = rel_arg(it, &a, 0, v.len(), 0)?; let e = rel_arg(it, &a, 1, v.len(), v.len())?; species_wrap(it, &this, if b < e { v[b..e].to_vec() } else { vec![] }) }),
        "splice" => nat!("splice", |it, this, a| {
            let len = arr_of(&this).len();
            let b = rel_arg(it, &a, 0, len, 0)?;
            let dc = if a.is_empty() { 0 } else if a.len() < 2 { len - b } else { let d = it.to_num(&arg(&a, 1))?; (if d.is_nan() { 0.0 } else { d.max(0.0) } as usize).min(len - b) };
            let ins: Vec<Val> = a.iter().skip(2).cloned().collect();
            if dc > 0 || !ins.is_empty() { frozen_arr(it, &this)?; }
            let removed: Vec<Val> = with_arr(&this, |v| v.splice(b..b + dc, ins).collect()).unwrap_or_default();
            species_wrap(it, &this, removed)
        }),
        // concat (§23.1.3.1): IsConcatSpreadable — @@isConcatSpreadable decides, else arrays spread
        "concat" => nat!("concat", |it, this, a| { let mut v = Vec::new(); let mut hl = false; for x in std::iter::once(this.clone()).chain(a.into_iter()) { hl |= holey(&x); let spread = if let Val::Obj(_) = &x { let f = it.get(&x, "\u{0}@@isConcatSpreadable")?; if matches!(f, Val::Undef) { matches!(&x, Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_))) } else { f.truthy() } } else { false }; if spread { v.extend(list_of(it, &x)?); } else { v.push(x); } } let r = species_wrap(it, &this, v)?; Ok(if hl { mark_holey(r) } else { r }) }),
        "join" => nat!("join", |it, this, a| {
            let sep = if matches!(arg(&a, 0), Val::Undef) { ",".to_string() } else { it.to_str(&arg(&a, 0))?.to_string() };
            if let Val::Obj(o) = &this { if matches!(o.borrow().kind, Kind::Array(_)) { let o = o.clone(); return Ok(Val::Str(it.array_join(&o, &sep)?)); } }
            let v = list_of(it, &this)?; let mut parts = Vec::new(); for x in &v { parts.push(if x.is_nullish() { String::new() } else { it.to_str(x)?.to_string() }); } Ok(st(&crate::jsi::fix_pairs(parts.join(&sep))))
        }),
        "reverse" => nat!("reverse", |it, this, _a| { if arr_of(&this).len() > 1 { frozen_arr(it, &this)?; } with_arr(&this, |v| v.reverse()); Ok(this) }),
        "indexOf" => nat!("indexOf", |it, this, a| { let x = arg(&a, 0); let v = list_of(it, &this)?; let from = rel_arg(it, &a, 1, v.len(), 0)?; Ok(Val::Num(v.iter().skip(from).position(|y| Interp::strict_eq(y, &x)).map(|i| (i + from) as f64).unwrap_or(-1.0))) }),
        "lastIndexOf" => nat!("lastIndexOf", |it, this, a| { let x = arg(&a, 0); let v = list_of(it, &this)?; let end = if a.len() > 1 { let f = it.to_num(&a[1])?.trunc(); if f < 0.0 { v.len() as f64 + f } else { f } } else { v.len() as f64 - 1.0 }; if end < 0.0 { return Ok(Val::Num(-1.0)); } let end = (end as usize).min(v.len().saturating_sub(1)); Ok(Val::Num(v.iter().take(end + 1).rposition(|y| Interp::strict_eq(y, &x)).map(|i| i as f64).unwrap_or(-1.0))) }),
        "includes" => nat!("includes", |it, this, a| { let x = arg(&a, 0); let v = list_of(it, &this)?; let from = rel_arg(it, &a, 1, v.len(), 0)?; Ok(Val::Bool(v.iter().skip(from).any(|y| Interp::same_value_zero(y, &x)))) }),
        "at" => nat!("at", |it, this, a| { let v = list_of(it, &this)?; let i = it.to_num(&arg(&a, 0))?; let i = if i.is_nan() { 0.0 } else { i.trunc() }; let i = if i < 0.0 { v.len() as f64 + i } else { i }; Ok(if i >= 0.0 && (i as usize) < v.len() { v[i as usize].clone() } else { Val::Undef }) }),
        "map" => nat!("map", |it, this, a| { let v = list_of(it, &this)?; let f = arg(&a, 0); if !f.is_callable() { return Err(throw(it, "TypeError", "callback is not a function")); } let t = arg(&a, 1); let hl = holey(&this); let mut out = Vec::with_capacity(v.len()); for (i, x) in v.into_iter().enumerate() { if hl && matches!(x, Val::Undef) { out.push(Val::Undef); continue; } out.push(it.call(&f, t.clone(), vec![x, Val::Num(i as f64), this.clone()])?); } let r = species_wrap(it, &this, out)?; Ok(if hl { mark_holey(r) } else { r }) }),
        "filter" => nat!("filter", |it, this, a| { let v = list_of(it, &this)?; let f = arg(&a, 0); if !f.is_callable() { return Err(throw(it, "TypeError", "callback is not a function")); } let t = arg(&a, 1); let hl = holey(&this); let mut out = Vec::new(); for (i, x) in v.into_iter().enumerate() { if hl && matches!(x, Val::Undef) { continue; } if it.call(&f, t.clone(), vec![x.clone(), Val::Num(i as f64), this.clone()])?.truthy() { out.push(x); } } species_wrap(it, &this, out) }),
        "forEach" => nat!("forEach", |it, this, a| { let v = list_of(it, &this)?; let f = arg(&a, 0); if !f.is_callable() { return Err(throw(it, "TypeError", "callback is not a function")); } let t = arg(&a, 1); let hl = holey(&this); for (i, x) in v.into_iter().enumerate() { if hl && matches!(x, Val::Undef) { continue; } it.call(&f, t.clone(), vec![x, Val::Num(i as f64), this.clone()])?; } Ok(Val::Undef) }),
        "some" => nat!("some", |it, this, a| { let v = list_of(it, &this)?; let f = arg(&a, 0); let t = arg(&a, 1); let hl = holey(&this); for (i, x) in v.into_iter().enumerate() { if hl && matches!(x, Val::Undef) { continue; } if it.call(&f, t.clone(), vec![x, Val::Num(i as f64), this.clone()])?.truthy() { return Ok(Val::Bool(true)); } } Ok(Val::Bool(false)) }),
        "every" => nat!("every", |it, this, a| { let v = list_of(it, &this)?; let f = arg(&a, 0); let t = arg(&a, 1); let hl = holey(&this); for (i, x) in v.into_iter().enumerate() { if hl && matches!(x, Val::Undef) { continue; } if !it.call(&f, t.clone(), vec![x, Val::Num(i as f64), this.clone()])?.truthy() { return Ok(Val::Bool(false)); } } Ok(Val::Bool(true)) }),
        "find" | "findIndex" | "findLast" | "findLastIndex" => { let k = key.to_string(); nat!("find", |it, this, a| {
            let v = list_of(it, &this)?; let f = arg(&a, 0); let t = arg(&a, 1);
            if !f.is_callable() { return Err(throw(it, "TypeError", "predicate is not a function")); }
            let idx: Vec<usize> = if k.contains("Last") { (0..v.len()).rev().collect() } else { (0..v.len()).collect() };
            for i in idx { if it.call(&f, t.clone(), vec![v[i].clone(), Val::Num(i as f64), this.clone()])?.truthy() { return Ok(if k.ends_with("Index") { Val::Num(i as f64) } else { v[i].clone() }); } }
            Ok(if k.ends_with("Index") { Val::Num(-1.0) } else { Val::Undef })
        }) }
        "reduce" | "reduceRight" => { let right = key == "reduceRight"; nat!("reduce", |it, this, a| {
            let v = list_of(it, &this)?; let f = arg(&a, 0);
            if !f.is_callable() { return Err(throw(it, "TypeError", "callback is not a function")); }
            let order: Vec<usize> = if right { (0..v.len()).rev().collect() } else { (0..v.len()).collect() };
            let mut it_v = order.into_iter();
            let mut acc = if a.len() > 1 { arg(&a, 1) } else { match it_v.next() { Some(i) => v[i].clone(), None => return Err(throw(it, "TypeError", "Reduce of empty array with no initial value")) } };
            for i in it_v { acc = it.call(&f, Val::Undef, vec![acc, v[i].clone(), Val::Num(i as f64), this.clone()])?; }
            Ok(acc)
        }) }
        "sort" => nat!("sort", |it, this, a| { let f = arg(&a, 0); if !f.is_callable() && !matches!(f, Val::Undef) { return Err(throw(it, "TypeError", "The comparison function must be either a function or undefined")); } let v = list_of(it, &this)?; if v.len() > 1 { frozen_arr(it, &this)?; } let v = sort_vals(it, v, &f)?; with_arr(&this, |a| *a = v); Ok(this) }),
        "toSorted" => nat!("toSorted", |it, this, a| { let f = arg(&a, 0); let v = list_of(it, &this)?; Ok(Val::array(sort_vals(it, v, &f)?)) }),
        "toReversed" => nat!("toReversed", |it, this, _a| { let mut v = list_of(it, &this)?; v.reverse(); Ok(Val::array(v)) }),
        "toSpliced" => nat!("toSpliced", |it, this, a| { let mut v = list_of(it, &this)?; let len = v.len(); let b = rel_arg(it, &a, 0, len, 0)?; let dc = if a.is_empty() { 0 } else if a.len() < 2 { len - b } else { (it.to_num(&arg(&a, 1))?.max(0.0) as usize).min(len - b) }; v.splice(b..b + dc, a.iter().skip(2).cloned()); Ok(Val::array(v)) }),
        "with" => nat!("with", |it, this, a| { let mut v = list_of(it, &this)?; let i = it.to_num(&arg(&a, 0))?.trunc(); let i = if i < 0.0 { v.len() as f64 + i } else { i }; if i < 0.0 || i as usize >= v.len() { return Err(throw(it, "RangeError", "Invalid index")); } v[i as usize] = arg(&a, 1); Ok(Val::array(v)) }),
        "flat" => nat!("flat", |it, this, a| { let d = if matches!(arg(&a, 0), Val::Undef) { 1.0 } else { let d = it.to_num(&arg(&a, 0))?; if d.is_nan() { 0.0 } else { d } }; let v = list_of(it, &this)?; let mut out = Vec::new(); flatten(it, v, d, &mut out)?; species_wrap(it, &this, out) }),
        "flatMap" => nat!("flatMap", |it, this, a| { let f = arg(&a, 0); let t = arg(&a, 1); let mut out = Vec::new(); for (i, x) in list_of(it, &this)?.into_iter().enumerate() { let r = it.call(&f, t.clone(), vec![x, Val::Num(i as f64), this.clone()])?; match &r { Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_)) => out.extend(arr_of(&r)), _ => out.push(r) } } species_wrap(it, &this, out) }),
        "fill" => nat!("fill", |it, this, a| { let x = arg(&a, 0); let len = arr_of(&this).len(); let b = rel_arg(it, &a, 1, len, 0)?; let e = rel_arg(it, &a, 2, len, len)?; if b < e { frozen_arr(it, &this)?; } with_arr(&this, |v| for y in v[b.min(e)..e].iter_mut() { *y = x.clone(); }); Ok(this) }),
        "copyWithin" => nat!("copyWithin", |it, this, a| { let v = arr_of(&this); let len = v.len(); let t = rel_arg(it, &a, 0, len, 0)?; let s = rel_arg(it, &a, 1, len, 0)?; let e = rel_arg(it, &a, 2, len, len)?; let n = e.saturating_sub(s).min(len - t); if n > 0 { frozen_arr(it, &this)?; } with_arr(&this, |w| { let src: Vec<Val> = v[s..s + n].to_vec(); for (i, x) in src.into_iter().enumerate() { w[t + i] = x; } }); Ok(this) }),
        "keys" => nat!("keys", |_it, this, _a| Ok(new_iter(IterSt::Arr(this, 0, 0)))),
        "values" => nat!("values", |_it, this, _a| Ok(new_iter(IterSt::Arr(this, 0, 1)))),
        "entries" => nat!("entries", |_it, this, _a| Ok(new_iter(IterSt::Arr(this, 0, 2)))),
        "toString" | "toLocaleString" => nat!("toString", |it, this, _a| { if let Val::Obj(o) = &this { if matches!(o.borrow().kind, Kind::Array(_)) { let o = o.clone(); return Ok(Val::Str(it.array_join(&o, ",")?)); } } let j = it.get(&this, "join")?; if j.is_callable() { return it.call(&j, this, vec![]); } Ok(st("[object Object]")) }),
        _ => return None,
    })
}

// ── Map / Set (§24.1, §24.2) ──
fn md<R>(it: &Interp, this: &Val, f: impl FnOnce(&mut MapData) -> R) -> Result<R, Ctl> {
    if let Val::Obj(o) = this { if let Kind::Map(m) | Kind::Set(m) = &mut o.borrow_mut().kind { return Ok(f(m)); } }
    Err(throw(it, "TypeError", "Method called on incompatible receiver"))
}
pub const MAP_METHODS: &[&str] = &["get", "set", "has", "delete", "clear", "forEach", "keys", "values", "entries"];
pub fn map_method(key: &str, _v: Val) -> Option<Val> {
    Some(match key {
        "get" => nat!("get", |it, this, a| { let k = arg(&a, 0); md(it, &this, |m| m.get(&k).cloned().unwrap_or(Val::Undef)) }),
        "has" => nat!("has", |it, this, a| { let k = arg(&a, 0); md(it, &this, |m| Val::Bool(m.has(&k))) }),
        "set" => nat!("set", |it, this, a| { let (k, v) = (arg(&a, 0), arg(&a, 1)); md(it, &this, |m| m.set(k, v))?; Ok(this) }),
        "delete" => nat!("delete", |it, this, a| { let k = arg(&a, 0); md(it, &this, |m| Val::Bool(m.delete(&k))) }),
        "clear" => nat!("clear", |it, this, _a| { md(it, &this, |m| m.clear())?; Ok(Val::Undef) }),
        "keys" => nat!("keys", |it, this, _a| { md(it, &this, |_| ())?; Ok(new_iter(IterSt::Map(this, 0, 0))) }),
        "values" => nat!("values", |it, this, _a| { md(it, &this, |_| ())?; Ok(new_iter(IterSt::Map(this, 0, 1))) }),
        "entries" => nat!("entries", |it, this, _a| { md(it, &this, |_| ())?; Ok(new_iter(IterSt::Map(this, 0, 2))) }),
        "forEach" => nat!("forEach", |it, this, a| {
            let f = arg(&a, 0); let t = arg(&a, 1);
            let mut i = 0;
            loop {
                let e = md(it, &this, |m| { while i < m.ents.len() { let e = m.ents[i].clone(); i += 1; if e.is_some() { return e; } } None })?;
                let Some((k, v)) = e else { break };
                let is_set = matches!(&this, Val::Obj(o) if matches!(o.borrow().kind, Kind::Set(_)));
                it.call(&f, t.clone(), vec![if is_set { k.clone() } else { v }, k, this.clone()])?;
            }
            Ok(Val::Undef)
        }),
        _ => return None,
    })
}
pub const SET_METHODS: &[&str] = &["has", "add", "delete", "clear", "values", "keys", "entries", "forEach", "union", "intersection", "difference", "symmetricDifference", "isSubsetOf", "isSupersetOf", "isDisjointFrom"];
pub fn set_method(key: &str, v: Val) -> Option<Val> {
    Some(match key {
        "add" => nat!("add", |it, this, a| { let k = arg(&a, 0); md(it, &this, |m| m.set(k, Val::Undef))?; Ok(this) }),
        "values" | "keys" => nat!("values", |it, this, _a| { md(it, &this, |_| ())?; Ok(new_iter(IterSt::Map(this, 0, 1))) }),
        "union" | "intersection" | "difference" | "symmetricDifference" | "isSubsetOf" | "isSupersetOf" | "isDisjointFrom" => { let k = key.to_string(); nat!("setop", |it, this, a| {
            let mine: Vec<Val> = md(it, &this, |m| m.entries().map(|e| e.0.clone()).collect())?;
            let other = arg(&a, 0);
            // GetSetRecord (§24.2.1.2): size must be a non-negative number, has / keys callable
            if !matches!(other, Val::Obj(_)) { return Err(throw(it, "TypeError", "The argument must be an object")); }
            let sz = it.get(&other, "size")?; let n = it.to_num(&sz)?;
            if n.is_nan() { return Err(throw(it, "TypeError", "The 'size' property must be a number")); }
            if n.trunc() < 0.0 { return Err(throw(it, "RangeError", "'size' must be non-negative")); }
            for m in ["has", "keys"] { if !it.get(&other, m)?.is_callable() { return Err(throw(it, "TypeError", &format!("'{m}' must be a function"))); } }
            let theirs: Vec<Val> = match &other { Val::Obj(o) if matches!(o.borrow().kind, Kind::Set(_) | Kind::Map(_)) => md(it, &other, |m| m.entries().map(|e| e.0.clone()).collect())?, _ => {
                // GetSetRecord + GetIteratorFromMethod (§24.2.1.2): the set-like's keys() result is used as an iterator as is
                let ks = it.get(&other, "keys")?; if !ks.is_callable() { return Err(throw(it, "TypeError", "keys is not a function")); }
                let ki = it.call(&ks, other.clone(), vec![])?; if !matches!(ki, Val::Obj(_)) { return Err(throw(it, "TypeError", "keys() result is not an object")); }
                let nx = it.get(&ki, "next")?; let mut rec = crate::jsi::IterRec::Proto(ki, nx); let mut v = Vec::new(); while let Some(x) = it.iter_step(&mut rec)? { v.push(x); } v
            } };
            let has = |list: &[Val], x: &Val| list.iter().any(|y| Interp::same_value_zero(x, y));
            let set_of = |xs: Vec<Val>| { let mut m = MapData::new(); for x in xs { m.set(x, Val::Undef); } Val::obj(Kind::Set(Box::new(m))) };
            Ok(match k.as_str() {
                "union" => set_of(mine.iter().chain(theirs.iter()).cloned().collect()),
                // §24.2.4.9: walk the smaller side (the other set's keys when this one is larger)
                "intersection" => if mine.len() <= theirs.len() { set_of(mine.iter().filter(|x| has(&theirs, x)).cloned().collect()) } else { set_of(theirs.iter().filter(|x| has(&mine, x)).cloned().collect()) },
                "difference" => set_of(mine.iter().filter(|x| !has(&theirs, x)).cloned().collect()),
                "symmetricDifference" => set_of(mine.iter().filter(|x| !has(&theirs, x)).chain(theirs.iter().filter(|x| !has(&mine, x))).cloned().collect()),
                "isSubsetOf" => Val::Bool(mine.iter().all(|x| has(&theirs, x))),
                "isSupersetOf" => Val::Bool(theirs.iter().all(|x| has(&mine, x))),
                _ => Val::Bool(!mine.iter().any(|x| has(&theirs, x))),
            })
        }) }
        k => return map_method(k, v),
    })
}

// ── Promise.prototype (§27.2.5) ──
fn this_promise(it: &Interp, this: &Val) -> Result<(), Ctl> { if let Val::Obj(o) = this { if matches!(o.borrow().kind, Kind::Promise(_)) { return Ok(()); } } Err(throw(it, "TypeError", "Method Promise.prototype.then called on incompatible receiver")) }
pub fn resolved(v: Val) -> Val { if let Val::Obj(o) = &v { if matches!(o.borrow().kind, Kind::Promise(_)) { return v; } } let p = crate::jsi::new_promise(); if let Val::Obj(o) = &p { if let Kind::Promise(s) = &mut o.borrow_mut().kind { s.st = 1; s.v = v; } } p }
pub fn rejected(v: Val) -> Val { let p = crate::jsi::new_promise(); if let Val::Obj(o) = &p { if let Kind::Promise(s) = &mut o.borrow_mut().kind { s.st = 2; s.v = v; } } p }
pub const PROMISE_METHODS: &[&str] = &["then", "catch", "finally"];
pub fn promise_method(key: &str, _v: Val) -> Option<Val> {
    Some(match key {
        "then" => nat!("then", |it, this, a| { this_promise(it, &this)?; it.promise_then_species(&this, arg(&a, 0), arg(&a, 1)) }),
        "catch" => nat!("catch", |it, this, a| { let t = it.get(&this, "then")?; it.call(&t, this, vec![Val::Undef, arg(&a, 0)]) }),
        "finally" => nat!("finally", |it, this, a| {
            let f = arg(&a, 0);
            if !f.is_callable() { let t = it.get(&this, "then")?; return it.call(&t, this, vec![f.clone(), f]); }
            let (f1, f2) = (f.clone(), f);
            // §27.2.5.3: run onFinally, wait for its result, then pass the original outcome through
            let onf = Val::native("", move |it, _t, a| { let v = arg(&a, 0); let r = it.call(&f1, Val::Undef, vec![])?; let p = it.promise_resolve(r); Ok(it.promise_then(&p, Val::native("", move |_i, _t, _a| Ok(v.clone())), Val::Undef)) });
            let onr = Val::native("", move |it, _t, a| { let e = arg(&a, 0); let r = it.call(&f2, Val::Undef, vec![])?; let p = it.promise_resolve(r); Ok(it.promise_then(&p, Val::native("", move |_i, _t, _a| Err(Ctl::Throw(e.clone()))), Val::Undef)) });
            let t = it.get(&this, "then")?; it.call(&t, this, vec![onf, onr])
        }),
        _ => return None,
    })
}

// ── Function.prototype (§20.2.3) ──
pub const FUNCTION_METHODS: &[&str] = &["call", "apply", "bind", "toString"];
pub fn function_method(key: &str, _v: Val) -> Option<Val> {
    Some(match key {
        "call" => nat!("call", |it, this, a| { let t = arg(&a, 0); it.call(&this, t, a.into_iter().skip(1).collect()) }),
        "apply" => nat!("apply", |it, this, a| { let t = arg(&a, 0); let args = match arg(&a, 1) { Val::Undef | Val::Null => vec![], x => list_of(it, &x)? }; it.call(&this, t, args) }),
        "bind" => nat!("bind", |it, this, a| {
            if !this.is_callable() { return Err(throw(it, "TypeError", "Bind must be called on a function")); }
            let f = this.clone(); let t = arg(&a, 0); let pre: Vec<Val> = a.into_iter().skip(1).collect();
            let name = it.get(&f, "name").ok().and_then(|n| if let Val::Str(s) = n { Some(s.to_string()) } else { None }).unwrap_or_default();
            let leaked: &'static str = if name.is_empty() { "bound " } else { Box::leak(format!("bound {name}").into_boxed_str()) };
            // BoundFunctionCreate (§10.4.1.3): [[Construct]] goes to the target
            Ok(Val::native(leaked, move |it, this2, more| { let mut args = pre.clone(); args.extend(more); if is_new(&this2) { it.construct(&f, args) } else { it.call(&f, t.clone(), args) } }))
        }),
        "toString" => nat!("toString", |it, this, _a| { if !this.is_callable() { return Err(throw(it, "TypeError", "Function.prototype.toString requires that 'this' be a Function")); } Ok(Val::Str(it.to_str(&this)?)) }),
        _ => return None,
    })
}

pub const REGEXP_METHODS: &[&str] = &["test", "exec", "toString", "compile"];
/// GetMethod(v, @@sym) for an object argument (strings and other primitives never delegate here).
fn sym_method(it: &mut Interp, v: &Val, sym: &str) -> Result<Option<Val>, Ctl> {
    if !matches!(v, Val::Obj(_)) { return Ok(None); }
    let m = it.get(v, sym)?;
    if m.is_nullish() { return Ok(None); }
    if !m.is_callable() { return Err(throw(it, "TypeError", "Symbol method is not a function")); }
    Ok(Some(m))
}
/// IsRegExp (§7.2.8): @@match decides, else a RegExp object.
pub fn is_regexp(it: &mut Interp, v: &Val) -> Result<bool, Ctl> {
    if !matches!(v, Val::Obj(_)) { return Ok(false); }
    let m = it.get(v, "\u{0}@@match")?;
    if !matches!(m, Val::Undef) { return Ok(m.truthy()); }
    Ok(matches!(v, Val::Obj(o) if matches!(o.borrow().kind, Kind::RegExp(..))))
}
/// RegExpCreate (§22.2.3.1) through the global RegExp.
fn regexp_create(it: &mut Interp, p: &Val, flags: &str) -> Result<Val, Ctl> {
    let g = it.global.clone();
    let c = it.lookup_opt(&g, "RegExp").unwrap_or(Val::Undef);
    let p = if matches!(p, Val::Undef) { st("") } else { Val::Str(it.to_str(p)?) };
    it.construct(&c, vec![p, st(flags)])
}
/// Our own exec, unreplaced, on a real RegExp: the fast paths may run.
fn exec_is_builtin(it: &mut Interp, re: &Val) -> Result<bool, Ctl> {
    if !matches!(re, Val::Obj(o) if matches!(o.borrow().kind, Kind::RegExp(..))) { return Ok(false); }
    let e = it.get(re, "exec")?;
    Ok(matches!(&e, Val::Obj(o) if matches!(&o.borrow().kind, Kind::Native(_, "exec"))))
}
/// RegExpExec (§22.2.7.1): a user-supplied exec is honoured.
fn rx_exec(it: &mut Interp, re: &Val, s: &Rc<str>) -> Result<Val, Ctl> {
    let e = it.get(re, "exec")?;
    if e.is_callable() && !matches!(&e, Val::Obj(o) if matches!(&o.borrow().kind, Kind::Native(_, "exec"))) {
        let r = it.call(&e, re.clone(), vec![Val::Str(s.clone())])?;
        if !matches!(r, Val::Obj(_) | Val::Null) { return Err(throw(it, "TypeError", "exec result must be an object or null")); }
        return Ok(r);
    }
    if !matches!(re, Val::Obj(o) if matches!(o.borrow().kind, Kind::RegExp(..))) { return Err(throw(it, "TypeError", "RegExp exec method called on an incompatible receiver")); }
    crate::jsi_re::exec(it, s, re)
}
fn to_len(it: &mut Interp, v: &Val) -> Result<usize, Ctl> { let n = it.to_num(v)?; Ok(if n.is_nan() || n <= 0.0 { 0 } else { n.min(9007199254740991.0) as usize }) }
/// AdvanceStringIndex after an empty match (§22.2.7.3).
fn bump_last_index(it: &mut Interp, re: &Val) -> Result<(), Ctl> { let li = it.get(re, "lastIndex")?; let n = to_len(it, &li)?; it.set(re, "lastIndex", Val::Num((n + 1) as f64)) }
/// GetSubstitution (§22.1.3.19.1) over UTF-16 units.
fn get_substitution(it: &mut Interp, matched: &[u16], s: &[u16], pos: usize, caps: &[Val], groups: &Val, tpl: &[u16]) -> Result<Vec<u16>, Ctl> {
    let mut o = Vec::new(); let mut j = 0;
    let d = |c: u16| if (b'0' as u16..=b'9' as u16).contains(&c) { Some((c - b'0' as u16) as usize) } else { None };
    while j < tpl.len() {
        if tpl[j] == b'$' as u16 && j + 1 < tpl.len() {
            let c = tpl[j + 1];
            if c == b'$' as u16 { o.push(c); j += 2; continue; }
            if c == b'&' as u16 { o.extend_from_slice(matched); j += 2; continue; }
            if c == b'`' as u16 { o.extend_from_slice(&s[..pos.min(s.len())]); j += 2; continue; }
            if c == b'\'' as u16 { let e = (pos + matched.len()).min(s.len()); o.extend_from_slice(&s[e..]); j += 2; continue; }
            if let Some(d1) = d(c) {
                let two = tpl.get(j + 2).and_then(|&x| d(x)).map(|d2| d1 * 10 + d2).filter(|&n| n >= 1 && n <= caps.len());
                let (n, w) = match two { Some(n) => (n, 3), None => (d1, 2) };
                if n >= 1 && n <= caps.len() { if !matches!(caps[n - 1], Val::Undef) { let cs = it.to_str(&caps[n - 1])?; o.extend(utf16(&cs)); } j += w; continue; }
            }
            if c == b'<' as u16 && !matches!(groups, Val::Undef) {
                if let Some(e) = tpl[j + 2..].iter().position(|&x| x == b'>' as u16) { let name = from16(&tpl[j + 2..j + 2 + e]); let v = it.get(groups, &name)?; if !matches!(v, Val::Undef) { let vs = it.to_str(&v)?; o.extend(utf16(&vs)); } j += 3 + e; continue; }
            }
        }
        o.push(tpl[j]); j += 1;
    }
    Ok(o)
}
/// RegExp.prototype[@@match] / [@@matchAll] / [@@replace] / [@@search] / [@@split] (§22.2.6): our own exec on a plain
/// RegExp takes the native fast path; a replaced exec (or a species constructor for split) runs the spec algorithm.
fn regexp_symbol_methods(rp: &ORef) {
    Interp::define(rp, "\u{0}@@match", nat!("[Symbol.match]", |it, this, a| {
        let s = it.to_str(&arg(&a, 0))?;
        if exec_is_builtin(it, &this)? { return crate::jsi_re::match_(it, &s, &this); }
        let f = it.get(&this, "flags")?; let fs = it.to_str(&f)?;
        if !fs.contains('g') { return rx_exec(it, &this, &s); }
        it.set(&this, "lastIndex", Val::Num(0.0))?;
        let mut out = Vec::new();
        loop { let r = rx_exec(it, &this, &s)?; if matches!(r, Val::Null) { break; } let m0 = it.get(&r, "0")?; let m = it.to_str(&m0)?; if m.is_empty() { bump_last_index(it, &this)?; } out.push(Val::Str(m)); }
        Ok(if out.is_empty() { Val::Null } else { Val::array(out) })
    }), A_NOENUM);
    Interp::define(rp, "\u{0}@@matchAll", nat!("[Symbol.matchAll]", |it, this, a| {
        let s = it.to_str(&arg(&a, 0))?;
        if exec_is_builtin(it, &this)? { let all = crate::jsi_re::match_all(it, &s, &this)?; return Ok(new_iter(IterSt::List(arr_of(&all), 0))); }
        let f = it.get(&this, "flags")?; let fs = it.to_str(&f)?; let g = fs.contains('g');
        let mut out = Vec::new();
        loop { let r = rx_exec(it, &this, &s)?; if matches!(r, Val::Null) { break; } let m0 = it.get(&r, "0")?; let empty = it.to_str(&m0)?.is_empty(); out.push(r); if !g { break; } if empty { bump_last_index(it, &this)?; } }
        Ok(new_iter(IterSt::List(out, 0)))
    }), A_NOENUM);
    Interp::define(rp, "\u{0}@@search", nat!("[Symbol.search]", |it, this, a| {
        let s = it.to_str(&arg(&a, 0))?;
        if exec_is_builtin(it, &this)? { return crate::jsi_re::search(it, &s, &this); }
        let prev = it.get(&this, "lastIndex")?;
        it.set(&this, "lastIndex", Val::Num(0.0))?;
        let r = rx_exec(it, &this, &s)?;
        it.set(&this, "lastIndex", prev)?;
        if matches!(r, Val::Null) { Ok(Val::Num(-1.0)) } else { it.get(&r, "index") }
    }), A_NOENUM);
    Interp::define(rp, "\u{0}@@replace", nat!("[Symbol.replace]", |it, this, a| {
        let s = it.to_str(&arg(&a, 0))?; let rep = arg(&a, 1);
        if exec_is_builtin(it, &this)? { return crate::jsi_re::replace(it, &s, &this, &rep); }
        let f = it.get(&this, "flags")?; let fs = it.to_str(&f)?; let global = fs.contains('g');
        if global { it.set(&this, "lastIndex", Val::Num(0.0))?; }
        let mut results = Vec::new();
        loop { let r = rx_exec(it, &this, &s)?; if matches!(r, Val::Null) { break; } results.push(r.clone()); if !global { break; } let m0 = it.get(&r, "0")?; if it.to_str(&m0)?.is_empty() { bump_last_index(it, &this)?; } }
        let su = utf16(&s); let tpl = if rep.is_callable() { None } else { Some(utf16(&it.to_str(&rep)?)) };
        let mut out: Vec<u16> = Vec::new(); let mut next = 0usize;
        for r in results {
            let lv = it.get(&r, "length")?; let ncap = to_len(it, &lv)?.saturating_sub(1);
            let m0 = it.get(&r, "0")?; let matched = utf16(&it.to_str(&m0)?);
            let iv = it.get(&r, "index")?; let pos = (it.to_num(&iv)?.max(0.0) as usize).min(su.len());
            let mut caps = Vec::with_capacity(ncap);
            for n in 1..=ncap { let c = it.get(&r, &n.to_string())?; caps.push(if matches!(c, Val::Undef) { c } else { Val::Str(it.to_str(&c)?) }); }
            let groups = it.get(&r, "groups")?;
            let replacement = match &tpl {
                None => { let mut args = vec![st(&from16(&matched))]; args.extend(caps.iter().cloned()); args.push(Val::Num(pos as f64)); args.push(Val::Str(s.clone())); if !matches!(groups, Val::Undef) { args.push(groups.clone()); } let v = it.call(&rep, Val::Undef, args)?; utf16(&it.to_str(&v)?) }
                Some(t) => { let g = if matches!(groups, Val::Undef) { groups.clone() } else { Val::obj(Kind::Plain) }; let g = if matches!(groups, Val::Undef) { g } else { groups.clone() }; get_substitution(it, &matched, &su, pos, &caps, &g, t)? }
            };
            if pos >= next { out.extend_from_slice(&su[next..pos]); out.extend(replacement); next = pos + matched.len(); }
        }
        if next < su.len() { out.extend_from_slice(&su[next..]); }
        Ok(st(&from16(&out)))
    }), A_NOENUM);
    Interp::define(rp, "\u{0}@@split", nat!("[Symbol.split]", |it, this, a| {
        let s = it.to_str(&arg(&a, 0))?;
        let lim = match arg(&a, 1) { Val::Undef => u32::MAX as usize, v => crate::jsi::to_uint32(it.to_num(&v)?) as usize };
        // SpeciesConstructor(rx, %RegExp%) (§22.2.6.14 step 4)
        let g = it.global.clone();
        let re_ctor = it.lookup_opt(&g, "RegExp").unwrap_or(Val::Undef);
        let c = it.get(&this, "constructor")?;
        let c = if matches!(c, Val::Undef) { re_ctor.clone() } else { let sp = it.get(&c, "\u{0}@@species")?; if sp.is_nullish() { re_ctor.clone() } else { sp } };
        if Interp::strict_eq(&c, &re_ctor) && exec_is_builtin(it, &this)? {
            if lim == 0 { return Ok(Val::array(vec![])); }
            let v = crate::jsi_re::split(it, &s, &this)?;
            return Ok(Val::array(arr_of(&v).into_iter().take(lim).collect()));
        }
        let f = it.get(&this, "flags")?; let mut fs = it.to_str(&f)?.to_string(); if !fs.contains('y') { fs.push('y'); }
        let splitter = it.construct(&c, vec![this.clone(), st(&fs)])?;
        let su = utf16(&s); let size = su.len();
        let mut out = Vec::new();
        if lim == 0 { return Ok(Val::array(out)); }
        if size == 0 { let z = rx_exec(it, &splitter, &s)?; return Ok(Val::array(if matches!(z, Val::Null) { vec![Val::Str(s.clone())] } else { vec![] })); }
        let (mut p, mut q) = (0usize, 0usize);
        while q < size {
            it.set(&splitter, "lastIndex", Val::Num(q as f64))?;
            let z = rx_exec(it, &splitter, &s)?;
            if matches!(z, Val::Null) { q += 1; continue; }
            let li = it.get(&splitter, "lastIndex")?; let e = to_len(it, &li)?.min(size);
            if e == p { q += 1; continue; }
            out.push(st(&from16(&su[p..q]))); if out.len() == lim { return Ok(Val::array(out)); }
            p = e;
            let lv = it.get(&z, "length")?; let nc = to_len(it, &lv)?.saturating_sub(1);
            for i in 1..=nc { out.push(it.get(&z, &i.to_string())?); if out.len() == lim { return Ok(Val::array(out)); } }
            q = p;
        }
        out.push(st(&from16(&su[p..])));
        Ok(Val::array(out))
    }), A_NOENUM);
}
pub fn regexp_method(key: &str, _v: Val) -> Option<Val> {
    Some(match key {
        // RegExp.prototype.test (§22.2.6.16) = RegExpExec(R, S) !== null: a replaced exec is honoured
        "test" => nat!("test", |it, this, a| { let s = it.to_str(&arg(&a, 0))?; if !exec_is_builtin(it, &this)? { return Ok(Val::Bool(!matches!(rx_exec(it, &this, &s)?, Val::Null))); } crate::jsi_re::test(it, &this, &s) }),
        "exec" => nat!("exec", |it, this, a| { let s = it.to_str(&arg(&a, 0))?; crate::jsi_re::exec(it, &s, &this) }),
        "toString" => nat!("toString", |it, this, _a| { let s = it.get(&this, "source")?; let f = it.get(&this, "flags")?; Ok(st(&format!("/{}/{}", it.to_str(&s)?, it.to_str(&f)?))) }),
        "compile" => nat!("compile", |_it, this, _a| Ok(this)),
        _ => return None,
    })
}

/// Object.prototype.toString (§20.1.3.6): the builtin tag, or @@toStringTag.
pub fn object_to_string(it: &mut Interp, this: &Val) -> Result<Val, Ctl> {
    let builtin = match this {
        Val::Undef => return Ok(st("[object Undefined]")), Val::Null => return Ok(st("[object Null]")),
        Val::Num(_) => "Number", Val::Str(_) => "String", Val::Bool(_) => "Boolean", Val::Sym(_) => "Symbol", Val::Big(_) => "BigInt",
        Val::Obj(o) => match &o.borrow().kind {
            Kind::Array(_) => "Array", Kind::Func(_) | Kind::Native(..) => "Function", Kind::Error => "Error", Kind::Date(_) => "Date", Kind::RegExp(..) => "RegExp",
            Kind::Prim(Val::Num(_)) => "Number", Kind::Prim(Val::Str(_)) => "String", Kind::Prim(Val::Bool(_)) => "Boolean",
            Kind::Global => "global", Kind::Host(_, 4) => "Window", Kind::Host(..) => "HTMLElement", _ => "Object",
        },
    };
    let tag = it.get(this, K_TAG)?;
    Ok(st(&format!("[object {}]", match &tag { Val::Str(s) => s.to_string(), _ => builtin.to_string() })))
}
pub const OBJECT_METHODS: &[&str] = &["hasOwnProperty", "isPrototypeOf", "propertyIsEnumerable", "toString", "toLocaleString", "valueOf", "__defineGetter__", "__defineSetter__", "__lookupGetter__", "__lookupSetter__"];
pub fn object_method(key: &str, _v: Val) -> Option<Val> {
    Some(match key {
        "hasOwnProperty" => nat!("hasOwnProperty", |it, this, a| { let k = it.to_key(&arg(&a, 0))?; if this.is_nullish() { return Err(throw(it, "TypeError", "Cannot convert undefined or null to object")); } Ok(Val::Bool(it.has_own(&this, &k)?)) }),
        "isPrototypeOf" => nat!("isPrototypeOf", |it, this, a| { let Val::Obj(t) = &this else { return Ok(Val::Bool(false)) }; let v = arg(&a, 0); if !matches!(v, Val::Obj(_)) { return Ok(Val::Bool(false)); } let mut cur = it.proto_of(&v)?; while let Some(p) = cur { if Rc::ptr_eq(&p, t) { return Ok(Val::Bool(true)); } cur = it.proto_of(&Val::Obj(p))?; } Ok(Val::Bool(false)) }),
        "propertyIsEnumerable" => nat!("propertyIsEnumerable", |it, this, a| { let k = it.to_key(&arg(&a, 0))?; Ok(Val::Bool(match it.own_prop(&this, &k)? { Some((_, at)) => at & A_NOENUM == 0, None => false })) }),
        "toString" => nat!("toString", |it, this, _a| object_to_string(it, &this)),
        "toLocaleString" => nat!("toLocaleString", |it, this, _a| { let f = it.get(&this, "toString")?; it.call(&f, this, vec![]) }),
        "valueOf" => nat!("valueOf", |it, this, _a| { if this.is_nullish() { return Err(throw(it, "TypeError", "Cannot convert undefined or null to object")); } Ok(this) }),
        "__defineGetter__" => nat!("__defineGetter__", |it, this, a| { let k = it.to_key(&arg(&a, 0))?; if let Val::Obj(o) = &this { Interp::define_accessor(o, &k, Some(arg(&a, 1)), None, 0); } Ok(Val::Undef) }),
        "__defineSetter__" => nat!("__defineSetter__", |it, this, a| { let k = it.to_key(&arg(&a, 0))?; if let Val::Obj(o) = &this { Interp::define_accessor(o, &k, None, Some(arg(&a, 1)), 0); } Ok(Val::Undef) }),
        "__lookupGetter__" | "__lookupSetter__" => { let g = key == "__lookupGetter__"; nat!("__lookupGetter__", |it, this, a| { let k = it.to_key(&arg(&a, 0))?; let mut cur = Some(this.clone()); while let Some(c) = cur { if let Some((Err((gg, ss)), _)) = it.own_prop(&c, &k)? { return Ok(if g { gg } else { ss }); } cur = it.proto_of(&c).ok().flatten().map(Val::Obj); } Ok(Val::Undef) }) }
        _ => return None,
    })
}
/// `performance.now()` — milliseconds since the time origin, monotonic non-decreasing.
///
/// The first cut returned `(as_micros() % 1_000_000_000) / 1000.0`: microseconds
/// labelled as milliseconds, **wrapping every second**. Every `now - t0` delta in script
/// was therefore wrong, and one spanning a second boundary went negative — which is how
/// the launcher streaming throttle (`now - r.last >= 80`, deciding whether to re-render a
/// message) mis-fired. Anchor an `Instant` once per process and report true elapsed ms.
fn perf_now_ms() -> f64 {
    if crate::jsi_date::vclock() { return crate::jsi_date::vclock_ms(); }
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// JS Number.prototype.toFixed: decimal expansion of the exact double, ties rounded up in magnitude.
pub fn to_fixed(x: f64, d: usize) -> String {
    if !x.is_finite() { return num_str(x); }
    if x.abs() >= 1e21 { return num_str(x); }
    let neg = x < 0.0;
    let exact = format!("{:.1100}", x.abs());
    let (ip, fp) = exact.split_once('.').unwrap();
    let mut digits: Vec<u8> = ip.bytes().chain(fp.bytes().take(d)).map(|b| b - b'0').collect();
    let rest: Vec<u8> = fp.bytes().skip(d).map(|b| b - b'0').collect();
    let round_up = rest.first().map_or(false, |&r| r >= 5);
    if round_up {
        let mut i = digits.len();
        loop { if i == 0 { digits.insert(0, 1); break; } i -= 1; if digits[i] == 9 { digits[i] = 0; } else { digits[i] += 1; break; } }
    }
    let il = digits.len() - d;
    let mut s: String = digits[..il].iter().map(|d| (b'0' + d) as char).collect();
    if s.is_empty() { s.push('0'); }
    if d > 0 { s.push('.'); s.extend(digits[il..].iter().map(|d| (b'0' + d) as char)); }
    let zero = s.bytes().all(|b| b == b'0' || b == b'.');
    if neg && !zero { format!("-{s}") } else { s }
}
fn json_quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() { match c { '"' => o += "\\\"", '\\' => o += "\\\\", '\n' => o += "\\n", '\r' => o += "\\r", '\t' => o += "\\t", '\u{8}' => o += "\\b", '\u{c}' => o += "\\f", c if (c as u32) < 0x20 => o += &format!("\\u{:04x}", c as u32), c if (0xF0000 + 0xD800..=0xF0000 + 0xDFFF).contains(&(c as u32)) => o += &format!("\\u{:04x}", c as u32 - 0xF0000), c => o.push(c) } }
    o.push('"'); o
}

struct JP<'a> { b: &'a [u8], i: usize }
impl JP<'_> {
    fn ws(&mut self) { while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') { self.i += 1; } }
    fn val(&mut self) -> Option<Val> {
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => { self.i += 1; let o = Val::obj(Kind::Plain); self.ws(); if self.b.get(self.i) == Some(&b'}') { self.i += 1; return Some(o); }
                loop { self.ws(); let k = match self.val()? { Val::Str(s) => s, _ => return None }; self.ws(); if self.b.get(self.i) != Some(&b':') { return None; } self.i += 1; let v = self.val()?;
                    if let Val::Obj(r) = &o { let mut b = r.borrow_mut(); if let Some(p) = b.props.iter_mut().find(|(kk, _)| *kk == k) { p.1 = v; } else { b.props.push((k, v)); } }
                    self.ws(); match self.b.get(self.i)? { b',' => self.i += 1, b'}' => { self.i += 1; return Some(o); } _ => return None } } }
            b'[' => { self.i += 1; let mut v = Vec::new(); self.ws(); if self.b.get(self.i) == Some(&b']') { self.i += 1; return Some(Val::array(v)); }
                loop { v.push(self.val()?); self.ws(); match self.b.get(self.i)? { b',' => self.i += 1, b']' => { self.i += 1; return Some(Val::array(v)); } _ => return None } } }
            b'"' => { self.i += 1; let mut out: Vec<u16> = Vec::new();
                loop { let c = *self.b.get(self.i)?; self.i += 1; match c {
                    b'"' => return Some(st(&crate::jsi::str_of16(&out))),
                    b'\\' => { let e = *self.b.get(self.i)?; self.i += 1; match e { b'n' => out.push(10), b't' => out.push(9), b'r' => out.push(13), b'b' => out.push(8), b'f' => out.push(12), b'u' => { let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?; out.push(u16::from_str_radix(h, 16).ok()?); self.i += 4; } x => out.push(x as u16) } }
                    x if x < 0x80 => out.push(x as u16),
                    _ => { let start = self.i - 1; let mut end = self.i; while end < self.b.len() && (self.b[end] & 0xC0) == 0x80 { end += 1; } out.extend(std::str::from_utf8(&self.b[start..end]).ok()?.encode_utf16()); self.i = end; }
                } } }
            b't' if self.b[self.i..].starts_with(b"true") => { self.i += 4; Some(Val::Bool(true)) }
            b'f' if self.b[self.i..].starts_with(b"false") => { self.i += 5; Some(Val::Bool(false)) }
            b'n' if self.b[self.i..].starts_with(b"null") => { self.i += 4; Some(Val::Null) }
            _ => { let s = self.i; while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') { self.i += 1; } std::str::from_utf8(&self.b[s..self.i]).ok()?.parse::<f64>().ok().map(Val::Num) }
        }
    }
}

pub fn json_parse(s: &str) -> Option<Val> { let mut p = JP { b: s.as_bytes(), i: 0 }; let v = p.val()?; p.ws(); if p.i == p.b.len() { Some(v) } else { None } }

// ── JSON (§25.5) ──
struct JsonCtx { replacer: Option<Val>, allow: Option<Vec<Rc<str>>>, gap: String, stack: Vec<usize> }
/// SerializeJSONProperty (§25.5.2.2): toJSON, the replacer function, then by type.
fn ser_prop(it: &mut Interp, holder: &Val, key: &str, v: Val, cx: &mut JsonCtx, indent: &str) -> Result<Option<String>, Ctl> {
    let mut v = v;
    if matches!(v, Val::Obj(_) | Val::Big(_)) { let tj = it.get(&v, "toJSON")?; if tj.is_callable() { v = it.call(&tj, v.clone(), vec![st(key)])?; } }
    if let Some(r) = cx.replacer.clone() { v = it.call(&r, holder.clone(), vec![st(key), v])?; }
    if let Val::Obj(o) = &v { let p = match &o.borrow().kind { Kind::Prim(p) => Some(p.clone()), _ => None }; if let Some(p) = p { v = match p { Val::Num(_) => Val::Num(it.to_num(&v)?), Val::Str(_) => Val::Str(it.to_str(&v)?), other => other }; } }
    Ok(Some(match &v {
        Val::Undef | Val::Sym(_) => return Ok(None), Val::Null => "null".into(), Val::Bool(b) => b.to_string(),
        Val::Num(n) => if n.is_finite() { num_str(*n) } else { "null".into() },
        Val::Str(s) => json_quote(s),
        Val::Big(_) => return Err(throw(it, "TypeError", "Do not know how to serialize a BigInt")),
        Val::Obj(o) => {
            if v.is_callable() { return Ok(None); }
            let id = Rc::as_ptr(o) as *const u8 as usize;
            if cx.stack.contains(&id) { return Err(throw(it, "TypeError", "Converting circular structure to JSON")); }
            cx.stack.push(id);
            let is_arr = matches!(o.borrow().kind, Kind::Array(_));
            let inner = format!("{indent}{}", cx.gap);
            let mut parts = Vec::new();
            if is_arr {
                let items = arr_of(&v);
                for (i, x) in items.into_iter().enumerate() { parts.push(ser_prop(it, &v, &i.to_string(), x, cx, &inner)?.unwrap_or_else(|| "null".into())); }
            } else {
                let keys = match &cx.allow { Some(a) => a.clone(), None => it.keys(&v) };
                for k in keys { let x = it.get(&v, &k)?; if let Some(s) = ser_prop(it, &v, &k, x, cx, &inner)? { parts.push(format!("{}:{}{s}", json_quote(&k), if cx.gap.is_empty() { "" } else { " " })); } }
            }
            cx.stack.pop();
            let (open, close) = if is_arr { ('[', ']') } else { ('{', '}') };
            if parts.is_empty() { format!("{open}{close}") }
            else if cx.gap.is_empty() { format!("{open}{}{close}", parts.join(",")) }
            else { format!("{open}\n{inner}{}\n{indent}{close}", parts.join(&format!(",\n{inner}"))) }
        }
    }))
}
/// JSON.stringify(value, replacer, space) (§25.5.2).
pub fn json_stringify(it: &mut Interp, v: &Val, replacer: &Val, space: &Val) -> Result<Option<String>, Ctl> {
    let mut cx = JsonCtx { replacer: None, allow: None, gap: String::new(), stack: Vec::new() };
    if replacer.is_callable() { cx.replacer = Some(replacer.clone()); }
    else if let Val::Obj(o) = replacer { if matches!(o.borrow().kind, Kind::Array(_)) { let mut ks: Vec<Rc<str>> = Vec::new(); for x in arr_of(replacer) { let k = match &x { Val::Str(s) => Some(s.clone()), Val::Num(_) => Some(it.to_str(&x)?), Val::Obj(xo) if matches!(xo.borrow().kind, Kind::Prim(Val::Str(_) | Val::Num(_))) => Some(it.to_str(&x)?), _ => None }; if let Some(k) = k { if !ks.contains(&k) { ks.push(k); } } } cx.allow = Some(ks); } }
    let space = match space { Val::Obj(o) => { let p = match &o.borrow().kind { Kind::Prim(p) => Some(p.clone()), _ => None }; p.unwrap_or(Val::Undef) } s => s.clone() };
    cx.gap = match &space { Val::Num(n) => " ".repeat(n.clamp(0.0, 10.0) as usize), Val::Str(s) => s.chars().take(10).collect(), _ => String::new() };
    let holder = Val::obj(Kind::Plain);
    if let Val::Obj(h) = &holder { Interp::put(h, Rc::from(""), v.clone()); }
    ser_prop(it, &holder, "", v.clone(), &mut cx, "")
}
fn stringify(it: &mut Interp, v: &Val, indent: &str, _cur: &str, _depth: usize) -> Result<Option<String>, Ctl> { json_stringify(it, v, &Val::Undef, &st(indent)) }
/// InternalizeJSONProperty (§25.5.1.1): the reviver walks the parsed tree bottom-up.
fn revive(it: &mut Interp, holder: &Val, key: &str, f: &Val) -> Result<Val, Ctl> {
    let v = it.get(holder, key)?;
    if let Val::Obj(o) = &v {
        let is_arr = matches!(o.borrow().kind, Kind::Array(_));
        let keys: Vec<Rc<str>> = if is_arr { (0..arr_of(&v).len()).map(|i| Rc::from(i.to_string().as_str())).collect() } else { it.keys(&v) };
        for k in keys { let nv = revive(it, &v, &k, f)?; if matches!(nv, Val::Undef) { it.delete(&v, &k)?; } else { Interp::put(o, k, nv); } }
    }
    it.call(f, holder.clone(), vec![st(key), v])
}

/// node-style inspect for console.log arguments (strings raw at top level)
pub fn inspect(it: &mut Interp, v: &Val, top: bool, depth: usize) -> String {
    match v {
        Val::Str(s) => if top { s.to_string() } else { format!("'{}'", s.replace('\'', "\\'")) },
        Val::Sym(s) => format!("Symbol({})", s.desc.as_deref().unwrap_or("")),
        Val::Big(b) => format!("{b}n"),
        Val::Obj(o) if depth < 3 => {
            if v.is_callable() {
                let (nm, cls) = match &o.borrow().kind { Kind::Func(c) => (if c.name.is_empty() { "(anonymous)".to_string() } else { c.name.clone() }, c.cls.is_some()), Kind::Native(_, n) => (n.to_string(), false), _ => (String::new(), false) };
                return if cls { format!("[class {nm}]") } else { format!("[Function: {nm}]") };
            }
            enum K { Arr, Err, Map, Set, Date(f64), Re, Prim(Val), Prom(u8, Val), Sym, Other }
            let k = match &o.borrow().kind { Kind::Array(_) => K::Arr, Kind::Error => K::Err, Kind::Map(_) => K::Map, Kind::Set(_) => K::Set, Kind::Date(t) => K::Date(*t), Kind::RegExp(..) => K::Re, Kind::Prim(p) => K::Prim(p.clone()), Kind::Promise(s) => K::Prom(s.st, s.v.clone()), Kind::Typed(_) => K::Sym, _ => K::Other };
            match k {
                K::Err => return it.to_str(v).map(|x| x.to_string()).unwrap_or_default(),
                K::Arr => { let xs: Vec<String> = arr_of(v).iter().map(|x| inspect(it, x, false, depth + 1)).collect(); return if xs.is_empty() { "[]".into() } else { format!("[ {} ]", xs.join(", ")) }; }
                K::Map => { let es: Vec<(Val, Val)> = if let Kind::Map(m) = &o.borrow().kind { m.entries().cloned().collect() } else { vec![] }; let xs: Vec<String> = es.iter().map(|(a, b)| format!("{} => {}", inspect(it, a, false, depth + 1), inspect(it, b, false, depth + 1))).collect(); return format!("Map({}) {{{}}}", xs.len(), if xs.is_empty() { String::new() } else { format!(" {} ", xs.join(", ")) }); }
                K::Set => { let es: Vec<Val> = if let Kind::Set(m) = &o.borrow().kind { m.entries().map(|e| e.0.clone()).collect() } else { vec![] }; let xs: Vec<String> = es.iter().map(|a| inspect(it, a, false, depth + 1)).collect(); return format!("Set({}) {{{}}}", xs.len(), if xs.is_empty() { String::new() } else { format!(" {} ", xs.join(", ")) }); }
                K::Date(t) => return if t.is_nan() { "Invalid Date".into() } else { let f = it.get(v, "toISOString").unwrap_or(Val::Undef); it.call(&f, v.clone(), vec![]).ok().and_then(|s| if let Val::Str(s) = s { Some(s.to_string()) } else { None }).unwrap_or_default() },
                K::Re => return it.to_str(v).map(|x| x.to_string()).unwrap_or_default(),
                K::Prim(p) => return format!("[{}: {}]", match p { Val::Num(_) => "Number", Val::Str(_) => "String", Val::Bool(_) => "Boolean", _ => "Object" }, inspect(it, &p, false, depth + 1)),
                K::Prom(s, x) => return format!("Promise {{ {} }}", match s { 0 => "<pending>".to_string(), 1 => inspect(it, &x, false, depth + 1), _ => format!("<rejected> {}", inspect(it, &x, false, depth + 1)) }),
                K::Sym => { let xs: Vec<String> = it.iterate(v).unwrap_or_default().iter().map(|x| inspect(it, x, false, depth + 1)).collect(); return format!("[ {} ]", xs.join(", ")); }
                K::Other => {}
            }
            let keys = it.keys(v);
            let xs: Vec<String> = keys.iter().map(|k| { let x = it.get(v, k).unwrap_or(Val::Undef); let kk = if k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') && !k.is_empty() && !k.starts_with(|c: char| c.is_ascii_digit()) { k.to_string() } else { format!("'{k}'") }; format!("{kk}: {}", inspect(it, &x, false, depth + 1)) }).collect();
            let cname = it.get(v, "constructor").ok().and_then(|c| if let Val::Obj(co) = &c { if let Kind::Func(cl) = &co.borrow().kind { Some(cl.name.clone()) } else { None } } else { None }).filter(|n| !n.is_empty() && n != "Object");
            let body = if xs.is_empty() { "{}".into() } else { format!("{{ {} }}", xs.join(", ")) };
            match cname { Some(n) => format!("{n} {body}"), None => body }
        }
        Val::Obj(o) => if matches!(o.borrow().kind, Kind::Array(_)) { "[Array]".into() } else { "[Object]".into() },
        Val::Num(n) => if *n == 0.0 && n.is_sign_negative() { "-0".into() } else { num_str(*n) },
        _ => it.to_str(v).map(|x| x.to_string()).unwrap_or_default(),
    }
}

fn parse_int(s: &str, radix: u32) -> f64 {
    let t = s.trim_start_matches(crate::jsi::es_ws);
    let (neg, t) = match t.as_bytes().first() { Some(b'-') => (true, &t[1..]), Some(b'+') => (false, &t[1..]), _ => (false, t) };
    let (mut r, mut t) = (radix, t);
    if (r == 0 || r == 16) && (t.starts_with("0x") || t.starts_with("0X")) { t = &t[2..]; r = 16; }
    if r == 0 { r = 10; }
    if !(2..=36).contains(&r) { return f64::NAN; }
    let digits: String = t.chars().take_while(|c| c.is_digit(r)).collect();
    if digits.is_empty() { return f64::NAN; }
    let v = if r == 10 { digits.parse::<f64>().unwrap_or(f64::NAN) } else { let mut v = 0f64; for c in digits.chars() { v = v * r as f64 + c.to_digit(r).unwrap() as f64; } v };
    if neg { -v } else { v }
}

fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(crate::jsi::es_ws);
    for p in ["Infinity", "+Infinity"] { if t.starts_with(p) { return f64::INFINITY; } }
    if t.starts_with("-Infinity") { return f64::NEG_INFINITY; }
    let b = t.as_bytes(); let mut i = 0; let mut seen_digit = false; let mut seen_dot = false; let mut end = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') { i += 1; }
    while i < b.len() { match b[i] { b'0'..=b'9' => { seen_digit = true; i += 1; end = i; } b'.' if !seen_dot => { seen_dot = true; i += 1; if seen_digit { end = i; } } b'e' | b'E' if seen_digit => { let mut j = i + 1; if j < b.len() && (b[j] == b'+' || b[j] == b'-') { j += 1; } let s0 = j; while j < b.len() && b[j].is_ascii_digit() { j += 1; } if j > s0 { end = j; } break; } _ => break } }
    if !seen_digit { return f64::NAN; }
    t[..end].trim_end_matches('.').parse::<f64>().or_else(|_| t[..end].parse::<f64>()).unwrap_or(f64::NAN)
}

// ── URI handling (§19.2.6) ──
/// Encode (§19.2.6.5): UTF-8 bytes of every code point outside the unescaped set; a lone surrogate is a URIError.
fn uri_encode(s: &str, keep: &str) -> Option<String> {
    let mut o = String::new();
    for c in s.chars() {
        if (0xF0000 + 0xD800..=0xF0000 + 0xDFFF).contains(&(c as u32)) { return None; }
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) || keep.contains(c) { o.push(c); } else { let mut b = [0u8; 4]; for x in c.encode_utf8(&mut b).bytes() { o += &format!("%{x:02X}"); } }
    }
    Some(o)
}
/// Decode (§19.2.6.6): %XX sequences forming UTF-8, except escapes of characters in `reserved`.
fn uri_decode(s: &str, reserved: &str) -> Option<String> {
    let b = s.as_bytes(); let mut out: Vec<u8> = Vec::new(); let mut i = 0;
    let hex = |i: usize| -> Option<u8> { u8::from_str_radix(std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?, 16).ok() };
    while i < b.len() {
        if b[i] != b'%' { out.push(b[i]); i += 1; continue; }
        let x = hex(i)?;
        if x < 0x80 { if reserved.as_bytes().contains(&x) { out.extend_from_slice(&b[i..i + 3]); } else { out.push(x); } i += 3; continue; }
        let n = if x & 0xE0 == 0xC0 { 2 } else if x & 0xF0 == 0xE0 { 3 } else if x & 0xF8 == 0xF0 { 4 } else { return None };
        let mut seq = vec![x]; i += 3;
        for _ in 1..n { if b.get(i) != Some(&b'%') { return None; } let y = hex(i)?; if y & 0xC0 != 0x80 { return None; } seq.push(y); i += 3; }
        std::str::from_utf8(&seq).ok()?; out.extend(seq);
    }
    String::from_utf8(out).ok()
}
/// escape / unescape (Annex B.2.1).
fn js_escape(s: &str) -> String { let mut o = String::new(); for u in utf16(s) { let c = u as u32; if c < 128 && ((c as u8).is_ascii_alphanumeric() || b"@*_+-./".contains(&(c as u8))) { o.push(c as u8 as char); } else if c < 256 { o += &format!("%{c:02X}"); } else { o += &format!("%u{c:04X}"); } } o }
fn js_unescape(s: &str) -> String {
    let u = utf16(s); let mut o: Vec<u16> = Vec::new(); let mut i = 0;
    let hexn = |a: &[u16]| -> Option<u16> { let t: String = a.iter().map(|&c| c as u8 as char).collect(); u16::from_str_radix(&t, 16).ok() };
    while i < u.len() { if u[i] == b'%' as u16 { if i + 5 < u.len() + 0 && u[i + 1] == b'u' as u16 { if let Some(v) = u.get(i + 2..i + 6).and_then(hexn) { o.push(v); i += 6; continue; } } if let Some(v) = u.get(i + 1..i + 3).and_then(hexn) { o.push(v); i += 3; continue; } } o.push(u[i]); i += 1; }
    from16(&o)
}

// ── base64 (HTML §8.3 atob / btoa, forgiving-base64) ──
const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut o = String::new();
    for c in bytes.chunks(3) { let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32; for k in 0..4 { if k <= c.len() { o.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char); } else { o.push('='); } } }
    o
}
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut t: Vec<u8> = s.bytes().filter(|b| !b" \t\n\x0c\r".contains(b)).collect();
    if t.len() % 4 == 0 { if t.ends_with(b"==") { t.truncate(t.len() - 2); } else if t.ends_with(b"=") { t.pop(); } }
    if t.len() % 4 == 1 || t.iter().any(|b| !B64.contains(b)) { return None; }
    let mut out = Vec::new(); let mut acc = 0u32; let mut bits = 0;
    for b in t { acc = acc << 6 | B64.iter().position(|x| *x == b).unwrap() as u32; bits += 6; if bits >= 8 { bits -= 8; out.push((acc >> bits) as u8); acc &= (1 << bits) - 1; } }
    Some(out)
}

// ── typed arrays (§23.2), ArrayBuffer (§25.1), DataView (§25.3) ──
pub const TNAMES: [&str; 11] = ["Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array", "Uint16Array", "Int32Array", "Uint32Array", "Float32Array", "Float64Array", "BigInt64Array", "BigUint64Array"];
pub fn elem_size(ty: u8) -> usize { match ty { 0..=2 => 1, 3 | 4 => 2, 5..=7 => 4, _ => 8 } }
fn read_elem(b: &[u8], ty: u8, little: bool) -> Val {
    let mut x = [0u8; 8]; let n = elem_size(ty); x[..n].copy_from_slice(&b[..n]); if !little { x[..n].reverse(); }
    match ty {
        0 => Val::Num(x[0] as i8 as f64), 1 | 2 => Val::Num(x[0] as f64), 3 => Val::Num(i16::from_le_bytes([x[0], x[1]]) as f64), 4 => Val::Num(u16::from_le_bytes([x[0], x[1]]) as f64),
        5 => Val::Num(i32::from_le_bytes([x[0], x[1], x[2], x[3]]) as f64), 6 => Val::Num(u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as f64), 7 => Val::Num(f32::from_le_bytes([x[0], x[1], x[2], x[3]]) as f64),
        8 => Val::Num(f64::from_le_bytes(x)), 9 => Val::Big(Rc::new(Big::from_i64(i64::from_le_bytes(x)))), _ => Val::Big(Rc::new(Big::from_u128(u64::from_le_bytes(x) as u128, false))),
    }
}
/// The element's bytes (little-endian) for a value already converted to Number (or BigInt for the 64-bit kinds).
fn elem_bytes(ty: u8, v: &Val) -> [u8; 8] {
    let n = match v { Val::Num(n) => *n, _ => 0.0 };
    let mut o = [0u8; 8];
    match ty {
        0 | 1 => o[0] = crate::jsi::to_int32(n) as u8,
        2 => o[0] = if n.is_nan() { 0 } else { let c = n.clamp(0.0, 255.0); let f = c.floor(); (if c - f > 0.5 || (c - f == 0.5 && f % 2.0 != 0.0) { f + 1.0 } else { f }) as u8 },
        3 | 4 => o[..2].copy_from_slice(&(crate::jsi::to_int32(n) as u16).to_le_bytes()),
        5 | 6 => o[..4].copy_from_slice(&crate::jsi::to_uint32(n).to_le_bytes()),
        7 => o[..4].copy_from_slice(&(n as f32).to_le_bytes()), 8 => o = n.to_le_bytes(),
        _ => { let b = match v { Val::Big(b) => b.to_i128_wrapping(), _ => 0 }; o = (b as i64).to_le_bytes(); }
    }
    o
}
pub fn typed_get(t: &TArr, i: usize) -> Val { if i >= t.len { return Val::Undef; } let b = t.buf.borrow(); let es = elem_size(t.ty); let p = t.off + i * es; if p + es > b.len() { return Val::Undef; } read_elem(&b[p..p + es], t.ty, true) }
pub fn typed_set(t: &TArr, i: usize, v: &Val) { if i >= t.len { return; } let mut b = t.buf.borrow_mut(); let es = elem_size(t.ty); let p = t.off + i * es; if p + es > b.len() { return; } let x = elem_bytes(t.ty, v); b[p..p + es].copy_from_slice(&x[..es]); }
fn ta_of(it: &Interp, this: &Val) -> Result<(Rc<std::cell::RefCell<Vec<u8>>>, Val, u8, usize, usize), Ctl> {
    if let Val::Obj(o) = this { if let Kind::Typed(t) = &o.borrow().kind { return Ok((t.buf.clone(), t.bufobj.clone(), t.ty, t.off, t.len)); } }
    Err(throw(it, "TypeError", "this is not a typed array."))
}
pub fn new_buffer(n: usize) -> Val { Val::obj(Kind::Buffer(Rc::new(std::cell::RefCell::new(vec![0u8; n])))) }
fn buf_rc(v: &Val) -> Option<Rc<std::cell::RefCell<Vec<u8>>>> { if let Val::Obj(o) = v { if let Kind::Buffer(b) = &o.borrow().kind { return Some(b.clone()); } } None }
/// A typed array of `ty` over `bufobj` (or a fresh buffer of `len` elements).
pub fn make_typed(ty: u8, bufobj: Option<Val>, off: usize, len: usize) -> Val {
    let bufobj = bufobj.unwrap_or_else(|| new_buffer(len * elem_size(ty)));
    let buf = buf_rc(&bufobj).unwrap_or_default();
    Val::obj(Kind::Typed(Box::new(TArr { buf, bufobj, ty, off, len })))
}
fn conv_elem(it: &mut Interp, ty: u8, v: &Val) -> Result<Val, Ctl> {
    if ty >= 9 { return match v { Val::Big(_) => Ok(v.clone()), Val::Bool(b) => Ok(Val::Big(Rc::new(Big::from_i64(*b as i64)))), Val::Str(s) => Big::parse(s).map(|b| Val::Big(Rc::new(b))).ok_or_else(|| throw(it, "SyntaxError", "Cannot convert to BigInt")), _ => Err(throw(it, "TypeError", "Cannot convert to a BigInt")) }; }
    Ok(Val::Num(it.to_num(v)?))
}
/// `new Uint8Array(length | typedArray | iterable | arrayLike | buffer[, byteOffset[, length]])` (§23.2.5.1).
fn typed_ctor(it: &mut Interp, ty: u8, a: &[Val]) -> Result<Val, Ctl> {
    let es = elem_size(ty);
    let a0 = arg(a, 0);
    match &a0 {
        Val::Undef | Val::Null => Ok(make_typed(ty, None, 0, 0)),
        Val::Obj(o) => {
            let is_buf = matches!(o.borrow().kind, Kind::Buffer(_));
            if is_buf {
                let blen = buf_rc(&a0).map_or(0, |b| b.borrow().len());
                let off = match arg(a, 1) { Val::Undef => 0, v => it.to_num(&v)?.max(0.0) as usize };
                if off % es != 0 { return Err(throw(it, "RangeError", &format!("start offset of {} should be a multiple of {es}", TNAMES[ty as usize]))); }
                let len = match arg(a, 2) { Val::Undef => { if (blen - off.min(blen)) % es != 0 { return Err(throw(it, "RangeError", &format!("byte length of {} should be a multiple of {es}", TNAMES[ty as usize]))); } (blen.saturating_sub(off)) / es } v => it.to_num(&v)?.max(0.0) as usize };
                if off + len * es > blen { return Err(throw(it, "RangeError", &format!("Invalid typed array length: {len}"))); }
                return Ok(make_typed(ty, Some(a0.clone()), off, len));
            }
            let items = it.iterate(&a0).or_else(|_| list_of(it, &a0))?;
            let t = make_typed(ty, None, 0, items.len());
            if let Val::Obj(to) = &t { if let Kind::Typed(ta) = &to.borrow().kind { for (i, x) in items.iter().enumerate() { let v = conv_elem(it, ty, x)?; typed_set(ta, i, &v); } } }
            Ok(t)
        }
        v => { let n = it.to_num(v)?; if n < 0.0 || n.is_nan() && !matches!(v, Val::Undef) || n > 2e9 { return Err(throw(it, "RangeError", &format!("Invalid typed array length: {}", num_str(n)))); } Ok(make_typed(ty, None, 0, if n.is_nan() { 0 } else { n as usize })) }
    }
}
fn typed_from_vals(it: &mut Interp, ty: u8, vals: &[Val]) -> Result<Val, Ctl> {
    let t = make_typed(ty, None, 0, vals.len());
    if let Val::Obj(to) = &t { if let Kind::Typed(ta) = &to.borrow().kind { for (i, x) in vals.iter().enumerate() { let v = conv_elem(it, ty, x)?; typed_set(ta, i, &v); } } }
    Ok(t)
}
pub const TYPED_METHODS: &[&str] = &["set", "subarray", "slice", "fill", "map", "forEach", "filter", "reduce", "reduceRight", "join", "indexOf", "lastIndexOf", "includes", "find", "findIndex", "findLast", "findLastIndex",
    "every", "some", "reverse", "sort", "at", "keys", "values", "entries", "toString", "toLocaleString", "copyWithin", "toSorted", "toReversed", "with"];
pub fn typed_method(key: &str) -> Option<Val> {
    Some(match key {
        "set" => nat!("set", |it, this, a| { let (_, _, ty, _, len) = ta_of(it, &this)?; let src = list_of(it, &arg(a.as_slice(), 0))?; let off = match arg(&a, 1) { Val::Undef => 0, v => it.to_num(&v)?.max(0.0) as usize }; if off + src.len() > len { return Err(throw(it, "RangeError", "offset is out of bounds")); } let mut conv = Vec::new(); for x in &src { conv.push(conv_elem(it, ty, x)?); } if let Val::Obj(o) = &this { if let Kind::Typed(t) = &o.borrow().kind { for (i, v) in conv.iter().enumerate() { typed_set(t, off + i, v); } } } Ok(Val::Undef) }),
        "subarray" => nat!("subarray", |it, this, a| { let (_, bo, ty, off, len) = ta_of(it, &this)?; let b = rel_arg(it, &a, 0, len, 0)?; let e = rel_arg(it, &a, 1, len, len)?; Ok(make_typed(ty, Some(bo), off + b * elem_size(ty), e.saturating_sub(b))) }),
        "slice" => nat!("slice", |it, this, a| { let (_, _, ty, _, len) = ta_of(it, &this)?; let v = list_of(it, &this)?; let b = rel_arg(it, &a, 0, len, 0)?; let e = rel_arg(it, &a, 1, len, len)?; typed_from_vals(it, ty, if b < e { &v[b..e] } else { &[] }) }),
        "fill" => nat!("fill", |it, this, a| { let (_, _, ty, _, len) = ta_of(it, &this)?; let x = conv_elem(it, ty, &arg(&a, 0))?; let b = rel_arg(it, &a, 1, len, 0)?; let e = rel_arg(it, &a, 2, len, len)?; if let Val::Obj(o) = &this { if let Kind::Typed(t) = &o.borrow().kind { for i in b..e { typed_set(t, i, &x); } } } Ok(this) }),
        "map" => nat!("map", |it, this, a| { let (_, _, ty, _, _) = ta_of(it, &this)?; let v = list_of(it, &this)?; let f = arg(&a, 0); let mut out = Vec::new(); for (i, x) in v.into_iter().enumerate() { out.push(it.call(&f, arg(&a, 1), vec![x, Val::Num(i as f64), this.clone()])?); } typed_from_vals(it, ty, &out) }),
        "filter" => nat!("filter", |it, this, a| { let (_, _, ty, _, _) = ta_of(it, &this)?; let v = list_of(it, &this)?; let f = arg(&a, 0); let mut out = Vec::new(); for (i, x) in v.into_iter().enumerate() { if it.call(&f, arg(&a, 1), vec![x.clone(), Val::Num(i as f64), this.clone()])?.truthy() { out.push(x); } } typed_from_vals(it, ty, &out) }),
        "reverse" => nat!("reverse", |it, this, _a| { let (_, _, ty, _, _) = ta_of(it, &this)?; let mut v = list_of(it, &this)?; v.reverse(); if let Val::Obj(o) = &this { if let Kind::Typed(t) = &o.borrow().kind { for (i, x) in v.iter().enumerate() { typed_set(t, i, x); } } } let _ = ty; Ok(this) }),
        "sort" | "toSorted" => { let copy = key == "toSorted"; nat!("sort", |it, this, a| {
            let (_, _, ty, _, _) = ta_of(it, &this)?; let mut v = list_of(it, &this)?; let f = arg(&a, 0);
            if f.is_callable() { v = sort_vals(it, v, &f)?; } else { v.sort_by(|x, y| match (x, y) { (Val::Num(p), Val::Num(q)) => p.partial_cmp(q).unwrap_or(if p.is_nan() { Ordering::Greater } else { Ordering::Less }), (Val::Big(p), Val::Big(q)) => p.cmp(q), _ => Ordering::Equal }); }
            if copy { return typed_from_vals(it, ty, &v); }
            if let Val::Obj(o) = &this { if let Kind::Typed(t) = &o.borrow().kind { for (i, x) in v.iter().enumerate() { typed_set(t, i, x); } } }
            Ok(this)
        }) }
        "toReversed" => nat!("toReversed", |it, this, _a| { let (_, _, ty, _, _) = ta_of(it, &this)?; let mut v = list_of(it, &this)?; v.reverse(); typed_from_vals(it, ty, &v) }),
        "with" => nat!("with", |it, this, a| { let (_, _, ty, _, _) = ta_of(it, &this)?; let mut v = list_of(it, &this)?; let i = it.to_num(&arg(&a, 0))?.trunc(); let i = if i < 0.0 { v.len() as f64 + i } else { i }; if i < 0.0 || i as usize >= v.len() { return Err(throw(it, "RangeError", "Invalid typed array index")); } v[i as usize] = arg(&a, 1); typed_from_vals(it, ty, &v) }),
        "copyWithin" => nat!("copyWithin", |it, this, a| { let (_, _, _, _, len) = ta_of(it, &this)?; let v = list_of(it, &this)?; let t = rel_arg(it, &a, 0, len, 0)?; let s = rel_arg(it, &a, 1, len, 0)?; let e = rel_arg(it, &a, 2, len, len)?; let n = e.saturating_sub(s).min(len - t); if let Val::Obj(o) = &this { if let Kind::Typed(ta) = &o.borrow().kind { for i in 0..n { typed_set(ta, t + i, &v[s + i]); } } } Ok(this) }),
        "keys" => nat!("keys", |_it, this, _a| Ok(new_iter(IterSt::Arr(this, 0, 0)))),
        "values" => nat!("values", |_it, this, _a| Ok(new_iter(IterSt::Arr(this, 0, 1)))),
        "entries" => nat!("entries", |_it, this, _a| Ok(new_iter(IterSt::Arr(this, 0, 2)))),
        "join" | "toString" | "toLocaleString" => { let is_join = key == "join"; nat!("join", |it, this, a| { let v = list_of(it, &this)?; let sep = if is_join && !matches!(arg(&a, 0), Val::Undef) { it.to_str(&arg(&a, 0))?.to_string() } else { ",".into() }; let mut parts = Vec::new(); for x in &v { parts.push(it.to_str(x)?.to_string()); } Ok(st(&parts.join(&sep))) }) }
        k => return array_method(k, Val::Undef),
    })
}
fn dataview_method(key: &'static str) -> Val {
    let (get, ty) = match key { "getInt8" => (true, 0), "getUint8" => (true, 1), "getInt16" => (true, 3), "getUint16" => (true, 4), "getInt32" => (true, 5), "getUint32" => (true, 6), "getFloat32" => (true, 7), "getFloat64" => (true, 8), "getBigInt64" => (true, 9), "getBigUint64" => (true, 10),
        "setInt8" => (false, 0), "setUint8" => (false, 1), "setInt16" => (false, 3), "setUint16" => (false, 4), "setInt32" => (false, 5), "setUint32" => (false, 6), "setFloat32" => (false, 7), "setFloat64" => (false, 8), "setBigInt64" => (false, 9), _ => (false, 10) };
    Val::native(key, move |it, this, a| {
        let (buf, off, len) = if let Val::Obj(o) = &this { if let Kind::View(t) = &o.borrow().kind { (t.buf.clone(), t.off, t.len) } else { return Err(throw(it, "TypeError", "not a DataView")); } } else { return Err(throw(it, "TypeError", "not a DataView")); };
        let i = it.to_num(&arg(&a, 0))?; let es = elem_size(ty);
        if i < 0.0 || i.is_nan() || i as usize + es > len { return Err(throw(it, "RangeError", "Offset is outside the bounds of the DataView")); }
        let p = off + i as usize;
        if get { let little = arg(&a, 1).truthy(); let b = buf.borrow(); return Ok(read_elem(&b[p..p + es], ty, little)); }
        let little = arg(&a, 2).truthy();
        let v = conv_elem(it, ty, &arg(&a, 1))?;
        let mut x = elem_bytes(ty, &v); if !little { x[..es].reverse(); }
        buf.borrow_mut()[p..p + es].copy_from_slice(&x[..es]);
        Ok(Val::Undef)
    })
}

// ── WHATWG URL (URL Standard §4 basic URL parser, the parts pages use) ──
#[derive(Clone, Default)]
pub struct Url { pub scheme: String, pub user: String, pub pass: String, pub host: Option<String>, pub port: Option<u16>, pub path: String, pub query: Option<String>, pub frag: Option<String> }
fn special_port(s: &str) -> Option<Option<u16>> { match s { "http" | "ws" => Some(Some(80)), "https" | "wss" => Some(Some(443)), "ftp" => Some(Some(21)), "file" => Some(None), _ => None } }
fn pct(s: &str, set: &str) -> String { let mut o = String::new(); for c in s.chars() { if (c as u32) < 0x21 || (c as u32) > 0x7e || set.contains(c) { let mut b = [0u8; 4]; for x in c.encode_utf8(&mut b).bytes() { o += &format!("%{x:02X}"); } } else { o.push(c); } } o }
fn norm_path(segs: &mut Vec<String>, input: &str, special: bool) {
    let parts: Vec<&str> = if special { input.split(['/', '\\']).collect() } else { input.split('/').collect() };
    let n = parts.len();
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == n;
        match p.to_ascii_lowercase().as_str() {
            ".." | ".%2e" | "%2e." | "%2e%2e" => { segs.pop(); if last { segs.push(String::new()); } }
            "." | "%2e" => { if last { segs.push(String::new()); } }
            _ => segs.push(pct(p, " \"<>`#?{}^|")),
        }
    }
}
/// Punycode (RFC 3492 §6.3) of one label's code points.
fn punycode(input: &[u32]) -> String {
    let (base, tmin, tmax, skew, damp) = (36u32, 1u32, 26u32, 38u32, 700u32);
    let digit = |d: u32| if d < 26 { (b'a' + d as u8) as char } else { (b'0' + (d - 26) as u8) as char };
    let adapt = |mut delta: u32, num: u32, first: bool| { delta = if first { delta / damp } else { delta / 2 }; delta += delta / num; let mut k = 0; while delta > ((base - tmin) * tmax) / 2 { delta /= base - tmin; k += base; } k + (base - tmin + 1) * delta / (delta + skew) };
    let mut out: String = input.iter().filter(|&&c| c < 0x80).map(|&c| char::from_u32(c).unwrap()).collect();
    let b = out.len() as u32; let mut h = b; if b > 0 { out.push('-'); }
    let (mut n, mut delta, mut bias) = (0x80u32, 0u32, 72u32);
    while (h as usize) < input.len() {
        let m = input.iter().copied().filter(|&c| c >= n).min().unwrap();
        delta = delta.saturating_add((m - n).saturating_mul(h + 1)); n = m;
        for &c in input {
            if c < n { delta += 1; }
            if c == n {
                let mut q = delta; let mut k = base;
                loop { let t = if k <= bias { tmin } else if k >= bias + tmax { tmax } else { k - bias }; if q < t { break; } out.push(digit(t + (q - t) % (base - t))); q = (q - t) / (base - t); k += base; }
                out.push(digit(q)); bias = adapt(delta, h + 1, h == b); delta = 0; h += 1;
            }
        }
        delta += 1; n += 1;
    }
    out
}
/// domainToASCII (URL Standard §3.3, UTS #46 without the mapping tables): lowercase, then xn-- punycode per non-ASCII label.
fn domain_to_ascii(h: &str) -> String {
    h.to_lowercase().split('.').map(|l| if l.is_ascii() { l.to_string() } else { format!("xn--{}", punycode(&l.chars().map(|c| c as u32).collect::<Vec<_>>())) }).collect::<Vec<_>>().join(".")
}
pub fn parse_url(input: &str, base: Option<&Url>) -> Option<Url> {
    let s: String = input.trim_matches(|c: char| (c as u32) <= 0x20).chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let mut u = Url::default();
    let colon = s.find(':');
    let scheme_ok = colon.map_or(false, |i| i > 0 && s[..i].chars().next().unwrap().is_ascii_alphabetic() && s[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)));
    let mut rest: &str;
    if scheme_ok {
        let i = colon.unwrap(); u.scheme = s[..i].to_ascii_lowercase(); rest = &s[i + 1..];
        let special = special_port(&u.scheme).is_some();
        if special && !rest.starts_with("//") && !rest.starts_with("\\\\") { if let Some(b) = base.filter(|b| b.scheme == u.scheme) { return parse_relative(&u.scheme.clone(), rest, b); } }
        if rest.starts_with("//") || (special && (rest.starts_with("\\\\") || rest.starts_with("/\\") || rest.starts_with("\\/"))) { rest = &rest[2..]; }
        else if special { /* "http:example.com" → authority anyway */ }
        else { // opaque or absolute path without authority
            let (p, q, f) = split_pqf(rest);
            u.path = if p.starts_with('/') { let mut segs = Vec::new(); norm_path(&mut segs, &p[1..], false); format!("/{}", segs.join("/")) } else { pct(p, "") };
            u.query = q.map(|q| pct(q, " \"#<>")); u.frag = f.map(|f| pct(f, " \"<>`"));
            return Some(u);
        }
        parse_authority_and_rest(&mut u, rest)?;
        Some(u)
    } else {
        let b = base?;
        parse_relative(&b.scheme.clone(), &s, b)
    }
}
fn split_pqf(s: &str) -> (&str, Option<&str>, Option<&str>) {
    let (s, f) = match s.find('#') { Some(i) => (&s[..i], Some(&s[i + 1..])), None => (s, None) };
    let (p, q) = match s.find('?') { Some(i) => (&s[..i], Some(&s[i + 1..])), None => (s, None) };
    (p, q, f)
}
fn parse_authority_and_rest(u: &mut Url, rest: &str) -> Option<()> {
    let special = special_port(&u.scheme).is_some();
    let end = rest.find(|c: char| c == '/' || c == '?' || c == '#' || (special && c == '\\')).unwrap_or(rest.len());
    let (auth, tail) = rest.split_at(end);
    let (userinfo, hostport) = match auth.rfind('@') { Some(i) => (Some(&auth[..i]), &auth[i + 1..]), None => (None, auth) };
    if let Some(ui) = userinfo { let (us, pw) = ui.split_once(':').unwrap_or((ui, "")); u.user = pct(us, " \"#<>?`{}/:;=@[\\]^|"); u.pass = pct(pw, " \"#<>?`{}/:;=@[\\]^|"); }
    let (h, p) = if hostport.starts_with('[') { match hostport.find(']') { Some(i) => (&hostport[..=i], hostport[i + 1..].strip_prefix(':')), None => return None } } else { match hostport.rfind(':') { Some(i) => (&hostport[..i], Some(&hostport[i + 1..])), None => (hostport, None) } };
    if h.is_empty() && special && u.scheme != "file" { return None; }
    u.host = Some(if h.is_ascii() { pct(&h.to_ascii_lowercase(), " #%/:<>?@[\\]^|").replace("%5B", "[").replace("%5D", "]") } else { domain_to_ascii(h) });
    if let Some(p) = p.filter(|p| !p.is_empty()) { let n: u32 = p.parse().ok()?; if n > 65535 { return None; } let n = n as u16; if special_port(&u.scheme).flatten() != Some(n) { u.port = Some(n); } }
    let (path, q, f) = split_pqf(tail);
    let mut segs = Vec::new();
    if !path.is_empty() { norm_path(&mut segs, &path[1..], special); }
    u.path = if segs.is_empty() { if special { "/".into() } else { String::new() } } else { format!("/{}", segs.join("/")) };
    u.query = q.map(|q| pct(q, if special { " \"#<>'" } else { " \"#<>" })); u.frag = f.map(|f| pct(f, " \"<>`"));
    Some(())
}
fn parse_relative(scheme: &str, s: &str, b: &Url) -> Option<Url> {
    let mut u = Url { scheme: scheme.to_string(), user: b.user.clone(), pass: b.pass.clone(), host: b.host.clone(), port: b.port, ..Default::default() };
    let special = special_port(scheme).is_some();
    if s.starts_with("//") || (special && s.starts_with("\\\\")) { parse_authority_and_rest(&mut u, &s[2..])?; return Some(u); }
    let (p, q, f) = split_pqf(s);
    if p.is_empty() {
        u.path = b.path.clone();
        u.query = match q { Some(q) => Some(pct(q, if special { " \"#<>'" } else { " \"#<>" })), None => b.query.clone() };
        u.frag = f.map(|f| pct(f, " \"<>`"));
        return Some(u);
    }
    let mut segs: Vec<String> = Vec::new();
    if p.starts_with('/') || (special && p.starts_with('\\')) { norm_path(&mut segs, &p[1..], special); }
    else { let bp = b.path.trim_start_matches('/'); let mut bsegs: Vec<String> = bp.split('/').map(|x| x.to_string()).collect(); bsegs.pop(); segs = bsegs; norm_path(&mut segs, p, special); }
    u.path = format!("/{}", segs.join("/"));
    u.query = q.map(|q| pct(q, if special { " \"#<>'" } else { " \"#<>" })); u.frag = f.map(|f| pct(f, " \"<>`"));
    Some(u)
}
impl Url {
    pub fn host_port(&self) -> String { match (&self.host, self.port) { (Some(h), Some(p)) => format!("{h}:{p}"), (Some(h), None) => h.clone(), _ => String::new() } }
    pub fn href(&self) -> String {
        let mut o = format!("{}:", self.scheme);
        if let Some(_h) = &self.host { o += "//"; if !self.user.is_empty() || !self.pass.is_empty() { o += &self.user; if !self.pass.is_empty() { o += ":"; o += &self.pass; } o += "@"; } o += &self.host_port(); }
        o += &self.path;
        if let Some(q) = &self.query { o += "?"; o += q; }
        if let Some(f) = &self.frag { o += "#"; o += f; }
        o
    }
    pub fn origin(&self) -> String { if special_port(&self.scheme).is_some() && self.scheme != "file" { format!("{}://{}", self.scheme, self.host_port()) } else { "null".into() } }
}
/// application/x-www-form-urlencoded parsing and serializing (URL Standard §5).
pub fn form_decode(s: &str) -> Vec<(String, String)> {
    s.split('&').filter(|x| !x.is_empty()).map(|kv| { let (k, v) = kv.split_once('=').unwrap_or((kv, "")); let d = |x: &str| { let x = x.replace('+', " "); let b = x.as_bytes(); let mut out = Vec::new(); let mut i = 0; while i < b.len() { if b[i] == b'%' && i + 2 < b.len() { if let Some(h) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) { out.push(h); i += 3; continue; } } out.push(b[i]); i += 1; } String::from_utf8_lossy(&out).into_owned() }; (d(k), d(v)) }).collect()
}
pub fn form_encode(pairs: &[(String, String)]) -> String {
    let e = |s: &str| { let mut o = String::new(); for b in s.bytes() { if b.is_ascii_alphanumeric() || b"*-._".contains(&b) { o.push(b as char); } else if b == b' ' { o.push('+'); } else { o += &format!("%{b:02X}"); } } o };
    pairs.iter().map(|(k, v)| format!("{}={}", e(k), e(v))).collect::<Vec<_>>().join("&")
}

// ── structuredClone (HTML §2.7.3 StructuredSerialize / Deserialize, in one pass) ──
fn sclone(it: &mut Interp, v: &Val, memo: &mut Vec<(usize, Val)>) -> Result<Val, Ctl> {
    let Val::Obj(o) = v else { return match v { Val::Sym(_) => Err(throw(it, "DataCloneError", "Symbol could not be cloned.")), _ => Ok(v.clone()) } };
    let id = Rc::as_ptr(o) as *const u8 as usize;
    if let Some((_, c)) = memo.iter().find(|(k, _)| *k == id) { return Ok(c.clone()); }
    enum C { Arr(Vec<Val>), Map(Vec<(Val, Val)>), Set(Vec<Val>), Date(f64), Re(String, String), Prim(Val), Buf(Vec<u8>), Typed(u8, Vec<u8>), Err, Plain, No(&'static str) }
    let c = match &o.borrow().kind {
        Kind::Array(a) => C::Arr(a.clone()), Kind::Map(m) => C::Map(m.entries().cloned().collect()), Kind::Set(m) => C::Set(m.entries().map(|e| e.0.clone()).collect()),
        Kind::Date(t) => C::Date(*t), Kind::RegExp(p, f) => C::Re(p.clone(), f.clone()), Kind::Prim(p) => C::Prim(p.clone()), Kind::Buffer(b) => C::Buf(b.borrow().clone()),
        Kind::Typed(t) => { let b = t.buf.borrow(); let es = elem_size(t.ty); C::Typed(t.ty, b[t.off..(t.off + t.len * es).min(b.len())].to_vec()) }
        Kind::Error => C::Err, Kind::Plain => C::Plain,
        Kind::Func(_) | Kind::Native(..) => C::No("function"), Kind::Promise(_) => C::No("#<Promise>"), Kind::WeakMap(_) | Kind::WeakSet(_) | Kind::WeakRef(_) => C::No("#<WeakMap>"), Kind::Host(..) => C::No("#<HTMLElement>"), Kind::Proxy(..) => C::No("#<Object>"), Kind::Gen(_) => C::No("#<Generator>"),
        _ => C::Plain,
    };
    let out = match c {
        C::No(what) => return Err(throw(it, "DataCloneError", &format!("{what} could not be cloned."))),
        C::Arr(a) => { let r = Val::array(vec![]); memo.push((id, r.clone())); let mut v2 = Vec::with_capacity(a.len()); for x in &a { v2.push(sclone(it, x, memo)?); } with_arr(&r, |w| *w = v2); return Ok(r); }
        C::Map(es) => { let r = Val::obj(Kind::Map(Box::new(MapData::new()))); memo.push((id, r.clone())); for (k, x) in es { let k2 = sclone(it, &k, memo)?; let x2 = sclone(it, &x, memo)?; if let Val::Obj(ro) = &r { if let Kind::Map(m) = &mut ro.borrow_mut().kind { m.set(k2, x2); } } } return Ok(r); }
        C::Set(es) => { let r = Val::obj(Kind::Set(Box::new(MapData::new()))); memo.push((id, r.clone())); for k in es { let k2 = sclone(it, &k, memo)?; if let Val::Obj(ro) = &r { if let Kind::Set(m) = &mut ro.borrow_mut().kind { m.set(k2, Val::Undef); } } } return Ok(r); }
        C::Date(t) => Val::obj(Kind::Date(t)),
        C::Re(p, f) => { let r = Val::obj(Kind::RegExp(p, f)); if let Val::Obj(ro) = &r { Interp::define(ro, "lastIndex", Val::Num(0.0), A_NOENUM); } r }
        C::Prim(p) => Val::obj(Kind::Prim(p)),
        C::Buf(b) => Val::obj(Kind::Buffer(Rc::new(std::cell::RefCell::new(b)))),
        C::Typed(ty, b) => { let n = b.len() / elem_size(ty); let bo = Val::obj(Kind::Buffer(Rc::new(std::cell::RefCell::new(b)))); make_typed(ty, Some(bo), 0, n) }
        C::Err => { let name = it.get(v, "name")?; let msg = it.get(v, "message")?; let (name, msg) = (it.to_str(&name)?.to_string(), it.to_str(&msg)?.to_string()); let e = it.error(if ["Error", "TypeError", "RangeError", "SyntaxError", "ReferenceError", "EvalError", "URIError"].contains(&name.as_str()) { &name } else { "Error" }, &msg); let stack = it.get(v, "stack")?; if let Val::Obj(eo) = &e { Interp::define(eo, "stack", stack, A_NOENUM); } e }
        C::Plain => { let r = Val::obj(Kind::Plain); memo.push((id, r.clone())); for k in it.keys(v) { let x = it.get(v, &k)?; let x2 = sclone(it, &x, memo)?; if let Val::Obj(ro) = &r { Interp::put(ro, k, x2); } } return Ok(r); }
    };
    memo.push((id, out.clone()));
    Ok(out)
}

// ── install: constructors, prototypes, globals ──
fn ctor(name: &'static str, proto: &ORef, f: impl Fn(&mut Interp, Val, Vec<Val>) -> Result<Val, Ctl> + 'static) -> Val {
    let c = Val::native(name, f);
    if let Val::Obj(co) = &c { Interp::define(co, "prototype", Val::Obj(proto.clone()), A_NOENUM | A_NOWRITE | A_NOCONF); }
    Interp::define(proto, "constructor", c.clone(), A_NOENUM);
    c
}
/// Install builtin methods; each native is (re)named after its property (`padEnd.name` is "padEnd", not a shared helper's name).
fn fill(proto: &ORef, names: &[&'static str], f: impl Fn(&str) -> Option<Val>) { for n in names { if let Some(v) = f(n) { def(proto, n, retag(v, n)); } } }
fn retag(v: Val, name: &'static str) -> Val { if let Val::Obj(o) = &v { if let Kind::Native(f, _) = &o.borrow().kind { return Val::obj(Kind::Native(f.clone(), name)); } } v }
/// The result of Array.from / Array.of for a `this` constructor (§23.1.2.1 step 5.a, §23.1.2.3): a typed-array constructor
/// builds that kind, any other constructor than Array is constructed and filled, else a plain array.
fn array_like_of(it: &mut Interp, this: &Val, items: Vec<Val>) -> Result<Val, Ctl> {
    let kind = match this { Val::Obj(co) => match &co.borrow().kind { Kind::Native(_, n) => TNAMES.iter().position(|x| x == n).map(|t| t as i32).unwrap_or(if *n == "Array" { -1 } else { -2 }), Kind::Func(c) if !c.func.method => -3, _ => -1 }, _ => -1 };
    match kind {
        t if t >= 0 => typed_from_vals(it, t as u8, &items),
        -3 => { let n = items.len(); let o = it.construct(this, vec![Val::Num(n as f64)])?; for (i, x) in items.into_iter().enumerate() { if let Val::Obj(oo) = &o { Interp::define(oo, &i.to_string(), x, 0); } } it.set(&o, "length", Val::Num(n as f64))?; Ok(o) }
        _ => Ok(Val::array(items)),
    }
}
/// The `length` of a built-in function (its spec arity), by name; 0 when unknown.
pub fn native_len(name: &str) -> f64 {
    (match name {
        "assign" | "defineProperties" | "create" | "getOwnPropertyDescriptor" | "setPrototypeOf" | "is" | "groupBy" | "hasOwn" | "apply" | "slice" | "splice" | "copyWithin" | "toSpliced" | "with"
        | "replace" | "replaceAll" | "split" | "substring" | "substr" | "max" | "min" | "pow" | "atan2" | "imul" | "hypot" | "set" | "parse" | "construct" | "parseInt" | "asIntN" | "asUintN"
        | "RegExp" | "Proxy" | "then" | "__defineGetter__" | "__defineSetter__" | "fill_typed" => 2,
        "defineProperty" | "stringify" | "Int8Array" | "Uint8Array" | "Uint8ClampedArray" | "Int16Array" | "Uint16Array" | "Int32Array" | "Uint32Array" | "Float32Array" | "Float64Array" | "BigInt64Array" | "BigUint64Array" => 3,
        "Date" => 7, "UTC" => 7,
        "pop" | "shift" | "reverse" | "keys" | "values" | "entries" | "toString" | "toLocaleString" | "valueOf" | "normalize" | "toUpperCase" | "toLowerCase" | "toLocaleUpperCase" | "toLocaleLowerCase"
        | "trim" | "trimStart" | "trimEnd" | "trimLeft" | "trimRight" | "isWellFormed" | "toWellFormed" | "flat" | "toReversed" | "clear" | "random" | "now" | "of" | "Symbol" | "Map" | "Set" | "WeakMap"
        | "WeakSet" | "next" | "deref" | "revoke" | "getEntries" | "toJSON" | "randomUUID" | "URLSearchParams" | "TextEncoder" | "TextDecoder" | "Iterator" | "AbortController" | "abort"
        | "[Symbol.iterator]" | "[Symbol.asyncIterator]" | "description" | "size" | "withResolvers" | "noop" | "log" | "eval_noarg" => 0,
        _ => 1,
    }) as f64
}
fn statics(c: &Val, kv: Vec<(&str, Val)>) { if let Val::Obj(o) = c { for (k, v) in kv { def(o, k, v); } } }
fn this_obj(it: &Interp, v: &Val, what: &str) -> Result<ORef, Ctl> { match v { Val::Obj(o) => Ok(o.clone()), _ => Err(throw(it, "TypeError", &format!("{what} called on non-object"))) } }
fn sym_of(it: &Interp, this: &Val) -> Result<Rc<crate::jsi::Sym>, Ctl> { match this { Val::Sym(s) => Ok(s.clone()), Val::Obj(o) => match &o.borrow().kind { Kind::Prim(Val::Sym(s)) => Ok(s.clone()), _ => Err(throw(it, "TypeError", "not a symbol")) }, _ => Err(throw(it, "TypeError", "not a symbol")) } }
fn big_of(it: &Interp, this: &Val) -> Result<Rc<Big>, Ctl> { match this { Val::Big(b) => Ok(b.clone()), Val::Obj(o) => match &o.borrow().kind { Kind::Prim(Val::Big(b)) => Ok(b.clone()), _ => Err(throw(it, "TypeError", "not a BigInt")) }, _ => Err(throw(it, "TypeError", "not a BigInt")) } }
/// ToBigInt (§7.1.13) / the BigInt function (§21.2.1.1).
fn to_bigint(it: &mut Interp, v: &Val, from_number: bool) -> Result<Big, Ctl> {
    match v {
        Val::Big(b) => Ok((**b).clone()), Val::Bool(b) => Ok(Big::from_i64(*b as i64)),
        Val::Num(n) if from_number => Big::from_f64(*n).ok_or_else(|| throw(it, "RangeError", &format!("The number {} cannot be converted to a BigInt because it is not an integer", num_str(*n)))),
        Val::Str(s) => Big::parse(s).ok_or_else(|| throw(it, "SyntaxError", &format!("Cannot convert {s} to a BigInt"))),
        Val::Obj(_) => { let p = it.to_primitive(v, 1)?; to_bigint(it, &p, from_number) }
        _ => { let s = it.to_str(v).map(|x| x.to_string()).unwrap_or_default(); Err(throw(it, "TypeError", &format!("Cannot convert {s} to a BigInt"))) }
    }
}
/// A property descriptor object → [[DefineOwnProperty]] (§10.1.6.3 ValidateAndApplyPropertyDescriptor, the common cases).
pub fn define_from_desc(it: &mut Interp, o: &Val, key: &str, d: &Val) -> Result<(), Ctl> {
    let ob = match o { Val::Obj(x) => x.clone(), _ => return Err(throw(it, "TypeError", "Object.defineProperty called on non-object")) };
    if !matches!(d, Val::Obj(_)) { let ds = it.to_str(d).map(|x| x.to_string()).unwrap_or_default(); return Err(throw(it, "TypeError", &format!("Property description must be an object: {ds}"))); }
    let px = if let Kind::Proxy(t, h) = &ob.borrow().kind { Some((t.clone(), h.clone())) } else { None };
    if let Some((t, h)) = px { let trap = it.get(&h, "defineProperty")?; if trap.is_callable() { let k = it.key_val(key); it.call(&trap, h, vec![t, k, d.clone()])?; return Ok(()); } return define_from_desc(it, &t, key, d); }
    let field = |it: &mut Interp, k: &str| -> Result<Option<Val>, Ctl> { if it.has_prop(d, k)? { Ok(Some(it.get(d, k)?)) } else { Ok(None) } };
    let (value, writable, enumerable, configurable, getter, setter) = (field(it, "value")?, field(it, "writable")?, field(it, "enumerable")?, field(it, "configurable")?, field(it, "get")?, field(it, "set")?);
    if let Some(g) = &getter { if !g.is_callable() && !matches!(g, Val::Undef) { let gs = it.to_str(g).map(|x| x.to_string()).unwrap_or_default(); return Err(throw(it, "TypeError", &format!("Getter must be a function: {gs}"))); } }
    if (getter.is_some() || setter.is_some()) && (value.is_some() || writable.is_some()) { return Err(throw(it, "TypeError", "Invalid property descriptor. Cannot both specify accessors and a value or writable attribute")); }
    // the global object's data properties are the global scope's bindings
    if matches!(ob.borrow().kind, Kind::Global) && getter.is_none() && setter.is_none() { let v = match value { Some(v) => v, None => it.get(o, key)? }; it.global.borrow_mut().vars.insert(Rc::from(key), v); return Ok(()); }
    let host = matches!(ob.borrow().kind, Kind::Host(..));
    if host { let v = match (&value, &getter) { (Some(v), _) => v.clone(), (None, Some(g)) if g.is_callable() => it.call(g, o.clone(), vec![]).unwrap_or(Val::Undef), _ => Val::Undef }; return it.set(o, key, v); }
    let existing = it.own_prop(o, key)?;
    let (fl, is_arr) = { let b = ob.borrow(); (b.flags, matches!(b.kind, Kind::Array(_))) };
    if existing.is_none() && fl & (F_NOEXT | F_SEALED | F_FROZEN) != 0 { return Err(throw(it, "TypeError", &format!("Cannot define property {key}, object is not extensible"))); }
    let (cur_attrs, cur) = match &existing { Some((v, a)) => (*a, Some(v.clone())), None => (A_NOENUM | A_NOWRITE | A_NOCONF, None) };
    if cur_attrs & A_NOCONF != 0 && existing.is_some() {
        let bad = configurable.as_ref().map_or(false, |c| c.truthy()) || enumerable.as_ref().map_or(false, |e| e.truthy() == (cur_attrs & A_NOENUM != 0))
            || (matches!(cur, Some(Err(_))) != (getter.is_some() || setter.is_some()) && (getter.is_some() || setter.is_some() || value.is_some()))
            || (cur_attrs & A_NOWRITE != 0 && (writable.as_ref().map_or(false, |w| w.truthy()) || matches!((&cur, &value), (Some(Ok(c)), Some(v)) if !same_value(c, v))));
        if bad { return Err(throw(it, "TypeError", &format!("Cannot redefine property: {}", it.key_val(key).as_obj().map(|_| key.to_string()).unwrap_or_else(|| key.trim_start_matches('\u{0}').to_string())))); }
    }
    let flag = |v: &Option<Val>, bit: u8| -> u8 { match v { Some(x) => if x.truthy() { 0 } else { bit }, None => cur_attrs & bit } };
    if getter.is_some() || setter.is_some() {
        let attrs = flag(&enumerable, A_NOENUM) | flag(&configurable, A_NOCONF);
        let keep = matches!(cur, Some(Err(_)));
        Interp::define_accessor(&ob, key, getter.or(if keep { None } else { Some(Val::Undef) }), setter.or(if keep { None } else { Some(Val::Undef) }), attrs);
        return Ok(());
    }
    if is_arr && key == "length" { if let Some(v) = value { it.set(o, "length", v)?; } if matches!(writable, Some(Val::Bool(false))) { ob.borrow_mut().flags |= F_FROZEN; } return Ok(()); }
    let was_acc = matches!(cur, Some(Err(_)));
    let v = match value { Some(v) => v, None => match cur { Some(Ok(v)) => v, _ => Val::Undef } };
    let attrs = flag(&enumerable, A_NOENUM) | (if was_acc && writable.is_none() { A_NOWRITE } else { flag(&writable, A_NOWRITE) }) | flag(&configurable, A_NOCONF);
    if was_acc { ob.borrow_mut().props.retain(|(k, _)| &**k != key); }
    Interp::define(&ob, key, v, if is_arr && crate::jsi::index_of(key).is_some() { 0 } else { attrs });
    Ok(())
}
fn same_value(a: &Val, b: &Val) -> bool { match (a, b) { (Val::Num(x), Val::Num(y)) => (x.is_nan() && y.is_nan()) || (x == y && x.is_sign_negative() == y.is_sign_negative()), _ => Interp::strict_eq(a, b) } }
/// FromPropertyDescriptor (§6.2.6.4).
pub fn desc_obj(it: &mut Interp, o: &Val, key: &str) -> Result<Val, Ctl> {
    if let Val::Obj(ob) = o {
        let px = if let Kind::Proxy(t, h) = &ob.borrow().kind { Some((t.clone(), h.clone())) } else { None };
        if let Some((t, h)) = px { let trap = it.get(&h, "getOwnPropertyDescriptor")?; if trap.is_callable() { let k = it.key_val(key); return it.call(&trap, h, vec![t, k]); } return desc_obj(it, &t, key); }
        if matches!(ob.borrow().kind, Kind::Host(..)) && it.own_prop(o, key)?.is_none() {
            let v = it.get(o, key)?; if matches!(v, Val::Undef) { return Ok(Val::Undef); }
            return Ok(Val::plain(vec![("value", v), ("writable", Val::Bool(true)), ("enumerable", Val::Bool(true)), ("configurable", Val::Bool(true))]));
        }
    }
    Ok(match it.own_prop(o, key)? {
        None => Val::Undef,
        Some((Ok(v), a)) => Val::plain(vec![("value", v), ("writable", Val::Bool(a & A_NOWRITE == 0)), ("enumerable", Val::Bool(a & A_NOENUM == 0)), ("configurable", Val::Bool(a & A_NOCONF == 0))]),
        Some((Err((g, s)), a)) => Val::plain(vec![("get", g), ("set", s), ("enumerable", Val::Bool(a & A_NOENUM == 0)), ("configurable", Val::Bool(a & A_NOCONF == 0))]),
    })
}
fn obj_flags(v: &Val) -> Option<u8> { if let Val::Obj(o) = v { Some(o.borrow().flags) } else { None } }
fn obj_empty(v: &Val) -> bool { if let Val::Obj(o) = v { let b = o.borrow(); b.props.is_empty() && !matches!(&b.kind, Kind::Array(a) if !a.is_empty()) } else { true } }
fn weak_key(it: &Interp, k: &Val) -> Result<ORef, Ctl> { match k { Val::Obj(o) => Ok(o.clone()), _ => Err(throw(it, "TypeError", "Invalid value used as weak map key")) } }
fn first(a: Vec<Val>) -> Val { a.into_iter().next().unwrap_or(Val::Undef) }

/// Promise.all / allSettled / any / race (§27.2.4.1–5).
fn promise_combinator(it: &mut Interp, iterable: &Val, kind: u8) -> Result<Val, Ctl> {
    let p = crate::jsi::new_promise();
    let items = match it.iterate(iterable) { Ok(v) => v, Err(Ctl::Throw(e)) => { it.settle(&p, false, e); return Ok(p); } Err(e) => return Err(e) };
    let n = items.len();
    let results = Rc::new(std::cell::RefCell::new(vec![Val::Undef; n]));
    let remaining = Rc::new(std::cell::Cell::new(n));
    if n == 0 {
        match kind { 0 | 1 => it.resolve_promise(&p, Val::array(vec![])), 2 => { let e = it.error("AggregateError", "All promises were rejected"); if let Val::Obj(eo) = &e { Interp::define(eo, "errors", Val::array(vec![]), A_NOENUM); } it.settle(&p, false, e) } _ => {} }
        return Ok(p);
    }
    for (i, x) in items.into_iter().enumerate() {
        let xp = it.promise_resolve(x);
        let (p1, r1, c1, p2, r2, c2) = (p.clone(), results.clone(), remaining.clone(), p.clone(), results.clone(), remaining.clone());
        let done = move |it: &mut Interp, p: &Val, r: &Rc<std::cell::RefCell<Vec<Val>>>, c: &Rc<std::cell::Cell<usize>>, ok: bool| { c.set(c.get() - 1); if c.get() == 0 { let arr = Val::array(r.borrow().clone()); if ok { it.resolve_promise(p, arr); } else { let e = it.error("AggregateError", "All promises were rejected"); if let Val::Obj(eo) = &e { Interp::define(eo, "errors", arr, A_NOENUM); } it.settle(p, false, e); } } };
        let (onf, onr) = match kind {
            0 => (Val::native("", move |it, _t, a| { r1.borrow_mut()[i] = first(a); done(it, &p1, &r1, &c1, true); Ok(Val::Undef) }), Val::native("", move |it, _t, a| { let _ = (&r2, &c2); it.settle(&p2, false, first(a)); Ok(Val::Undef) })),
            1 => (Val::native("", move |it, _t, a| { r1.borrow_mut()[i] = Val::plain(vec![("status", st("fulfilled")), ("value", first(a))]); done(it, &p1, &r1, &c1, true); Ok(Val::Undef) }), Val::native("", move |it, _t, a| { r2.borrow_mut()[i] = Val::plain(vec![("status", st("rejected")), ("reason", first(a))]); done(it, &p2, &r2, &c2, true); Ok(Val::Undef) })),
            2 => (Val::native("", move |it, _t, a| { let _ = (&r1, &c1); it.resolve_promise(&p1, first(a)); Ok(Val::Undef) }), Val::native("", move |it, _t, a| { r2.borrow_mut()[i] = first(a); done(it, &p2, &r2, &c2, false); Ok(Val::Undef) })),
            _ => (Val::native("", move |it, _t, a| { let _ = (&r1, &c1); it.resolve_promise(&p1, first(a)); Ok(Val::Undef) }), Val::native("", move |it, _t, a| { let _ = (&r2, &c2); it.settle(&p2, false, first(a)); Ok(Val::Undef) })),
        };
        it.promise_react(&xp, onf, onr, None);
    }
    Ok(p)
}

impl Interp {
    pub fn install_globals(&mut self) {
        let wk: Vec<(&'static str, Val)> = WELL_KNOWN.iter().map(|n| (*n, self.well_known(n))).collect();
        let r = &self.realm;
        let (op, fp, ap, sp, np, bp, syp, bgp, ep, mp, setp, wmp, wsp, wrp, pp, rp, dp, itp, aip, gp, agp, tp, bufp, vp) = (r.object.clone(), r.function.clone(), r.array.clone(), r.string.clone(), r.number.clone(), r.boolean.clone(), r.symbol.clone(), r.bigint.clone(), r.error.clone(), r.map.clone(), r.set.clone(), r.weakmap.clone(), r.weakset.clone(), r.weakref.clone(), r.promise.clone(), r.regexp.clone(), r.date.clone(), r.iter.clone(), r.array_iter.clone(), r.generator.clone(), r.async_gen.clone(), r.typed.clone(), r.buffer.clone(), r.view.clone());
        let errs: Vec<(&'static str, ORef)> = r.errors.clone();
        let each: Vec<ORef> = r.typed_each.clone();
        let g = self.global.clone();
        let put = |name: &str, v: Val| { g.borrow_mut().vars.insert(Rc::from(name), v); };
        let tag = |p: &ORef, t: &str| Interp::define(p, K_TAG, st(t), A_NOENUM | A_NOWRITE);

        // console
        let log = nat!("log", |it, _this, a| { let parts: Vec<String> = a.iter().map(|x| inspect(it, x, true, 0)).collect(); it.out += &parts.join(" "); it.out.push('\n'); Ok(Val::Undef) });
        let noop = nat!("noop", |_it, _t, _a| Ok(Val::Undef));
        put("console", Val::plain(vec![("log", log.clone()), ("info", log.clone()), ("warn", log.clone()), ("error", log.clone()), ("debug", log.clone()), ("trace", log.clone()), ("dir", log.clone()), ("table", log.clone()),
            ("group", log.clone()), ("groupCollapsed", log), ("groupEnd", noop.clone()), ("time", noop.clone()), ("timeEnd", noop.clone()), ("timeLog", noop.clone()), ("count", noop.clone()), ("countReset", noop.clone()),
            ("assert", nat!("assert", |it, _t, a| { if !arg(&a, 0).truthy() { let parts: Vec<String> = a.iter().skip(1).map(|x| inspect(it, x, true, 0)).collect(); it.out += &format!("Assertion failed{}\n", if parts.is_empty() { String::new() } else { format!(": {}", parts.join(" ")) }); } Ok(Val::Undef) }))]));

        // Object (§20.1)
        fill(&op, OBJECT_METHODS, |k| object_method(k, Val::Undef));
        let object = ctor("Object", &op, |_it, _this, a| { let v = arg(&a, 0); Ok(match v { Val::Undef | Val::Null => Val::obj(Kind::Plain), Val::Obj(_) => v, p => Val::obj(Kind::Prim(p)) }) });
        statics(&object, vec![
            ("keys", nat!("keys", |it, _this, a| { let o = arg(&a, 0); if o.is_nullish() { return Err(throw(it, "TypeError", "Cannot convert undefined or null to object")); } Ok(Val::array(it.keys(&o).into_iter().map(Val::Str).collect())) })),
            ("values", nat!("values", |it, _this, a| { let o = arg(&a, 0); if o.is_nullish() { return Err(throw(it, "TypeError", "Cannot convert undefined or null to object")); } let mut v = Vec::new(); for k in it.keys(&o) { v.push(it.get(&o, &k)?); } Ok(Val::array(v)) })),
            ("entries", nat!("entries", |it, _this, a| { let o = arg(&a, 0); if o.is_nullish() { return Err(throw(it, "TypeError", "Cannot convert undefined or null to object")); } let mut v = Vec::new(); for k in it.keys(&o) { let x = it.get(&o, &k)?; v.push(Val::array(vec![Val::Str(k), x])); } Ok(Val::array(v)) })),
            // Object.assign (§20.1.2.1): each key's enumerability is read when it is reached (a getter may change it)
            ("assign", nat!("assign", |it, _this, a| { let t = arg(&a, 0); if t.is_nullish() { return Err(throw(it, "TypeError", "Cannot convert undefined or null to object")); } let t = if matches!(t, Val::Obj(_)) { t } else { Val::obj(Kind::Prim(t)) }; for s in a.iter().skip(1) { if s.is_nullish() { continue; } for k in it.own_keys(s, true, true, true) { match it.own_prop(s, &k)? { Some((_, at)) if at & A_NOENUM == 0 => { let x = it.get(s, &k)?; it.set(&t, &k, x)?; } _ => {} } } } Ok(t) })),
            ("freeze", nat!("freeze", |_it, _this, a| { let o = arg(&a, 0); if let Val::Obj(x) = &o { x.borrow_mut().flags |= F_FROZEN | F_SEALED | F_NOEXT; } Ok(o) })),
            ("seal", nat!("seal", |_it, _this, a| { let o = arg(&a, 0); if let Val::Obj(x) = &o { x.borrow_mut().flags |= F_SEALED | F_NOEXT; } Ok(o) })),
            ("preventExtensions", nat!("preventExtensions", |_it, _this, a| { let o = arg(&a, 0); if let Val::Obj(x) = &o { x.borrow_mut().flags |= F_NOEXT; } Ok(o) })),
            ("isFrozen", nat!("isFrozen", |_it, _this, a| { let o = arg(&a, 0); Ok(Val::Bool(match obj_flags(&o) { None => true, Some(f) => f & F_FROZEN != 0 || (f & (F_NOEXT | F_SEALED) != 0 && obj_empty(&o)) })) })),
            ("isSealed", nat!("isSealed", |_it, _this, a| { let o = arg(&a, 0); Ok(Val::Bool(match obj_flags(&o) { None => true, Some(f) => f & (F_SEALED | F_FROZEN) != 0 || (f & F_NOEXT != 0 && obj_empty(&o)) })) })),
            ("isExtensible", nat!("isExtensible", |_it, _this, a| Ok(Val::Bool(match obj_flags(&arg(&a, 0)) { None => false, Some(f) => f & (F_NOEXT | F_SEALED | F_FROZEN) == 0 })))),
            ("fromEntries", nat!("fromEntries", |it, _this, a| { let o = Val::obj(Kind::Plain); let ob = o.as_obj().unwrap().clone(); for e in it.iterate(&arg(&a, 0))? { let k = it.get(&e, "0")?; let k = it.to_key(&k)?; let v = it.get(&e, "1")?; Interp::put(&ob, k, v); } Ok(o) })),
            ("create", nat!("create", |it, _this, a| { let p = match arg(&a, 0) { Val::Obj(p) => Some(p), Val::Null => None, _ => return Err(throw(it, "TypeError", "Object prototype may only be an Object or null")) }; let o = Val::obj_p(Kind::Plain, p); let props = arg(&a, 1); if !props.is_nullish() { for k in it.own_keys(&props, true, true, false) { let d = it.get(&props, &k)?; define_from_desc(it, &o, &k, &d)?; } } Ok(o) })),
            ("getPrototypeOf", nat!("getPrototypeOf", |it, _this, a| Ok(match it.proto_of(&arg(&a, 0))? { Some(p) => Val::Obj(p), None => Val::Null }))),
            ("setPrototypeOf", nat!("setPrototypeOf", |it, _this, a| { let o = arg(&a, 0); let p = match arg(&a, 1) { Val::Obj(p) => Some(p), Val::Null => None, _ => return Err(throw(it, "TypeError", "Object prototype may only be an Object or null")) }; if let Val::Obj(x) = &o { if !it.set_proto(x, p) { return Err(throw(it, "TypeError", "Cyclic __proto__ value")); } } Ok(o) })),
            ("defineProperty", nat!("defineProperty", |it, _this, a| { let o = arg(&a, 0); let k = it.to_key(&arg(&a, 1))?; define_from_desc(it, &o, &k, &arg(&a, 2))?; Ok(o) })),
            ("defineProperties", nat!("defineProperties", |it, _this, a| { let o = arg(&a, 0); let ds = arg(&a, 1); for k in it.own_keys(&ds, true, true, false) { let d = it.get(&ds, &k)?; define_from_desc(it, &o, &k, &d)?; } Ok(o) })),
            ("getOwnPropertyNames", nat!("getOwnPropertyNames", |it, _this, a| Ok(Val::array(it.own_keys(&arg(&a, 0), true, false, true).into_iter().map(Val::Str).collect())))),
            ("getOwnPropertySymbols", nat!("getOwnPropertySymbols", |it, _this, a| { let ks = it.own_keys(&arg(&a, 0), false, true, true); Ok(Val::array(ks.iter().map(|k| it.key_val(k)).collect())) })),
            ("getOwnPropertyDescriptor", nat!("getOwnPropertyDescriptor", |it, _this, a| { let k = it.to_key(&arg(&a, 1))?; desc_obj(it, &arg(&a, 0), &k) })),
            ("getOwnPropertyDescriptors", nat!("getOwnPropertyDescriptors", |it, _this, a| { let o = arg(&a, 0); let r = Val::obj(Kind::Plain); let ro = r.as_obj().unwrap().clone(); for k in it.own_keys(&o, true, true, true) { let d = desc_obj(it, &o, &k)?; Interp::put(&ro, k, d); } Ok(r) })),
            ("is", nat!("is", |_it, _t, a| Ok(Val::Bool(same_value(&arg(&a, 0), &arg(&a, 1)))))),
            ("hasOwn", nat!("hasOwn", |it, _t, a| { let k = it.to_key(&arg(&a, 1))?; Ok(Val::Bool(it.has_own(&arg(&a, 0), &k)?)) })),
            ("groupBy", nat!("groupBy", |it, _t, a| { let f = arg(&a, 1); let o = Val::obj_p(Kind::Plain, None); for (i, x) in it.iterate(&arg(&a, 0))?.into_iter().enumerate() { let k = it.call(&f, Val::Undef, vec![x.clone(), Val::Num(i as f64)])?; let k = it.to_key(&k)?; let cur = it.get(&o, &k)?; match cur { Val::Obj(_) => { with_arr(&cur, |v| v.push(x)); } _ => { it.set(&o, &k, Val::array(vec![x]))?; } } } Ok(o) })),
        ]);
        put("Object", object);

        // Function (§20.2)
        fill(&fp, FUNCTION_METHODS, |k| function_method(k, Val::Undef));
        Interp::define(&fp, K_HAS_INSTANCE, Val::native("[Symbol.hasInstance]", |it, this, a| Ok(Val::Bool(it.ordinary_has_instance(&this, &arg(&a, 0))?))), A_NOENUM | A_NOWRITE | A_NOCONF);
        put("Function", ctor("Function", &fp, |it, _t, a| it.dynamic_function(&a, "function")));

        // Array (§23.1)
        fill(&ap, ARRAY_METHODS, |k| array_method(k, Val::Undef));
        let values = it_get(&ap, "values");
        Interp::define(&ap, K_ITER, values, A_NOENUM);
        let array = ctor("Array", &ap, |it, _this, a| Ok(match a.as_slice() { [Val::Num(n)] => { if n.fract() != 0.0 || *n < 0.0 || *n > 4294967295.0 { return Err(throw(it, "RangeError", "Invalid array length")); } if *n > (1u64 << 28) as f64 { return Err(throw(it, "RangeError", "Invalid array length")); } { let r = Val::array(vec![Val::Undef; *n as usize]); if *n > 0.0 { mark_holey(r) } else { r } } } _ => Val::array(a) }));
        statics(&array, vec![
            ("isArray", nat!("isArray", |_it, _this, a| Ok(Val::Bool(matches!(arg(&a, 0), Val::Obj(ref o) if matches!(o.borrow().kind, Kind::Array(_))))))),
            ("from", nat!("from", |it, this, a| {
                // Array.from (§23.1.2.1): iterate step by step, map each value, close the iterator if the mapper throws
                let src = arg(&a, 0);
                if src.is_nullish() { return Err(throw(it, "TypeError", &format!("{} is not iterable", if matches!(src, Val::Null) { "null" } else { "undefined" }))); }
                let f = arg(&a, 1); let t = arg(&a, 2);
                if !matches!(f, Val::Undef) && !f.is_callable() { let fs = it.to_str(&f).map(|x| x.to_string()).unwrap_or_default(); return Err(throw(it, "TypeError", &format!("{fs} is not a function"))); }
                let iterable = matches!(src, Val::Str(_)) || (matches!(src, Val::Obj(_)) && it.get(&src, K_ITER)?.is_callable());
                let mut out = Vec::new();
                if iterable {
                    let mut rec = it.iter_open(&src)?;
                    let mut i = 0usize;
                    while let Some(x) = it.iter_step(&mut rec)? {
                        let v = if f.is_callable() { match it.call(&f, t.clone(), vec![x, Val::Num(i as f64)]) { Ok(v) => v, Err(e) => { let _ = it.iter_close(&rec); return Err(e); } } } else { x };
                        out.push(v); i += 1;
                    }
                } else {
                    for (i, x) in list_of(it, &src)?.into_iter().enumerate() { out.push(if f.is_callable() { it.call(&f, t.clone(), vec![x, Val::Num(i as f64)])? } else { x }); }
                }
                array_like_of(it, &this, out)
            })),
            ("of", nat!("of", |it, this, a| array_like_of(it, &this, a))),
        ]);
        put("Array", array);

        // String (§22.1)
        fill(&sp, STRING_METHODS, |k| string_method(k, Rc::from("")));
        // Annex B aliases are the same function objects (§B.2.2.15–16)
        let (ts, te) = (it_get(&sp, "trimStart"), it_get(&sp, "trimEnd")); def(&sp, "trimLeft", ts); def(&sp, "trimRight", te);
        Interp::define(&sp, K_ITER, nat!("[Symbol.iterator]", |it, this, _a| { let s = this_str(it, &this)?; Ok(new_iter(IterSt::Str(Rc::from(s.as_str()), 0))) }), A_NOENUM);
        let string = ctor("String", &sp, |it, this, a| {
            let s = match a.first() { None => Rc::from(""), Some(Val::Sym(s)) if !is_new(&this) => Rc::from(format!("Symbol({})", s.desc.as_deref().unwrap_or("")).as_str()), Some(v) => it.to_str(v)? };
            Ok(if is_new(&this) { Val::obj(Kind::Prim(Val::Str(s))) } else { Val::Str(s) })
        });
        statics(&string, vec![
            ("fromCharCode", nat!("fromCharCode", |it, _t, a| { let mut u = Vec::with_capacity(a.len()); for x in &a { u.push(crate::jsi::to_uint32(it.to_num(x)?) as u16); } Ok(st(&from16(&u))) })),
            ("fromCodePoint", nat!("fromCodePoint", |it, _t, a| { let mut s = String::new(); for x in &a { let n = it.to_num(x)?; if n.fract() != 0.0 || !(0.0..=1114111.0).contains(&n) { return Err(throw(it, "RangeError", &format!("Invalid code point {}", num_str(n)))); } let c = n as u32; if (0xD800..=0xDFFF).contains(&c) { s.push(char::from_u32(0xF0000 + c).unwrap()); } else { s.push(char::from_u32(c).unwrap()); } } Ok(st(&crate::jsi::fix_pairs(s))) })),
            ("raw", nat!("raw", |it, _t, a| { let strs = arg(&a, 0); let raw = it.get(&strs, "raw")?; let parts = list_of(it, &raw)?; let mut o = String::new(); for (i, p) in parts.iter().enumerate() { o += &it.to_str(p)?; if i + 1 < parts.len() { if let Some(v) = a.get(i + 1) { o += &it.to_str(v)?; } } } Ok(st(&o)) })),
        ]);
        put("String", string);

        // Number (§21.1)
        fill(&np, NUMBER_METHODS, |k| number_method(k, 0.0));
        let pint = nat!("parseInt", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; let r = if matches!(arg(&a, 1), Val::Undef) { 0 } else { let n = it.to_num(&arg(&a, 1))?; if n.is_nan() || n == 0.0 { 0 } else { crate::jsi::to_int32(n) as u32 } }; Ok(Val::Num(parse_int(&s, r))) });
        let pfloat = nat!("parseFloat", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; Ok(Val::Num(parse_float(&s))) });
        let number = ctor("Number", &np, |it, this, a| { let n = match a.first() { None => 0.0, Some(Val::Big(b)) => b.to_f64(), Some(v) => { let p = it.to_numeric(v)?; match p { Val::Big(b) => b.to_f64(), x => it.to_num(&x)? } } }; Ok(if is_new(&this) { Val::obj(Kind::Prim(Val::Num(n))) } else { Val::Num(n) }) });
        statics(&number, vec![
            ("isInteger", nat!("isInteger", |_it, _this, a| Ok(Val::Bool(matches!(arg(&a, 0), Val::Num(x) if x.is_finite() && x.fract() == 0.0))))),
            ("isFinite", nat!("isFinite", |_it, _this, a| Ok(Val::Bool(matches!(arg(&a, 0), Val::Num(x) if x.is_finite()))))),
            ("isNaN", nat!("isNaN", |_it, _this, a| Ok(Val::Bool(matches!(arg(&a, 0), Val::Num(x) if x.is_nan()))))),
            ("isSafeInteger", nat!("isSafeInteger", |_it, _this, a| Ok(Val::Bool(matches!(arg(&a, 0), Val::Num(x) if x.is_finite() && x.fract() == 0.0 && x.abs() <= 9007199254740991.0))))),
            ("parseFloat", pfloat.clone()), ("parseInt", pint.clone()),
            ("MAX_SAFE_INTEGER", Val::Num(9007199254740991.0)), ("MIN_SAFE_INTEGER", Val::Num(-9007199254740991.0)),
            ("EPSILON", Val::Num(f64::EPSILON)), ("MAX_VALUE", Val::Num(f64::MAX)), ("MIN_VALUE", Val::Num(5e-324)),
            ("POSITIVE_INFINITY", Val::Num(f64::INFINITY)), ("NEGATIVE_INFINITY", Val::Num(f64::NEG_INFINITY)), ("NaN", Val::Num(f64::NAN)),
        ]);
        put("Number", number);
        put("parseInt", pint); put("parseFloat", pfloat);

        // Boolean (§20.3)
        def(&bp, "toString", nat!("toString", |it, this, _a| Ok(st(match &this { Val::Bool(b) => if *b { "true" } else { "false" }, Val::Obj(o) => match &o.borrow().kind { Kind::Prim(Val::Bool(b)) => if *b { "true" } else { "false" }, _ => return Err(throw(it, "TypeError", "not a boolean")) }, _ => return Err(throw(it, "TypeError", "not a boolean")) }))));
        def(&bp, "valueOf", nat!("valueOf", |it, this, _a| Ok(match &this { Val::Bool(_) => this.clone(), Val::Obj(o) => match &o.borrow().kind { Kind::Prim(v @ Val::Bool(_)) => v.clone(), _ => return Err(throw(it, "TypeError", "not a boolean")) }, _ => return Err(throw(it, "TypeError", "not a boolean")) })));
        put("Boolean", ctor("Boolean", &bp, |_it, this, a| { let b = arg(&a, 0).truthy(); Ok(if is_new(&this) { Val::obj(Kind::Prim(Val::Bool(b))) } else { Val::Bool(b) }) }));

        // Symbol (§20.4)
        def(&syp, "toString", nat!("toString", |it, this, _a| { let s = sym_of(it, &this)?; Ok(st(&format!("Symbol({})", s.desc.as_deref().unwrap_or("")))) }));
        def(&syp, "valueOf", nat!("valueOf", |it, this, _a| Ok(Val::Sym(sym_of(it, &this)?))));
        Interp::define_accessor(&syp, "description", Some(nat!("description", |it, this, _a| Ok(sym_of(it, &this)?.desc.clone().map(Val::Str).unwrap_or(Val::Undef)))), None, A_NOENUM);
        Interp::define(&syp, K_TO_PRIM, nat!("[Symbol.toPrimitive]", |it, this, _a| Ok(Val::Sym(sym_of(it, &this)?))), A_NOENUM | A_NOWRITE);
        tag(&syp, "Symbol");
        let symbol = ctor("Symbol", &syp, |it, this, a| { if is_new(&this) { return Err(throw(it, "TypeError", "Symbol is not a constructor")); } let d = match arg(&a, 0) { Val::Undef => None, v => Some(it.to_str(&v)?) }; Ok(it.new_symbol(d)) });
        let mut sstat: Vec<(&str, Val)> = wk.iter().map(|(n, v)| (*n, v.clone())).collect();
        sstat.push(("for", nat!("for", |it, _t, a| { let k = it.to_str(&arg(&a, 0))?; if let Some(v) = it.sym_for.get(&k) { return Ok(v.clone()); } let s = it.new_symbol(Some(k.clone())); it.sym_for.insert(k, s.clone()); Ok(s) })));
        sstat.push(("keyFor", nat!("keyFor", |it, _t, a| { let s = sym_of(it, &arg(&a, 0))?; Ok(match it.sym_for.iter().find(|(_, v)| matches!(v, Val::Sym(x) if Rc::ptr_eq(x, &s))) { Some((k, _)) => Val::Str(k.clone()), None => Val::Undef }) })));
        statics(&symbol, sstat);
        put("Symbol", symbol);

        // BigInt (§21.2)
        def(&bgp, "toString", nat!("toString", |it, this, a| { let b = big_of(it, &this)?; let r = if matches!(arg(&a, 0), Val::Undef) { 10.0 } else { it.to_num(&arg(&a, 0))? }; if !(2.0..=36.0).contains(&r) { return Err(throw(it, "RangeError", "toString() radix must be between 2 and 36")); } Ok(st(&b.to_string_radix(r as u32))) }));
        def(&bgp, "toLocaleString", nat!("toLocaleString", |it, this, _a| { let b = big_of(it, &this)?; let s = b.to_string(); let (sign, d) = if let Some(x) = s.strip_prefix('-') { ("-", x.to_string()) } else { ("", s.clone()) }; let bb = d.as_bytes(); let mut o = String::new(); for (i, c) in bb.iter().enumerate() { if i > 0 && (bb.len() - i) % 3 == 0 { o.push(','); } o.push(*c as char); } Ok(st(&format!("{sign}{o}"))) }));
        def(&bgp, "valueOf", nat!("valueOf", |it, this, _a| Ok(Val::Big(big_of(it, &this)?))));
        tag(&bgp, "BigInt");
        let bigint = ctor("BigInt", &bgp, |it, this, a| { if is_new(&this) { return Err(throw(it, "TypeError", "BigInt is not a constructor")); } let v = arg(&a, 0); let p = it.to_primitive(&v, 1)?; Ok(Val::Big(Rc::new(to_bigint(it, &p, true)?))) });
        statics(&bigint, vec![
            ("asIntN", nat!("asIntN", |it, _t, a| { let n = it.to_num(&arg(&a, 0))?.max(0.0) as usize; let b = to_bigint(it, &arg(&a, 1), false)?; Ok(Val::Big(Rc::new(b.as_n(n, true)))) })),
            ("asUintN", nat!("asUintN", |it, _t, a| { let n = it.to_num(&arg(&a, 0))?.max(0.0) as usize; let b = to_bigint(it, &arg(&a, 1), false)?; Ok(Val::Big(Rc::new(b.as_n(n, false)))) })),
        ]);
        put("BigInt", bigint);

        // Errors (§20.5)
        def(&ep, "name", st("Error")); def(&ep, "message", st(""));
        def(&ep, "toString", nat!("toString", |it, this, _a| { if !matches!(this, Val::Obj(_)) { return Err(throw(it, "TypeError", "Error.prototype.toString called on non-object")); } let n = it.get(&this, "name")?; let m = it.get(&this, "message")?; let n = if matches!(n, Val::Undef) { Rc::from("Error") } else { it.to_str(&n)? }; let m = if matches!(m, Val::Undef) { Rc::from("") } else { it.to_str(&m)? }; Ok(st(&if m.is_empty() { n.to_string() } else if n.is_empty() { m.to_string() } else { format!("{n}: {m}") })) }));
        let make_err = |name: &'static str, proto: &ORef| ctor(name, proto, move |it, _this, a| {
            let (msg_i, errors) = if name == "AggregateError" { (1, Some(arg(&a, 0))) } else { (0, None) };
            let m = arg(&a, msg_i);
            let ms = if matches!(m, Val::Undef) { String::new() } else { it.to_str(&m)?.to_string() };
            let e = it.error(name, &ms);
            if let Val::Obj(o) = &e {
                if matches!(m, Val::Undef) { o.borrow_mut().props.retain(|(k, _)| &**k != "message"); }
                let opts = arg(&a, msg_i + 1);
                if matches!(opts, Val::Obj(_)) && it.has_prop(&opts, "cause")? { let c = it.get(&opts, "cause")?; Interp::define(o, "cause", c, A_NOENUM); }
                if let Some(es) = errors { let list = it.iterate(&es)?; Interp::define(o, "errors", Val::array(list), A_NOENUM); }
            }
            Ok(e)
        });
        let error = make_err("Error", &ep);
        statics(&error, vec![("captureStackTrace", nat!("captureStackTrace", |it, _t, a| { let o = arg(&a, 0); if let Val::Obj(x) = &o { let s = it.to_str(&o).map(|s| s.to_string()).unwrap_or_default(); Interp::define(x, "stack", st(&format!("{s}\n    at <anonymous>")), A_NOENUM); } Ok(Val::Undef) })), ("stackTraceLimit", Val::Num(10.0))]);
        put("Error", error.clone());
        for (name, proto) in &errs {
            def(proto, "name", st(name)); def(proto, "message", st(""));
            let c = make_err(name, proto);
            if let (Val::Obj(co), Val::Obj(eo)) = (&c, &error) { co.borrow_mut().proto = Some(eo.clone()); }
            put(name, c);
        }

        // Map / Set / WeakMap / WeakSet / WeakRef / FinalizationRegistry (§24, §26)
        fill(&mp, MAP_METHODS, |k| map_method(k, Val::Undef));
        let entries = it_get(&mp, "entries"); Interp::define(&mp, K_ITER, entries, A_NOENUM); tag(&mp, "Map");
        let map = ctor("Map", &mp, |it, _this, a| { let m = Val::obj(Kind::Map(Box::new(MapData::new()))); let src = arg(&a, 0); if !src.is_nullish() { for e in it.iterate(&src)? { if !matches!(e, Val::Obj(_)) { let es = it.to_str(&e).map(|x| x.to_string()).unwrap_or_default(); return Err(throw(it, "TypeError", &format!("Iterator value {es} is not an entry object"))); } let k = it.get(&e, "0")?; let v = it.get(&e, "1")?; if let Val::Obj(mo) = &m { if let Kind::Map(md) = &mut mo.borrow_mut().kind { md.set(k, v); } } } } Ok(m) });
        statics(&map, vec![("groupBy", nat!("groupBy", |it, _t, a| { let f = arg(&a, 1); let mut md = MapData::new(); for (i, x) in it.iterate(&arg(&a, 0))?.into_iter().enumerate() { let k = it.call(&f, Val::Undef, vec![x.clone(), Val::Num(i as f64)])?; match md.get(&k).cloned() { Some(v) => { with_arr(&v, |w| w.push(x)); } None => md.set(k, Val::array(vec![x])) } } Ok(Val::obj(Kind::Map(Box::new(md)))) }))]);
        put("Map", map);
        fill(&setp, SET_METHODS, |k| set_method(k, Val::Undef));
        let sv = it_get(&setp, "values"); Interp::define(&setp, K_ITER, sv, A_NOENUM); tag(&setp, "Set");
        put("Set", ctor("Set", &setp, |it, _this, a| { let m = Val::obj(Kind::Set(Box::new(MapData::new()))); let src = arg(&a, 0); if !src.is_nullish() { for x in it.iterate(&src)? { if let Val::Obj(mo) = &m { if let Kind::Set(md) = &mut mo.borrow_mut().kind { md.set(x, Val::Undef); } } } } Ok(m) }));
        let wm_key = |o: &ORef| Rc::as_ptr(o) as *const u8 as usize;
        def(&wmp, "get", nat!("get", |it, this, a| { let Val::Obj(k) = arg(&a, 0) else { return Ok(Val::Undef) }; let me = this_obj(it, &this, "WeakMap.prototype.get")?; let b = me.borrow(); Ok(match &b.kind { Kind::WeakMap(m) => m.get(&wm_key(&k)).filter(|(w, _)| w.upgrade().map_or(false, |x| Rc::ptr_eq(&x, &k))).map(|e| e.1.clone()).unwrap_or(Val::Undef), _ => Val::Undef }) }));
        def(&wmp, "has", nat!("has", |it, this, a| { let Val::Obj(k) = arg(&a, 0) else { return Ok(Val::Bool(false)) }; let me = this_obj(it, &this, "WeakMap.prototype.has")?; let b = me.borrow(); Ok(Val::Bool(match &b.kind { Kind::WeakMap(m) => m.get(&wm_key(&k)).map_or(false, |(w, _)| w.upgrade().map_or(false, |x| Rc::ptr_eq(&x, &k))), _ => false })) }));
        def(&wmp, "set", nat!("set", |it, this, a| { let k = weak_key(it, &arg(&a, 0))?; let me = this_obj(it, &this, "WeakMap.prototype.set")?; if let Kind::WeakMap(m) = &mut me.borrow_mut().kind { if m.len() > 64 && m.len().is_power_of_two() { m.retain(|_, (w, _)| w.strong_count() > 0); } m.insert(wm_key(&k), (Rc::downgrade(&k), arg(&a, 1))); } Ok(this) }));
        def(&wmp, "delete", nat!("delete", |it, this, a| { let Val::Obj(k) = arg(&a, 0) else { return Ok(Val::Bool(false)) }; let me = this_obj(it, &this, "WeakMap.prototype.delete")?; let mut b = me.borrow_mut(); Ok(Val::Bool(match &mut b.kind { Kind::WeakMap(m) => { let hit = m.get(&wm_key(&k)).map_or(false, |(w, _)| w.upgrade().map_or(false, |x| Rc::ptr_eq(&x, &k))); if hit { m.remove(&wm_key(&k)); } hit } _ => false })) }));
        tag(&wmp, "WeakMap");
        put("WeakMap", ctor("WeakMap", &wmp, |it, _this, a| { let m = Val::obj(Kind::WeakMap(std::collections::HashMap::new())); let src = arg(&a, 0); if !src.is_nullish() { let set = it.get(&m, "set")?; for e in it.iterate(&src)? { let k = it.get(&e, "0")?; let v = it.get(&e, "1")?; it.call(&set, m.clone(), vec![k, v])?; } } Ok(m) }));
        def(&wsp, "add", nat!("add", |it, this, a| { let k = weak_key(it, &arg(&a, 0))?; let me = this_obj(it, &this, "WeakSet.prototype.add")?; if let Kind::WeakSet(m) = &mut me.borrow_mut().kind { m.insert(wm_key(&k), Rc::downgrade(&k)); } Ok(this) }));
        def(&wsp, "has", nat!("has", |it, this, a| { let Val::Obj(k) = arg(&a, 0) else { return Ok(Val::Bool(false)) }; let me = this_obj(it, &this, "WeakSet.prototype.has")?; let b = me.borrow(); Ok(Val::Bool(match &b.kind { Kind::WeakSet(m) => m.get(&wm_key(&k)).map_or(false, |w| w.upgrade().map_or(false, |x| Rc::ptr_eq(&x, &k))), _ => false })) }));
        def(&wsp, "delete", nat!("delete", |it, this, a| { let Val::Obj(k) = arg(&a, 0) else { return Ok(Val::Bool(false)) }; let me = this_obj(it, &this, "WeakSet.prototype.delete")?; let mut b = me.borrow_mut(); Ok(Val::Bool(match &mut b.kind { Kind::WeakSet(m) => m.remove(&wm_key(&k)).is_some(), _ => false })) }));
        tag(&wsp, "WeakSet");
        put("WeakSet", ctor("WeakSet", &wsp, |it, _this, a| { let m = Val::obj(Kind::WeakSet(std::collections::HashMap::new())); let src = arg(&a, 0); if !src.is_nullish() { let add = it.get(&m, "add")?; for x in it.iterate(&src)? { it.call(&add, m.clone(), vec![x])?; } } Ok(m) }));
        def(&wrp, "deref", nat!("deref", |_it, this, _a| { if let Val::Obj(o) = &this { if let Kind::WeakRef(w) = &o.borrow().kind { return Ok(w.upgrade().map(Val::Obj).unwrap_or(Val::Undef)); } } Ok(Val::Undef) }));
        tag(&wrp, "WeakRef");
        put("WeakRef", ctor("WeakRef", &wrp, |it, _this, a| { let k = weak_key(it, &arg(&a, 0))?; Ok(Val::obj(Kind::WeakRef(Rc::downgrade(&k)))) }));
        let frp = Rc::new(std::cell::RefCell::new(crate::jsi::Obj { props: Vec::new(), kind: Kind::Plain, proto: Some(op.clone()), flags: 0, attrs: Vec::new() }));
        def(&frp, "register", noop.clone()); def(&frp, "unregister", nat!("unregister", |_i, _t, _a| Ok(Val::Bool(false))));
        put("FinalizationRegistry", ctor("FinalizationRegistry", &frp, |_it, _this, _a| Ok(Val::obj(Kind::Plain))));

        // Promise (§27.2)
        fill(&pp, PROMISE_METHODS, |k| promise_method(k, Val::Undef)); tag(&pp, "Promise");
        let promise = ctor("Promise", &pp, |it, _this, a| {
            let exec = arg(&a, 0);
            if !exec.is_callable() { let es = it.to_str(&exec).map(|x| x.to_string()).unwrap_or_default(); return Err(throw(it, "TypeError", &format!("Promise resolver {es} is not a function"))); }
            let p = crate::jsi::new_promise();
            let (res, rej) = it.resolving_functions(&p);
            if let Err(Ctl::Throw(e)) = it.call(&exec, Val::Undef, vec![res, rej.clone()]) { it.call(&rej, Val::Undef, vec![e])?; }
            Ok(p)
        });
        statics(&promise, vec![
            ("resolve", nat!("resolve", |it, _this, a| Ok(it.promise_resolve(arg(&a, 0))))),
            ("reject", nat!("reject", |_it, _this, a| Ok(rejected(arg(&a, 0))))),
            ("all", nat!("all", |it, _this, a| promise_combinator(it, &arg(&a, 0), 0))),
            ("allSettled", nat!("allSettled", |it, _this, a| promise_combinator(it, &arg(&a, 0), 1))),
            ("any", nat!("any", |it, _this, a| promise_combinator(it, &arg(&a, 0), 2))),
            ("race", nat!("race", |it, _this, a| promise_combinator(it, &arg(&a, 0), 3))),
            ("withResolvers", nat!("withResolvers", |it, _this, _a| { let p = crate::jsi::new_promise(); let (res, rej) = it.resolving_functions(&p); Ok(Val::plain(vec![("promise", p), ("resolve", res), ("reject", rej)])) })),
        ]);
        put("Promise", promise);

        // RegExp (§22.2)
        fill(&rp, REGEXP_METHODS, |k| regexp_method(k, Val::Undef));
        regexp_symbol_methods(&rp);
        // RegExp(pattern, flags) (§22.2.4.1): IsRegExp via @@match, a regexp passed through unchanged when called as a
        // function with the same constructor, source/flags copied otherwise; flags canonicalised to "dgimsuvy" order
        put("RegExp", ctor("RegExp", &rp, |it, this, a| {
            let p0 = arg(&a, 0);
            let is_re_obj = matches!(&p0, Val::Obj(o) if matches!(o.borrow().kind, Kind::RegExp(..)));
            let is_regexp = if matches!(p0, Val::Obj(_)) { let m = it.get(&p0, "\u{0}@@match")?; if !matches!(m, Val::Undef) { m.truthy() } else { is_re_obj } } else { false };
            if !is_new(&this) && is_regexp && matches!(arg(&a, 1), Val::Undef) {
                let pc = it.get(&p0, "constructor")?;
                let me = it.lookup_opt(&it.global.clone(), "RegExp").unwrap_or(Val::Undef);
                if Interp::strict_eq(&pc, &me) { return Ok(p0); }
            }
            let (pat, f0) = if is_re_obj { match &p0 { Val::Obj(o) => match &o.borrow().kind { Kind::RegExp(p, f) => (p.clone(), Some(f.clone())), _ => unreachable!() }, _ => unreachable!() } }
                else if is_regexp { let src = it.get(&p0, "source")?; let fl = it.get(&p0, "flags")?; (it.to_str(&src)?.to_string(), Some(it.to_str(&fl)?.to_string())) }
                else { (match &p0 { Val::Undef => String::new(), v => it.to_str(v)?.to_string() }, None) };
            let flags = match arg(&a, 1) { Val::Undef => f0.unwrap_or_default(), v => it.to_str(&v)?.to_string() };
            if let Some(c) = flags.chars().find(|c| !"dgimsuvy".contains(*c)) { return Err(throw(it, "SyntaxError", &format!("Invalid flags supplied to RegExp constructor '{c}'"))); }
            if flags.chars().enumerate().any(|(i, c)| flags[..i].contains(c)) { return Err(throw(it, "SyntaxError", &format!("Invalid flags supplied to RegExp constructor '{flags}'"))); }
            let flags: String = "dgimsuvy".chars().filter(|c| flags.contains(*c)).collect();
            crate::jsi_re::compile(&pat, &flags).map_err(|e| Ctl::Throw(it.error("SyntaxError", &format!("Invalid regular expression: /{pat}/{flags}: {e}"))))?;
            let r = Val::obj(Kind::RegExp(pat, flags));
            if let Val::Obj(ro) = &r { Interp::define(ro, "lastIndex", Val::Num(0.0), A_NOENUM); }
            Ok(r)
        }));

        // Date (§21.4)
        for (k, v) in crate::jsi_date::date_proto() { def(&dp, k, v); }
        Interp::define(&dp, K_TO_PRIM, nat!("[Symbol.toPrimitive]", |it, this, a| { let h = arg(&a, 0); let hs = it.to_str(&h)?; let order = if &*hs == "number" { ["valueOf", "toString"] } else { ["toString", "valueOf"] }; for m in order { let f = it.get(&this, m)?; if f.is_callable() { let r = it.call(&f, this.clone(), vec![])?; if !matches!(r, Val::Obj(_)) { return Ok(r); } } } Err(throw(it, "TypeError", "Cannot convert object to primitive value")) }), A_NOENUM | A_NOWRITE);
        let date = ctor("Date", &dp, |it, t, a| crate::jsi_date::date_ctor(it, t, a));
        if let Val::Obj(o) = &date { for (k, v) in crate::jsi_date::date_statics() { def(o, &k, v); } }
        put("Date", date);

        // Math (§21.3)
        let m1 = |name: &'static str, f: fn(f64) -> f64| (name, nat!(name, |it, _this, a| Ok(Val::Num(f(it.to_num(&arg(&a, 0))?)))));
        let mut math = vec![
            m1("abs", f64::abs), m1("floor", f64::floor), m1("ceil", f64::ceil), m1("trunc", f64::trunc), m1("sqrt", f64::sqrt), m1("cbrt", f64::cbrt),
            m1("exp", f64::exp), m1("log", f64::ln), m1("log2", f64::log2), m1("log10", f64::log10), m1("sin", f64::sin), m1("cos", f64::cos), m1("tan", f64::tan),
            m1("asin", f64::asin), m1("acos", f64::acos), m1("atan", f64::atan), m1("sinh", f64::sinh), m1("cosh", f64::cosh), m1("tanh", f64::tanh), m1("asinh", f64::asinh), m1("acosh", f64::acosh), m1("atanh", f64::atanh),
            m1("log1p", f64::ln_1p), m1("expm1", f64::exp_m1),
            m1("round", |x| if x.is_finite() { let r = (x + 0.5).floor(); if r == 0.0 && x < 0.0 { -0.0 } else { r } } else { x }),
            m1("sign", |x| if x.is_nan() { f64::NAN } else if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { x }),
            m1("fround", |x| x as f32 as f64),
            m1("clz32", |x| crate::jsi::to_uint32(x).leading_zeros() as f64),
        ];
        math.push(("atan2", nat!("atan2", |it, _this, a| { let y = it.to_num(&arg(&a, 0))?; let x = it.to_num(&arg(&a, 1))?; Ok(Val::Num(y.atan2(x))) })));
        math.push(("imul", nat!("imul", |it, _this, a| { let x = crate::jsi::to_int32(it.to_num(&arg(&a, 0))?); let y = crate::jsi::to_int32(it.to_num(&arg(&a, 1))?); Ok(Val::Num(x.wrapping_mul(y) as f64)) })));
        math.push(("pow", nat!("pow", |it, _this, a| { let x = it.to_num(&arg(&a, 0))?; let y = it.to_num(&arg(&a, 1))?; Ok(Val::Num(if y.is_nan() || (x.abs() == 1.0 && y.is_infinite()) { f64::NAN } else { x.powf(y) })) })));
        math.push(("max", nat!("max", |it, _this, a| { let mut m = f64::NEG_INFINITY; let mut nan = false; for x in &a { let n = it.to_num(x)?; if n.is_nan() { nan = true; } if n > m || (n == 0.0 && m == 0.0 && m.is_sign_negative()) { m = n; } } Ok(Val::Num(if nan { f64::NAN } else { m })) })));
        math.push(("min", nat!("min", |it, _this, a| { let mut m = f64::INFINITY; let mut nan = false; for x in &a { let n = it.to_num(x)?; if n.is_nan() { nan = true; } if n < m || (n == 0.0 && m == 0.0 && n.is_sign_negative()) { m = n; } } Ok(Val::Num(if nan { f64::NAN } else { m })) })));
        math.push(("hypot", nat!("hypot", |it, _this, a| { let mut s = 0.0; let mut inf = false; for x in &a { let n = it.to_num(x)?; if n.is_infinite() { inf = true; } s += n * n; } Ok(Val::Num(if inf { f64::INFINITY } else { s.sqrt() })) })));
        math.push(("random", nat!("random", |it, _this, _a| { let x = (it.next_rand() >> 11) as f64 / (1u64 << 53) as f64; Ok(Val::Num(x)) })));
        let mut mo = math; mo.push(("PI", Val::Num(std::f64::consts::PI))); mo.push(("E", Val::Num(std::f64::consts::E))); mo.push(("LN2", Val::Num(std::f64::consts::LN_2))); mo.push(("LN10", Val::Num(std::f64::consts::LN_10)));
        mo.push(("LOG2E", Val::Num(std::f64::consts::LOG2_E))); mo.push(("LOG10E", Val::Num(std::f64::consts::LOG10_E))); mo.push(("SQRT2", Val::Num(std::f64::consts::SQRT_2))); mo.push(("SQRT1_2", Val::Num(std::f64::consts::FRAC_1_SQRT_2)));
        let math = Val::plain(mo); if let Val::Obj(o) = &math { let at: Vec<(Rc<str>, u8)> = o.borrow().props.iter().map(|(k, _)| (k.clone(), A_NOENUM)).collect(); o.borrow_mut().attrs = at; tag(o, "Math"); }
        put("Math", math);

        // JSON (§25.5)
        let json = Val::plain(vec![
            ("stringify", nat!("stringify", |it, _this, a| Ok(match json_stringify(it, &arg(&a, 0), &arg(&a, 1), &arg(&a, 2))? { Some(s) => st(&s), None => Val::Undef }))),
            ("parse", nat!("parse", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; let v = json_parse(&s).ok_or_else(|| throw(it, "SyntaxError", &format!("Unexpected token in JSON at position 0 (\"{}\")", s.chars().take(20).collect::<String>())))?; let f = arg(&a, 1); if f.is_callable() { let holder = Val::obj(Kind::Plain); Interp::put(holder.as_obj().unwrap(), Rc::from(""), v); return revive(it, &holder, "", &f); } Ok(v) })),
        ]);
        if let Val::Obj(o) = &json { tag(o, "JSON"); }
        put("JSON", json);

        // Reflect (§28.1) and Proxy (§28.2)
        put("Reflect", Val::plain(vec![
            ("apply", nat!("apply", |it, _t, a| { let args = list_of(it, &arg(&a, 2))?; it.call(&arg(&a, 0), arg(&a, 1), args) })),
            ("construct", nat!("construct", |it, _t, a| { let f = arg(&a, 0); let args = list_of(it, &arg(&a, 1))?; let nt = if a.len() > 2 { arg(&a, 2) } else { f.clone() }; it.construct_nt(&f, args, &nt) })),
            ("defineProperty", nat!("defineProperty", |it, _t, a| { let k = it.to_key(&arg(&a, 1))?; Ok(Val::Bool(define_from_desc(it, &arg(&a, 0), &k, &arg(&a, 2)).is_ok())) })),
            ("deleteProperty", nat!("deleteProperty", |it, _t, a| { let k = it.to_key(&arg(&a, 1))?; Ok(Val::Bool(it.delete(&arg(&a, 0), &k)?)) })),
            ("get", nat!("get", |it, _t, a| { let o = arg(&a, 0); if !matches!(o, Val::Obj(_)) { return Err(throw(it, "TypeError", "Reflect.get called on non-object")); } let k = it.to_key(&arg(&a, 1))?; if a.len() > 2 { let o2 = o.as_obj().unwrap().clone(); return it.get_obj(&o2, &k, &arg(&a, 2)); } it.get(&o, &k) })),
            ("set", nat!("set", |it, _t, a| { let o = arg(&a, 0); if !matches!(o, Val::Obj(_)) { return Err(throw(it, "TypeError", "Reflect.set called on non-object")); } let k = it.to_key(&arg(&a, 1))?; let recv = if a.len() > 3 { arg(&a, 3) } else { o.clone() }; Ok(Val::Bool(it.set_with_receiver(&o, &k, arg(&a, 2), &recv)?)) })),
            ("has", nat!("has", |it, _t, a| { let k = it.to_key(&arg(&a, 1))?; Ok(Val::Bool(it.has_prop(&arg(&a, 0), &k)?)) })),
            ("ownKeys", nat!("ownKeys", |it, _t, a| { let o = arg(&a, 0); if let Val::Obj(ob) = &o { let px = if let Kind::Proxy(t, h) = &ob.borrow().kind { Some((t.clone(), h.clone())) } else { None }; if let Some((t, h)) = px { let trap = it.get(&h, "ownKeys")?; if trap.is_callable() { return it.call(&trap, h, vec![t]); } } } let ks = it.own_keys(&o, true, true, true); Ok(Val::array(ks.iter().map(|k| it.key_val(k)).collect())) })),
            ("getOwnPropertyDescriptor", nat!("getOwnPropertyDescriptor", |it, _t, a| { let k = it.to_key(&arg(&a, 1))?; desc_obj(it, &arg(&a, 0), &k) })),
            ("getPrototypeOf", nat!("getPrototypeOf", |it, _t, a| Ok(match it.proto_of(&arg(&a, 0))? { Some(p) => Val::Obj(p), None => Val::Null }))),
            ("setPrototypeOf", nat!("setPrototypeOf", |it, _t, a| { let p = match arg(&a, 1) { Val::Obj(p) => Some(p), _ => None }; Ok(Val::Bool(match arg(&a, 0) { Val::Obj(o) => it.set_proto(&o, p), _ => false })) })),
            ("isExtensible", nat!("isExtensible", |_it, _t, a| Ok(Val::Bool(obj_flags(&arg(&a, 0)).map_or(false, |f| f & (F_NOEXT | F_SEALED | F_FROZEN) == 0))))),
            ("preventExtensions", nat!("preventExtensions", |_it, _t, a| { if let Val::Obj(o) = arg(&a, 0) { o.borrow_mut().flags |= F_NOEXT; } Ok(Val::Bool(true)) })),
        ]));
        let proxy = Val::native("Proxy", |it, this, a| { if !is_new(&this) { return Err(throw(it, "TypeError", "Constructor Proxy requires 'new'")); } let (t, h) = (arg(&a, 0), arg(&a, 1)); if !matches!(t, Val::Obj(_)) || !matches!(h, Val::Obj(_)) { return Err(throw(it, "TypeError", "Cannot create proxy with a non-object as target or handler")); } Ok(Val::obj(Kind::Proxy(t, h))) });
        statics(&proxy, vec![("revocable", nat!("revocable", |it, _t, a| { let p = Val::obj(Kind::Proxy(arg(&a, 0), arg(&a, 1))); let p2 = p.clone(); let _ = it; Ok(Val::plain(vec![("proxy", p), ("revoke", Val::native("revoke", move |_i, _t, _a| { if let Val::Obj(o) = &p2 { o.borrow_mut().kind = Kind::Proxy(Val::obj(Kind::Plain), Val::obj(Kind::Plain)); } Ok(Val::Undef) }))])) }))]);
        put("Proxy", proxy);

        // iterators and generators (§27.1, §27.5, §27.6)
        Interp::define(&itp, K_ITER, nat!("[Symbol.iterator]", |_it, this, _a| Ok(this)), A_NOENUM);
        for name in ["toArray", "forEach", "map", "filter", "take", "drop", "reduce", "some", "every", "find", "flatMap"] {
            def(&itp, name, Val::native(name, move |it, this, a| {
                let f = arg(&a, 0);
                let mut src = it.iter_open(&this)?;
                let mut out = Vec::new(); let mut i = 0usize;
                let lim = if name == "take" || name == "drop" { let n = it.to_num(&f)?; if n.is_nan() || n < 0.0 { return Err(throw(it, "RangeError", &format!("{} must be positive", num_str(n)))); } n as usize } else { 0 };
                let mut acc = if a.len() > 1 { Some(arg(&a, 1)) } else { None };
                loop {
                    if name == "take" && i >= lim { it.iter_close(&src)?; break; }
                    let Some(x) = it.iter_step(&mut src)? else { break };
                    match name {
                        "toArray" | "take" => out.push(x),
                        "drop" => if i >= lim { out.push(x) },
                        "forEach" => { it.call(&f, Val::Undef, vec![x, Val::Num(i as f64)])?; }
                        "map" => out.push(it.call(&f, Val::Undef, vec![x, Val::Num(i as f64)])?),
                        "filter" => if it.call(&f, Val::Undef, vec![x.clone(), Val::Num(i as f64)])?.truthy() { out.push(x) },
                        "flatMap" => { let r = it.call(&f, Val::Undef, vec![x, Val::Num(i as f64)])?; out.extend(it.iterate(&r)?); }
                        "reduce" => acc = Some(match acc.take() { None => x, Some(p) => it.call(&f, Val::Undef, vec![p, x, Val::Num(i as f64)])? }),
                        "some" => if it.call(&f, Val::Undef, vec![x, Val::Num(i as f64)])?.truthy() { it.iter_close(&src)?; return Ok(Val::Bool(true)); },
                        "every" => if !it.call(&f, Val::Undef, vec![x, Val::Num(i as f64)])?.truthy() { it.iter_close(&src)?; return Ok(Val::Bool(false)); },
                        _ => if it.call(&f, Val::Undef, vec![x.clone(), Val::Num(i as f64)])?.truthy() { it.iter_close(&src)?; return Ok(x); },
                    }
                    i += 1;
                }
                Ok(match name { "toArray" => Val::array(out), "forEach" => Val::Undef, "reduce" => match acc { Some(v) => v, None => return Err(throw(it, "TypeError", "Reduce of empty iterator with no initial value")) }, "some" => Val::Bool(false), "every" => Val::Bool(true), "find" => Val::Undef, _ => new_iter(IterSt::List(out, 0)) })
            }));
        }
        // %Iterator% is abstract (§27.1.3.1): neither callable nor directly constructible
        let iterator = ctor("Iterator", &itp, |it, t, _a| { let direct = !is_new(&t) || matches!(&it.new_target, Some(Val::Obj(o)) if matches!(&o.borrow().kind, Kind::Native(_, "Iterator"))); if direct { return Err(throw(it, "TypeError", "Abstract class Iterator not directly constructable")); } Ok(Val::obj(Kind::Plain)) });
        statics(&iterator, vec![("from", nat!("from", |it, _t, a| { let v = arg(&a, 0); let r = it.iter_open(&v)?; Ok(match r { crate::jsi::IterRec::Proto(i, _) => i, crate::jsi::IterRec::Bi(o) => Val::Obj(o), _ => { let items = it.iterate(&v)?; new_iter(IterSt::List(items, 0)) } }) }))]);
        put("Iterator", iterator);
        def(&aip, "next", nat!("next", |it, this, _a| { let o = this_obj(it, &this, "Iterator.prototype.next")?; let v = it.builtin_next(&o); Ok(match v { Some(v) => Interp::iter_result(v, false), None => Interp::iter_result(Val::Undef, true) }) }));
        tag(&aip, "Array Iterator");
        let gen_of = |it: &Interp, this: &Val| -> Result<Rc<std::cell::RefCell<crate::jsi::Coro>>, Ctl> { if let Val::Obj(o) = this { if let Kind::Gen(c) = &o.borrow().kind { return Ok(c.clone()); } } Err(throw(it, "TypeError", "next method called on incompatible receiver")) };
        for (name, k) in [("next", 0u8), ("return", 1), ("throw", 2)] {
            def(&gp, name, Val::native(name, move |it, this, a| { let c = gen_of(it, &this)?; let r = match k { 0 => Resume::Next(arg(&a, 0)), 1 => Resume::Return(arg(&a, 0)), _ => Resume::Throw(arg(&a, 0)) }; let (v, done) = it.gen_resume(&c, r)?; Ok(Interp::iter_result(v, done)) }));
            def(&agp, name, Val::native(name, move |it, this, a| { let c = match gen_of(it, &this) { Ok(c) => c, Err(Ctl::Throw(e)) => return Ok(rejected(e)), Err(e) => return Err(e) }; let r = match k { 0 => Resume::Next(arg(&a, 0)), 1 => Resume::Return(arg(&a, 0)), _ => Resume::Throw(arg(&a, 0)) }; Ok(it.agen_request(&c, r)) }));
        }
        tag(&gp, "Generator"); tag(&agp, "AsyncGenerator");
        Interp::define(&agp, K_ASYNC_ITER, nat!("[Symbol.asyncIterator]", |_it, this, _a| Ok(this)), A_NOENUM);

        // typed arrays, ArrayBuffer, DataView (§23.2, §25.1, §25.3)
        fill(&tp, TYPED_METHODS, typed_method);
        let tv = it_get(&tp, "values"); Interp::define(&tp, K_ITER, tv, A_NOENUM);
        for (ty, name) in TNAMES.iter().enumerate() {
            let proto = &each[ty];
            let c = ctor(name, proto, move |it, t, a| { if !is_new(&t) { return Err(throw(it, "TypeError", &format!("Constructor {} requires 'new'", TNAMES[ty]))); } typed_ctor(it, ty as u8, &a) });
            let bpe = Val::Num(elem_size(ty as u8) as f64);
            def(proto, "BYTES_PER_ELEMENT", bpe.clone());
            let (from, of) = (it_get_v(&put_lookup_array(&g), "from"), it_get_v(&put_lookup_array(&g), "of"));
            statics(&c, vec![("BYTES_PER_ELEMENT", bpe), ("from", from), ("of", of)]);
            tag(proto, name);
            put(name, c);
        }
        def(&bufp, "slice", nat!("slice", |it, this, a| { let b = buf_rc(&this).ok_or_else(|| throw(it, "TypeError", "not an ArrayBuffer"))?; let len = b.borrow().len(); let s = rel_arg(it, &a, 0, len, 0)?; let e = rel_arg(it, &a, 1, len, len)?; let v = if s < e { b.borrow()[s..e].to_vec() } else { vec![] }; Ok(Val::obj(Kind::Buffer(Rc::new(std::cell::RefCell::new(v))))) }));
        tag(&bufp, "ArrayBuffer");
        let abuf = ctor("ArrayBuffer", &bufp, |it, t, a| { if !is_new(&t) { return Err(throw(it, "TypeError", "Constructor ArrayBuffer requires 'new'")); } let n = it.to_num(&arg(&a, 0))?; if n < 0.0 || n > 2e9 { return Err(throw(it, "RangeError", "Array buffer allocation failed")); } Ok(new_buffer(if n.is_nan() { 0 } else { n as usize })) });
        statics(&abuf, vec![("isView", nat!("isView", |_it, _t, a| Ok(Val::Bool(matches!(arg(&a, 0), Val::Obj(ref o) if matches!(o.borrow().kind, Kind::Typed(_) | Kind::View(_)))))))]);
        put("ArrayBuffer", abuf);
        for m in ["getInt8", "getUint8", "getInt16", "getUint16", "getInt32", "getUint32", "getFloat32", "getFloat64", "getBigInt64", "getBigUint64", "setInt8", "setUint8", "setInt16", "setUint16", "setInt32", "setUint32", "setFloat32", "setFloat64", "setBigInt64", "setBigUint64"] { def(&vp, m, dataview_method(m)); }
        tag(&vp, "DataView");
        put("DataView", ctor("DataView", &vp, |it, t, a| { if !is_new(&t) { return Err(throw(it, "TypeError", "Constructor DataView requires 'new'")); } let bo = arg(&a, 0); let b = buf_rc(&bo).ok_or_else(|| throw(it, "TypeError", "First argument to DataView constructor must be an ArrayBuffer"))?; let blen = b.borrow().len(); let off = match arg(&a, 1) { Val::Undef => 0, v => it.to_num(&v)? as usize }; let len = match arg(&a, 2) { Val::Undef => blen.saturating_sub(off), v => it.to_num(&v)? as usize }; if off + len > blen { return Err(throw(it, "RangeError", "Invalid DataView length")); } Ok(Val::obj(Kind::View(Box::new(TArr { buf: b, bufobj: bo, ty: 0, off, len })))) }));

        // global functions and objects (§19, HTML, WHATWG)
        put("globalThis", Val::obj(Kind::Global));
        put("isNaN", nat!("isNaN", |it, _this, a| Ok(Val::Bool(it.to_num(&arg(&a, 0))?.is_nan()))));
        put("isFinite", nat!("isFinite", |it, _this, a| Ok(Val::Bool(it.to_num(&arg(&a, 0))?.is_finite()))));
        let eval = nat!("eval", |it, _this, a| { match a.first() { Some(Val::Str(s)) => { let s = s.clone(); let g = it.global.clone(); it.eval_src(&s, &g) } Some(v) => Ok(v.clone()), None => Ok(Val::Undef) } });
        self.eval_fn = Some(eval.clone());
        put("eval", eval);
        let timer = |repeat: bool| Val::native(if repeat { "setInterval" } else { "setTimeout" }, move |it, _this, a| { let ms = it.to_num(&arg(&a, 1)).unwrap_or(0.0); let f = arg(&a, 0); let f = if let Val::Str(src) = &f { let src = src.clone(); Val::native("timer", move |it, _t, _a| { let g = it.global.clone(); it.eval_src(&src, &g) }) } else { f }; let args: Vec<Val> = a.iter().skip(2).cloned().collect(); Ok(Val::Num(it.add_timer(f, ms, repeat, args) as f64)) });
        put("setTimeout", timer(false)); put("setInterval", timer(true));
        let clear = nat!("clearTimeout", |it, _this, a| { if let Val::Num(n) = arg(&a, 0) { let i = n as usize; if i >= 1 && i <= it.timers.len() { it.timers[i - 1].0 = Val::Undef; } } Ok(Val::Undef) });
        put("clearTimeout", clear.clone()); put("clearInterval", clear);
        put("queueMicrotask", nat!("queueMicrotask", |it, _this, a| { let f = arg(&a, 0); if !f.is_callable() { return Err(throw(it, "TypeError", "The \"callback\" argument must be of type function")); } it.jobs.push_back(crate::jsi::Job::Call(f, vec![])); Ok(Val::Undef) }));
        put("encodeURIComponent", nat!("encodeURIComponent", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; uri_encode(&s, "").map(|x| st(&x)).ok_or_else(|| throw(it, "URIError", "URI malformed")) }));
        put("encodeURI", nat!("encodeURI", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; uri_encode(&s, ";,/?:@&=+$#").map(|x| st(&x)).ok_or_else(|| throw(it, "URIError", "URI malformed")) }));
        put("decodeURIComponent", nat!("decodeURIComponent", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; uri_decode(&s, "").map(|x| st(&x)).ok_or_else(|| throw(it, "URIError", "URI malformed")) }));
        put("decodeURI", nat!("decodeURI", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; uri_decode(&s, ";/?:@&=+$,#").map(|x| st(&x)).ok_or_else(|| throw(it, "URIError", "URI malformed")) }));
        put("escape", nat!("escape", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; Ok(st(&js_escape(&s))) }));
        put("unescape", nat!("unescape", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; Ok(st(&js_unescape(&s))) }));
        put("btoa", nat!("btoa", |it, _t, a| { if a.is_empty() { return Err(throw(it, "TypeError", "1 argument required, but only 0 present.")); } let s = it.to_str(&arg(&a, 0))?; let u = utf16(&s); if u.iter().any(|&c| c > 255) { return Err(throw(it, "InvalidCharacterError", "Invalid character")); } let b: Vec<u8> = u.iter().map(|&c| c as u8).collect(); Ok(st(&b64_encode(&b))) }));
        put("atob", nat!("atob", |it, _t, a| { if a.is_empty() { return Err(throw(it, "TypeError", "1 argument required, but only 0 present.")); } let s = it.to_str(&arg(&a, 0))?; let b = b64_decode(&s).ok_or_else(|| throw(it, "InvalidCharacterError", "The string to be decoded is not correctly encoded."))?; Ok(st(&b.iter().map(|&c| c as char).collect::<String>())) }));
        put("structuredClone", nat!("structuredClone", |it, _t, a| { let mut memo = Vec::new(); sclone(it, &arg(&a, 0), &mut memo) }));
        put("AbortController", nat!("AbortController", |_it, _t, _a| {
            let signal = Val::plain(vec![("aborted", Val::Bool(false)), ("reason", Val::Undef), ("addEventListener", Val::native("ael", |_i, _t, _a| Ok(Val::Undef))), ("removeEventListener", Val::native("rel", |_i, _t, _a| Ok(Val::Undef)))]);
            let sg = signal.clone();
            Ok(Val::plain(vec![("signal", signal), ("abort", Val::native("abort", move |it, _t, a| { it.set(&sg, "aborted", Val::Bool(true))?; it.set(&sg, "reason", arg(&a, 0))?; Ok(Val::Undef) }))]))
        }));
        // High Resolution Time + Performance Timeline (W3C): entries are recorded marks/measures only
        let origin = crate::jsi_date::now_ms() as f64 - perf_now_ms();
        let entries = Rc::new(std::cell::RefCell::new(Vec::<Val>::new()));
        let (e1, e2, e3, e4, e5) = (entries.clone(), entries.clone(), entries.clone(), entries.clone(), entries.clone());
        put("performance", Val::plain(vec![("now", nat!("now", |_it, _t, _a| Ok(Val::Num(perf_now_ms())))), ("timeOrigin", Val::Num(origin)),
            ("mark", Val::native("mark", move |it, _t, a| { let n = it.to_str(&arg(&a, 0))?; let e = Val::plain(vec![("name", Val::Str(n)), ("entryType", st("mark")), ("startTime", Val::Num(perf_now_ms())), ("duration", Val::Num(0.0))]); e1.borrow_mut().push(e.clone()); Ok(e) })),
            ("measure", Val::native("measure", move |it, _t, a| { let n = it.to_str(&arg(&a, 0))?; let find = |k: &Val, it: &mut Interp| -> Result<f64, Ctl> { if matches!(k, Val::Undef) { return Ok(0.0); } let ks = it.to_str(k)?; for e in e2.borrow().iter().rev() { if matches!(it.get(e, "name")?, Val::Str(ref x) if *x == ks) { return it.get(e, "startTime").and_then(|v| it.to_num(&v)); } } Ok(0.0) }; let s0 = if matches!(arg(&a, 1), Val::Obj(_)) { 0.0 } else { find(&arg(&a, 1), it)? }; let s1 = if matches!(arg(&a, 2), Val::Undef) { perf_now_ms() } else { find(&arg(&a, 2), it)? }; let e = Val::plain(vec![("name", Val::Str(n)), ("entryType", st("measure")), ("startTime", Val::Num(s0)), ("duration", Val::Num(s1 - s0))]); e2.borrow_mut().push(e.clone()); Ok(e) })),
            ("getEntries", Val::native("getEntries", move |_it, _t, _a| Ok(Val::array(e3.borrow().clone())))),
            ("getEntriesByType", Val::native("getEntriesByType", move |it, _t, a| { let t = it.to_str(&arg(&a, 0))?; let mut out = Vec::new(); for e in e4.borrow().iter() { if matches!(it.get(e, "entryType")?, Val::Str(ref x) if *x == t) { out.push(e.clone()); } } Ok(Val::array(out)) })),
            ("getEntriesByName", Val::native("getEntriesByName", move |it, _t, a| { let t = it.to_str(&arg(&a, 0))?; let mut out = Vec::new(); for e in e5.borrow().iter() { if matches!(it.get(e, "name")?, Val::Str(ref x) if *x == t) { out.push(e.clone()); } } Ok(Val::array(out)) })),
            ("clearMarks", noop.clone()), ("clearMeasures", noop.clone()), ("clearResourceTimings", noop.clone()), ("setResourceTimingBufferSize", noop.clone()),
            ("timing", Val::plain(vec![("navigationStart", Val::Num(origin)), ("fetchStart", Val::Num(origin)), ("domLoading", Val::Num(origin)), ("domInteractive", Val::Num(origin)), ("domContentLoadedEventEnd", Val::Num(origin)), ("loadEventEnd", Val::Num(origin)), ("responseStart", Val::Num(origin))])),
            ("navigation", Val::plain(vec![("type", Val::Num(0.0)), ("redirectCount", Val::Num(0.0))])),
            ("toJSON", nat!("toJSON", |_it, _t, _a| Ok(Val::plain(vec![("timeOrigin", Val::Num(0.0))])))),
        ]));
        // crypto: randomness from the interpreter's own PRNG state, never the step counter
        put("crypto", Val::plain(vec![
            ("randomUUID", nat!("randomUUID", |it, _t, _a| { let (x, y) = (it.next_rand(), it.next_rand()); Ok(st(&format!("{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}", x >> 32, (x >> 16) & 0xffff, x & 0xfff, 8 + (y >> 62), (y >> 48) & 0xfff, y & 0xffff_ffff_ffff))) })),
            ("getRandomValues", nat!("getRandomValues", |it, _t, a| { let t = arg(&a, 0); let (buf, _, ty, off, len) = ta_of(it, &t)?; if ty == 7 || ty == 8 { return Err(throw(it, "TypeMismatchError", "The data is not of an integer type")); } let n = len * elem_size(ty); if n > 65536 { return Err(throw(it, "QuotaExceededError", "The ArrayBufferView's byte length exceeds 65536")); } for i in 0..n { let r = it.next_rand() as u8; buf.borrow_mut()[off + i] = r; } Ok(t) })),
        ]));
        // get [Symbol.species]() { return this } on the species-aware constructors (§23.1.2.5, §24.1.2.3, §27.2.4.8, …)
        for name in ["Array", "Map", "Set", "Promise", "RegExp", "ArrayBuffer"].iter().chain(TNAMES.iter()) { if let Some(Val::Obj(c)) = g.borrow().vars.get(*name).cloned() { Interp::define_accessor(&c, crate::jsi::K_SPECIES, Some(nat!("get [Symbol.species]", |_it, this, _a| Ok(this))), None, A_NOENUM); } }
        install_web(self, &g);
        let _ = (&itp, &op);
    }
}
fn it_get(o: &ORef, k: &str) -> Val { o.borrow().props.iter().find(|(x, _)| &**x == k).map(|x| x.1.clone()).unwrap_or(Val::Undef) }
fn it_get_v(v: &Val, k: &str) -> Val { match v { Val::Obj(o) => it_get(o, k), _ => Val::Undef } }
fn put_lookup_array(g: &crate::jsi::Scope) -> Val { g.borrow().vars.get("Array").cloned().unwrap_or(Val::Undef) }
/// pattern text for RegExp(x): a regex copies its source, a RegExpOptions-ish object reads source/flags
fn pattern_of(it: &mut Interp, v: &Val) -> Result<String, Ctl> {
    if let Val::Obj(o) = v {
        let borrowed = o.borrow();
        if let Kind::RegExp(p, _) = &borrowed.kind { return Ok(p.clone()); }
        drop(borrowed);
        let src = it.get(v, "source")?;
        if !matches!(src, Val::Undef) { return Ok(it.to_str(&src)?.to_string()); }
    }
    Ok(it.to_str(v)?.to_string())
}

// ── web globals: URL / URLSearchParams (URL Standard), TextEncoder / TextDecoder (Encoding Standard), Intl (ECMA-402, en-US) ──
fn hidden(v: &Val, k: &str) -> Val { match v { Val::Obj(o) => o.borrow().props.iter().find(|(x, _)| &**x == k).map(|x| x.1.clone()).unwrap_or(Val::Undef), _ => Val::Undef } }
fn set_hidden(v: &Val, k: &str, x: Val) { if let Val::Obj(o) = v { Interp::define(o, k, x, A_NOENUM); } }
fn url_of(it: &mut Interp, this: &Val) -> Result<Url, Ctl> { match hidden(this, "\u{0}href") { Val::Str(h) => parse_url(&h, None).ok_or_else(|| throw(it, "TypeError", "Invalid URL")), _ => Err(throw(it, "TypeError", "Receiver is not a URL")) } }
fn url_store(this: &Val, u: &Url) {
    set_hidden(this, "\u{0}href", st(&u.href()));
    let sp = hidden(this, "\u{0}sp");
    if matches!(sp, Val::Obj(_)) { set_hidden(&sp, "\u{0}list", pairs_val(&form_decode(u.query.as_deref().unwrap_or("")))); }
}
fn pairs_val(p: &[(String, String)]) -> Val { Val::array(p.iter().map(|(k, v)| Val::array(vec![st(k), st(v)])).collect()) }
fn pairs_of(it: &mut Interp, sp: &Val) -> Result<Vec<(String, String)>, Ctl> {
    let l = hidden(sp, "\u{0}list");
    let mut out = Vec::new();
    for e in arr_of(&l) { let k = it.get(&e, "0")?; let v = it.get(&e, "1")?; out.push((it.to_str(&k)?.to_string(), it.to_str(&v)?.to_string())); }
    Ok(out)
}
/// URLSearchParams update steps: the list changed — re-serialize into the owning URL's query.
fn sp_store(it: &mut Interp, sp: &Val, p: &[(String, String)]) -> Result<(), Ctl> {
    set_hidden(sp, "\u{0}list", pairs_val(p));
    let owner = hidden(sp, "\u{0}url");
    if matches!(owner, Val::Obj(_)) { let mut u = url_of(it, &owner)?; let q = form_encode(p); u.query = if q.is_empty() { None } else { Some(q) }; set_hidden(&owner, "\u{0}href", st(&u.href())); }
    Ok(())
}
fn bytes_of(it: &mut Interp, v: &Val) -> Result<Vec<u8>, Ctl> {
    match v {
        Val::Undef => Ok(vec![]),
        Val::Obj(o) => { let b = o.borrow(); match &b.kind {
            Kind::Typed(t) => { let bb = t.buf.borrow(); let n = t.len * elem_size(t.ty); Ok(bb[t.off..(t.off + n).min(bb.len())].to_vec()) }
            Kind::View(t) => { let bb = t.buf.borrow(); Ok(bb[t.off..(t.off + t.len).min(bb.len())].to_vec()) }
            Kind::Buffer(x) => Ok(x.borrow().clone()),
            _ => { drop(b); let l = list_of(it, v)?; let mut out = Vec::new(); for x in l { out.push(it.to_num(&x)? as u8); } Ok(out) }
        } }
        _ => Err(throw(it, "TypeError", "The \"input\" argument must be an instance of ArrayBuffer or ArrayBufferView")),
    }
}
/// UTF-8 encode (Encoding Standard §9.1): a lone surrogate becomes U+FFFD.
fn utf8_bytes(s: &str) -> Vec<u8> { s.chars().map(|c| if (0xF0000 + 0xD800..=0xF0000 + 0xDFFF).contains(&(c as u32)) { '\u{fffd}' } else { c }).collect::<String>().into_bytes() }
fn fmt_date_intl(t: f64, o: &[(String, String)]) -> String {
    if t.is_nan() { return "Invalid Date".into(); }
    let get = |k: &str| o.iter().find(|(x, _)| x == k).map(|x| x.1.as_str());
    let p = crate::jsi_date::local_parts(t as i64);
    let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let (ds, ts) = (get("dateStyle"), get("timeStyle"));
    let has_date = get("year").is_some() || get("month").is_some() || get("day").is_some() || get("weekday").is_some() || ds.is_some();
    let has_time = get("hour").is_some() || get("minute").is_some() || get("second").is_some() || ts.is_some();
    let (has_date, has_time) = if !has_date && !has_time { (true, false) } else { (has_date, has_time) };
    let mut date = String::new();
    if has_date {
        let month = get("month").or(match ds { Some("full") | Some("long") => Some("long"), Some("medium") => Some("short"), Some("short") => Some("numeric"), _ => None }).unwrap_or("numeric");
        let wd = get("weekday").or(if ds == Some("full") { Some("long") } else { None });
        let mname = months[(p.mon - 1).clamp(0, 11) as usize];
        let y = if ds == Some("short") { format!("{:02}", p.year % 100) } else if get("year") == Some("2-digit") { format!("{:02}", p.year % 100) } else { p.year.to_string() };
        let d = if get("day") == Some("2-digit") { format!("{:02}", p.day) } else { p.day.to_string() };
        let show_y = get("year").is_some() || ds.is_some() || (get("month").is_none() && get("day").is_none() && get("weekday").is_none());
        let show_d = get("day").is_some() || ds.is_some() || (get("month").is_none() && get("year").is_none() && get("weekday").is_none());
        let show_m = get("month").is_some() || ds.is_some() || (get("day").is_none() && get("year").is_none() && get("weekday").is_none());
        date = match month {
            "long" | "short" => { let m = if month == "long" { mname.to_string() } else { mname[..3].to_string() }; let mut s = String::new(); if show_m { s += &m; } if show_d { if !s.is_empty() { s.push(' '); } s += &d; } if show_y { if !s.is_empty() { s += if show_d { ", " } else { " " }; } s += &y; } s }
            _ => { let m = if month == "2-digit" { format!("{:02}", p.mon) } else { p.mon.to_string() }; let mut v = Vec::new(); if show_m { v.push(m); } if show_d { v.push(d.clone()); } if show_y { v.push(y.clone()); } v.join("/") }
        };
        if let Some(w) = wd { let n = days[p.wday as usize]; let n = if w == "long" { n.to_string() } else { n[..3].to_string() }; date = if date.is_empty() { n } else { format!("{n}, {date}") }; }
    }
    let mut time = String::new();
    if has_time {
        let h12 = get("hour12") != Some("false") && get("hourCycle").map_or(true, |c| c == "h12" || c == "h11");
        let show_s = get("second").is_some() || matches!(ts, Some("medium") | Some("long") | Some("full"));
        let h = if h12 { let h = p.h % 12; if h == 0 { 12 } else { h } } else { p.h };
        let hs = if get("hour") == Some("2-digit") || !h12 { format!("{h:02}") } else { h.to_string() };
        time = if get("hour").is_some() || ts.is_some() || get("minute").is_some() { format!("{hs}:{:02}", p.mi) } else { String::new() };
        if show_s { time += &format!(":{:02}", p.s); }
        if h12 { time += if p.h < 12 { " AM" } else { " PM" }; }
    }
    match (date.is_empty(), time.is_empty()) { (false, false) => format!("{date}, {time}"), (false, true) => date, _ => time }
}
fn opts_list(it: &mut Interp, o: &Val) -> Result<Vec<(String, String)>, Ctl> {
    let mut out = Vec::new();
    if matches!(o, Val::Obj(_)) { for k in it.keys(o) { let v = it.get(o, &k)?; out.push((k.to_string(), it.to_str(&v)?.to_string())); } }
    Ok(out)
}

pub fn install_web(it: &mut Interp, g: &crate::jsi::Scope) {
    let put = |name: &str, v: Val| { g.borrow_mut().vars.insert(Rc::from(name), v); };
    let op = it.realm.object.clone();
    let new_proto = || Rc::new(std::cell::RefCell::new(crate::jsi::Obj { props: Vec::new(), kind: Kind::Plain, proto: Some(op.clone()), flags: 0, attrs: Vec::new() }));
    let tag = |p: &ORef, t: &str| Interp::define(p, K_TAG, st(t), A_NOENUM | A_NOWRITE);

    // URLSearchParams (URL Standard §6.2)
    let spp = new_proto();
    let spc = ctor("URLSearchParams", &spp, |it, _t, a| {
        let o = Val::obj(Kind::Plain);
        let init = arg(&a, 0);
        let pairs: Vec<(String, String)> = match &init {
            Val::Undef | Val::Null => vec![],
            Val::Obj(io) if !matches!(io.borrow().kind, Kind::Plain) || it.get(&init, K_ITER)?.is_callable() => {
                if hidden(&init, "\u{0}list").as_obj().is_some() { pairs_of(it, &init)? } else {
                    let mut v = Vec::new(); for e in it.iterate(&init)? { let l = list_of(it, &e)?; if l.len() != 2 { return Err(throw(it, "TypeError", "Each query pair must be an iterable [name, value] tuple")); } v.push((it.to_str(&l[0])?.to_string(), it.to_str(&l[1])?.to_string())); } v
                }
            }
            Val::Obj(_) => { let mut v = Vec::new(); for k in it.keys(&init) { let x = it.get(&init, &k)?; v.push((k.to_string(), it.to_str(&x)?.to_string())); } v }
            other => { let s = it.to_str(other)?; form_decode(s.strip_prefix('?').unwrap_or(&s)) }
        };
        set_hidden(&o, "\u{0}list", pairs_val(&pairs));
        Ok(o)
    });
    def(&spp, "append", nat!("append", |it, this, a| { let mut p = pairs_of(it, &this)?; let (k, v) = (it.to_str(&arg(&a, 0))?.to_string(), it.to_str(&arg(&a, 1))?.to_string()); p.push((k, v)); sp_store(it, &this, &p)?; Ok(Val::Undef) }));
    def(&spp, "delete", nat!("delete", |it, this, a| { let mut p = pairs_of(it, &this)?; let k = it.to_str(&arg(&a, 0))?.to_string(); let v = match arg(&a, 1) { Val::Undef => None, x => Some(it.to_str(&x)?.to_string()) }; p.retain(|(a, b)| !(a == &k && v.as_ref().map_or(true, |v| v == b))); sp_store(it, &this, &p)?; Ok(Val::Undef) }));
    def(&spp, "get", nat!("get", |it, this, a| { let p = pairs_of(it, &this)?; let k = it.to_str(&arg(&a, 0))?.to_string(); Ok(p.iter().find(|(x, _)| x == &k).map(|(_, v)| st(v)).unwrap_or(Val::Null)) }));
    def(&spp, "getAll", nat!("getAll", |it, this, a| { let p = pairs_of(it, &this)?; let k = it.to_str(&arg(&a, 0))?.to_string(); Ok(Val::array(p.iter().filter(|(x, _)| x == &k).map(|(_, v)| st(v)).collect())) }));
    def(&spp, "has", nat!("has", |it, this, a| { let p = pairs_of(it, &this)?; let k = it.to_str(&arg(&a, 0))?.to_string(); let v = match arg(&a, 1) { Val::Undef => None, x => Some(it.to_str(&x)?.to_string()) }; Ok(Val::Bool(p.iter().any(|(a, b)| a == &k && v.as_ref().map_or(true, |v| v == b)))) }));
    def(&spp, "set", nat!("set", |it, this, a| { let mut p = pairs_of(it, &this)?; let (k, v) = (it.to_str(&arg(&a, 0))?.to_string(), it.to_str(&arg(&a, 1))?.to_string()); match p.iter().position(|(x, _)| x == &k) { Some(i) => { p[i].1 = v; let mut j = i + 1; while j < p.len() { if p[j].0 == k { p.remove(j); } else { j += 1; } } } None => p.push((k, v)) } sp_store(it, &this, &p)?; Ok(Val::Undef) }));
    def(&spp, "sort", nat!("sort", |it, this, _a| { let mut p = pairs_of(it, &this)?; p.sort_by(|a, b| utf16(&a.0).cmp(&utf16(&b.0))); sp_store(it, &this, &p)?; Ok(Val::Undef) }));
    def(&spp, "toString", nat!("toString", |it, this, _a| { let p = pairs_of(it, &this)?; Ok(st(&form_encode(&p))) }));
    // forEach walks the live list (URL Standard §6.2: an entry deleted by the callback is not visited)
    def(&spp, "forEach", nat!("forEach", |it, this, a| { let f = arg(&a, 0); let mut i = 0; loop { let p = pairs_of(it, &this)?; let Some((k, v)) = p.get(i).cloned() else { break }; it.call(&f, arg(&a, 1), vec![st(&v), st(&k), this.clone()])?; i += 1; } Ok(Val::Undef) }));
    def(&spp, "keys", nat!("keys", |it, this, _a| { let p = pairs_of(it, &this)?; Ok(new_iter(IterSt::List(p.iter().map(|x| st(&x.0)).collect(), 0))) }));
    def(&spp, "values", nat!("values", |it, this, _a| { let p = pairs_of(it, &this)?; Ok(new_iter(IterSt::List(p.iter().map(|x| st(&x.1)).collect(), 0))) }));
    let ent = nat!("entries", |it, this, _a| { let p = pairs_of(it, &this)?; Ok(new_iter(IterSt::List(p.iter().map(|x| Val::array(vec![st(&x.0), st(&x.1)])).collect(), 0))) });
    def(&spp, "entries", ent.clone()); Interp::define(&spp, K_ITER, ent, A_NOENUM);
    Interp::define_accessor(&spp, "size", Some(nat!("size", |it, this, _a| Ok(Val::Num(pairs_of(it, &this)?.len() as f64)))), None, A_NOENUM);
    tag(&spp, "URLSearchParams");
    put("URLSearchParams", spc.clone());

    // URL (URL Standard §6.1)
    let up = new_proto();
    let urlc = ctor("URL", &up, |it, _t, a| {
        let s = it.to_str(&arg(&a, 0))?.to_string();
        let base = match arg(&a, 1) { Val::Undef => None, b => { let bs = it.to_str(&b)?.to_string(); Some(parse_url(&bs, None).ok_or_else(|| throw(it, "TypeError", &format!("Invalid base URL: {bs}")))?) } };
        let u = parse_url(&s, base.as_ref()).ok_or_else(|| throw(it, "TypeError", &format!("Invalid URL: {s}")))?;
        let o = Val::obj(Kind::Plain);
        set_hidden(&o, "\u{0}href", st(&u.href()));
        Ok(o)
    });
    let acc = |name: &'static str, get: fn(&Url) -> String, set: Option<fn(&mut Url, &str) -> bool>| {
        let g = Val::native(name, move |it, this, _a| { let u = url_of(it, &this)?; Ok(st(&get(&u))) });
        let s = set.map(|f| Val::native(name, move |it, this, a| { let mut u = url_of(it, &this)?; let v = it.to_str(&arg(&a, 0))?.to_string(); if f(&mut u, &v) { url_store(&this, &u); } Ok(Val::Undef) }));
        Interp::define_accessor(&up, name, Some(g), s, A_NOENUM);
    };
    acc("href", |u| u.href(), None);
    acc("origin", |u| u.origin(), None);
    acc("protocol", |u| format!("{}:", u.scheme), Some(|u, v| { let s = v.trim_end_matches(':').to_ascii_lowercase(); if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) { return false; } if special_port(&s).is_some() != special_port(&u.scheme).is_some() { return false; } u.scheme = s; if special_port(&u.scheme).flatten() == u.port { u.port = None; } true }));
    acc("username", |u| u.user.clone(), Some(|u, v| { if u.host.is_none() { return false; } u.user = pct(v, " \"#<>?`{}/:;=@[\\]^|"); true }));
    acc("password", |u| u.pass.clone(), Some(|u, v| { if u.host.is_none() { return false; } u.pass = pct(v, " \"#<>?`{}/:;=@[\\]^|"); true }));
    acc("host", |u| u.host_port(), Some(|u, v| { let mut t = u.clone(); if parse_authority_and_rest(&mut t, v).is_none() { return false; } u.host = t.host; u.port = t.port; true }));
    acc("hostname", |u| u.host.clone().unwrap_or_default(), Some(|u, v| { if v.is_empty() && special_port(&u.scheme).is_some() { return false; } u.host = Some(v.to_ascii_lowercase()); true }));
    acc("port", |u| u.port.map(|p| p.to_string()).unwrap_or_default(), Some(|u, v| { if v.is_empty() { u.port = None; return true; } let d: String = v.chars().take_while(|c| c.is_ascii_digit()).collect(); match d.parse::<u32>() { Ok(n) if n <= 65535 => { u.port = if special_port(&u.scheme).flatten() == Some(n as u16) { None } else { Some(n as u16) }; true } _ => false } }));
    acc("pathname", |u| u.path.clone(), Some(|u, v| { let special = special_port(&u.scheme).is_some(); let mut segs = Vec::new(); norm_path(&mut segs, v.strip_prefix('/').unwrap_or(v), special); u.path = format!("/{}", segs.join("/")); true }));
    acc("search", |u| match &u.query { Some(q) if !q.is_empty() => format!("?{q}"), _ => String::new() }, Some(|u, v| { let v = v.strip_prefix('?').unwrap_or(v); u.query = if v.is_empty() { None } else { Some(pct(v, " \"#<>'")) }; true }));
    acc("hash", |u| match &u.frag { Some(f) if !f.is_empty() => format!("#{f}"), _ => String::new() }, Some(|u, v| { let v = v.strip_prefix('#').unwrap_or(v); u.frag = if v.is_empty() { None } else { Some(pct(v, " \"<>`")) }; true }));
    // href setter: a full re-parse (the getter above is replaced with a getter/setter pair)
    Interp::define_accessor(&up, "href", None, Some(nat!("href", |it, this, a| { let s = it.to_str(&arg(&a, 0))?.to_string(); let u = parse_url(&s, None).ok_or_else(|| throw(it, "TypeError", &format!("Invalid URL: {s}")))?; url_store(&this, &u); Ok(Val::Undef) })), A_NOENUM);
    let spc2 = spc.clone();
    Interp::define_accessor(&up, "searchParams", Some(Val::native("searchParams", move |it, this, _a| {
        let cur = hidden(&this, "\u{0}sp");
        if matches!(cur, Val::Obj(_)) { return Ok(cur); }
        let u = url_of(it, &this)?;
        let sp = it.construct(&spc2, vec![st(u.query.as_deref().unwrap_or(""))])?;
        set_hidden(&sp, "\u{0}url", this.clone());
        set_hidden(&this, "\u{0}sp", sp.clone());
        Ok(sp)
    })), None, A_NOENUM);
    def(&up, "toString", nat!("toString", |it, this, _a| Ok(st(&url_of(it, &this)?.href()))));
    def(&up, "toJSON", nat!("toJSON", |it, this, _a| Ok(st(&url_of(it, &this)?.href()))));
    tag(&up, "URL");
    statics(&urlc, vec![
        ("canParse", nat!("canParse", |it, _t, a| { if a.is_empty() { return Err(throw(it, "TypeError", "1 argument required, but only 0 present.")); } let s = it.to_str(&arg(&a, 0))?.to_string(); let base = match arg(&a, 1) { Val::Undef => None, b => { let bs = it.to_str(&b)?.to_string(); match parse_url(&bs, None) { Some(u) => Some(u), None => return Ok(Val::Bool(false)) } } }; Ok(Val::Bool(parse_url(&s, base.as_ref()).is_some())) })),
        ("parse", nat!("parse", |it, _t, a| { let s = it.to_str(&arg(&a, 0))?.to_string(); let base = match arg(&a, 1) { Val::Undef => None, b => { let bs = it.to_str(&b)?.to_string(); parse_url(&bs, None) } }; Ok(match parse_url(&s, base.as_ref()) { Some(u) => { let c = it.lookup_opt(&it.global.clone(), "URL").unwrap_or(Val::Undef); it.construct(&c, vec![st(&u.href())])? } None => Val::Null }) })),
    ]);
    put("URL", urlc);

    // TextEncoder / TextDecoder (Encoding Standard §8)
    let tep = new_proto();
    def(&tep, "encode", nat!("encode", |it, _this, a| { let s = match arg(&a, 0) { Val::Undef => Rc::from(""), v => it.to_str(&v)? }; let b = utf8_bytes(&s); let n = b.len(); Ok(make_typed(1, Some(Val::obj(Kind::Buffer(Rc::new(std::cell::RefCell::new(b))))), 0, n)) }));
    def(&tep, "encodeInto", nat!("encodeInto", |it, _this, a| { let s = it.to_str(&arg(&a, 0))?; let dst = arg(&a, 1); let (buf, _, _, off, len) = ta_of(it, &dst)?; let mut read = 0; let mut written = 0; for c in s.chars() { let mut tmp = [0u8; 4]; let e = c.encode_utf8(&mut tmp); if written + e.len() > len { break; } buf.borrow_mut()[off + written..off + written + e.len()].copy_from_slice(e.as_bytes()); written += e.len(); read += c.len_utf16(); } Ok(Val::plain(vec![("read", Val::Num(read as f64)), ("written", Val::Num(written as f64))])) }));
    def(&tep, "encoding", st("utf-8"));
    tag(&tep, "TextEncoder");
    put("TextEncoder", ctor("TextEncoder", &tep, |_it, _t, _a| Ok(Val::obj(Kind::Plain))));
    let tdp = new_proto();
    def(&tdp, "decode", nat!("decode", |it, this, a| {
        let b = bytes_of(it, &arg(&a, 0))?;
        let enc = hidden(&this, "\u{0}enc"); let enc = if let Val::Str(e) = &enc { e.to_string() } else { "utf-8".into() };
        let fatal = hidden(&this, "\u{0}fatal").truthy();
        let s = match enc.as_str() {
            "windows-1252" | "latin1" => b.iter().map(|&c| c as char).collect::<String>(),
            "utf-16le" => { let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect(); from16(&u) }
            _ => { let b = if b.starts_with(&[0xEF, 0xBB, 0xBF]) && !hidden(&this, "\u{0}ignoreBOM").truthy() { &b[3..] } else { &b[..] }; if fatal { match std::str::from_utf8(b) { Ok(s) => s.to_string(), Err(_) => return Err(throw(it, "TypeError", "The encoded data was not valid for encoding utf-8")) } } else { String::from_utf8_lossy(b).into_owned() } }
        };
        Ok(st(&s))
    }));
    Interp::define_accessor(&tdp, "encoding", Some(nat!("encoding", |_it, this, _a| Ok(match hidden(&this, "\u{0}enc") { Val::Undef => st("utf-8"), v => v }))), None, A_NOENUM);
    tag(&tdp, "TextDecoder");
    put("TextDecoder", ctor("TextDecoder", &tdp, |it, _t, a| {
        let label = match arg(&a, 0) { Val::Undef => "utf-8".to_string(), v => it.to_str(&v)?.trim().to_ascii_lowercase() };
        let enc = match label.as_str() { "utf-8" | "utf8" | "unicode-1-1-utf-8" => "utf-8", "latin1" | "iso-8859-1" | "windows-1252" | "ascii" | "us-ascii" => "windows-1252", "utf-16le" | "utf-16" => "utf-16le", _ => return Err(throw(it, "RangeError", &format!("The \"{label}\" encoding is not supported"))) };
        let o = Val::obj(Kind::Plain);
        set_hidden(&o, "\u{0}enc", st(enc));
        let opts = arg(&a, 1);
        if matches!(opts, Val::Obj(_)) { let f = it.get(&opts, "fatal")?; set_hidden(&o, "\u{0}fatal", f); let ib = it.get(&opts, "ignoreBOM")?; set_hidden(&o, "\u{0}ignoreBOM", ib); }
        Ok(o)
    }));

    // Intl (ECMA-402), en-US only
    let nf = nat!("NumberFormat", |it, _t, a| {
        let f = std::rc::Rc::new(num_fmt_opts(it, &arg(&a, 1))?);
        let (f1, f2) = (f.clone(), f.clone());
        let opts = arg(&a, 1);
        Ok(Val::plain(vec![
            ("format", Val::native("format", move |it, _t, a| { let x = arg(&a, 0); let n = match &x { Val::Big(b) => b.to_f64(), v => it.to_num(v)? }; Ok(st(&format_number(n, &f1))) })),
            ("formatToParts", Val::native("formatToParts", move |it, _t, a| { let n = it.to_num(&arg(&a, 0))?; Ok(Val::array(vec![Val::plain(vec![("type", st("literal")), ("value", st(&format_number(n, &f2)))])])) })),
            ("resolvedOptions", Val::native("resolvedOptions", move |_it, _t, _a| Ok(Val::plain(vec![("locale", st("en-US")), ("numberingSystem", st("latn")), ("style", st("decimal")), ("options", opts.clone())])))),
        ]))
    });
    let dtf = nat!("DateTimeFormat", |it, _t, a| {
        let o = std::rc::Rc::new(opts_list(it, &arg(&a, 1))?);
        let (o1, o2) = (o.clone(), o.clone());
        let tval = |it: &mut Interp, d: &Val| -> Result<f64, Ctl> { Ok(match d { Val::Undef => crate::jsi_date::now_ms() as f64, Val::Obj(x) => match x.borrow().kind { Kind::Date(t) => t, _ => f64::NAN }, v => it.to_num(v)? }) };
        Ok(Val::plain(vec![
            ("format", Val::native("format", move |it, _t, a| { let t = tval(it, &arg(&a, 0))?; Ok(st(&fmt_date_intl(t, &o1))) })),
            ("formatToParts", Val::native("formatToParts", move |it, _t, a| { let t = tval(it, &arg(&a, 0))?; Ok(Val::array(vec![Val::plain(vec![("type", st("literal")), ("value", st(&fmt_date_intl(t, &o2)))])])) })),
            ("resolvedOptions", Val::native("resolvedOptions", |_it, _t, _a| Ok(Val::plain(vec![("locale", st("en-US")), ("calendar", st("gregory")), ("numberingSystem", st("latn")), ("timeZone", st(&std::env::var("TZ").unwrap_or_else(|_| "UTC".into())))])))),
        ]))
    });
    let coll = nat!("Collator", |_it, _t, _a| Ok(Val::plain(vec![("compare", nat!("compare", |it, _t, a| { let x = it.to_str(&arg(&a, 0))?.to_string(); let y = it.to_str(&arg(&a, 1))?.to_string(); Ok(Val::Num(match locale_cmp(&x, &y) { Ordering::Less => -1.0, Ordering::Equal => 0.0, _ => 1.0 })) }))])));
    let plural = nat!("PluralRules", |it, _t, a| {
        let ordinal = { let o = arg(&a, 1); matches!(o, Val::Obj(_)) && matches!(it.get(&o, "type")?, Val::Str(ref s) if &**s == "ordinal") };
        Ok(Val::plain(vec![("select", Val::native("select", move |it, _t, a| { let n = it.to_num(&arg(&a, 0))?; Ok(st(if ordinal { let i = n.abs() as i64; match (i % 10, i % 100) { (1, x) if x != 11 => "one", (2, x) if x != 12 => "two", (3, x) if x != 13 => "few", _ => "other" } } else if n.abs() == 1.0 { "one" } else { "other" })) }))]))
    });
    let rtf = nat!("RelativeTimeFormat", |it, _t, a| {
        let auto = { let o = arg(&a, 1); matches!(o, Val::Obj(_)) && matches!(it.get(&o, "numeric")?, Val::Str(ref s) if &**s == "auto") };
        Ok(Val::plain(vec![("format", Val::native("format", move |it, _t, a| {
            let v = it.to_num(&arg(&a, 0))?; let unit = it.to_str(&arg(&a, 1))?.trim_end_matches('s').to_string();
            if auto { let w = match (unit.as_str(), v as i64) { ("day", 0) => Some("today"), ("day", 1) => Some("tomorrow"), ("day", -1) => Some("yesterday"), ("year", 0) => Some("this year"), ("year", 1) => Some("next year"), ("year", -1) => Some("last year"), ("month", 0) => Some("this month"), ("month", 1) => Some("next month"), ("month", -1) => Some("last month"), ("week", 0) => Some("this week"), ("week", 1) => Some("next week"), ("week", -1) => Some("last week"), ("second", 0) => Some("now"), _ => None }; if let Some(w) = w { if v.fract() == 0.0 { return Ok(st(w)); } } }
            let n = format_number(v.abs(), &NumFmt::default()); let u = if v.abs() == 1.0 { unit.clone() } else { format!("{unit}s") };
            Ok(st(&if v < 0.0 || (v == 0.0 && v.is_sign_negative()) { format!("{n} {u} ago") } else { format!("in {n} {u}") }))
        }))]))
    });
    let lf = nat!("ListFormat", |it, _t, a| {
        let disj = { let o = arg(&a, 1); matches!(o, Val::Obj(_)) && matches!(it.get(&o, "type")?, Val::Str(ref s) if &**s == "disjunction") };
        Ok(Val::plain(vec![("format", Val::native("format", move |it, _t, a| { let items: Vec<String> = it.iterate(&arg(&a, 0))?.iter().map(|x| it.to_str(x).map(|s| s.to_string()).unwrap_or_default()).collect(); let w = if disj { "or" } else { "and" }; Ok(st(&match items.len() { 0 => String::new(), 1 => items[0].clone(), 2 => format!("{} {w} {}", items[0], items[1]), n => format!("{}, {w} {}", items[..n - 1].join(", "), items[n - 1]) })) }))]))
    });
    // Intl.Segmenter (ECMA-402 §18): grapheme ≈ a code point plus its combining marks / joiners / modifiers (regional
    // indicators pair up); word = runs of letters, digits, _ and inner apostrophes, everything else one segment per run
    // of white space or per character; sentence = up to . ! ? and the white space after
    let seg = nat!("Segmenter", |it, _t, a| {
        let o = arg(&a, 1);
        let gran = if matches!(o, Val::Obj(_)) { match it.get(&o, "granularity")? { Val::Undef => "grapheme".to_string(), v => it.to_str(&v)?.to_string() } } else { "grapheme".into() };
        let g2 = gran.clone();
        Ok(Val::plain(vec![
            ("segment", Val::native("segment", move |it, _t, a| {
                let input = it.to_str(&arg(&a, 0))?;
                let chars: Vec<char> = input.chars().collect();
                let ext = |c: char| { let u = c as u32; matches!(u, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x200D | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F) || (u > 0x7f && !c.is_alphanumeric() && !c.is_whitespace() && !c.is_ascii_punctuation() && matches!(u, 0x0900..=0x0DFF | 0x0E31..=0x0E3A | 0x0E47..=0x0E4E)) };
                let wordc = |c: char| c.is_alphanumeric() || c == '_';
                let mut segs: Vec<(usize, usize, bool)> = Vec::new(); // char ranges + isWordLike
                let mut i = 0;
                while i < chars.len() {
                    let st0 = i;
                    match g2.as_str() {
                        "word" => {
                            if wordc(chars[i]) { while i < chars.len() && (wordc(chars[i]) || ext(chars[i]) || ((chars[i] == '\'' || chars[i] == '\u{2019}' || chars[i] == '.') && i + 1 < chars.len() && wordc(chars[i + 1]) && i > st0)) { i += 1; } segs.push((st0, i, true)); continue; }
                            if chars[i].is_whitespace() { while i < chars.len() && chars[i].is_whitespace() { i += 1; } } else { i += 1; while i < chars.len() && ext(chars[i]) { i += 1; } }
                            segs.push((st0, i, false));
                        }
                        "sentence" => {
                            while i < chars.len() && !matches!(chars[i], '.' | '!' | '?' | '\u{3002}') { i += 1; }
                            while i < chars.len() && matches!(chars[i], '.' | '!' | '?' | '\u{3002}') { i += 1; }
                            while i < chars.len() && chars[i].is_whitespace() { i += 1; }
                            segs.push((st0, i, false));
                        }
                        _ => {
                            let ri = |c: char| (0x1F1E6..=0x1F1FF).contains(&(c as u32));
                            if ri(chars[i]) && i + 1 < chars.len() && ri(chars[i + 1]) { i += 2; } else { i += 1; }
                            while i < chars.len() && (ext(chars[i]) || (i > 0 && chars[i - 1] == '\u{200D}')) { i += 1; }
                            if chars[st0] == '\r' && i < chars.len() && chars[i] == '\n' && i == st0 + 1 { i += 1; }
                            segs.push((st0, i, false));
                        }
                    }
                }
                let word = g2 == "word";
                let mut u16pos = 0usize; let mut items = Vec::with_capacity(segs.len());
                for (a0, b0, wl) in segs {
                    let text: String = chars[a0..b0].iter().collect();
                    let mut kv = vec![("segment", st(&text)), ("index", Val::Num(u16pos as f64)), ("input", Val::Str(input.clone()))];
                    if word { kv.push(("isWordLike", Val::Bool(wl))); }
                    u16pos += len16(&text);
                    items.push(Val::plain(kv));
                }
                let list = items.clone();
                let segments = Val::plain(vec![("containing", Val::native("containing", move |it, _t, a| { let i = it.to_num(&arg(&a, 0))?; for x in &list { let ix = it.get(x, "index")?; let ix = it.to_num(&ix)?; let sg = it.get(x, "segment")?; let l = len16(&it.to_str(&sg)?) as f64; if i >= ix && i < ix + l { return Ok(x.clone()); } } Ok(Val::Undef) }))]);
                let items2 = items.clone();
                if let Val::Obj(so) = &segments { Interp::define(so, K_ITER, Val::native("[Symbol.iterator]", move |_it, _t, _a| Ok(new_iter(IterSt::List(items2.clone(), 0)))), A_NOENUM); }
                Ok(segments)
            })),
            ("resolvedOptions", Val::native("resolvedOptions", move |_it, _t, _a| Ok(Val::plain(vec![("locale", st("en-US")), ("granularity", st(&gran))])))),
        ]))
    });
    let dn = nat!("DisplayNames", |it, _t, a| { let o = arg(&a, 1); let ty = if matches!(o, Val::Obj(_)) { it.get(&o, "type")? } else { Val::Undef }; let ty = it.to_str(&ty)?.to_string(); Ok(Val::plain(vec![("of", Val::native("of", move |it, _t, a| { let c = it.to_str(&arg(&a, 0))?.to_string(); Ok(st(&match (ty.as_str(), c.as_str()) { ("language", "en") | ("language", "en-US") => "English".to_string(), ("language", "fr") => "French".into(), ("language", "de") => "German".into(), ("language", "es") => "Spanish".into(), ("region", "US") => "United States".into(), ("region", "GB") => "United Kingdom".into(), ("currency", "USD") => "US Dollar".into(), ("currency", "EUR") => "Euro".into(), _ => c })) }))])) });
    let loc = nat!("Locale", |it, _t, a| { let tag = it.to_str(&arg(&a, 0))?.to_string(); let mut parts = tag.split(['-', '_']); let lang = parts.next().unwrap_or("en").to_lowercase(); let region = parts.find(|p| p.len() == 2).map(|p| p.to_uppercase()); Ok(Val::plain(vec![("baseName", st(&tag)), ("language", st(&lang)), ("region", region.map(|r| st(&r)).unwrap_or(Val::Undef)), ("toString", Val::native("toString", move |_i, _t, _a| Ok(st(&tag)))), ("maximize", Val::native("maximize", |_i, t, _a| Ok(t))), ("minimize", Val::native("minimize", |_i, t, _a| Ok(t)))])) });
    put("Intl", Val::plain(vec![("NumberFormat", nf), ("DateTimeFormat", dtf), ("Collator", coll), ("PluralRules", plural), ("RelativeTimeFormat", rtf), ("ListFormat", lf), ("Segmenter", seg), ("DisplayNames", dn), ("Locale", loc),
        ("supportedValuesOf", nat!("supportedValuesOf", |_it, _t, _a| Ok(Val::array(vec![])))),
        ("getCanonicalLocales", nat!("getCanonicalLocales", |it, _t, a| { let v = arg(&a, 0); Ok(match &v { Val::Undef => Val::array(vec![]), Val::Str(_) => Val::array(vec![v.clone()]), _ => Val::array(it.iterate(&v)?) }) }))]));
    let _ = (K_ASYNC_ITER, len16(""), str_num("0"), A_NOCONF, A_NOWRITE);
}
// ── host modules: svelte/store, svelte, tauri stubs ──
fn store_obj(it: &mut Interp, init: Val, start: Val) -> Val {
    // value + subscribers live in the store object; start(set) runs on the first subscribe (readable semantics)
    let st_o = Val::plain(vec![("__v", init), ("__subs", Val::array(vec![])), ("__start", start), ("__started", Val::Bool(false))]);
    let set = { let so = st_o.clone(); Val::native("set", move |it, _t, a| { let v = arg(&a, 0); it.set(&so, "__v", v.clone())?; let subs = it.get(&so, "__subs")?; for f in it.iterate(&subs)? { if f.is_callable() { it.call(&f, Val::Undef, vec![v.clone()])?; } } Ok(Val::Undef) }) };
    let update = { let so = st_o.clone(); let set = set.clone(); Val::native("update", move |it, _t, a| { let cur = it.get(&so, "__v")?; let nv = it.call(&arg(&a, 0), Val::Undef, vec![cur])?; it.call(&set, Val::Undef, vec![nv]) }) };
    let subscribe = { let so = st_o.clone(); let set = set.clone(); Val::native("subscribe", move |it, _t, a| {
        let f = arg(&a, 0);
        let subs = it.get(&so, "__subs")?; let push = it.get(&subs, "push")?; it.call(&push, subs.clone(), vec![f.clone()])?;
        if !it.get(&so, "__started")?.truthy() { it.set(&so, "__started", Val::Bool(true))?; let start = it.get(&so, "__start")?; if start.is_callable() { it.call(&start, Val::Undef, vec![set.clone()])?; } }
        let v = it.get(&so, "__v")?; if f.is_callable() { it.call(&f, Val::Undef, vec![v])?; }
        Ok(Val::native("unsubscribe", |_it, _t, _a| Ok(Val::Undef)))
    }) };
    let _ = it;
    if let Val::Obj(o) = &st_o { o.borrow_mut().props.extend([(Rc::from("set"), set), (Rc::from("update"), update), (Rc::from("subscribe"), subscribe)]); }
    st_o
}

/// Current value of a store (svelte/store `get`).
pub fn store_value(it: &mut Interp, store: &Val) -> Result<Val, Ctl> {
    // our own stores keep the value in place: read it (subscribing just to read would grow the subscriber list forever)
    if let Val::Obj(o) = store {
        let started = o.borrow().props.iter().find(|(k, _)| &**k == "__started").map(|x| x.1.truthy());
        if started == Some(true) { if let Some((_, v)) = o.borrow().props.iter().find(|(k, _)| &**k == "__v") { return Ok(v.clone()); } }
    }
    let sub = it.get(store, "subscribe")?;
    if !sub.is_callable() { return Ok(Val::Undef); }
    let slot = Rc::new(std::cell::RefCell::new(Val::Undef)); let s2 = slot.clone();
    it.call(&sub, store.clone(), vec![Val::native("get", move |_it, _t, a| { *s2.borrow_mut() = arg(&a, 0); Ok(Val::Undef) })])?;
    let v = slot.borrow().clone(); Ok(v)
}

pub fn host_module(it: &mut Interp, spec: &str) -> Option<Val> {
    let key = format!("host:{spec}");
    if let Some(m) = it.modules.get(&key) { return Some(m.clone()); }
    let m = match spec {
        "svelte/store" => Val::plain(vec![
            ("writable", nat!("writable", |it, _t, a| Ok(store_obj(it, arg(&a, 0), arg(&a, 1))))),
            ("readable", nat!("readable", |it, _t, a| Ok(store_obj(it, arg(&a, 0), arg(&a, 1))))),
            ("get", nat!("get", |it, _t, a| store_value(it, &arg(&a, 0)))),
            ("derived", nat!("derived", |it, _t, a| {
                // derived(store | [stores], fn): recomputed on every source change
                let srcs = arg(&a, 0); let f = arg(&a, 1);
                let list = match &srcs { Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_)) => it.iterate(&srcs)?, _ => vec![srcs.clone()] };
                let multi = matches!(&srcs, Val::Obj(o) if matches!(o.borrow().kind, Kind::Array(_)));
                let out = store_obj(it, Val::Undef, Val::Undef);
                let set = it.get(&out, "set")?;
                let recompute = { let list = list.clone(); let f = f.clone(); let set = set.clone(); Val::native("recompute", move |it, _t, _a| { let mut vals = Vec::new(); for s in &list { vals.push(store_value(it, s)?); } let v = it.call(&f, Val::Undef, vec![if multi { Val::array(vals) } else { vals.into_iter().next().unwrap_or(Val::Undef) }])?; it.call(&set, Val::Undef, vec![v]) }) };
                for s in &list { let sub = it.get(s, "subscribe")?; let r = recompute.clone(); it.call(&sub, s.clone(), vec![Val::native("dep", move |it, _t, _a| it.call(&r, Val::Undef, vec![]))])?; }
                Ok(out)
            })),
        ]),
        "svelte" => Val::plain(vec![
            ("onMount", nat!("onMount", |it, _t, a| { let f = arg(&a, 0); if f.is_callable() { it.mount_queue.push(f); } Ok(Val::Undef) })),
            ("onDestroy", nat!("onDestroy", |_it, _t, _a| Ok(Val::Undef))),
            ("beforeUpdate", nat!("beforeUpdate", |_it, _t, _a| Ok(Val::Undef))), ("afterUpdate", nat!("afterUpdate", |it, _t, a| { let f = arg(&a, 0); if f.is_callable() { it.after_update.push(f); } Ok(Val::Undef) })),
            ("tick", nat!("tick", |_it, _t, _a| Ok(resolved(Val::Undef)))),
            ("createEventDispatcher", nat!("createEventDispatcher", |_it, _t, _a| Ok(Val::native("dispatch", |_it, _t, _a| Ok(Val::Bool(true)))))),
            ("getContext", nat!("getContext", |_it, _t, _a| Ok(Val::Undef))), ("setContext", nat!("setContext", |_it, _t, a| Ok(arg(&a, 1)))),
        ]),
        // motion modules: a renderer at rest needs no animation; transitions resolve to empty specs, easings to identity
        "svelte/transition" | "svelte/animate" | "svelte/motion" => {
            let spec = nat!("transition", |_it, _t, _a| Ok(Val::plain(vec![("duration", Val::Num(0.0))])));
            let pair = nat!("crossfade", |_it, _t, _a| { let f = Val::native("t", |_i, _t, _a| Ok(Val::plain(vec![("duration", Val::Num(0.0))]))); Ok(Val::array(vec![f.clone(), f])) });
            Val::plain(vec![("fade", spec.clone()), ("fly", spec.clone()), ("slide", spec.clone()), ("scale", spec.clone()), ("blur", spec.clone()), ("draw", spec.clone()),
                ("flip", spec.clone()), ("crossfade", pair), ("tweened", nat!("tweened", |it, _t, a| Ok(store_obj(it, arg(&a, 0), Val::Undef)))), ("spring", nat!("spring", |it, _t, a| Ok(store_obj(it, arg(&a, 0), Val::Undef))))])
        }
        "svelte/easing" => {
            let id = nat!("ease", |it, _t, a| Ok(Val::Num(it.to_num(&arg(&a, 0))?)));
            let names = ["linear", "cubicOut", "cubicIn", "cubicInOut", "quintOut", "quadOut", "quadIn", "quadInOut", "expoOut", "backOut", "elasticOut", "sineInOut", "circOut", "quartOut"];
            Val::plain(names.iter().map(|n| (*n, id.clone())).collect())
        }
        s if s.starts_with("@tauri-apps/") => {
            // no Tauri shell under omni-render: every call rejects with a clear reason, callers fall back as they do in a browser
            let rej = nat!("tauri", |it, _t, _a| Ok(rejected(it.error("Error", "Tauri API unavailable under omni-render"))));
            Val::plain(vec![("invoke", rej.clone()), ("isPermissionGranted", rej.clone()), ("requestPermission", rej.clone()), ("sendNotification", rej.clone()), ("getCurrentWindow", rej)])
        }
        _ => return None,
    };
    it.modules.insert(key, m.clone());
    Some(m)
}

/// fetch(url[, opts]) through omni-render's broker client (HTTP/1.1 + TLS). GET only for now; other methods reject.
pub fn install_fetch(it: &mut Interp) {
    let f = nat!("fetch", |it, _t, a| {
        let url = it.to_str(&arg(&a, 0))?.to_string();
        let method = match arg(&a, 1) { Val::Obj(_) => { let m = it.get(&arg(&a, 1), "method")?; if matches!(m, Val::Undef) { "GET".to_string() } else { it.to_str(&m)?.to_uppercase() } } _ => "GET".to_string() };
        it.fetch_log.push(format!("{method} {url}"));
        if method != "GET" { return Ok(rejected(it.error("TypeError", &format!("fetch {method} not supported yet")))); }
        let client = crate::net::Client::new();
        let t0 = std::time::Instant::now();
        let got = client.get(&url);
        if let Some(last) = it.fetch_log.last_mut() { *last += &format!(" {:.0}ms", t0.elapsed().as_secs_f64() * 1e3); }
        match got {
            Some((final_url, body)) => {
                let text = String::from_utf8_lossy(&body).into_owned();
                let t2 = text.clone();
                let resp = Val::plain(vec![("ok", Val::Bool(true)), ("status", Val::Num(200.0)), ("url", st(&final_url)),
                    ("json", Val::native("json", move |it, _t, _a| Ok(match json_parse(&t2) { Some(v) => resolved(v), None => rejected(it.error("SyntaxError", "Unexpected token in JSON")) }))),
                    ("text", Val::native("text", move |_it, _t, _a| Ok(resolved(st(&text)))))]);
                Ok(resolved(resp))
            }
            None => Ok(rejected(it.error("TypeError", "Failed to fetch"))),
        }
    });
    it.global.borrow_mut().vars.insert(Rc::from("fetch"), f);
}

pub fn stringify_val(it: &mut Interp, v: &Val) -> String { match stringify(it, v, "", "", 0) { Ok(Some(s)) => s, _ => inspect(it, v, false, 0) } }
