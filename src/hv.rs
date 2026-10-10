// SPDX-License-Identifier: MIT OR Apache-2.0
//! Problem hypervectors for the self-repair core's codebook (GEOMETRIC-COMPUTING-LAWS R1: geometry where items are COMPARED).
//! 16-phase codewords (the omniring qFHRR form): symbols hash-rendered by splitmix64, BIND = phase addition mod 16, BUNDLE =
//! complex sum re-quantised to the nearest phase, numbers by fractional power encoding (phase_i = round(b_i·x) mod 16, so
//! nearby values give nearby codewords). Recall = the rendered sift kernel (`jit::sift`, bit-identical to libomniring_avx2)
//! scoring every stored row against the query; retrieval only RANKS candidate fixes — the gate stays exact (R4).
use crate::jit::sift::{C8, S8};

pub const D: usize = 1024;

fn splitmix(st: &mut u64) -> u64 { *st = st.wrapping_add(0x9E3779B97F4A7C15); let mut z = *st; z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB); z ^ (z >> 31) }
fn fnv(s: &str) -> u64 { let mut h = 0xcbf29ce484222325u64; for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } h }

pub fn sym(s: &str) -> Vec<u8> {
    let mut st = fnv(s); let mut out = vec![0u8; D];
    for c in out.chunks_mut(16) { let z = splitmix(&mut st); for (j, v) in c.iter_mut().enumerate() { *v = ((z >> (4 * j)) & 15) as u8; } }
    out
}
pub fn bind(a: &[u8], b: &[u8]) -> Vec<u8> { a.iter().zip(b).map(|(x, y)| (x + y) & 15).collect() }
/// Fractional power encoding of a real number under a key.
pub fn fpe(key: &str, x: f32) -> Vec<u8> {
    let mut st = fnv(key) ^ 0x5bd1e995; (0..D).map(|_| { let b = ((splitmix(&mut st) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 3.0; ((b * x as f64).round() as i64).rem_euclid(16) as u8 }).collect()
}
pub fn bundle(parts: &[Vec<u8>]) -> Vec<u8> {
    (0..D).map(|i| {
        let (mut re, mut im) = (0i32, 0i32);
        for p in parts { let k = (p[i] & 15) as usize; re += C8[k] as i32; im += S8[k] as i32; }
        let a = (im as f64).atan2(re as f64); (((a / (2.0 * std::f64::consts::PI) * 16.0).round() as i64).rem_euclid(16)) as u8
    }).collect()
}
/// A layout problem as a record: role-bound feature values + the signature parts + the error in px (FPE, 16 px per unit).
pub fn problem(sig: &str, feats: &[(String, String)], err_px: f32) -> Vec<u8> {
    let p: Vec<&str> = sig.split('|').collect();
    let mut parts = vec![bind(&sym("role:kind"), &sym(p.first().copied().unwrap_or(""))), bind(&sym("role:disp"), &sym(p.get(1).copied().unwrap_or(""))),
        bind(&sym("role:pdisp"), &sym(p.get(2).copied().unwrap_or(""))), bind(&sym("role:text"), &sym(p.get(3).copied().unwrap_or(""))),
        bind(&sym("role:err"), &fpe("err", err_px / 16.0))];
    for (f, v) in feats { parts.push(bind(&sym(&format!("f:{f}")), &sym(&format!("v:{v}")))); }
    bundle(&parts)
}
pub fn to_hex(v: &[u8]) -> String { v.iter().map(|x| format!("{:x}", x & 15)).collect() }
pub fn from_hex(s: &str) -> Option<Vec<u8>> { if s.len() != D { return None; } s.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect() }

/// Sift `q` over the codebook rows (n × D phases) with the rendered kernel; returns scores (max = D·127·127).
pub fn sift(rows: &[u8], q: &[u8]) -> Vec<i32> {
    let n = rows.len() / D; let mut out = vec![0i32; n];
    if n == 0 { return out; }
    let (qr, qi) = crate::jit::sift::query(q);
    thread_local! { static K: std::cell::OnceCell<crate::jit::sift::Sift> = const { std::cell::OnceCell::new() }; }
    K.with(|k| k.get_or_init(|| crate::jit::sift::render(D)).run(rows, &qr, &qi, &mut out, 1));
    out
}

/// Batch recall (IR-JIT-NEXT A1 rows8, rendered by our emitter): up to 8 queries held in registers per row load, row tiles in L2.
/// Same scores as `sift` per query (checked by `core --donors --sift-check`). Returns one score row per query.
pub fn sift_batch(rows: &[u8], qs: &[Vec<u8>]) -> Vec<Vec<i32>> {
    let n = rows.len() / D;
    if n == 0 || qs.is_empty() { return vec![Vec::new(); qs.len()]; }
    let (qr, qi) = crate::jit::rows8::planes(qs, D);
    let mut out = vec![0i32; qs.len() * n];
    thread_local! { static R8: std::cell::RefCell<crate::jit::rows8::Rows8> = std::cell::RefCell::new(crate::jit::rows8::Rows8::default()); }
    R8.with(|r| r.borrow_mut().sift(rows, n, D, D, &qr, &qi, qs.len(), D, &mut out, 1));
    out.chunks(n).map(|c| c.to_vec()).collect()
}
