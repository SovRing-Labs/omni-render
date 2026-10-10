// SPDX-License-Identifier: MIT OR Apache-2.0
//! JIT-2 engine twin: the omniring phase sift rendered by OUR emitter (no LLVM, no Python) — the same pshufb LUT form as
//! omniring_sift_cplx: C8/S8 tables in registers, |lut| × sign-transferred query through vpmaddubsw, pairs to int32 by vpmaddwd.
//! Specialised to d (inner loop fully unrolled), rows looped in machine code, rows split over threads by the caller.
//! score[k] = Σ_i C8[lat[k,i]]·qr[i] + S8[lat[k,i]]·qi[i]  (int32, bit-identical to libomniring_avx2).
use super::x86::*;

pub const C8: [i8; 16] = [127, 117, 90, 49, 0, -49, -90, -117, -127, -117, -90, -49, 0, 49, 90, 117];
pub const S8: [i8; 16] = [0, 49, 90, 117, 127, 117, 90, 49, 0, -49, -90, -117, -127, -117, -90, -49];

#[repr(C, align(32))]
pub struct Consts { pub b: [u8; 96] }

pub fn consts() -> Consts {
    let mut b = [0u8; 96];
    for i in 0..32 { b[i] = C8[i & 15] as u8; b[32 + i] = S8[i & 15] as u8; }
    for i in 0..16 { b[64 + 2 * i] = 1; }
    Consts { b }
}

pub struct Sift { pub exec: Exec, pub d: usize, pub stride: usize, pub consts: Consts, pub emit_us: f64 }

pub fn render(d: usize) -> Sift { render_strided(d, d) }

/// Score only the first `d` phases of rows laid out `stride` apart (a cascade's prefix pass reads the full lattice in place,
/// no slice copy — the CORTEX-2 SCALE-0 defect was a 128 MB copy per query).
pub fn render_strided(d: usize, stride: usize) -> Sift {
    assert!(d % 64 == 0 && stride >= d);
    let t0 = std::time::Instant::now();
    let mut a = Asm::default();
    // rdi lat, rsi n, rdx qr, rcx qi, r8 out, r9 consts
    a.mov64(RAX, R9);
    a.vmovdqu_ld(15, RAX, 0); a.vmovdqu_ld(14, RAX, 32); a.vmovdqu_ld(13, RAX, 64);
    a.mov64(RAX, R8);
    a.test64(RSI);
    let jz = a.jcc(false, 0);
    let top = a.b.len();
    a.vpxor(0, 0, 0); a.vpxor(1, 1, 1);
    for blk in 0..d / 32 {
        let (p, c, s, r, m) = if blk & 1 == 0 { (2, 3, 4, 5, 6) } else { (7, 8, 9, 10, 11) };
        let acc = (blk & 1) as u8;
        let o = (blk * 32) as i32;
        a.vmovdqu_ld(p, RDI, o);
        a.vpshufb(c, 15, p); a.vpshufb(s, 14, p);
        a.vmovdqu_ld(r, RDX, o); a.vmovdqu_ld(m, RCX, o);
        a.vpsignb(r, r, c); a.vpsignb(m, m, s);
        a.vpabsb(c, c); a.vpabsb(s, s);
        a.vpmaddubsw(c, c, r); a.vpmaddubsw(s, s, m);
        a.vpmaddwd(c, c, 13); a.vpmaddwd(s, s, 13);
        a.vpaddd(acc, acc, c); a.vpaddd(acc, acc, s);
    }
    a.vpaddd(0, 0, 1);
    a.vextracti128_hi(1, 0); a.vpaddd_x(0, 0, 1);
    a.vpshufd_x(1, 0, 0x4E); a.vpaddd_x(0, 0, 1);
    a.vpshufd_x(1, 0, 0xB1); a.vpaddd_x(0, 0, 1);
    a.vmovd_st(RAX, 0, 0);
    a.add64_i(RDI, stride as i32); a.add64_i(RAX, 4);
    a.dec64(RSI);
    a.jcc(true, top);
    let end = a.b.len(); a.patch(jz, end);
    a.vzeroupper(); a.ret();
    let us = t0.elapsed().as_secs_f64() * 1e6;
    Sift { exec: Exec::new(&a.b), d, stride, consts: consts(), emit_us: us }
}

impl Sift {
    /// Score rows [0, n) of `lat` (n × d phase indices) against (qr, qi), `threads` contiguous row chunks.
    pub fn run(&self, lat: &[u8], qr: &[i8], qi: &[i8], out: &mut [i32], threads: usize) {
        let n = out.len(); let d = self.stride; let f = self.exec.f6(); let cp = self.consts.b.as_ptr();
        if threads <= 1 || n < 256 { f(lat.as_ptr(), n, qr.as_ptr(), qi.as_ptr(), out.as_mut_ptr(), cp); return; }
        let step = n.div_ceil(threads);
        let cp = cp as usize;
        std::thread::scope(|sc| {
            for (ci, chunk) in out.chunks_mut(step).enumerate() {
                let l = &lat[ci * step * d..];
                sc.spawn(move || f(l.as_ptr(), chunk.len(), qr.as_ptr(), qi.as_ptr(), chunk.as_mut_ptr(), cp as *const u8));
            }
        });
    }
}

/// Query (phase indices) → (qr, qi) int8, as seed.query_int8 / omniring_sift_batch do.
pub fn query(q: &[u8]) -> (Vec<i8>, Vec<i8>) { (q.iter().map(|&p| C8[(p & 15) as usize]).collect(), q.iter().map(|&p| S8[(p & 15) as usize]).collect()) }

pub fn reference(lat: &[u8], d: usize, qr: &[i8], qi: &[i8], out: &mut [i32]) {
    for (k, o) in out.iter_mut().enumerate() {
        *o = (0..d).map(|i| { let p = (lat[k * d + i] & 15) as usize; C8[p] as i32 * qr[i] as i32 + S8[p] as i32 * qi[i] as i32 }).sum();
    }
}

/// The hand-written golden kernel, loaded from libomniring_avx2.so (read-only dlopen).
pub struct Hand { f: unsafe extern "C" fn(*const u8, usize, usize, *const i8, *const i8, *mut i32), pub threads: Option<unsafe extern "C" fn(i32)> }
impl Hand {
    pub fn load() -> Option<Hand> {
        let path = std::ffi::CString::new(format!("{}/ai-stack/packages/omni-ring/omni_ring/libomniring_avx2.so", std::env::var("HOME").ok()?)).ok()?;
        unsafe {
            let h = libc::dlopen(path.as_ptr(), libc::RTLD_NOW);
            if h.is_null() { return None; }
            let s = libc::dlsym(h, c"omniring_sift_cplx".as_ptr());
            if s.is_null() { return None; }
            let t = libc::dlsym(h, c"omniring_set_num_threads".as_ptr());
            Some(Hand { f: std::mem::transmute(s), threads: if t.is_null() { None } else { Some(std::mem::transmute(t)) } })
        }
    }
    pub fn run(&self, lat: &[u8], d: usize, qr: &[i8], qi: &[i8], out: &mut [i32]) { unsafe { (self.f)(lat.as_ptr(), out.len(), d, qr.as_ptr(), qi.as_ptr(), out.as_mut_ptr()) } }
    pub fn set_threads(&self, t: i32) { if let Some(f) = self.threads { unsafe { f(t) } } }
}

/// `omni-render jit2`: the engine twin vs the hand kernel on 8,738 × 5,120 (+ small shapes), equal threads.
pub fn main() {
    use serde_json::json;
    let hand = Hand::load().expect("libomniring_avx2.so");
    let mut s = 5u64;
    let mut rnd = || { s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (s >> 33) as u32 };
    let (n, d) = (8738usize, 5120usize);
    let lat: Vec<u8> = (0..n * d).map(|_| (rnd() & 15) as u8).collect();
    let k = render(d);
    let mut bad = 0; let mut cases = 0;
    for qi_ in 0..40 {
        let q: Vec<u8> = if qi_ < 8 { lat[qi_ * d..(qi_ + 1) * d].to_vec() } else { (0..d).map(|_| (rnd() & 15) as u8).collect() };
        let (qr, qi) = query(&q);
        let (mut a, mut b) = (vec![0i32; n], vec![0i32; n]);
        k.run(&lat, &qr, &qi, &mut a, 1 + qi_ % 4); hand.run(&lat, d, &qr, &qi, &mut b);
        bad += (a != b) as usize; cases += 1;
        if qi_ < 2 { let mut r = vec![0i32; n]; reference(&lat, d, &qr, &qi, &mut r); bad += (a != r) as usize; cases += 1; }
    }
    for &(nn, dd) in &[(20usize, 64usize), (300, 1024), (1000, 4096)] {
        let l: Vec<u8> = (0..nn * dd).map(|_| (rnd() & 15) as u8).collect();
        let q: Vec<u8> = (0..dd).map(|_| (rnd() & 15) as u8).collect(); let (qr, qi) = query(&q);
        let kk = render(dd); let (mut a, mut b) = (vec![0i32; nn], vec![0i32; nn]);
        kk.run(&l, &qr, &qi, &mut a, 4); hand.run(&l, dd, &qr, &qi, &mut b); bad += (a != b) as usize; cases += 1;
    }
    let (qr, qi) = query(&lat[..d]);
    let mut out = vec![0i32; n];
    let mut t = serde_json::Map::new();
    for th in [1usize, 4, 8] {
        hand.set_threads(th as i32);
        let h = super::time_ns(3, 300.0, || hand.run(&lat, d, &qr, &qi, &mut out)) / 1e6;
        let j = super::time_ns(3, 300.0, || k.run(&lat, &qr, &qi, &mut out, th)) / 1e6;
        println!("threads {th}: engine JIT {j:.2} ms vs hand {h:.2} ms ({:.2}x)", j / h);
        t.insert(format!("t{th}"), json!({"jit_ms": j, "hand_ms": h, "ratio": j / h}));
    }
    hand.set_threads(8);
    let worst = t.values().map(|v| v["ratio"].as_f64().unwrap()).fold(0.0, f64::max);
    let res = json!({"lattice": [n, d], "mismatches": bad, "of": cases, "code_bytes": k.exec.code_bytes, "emit_us": k.emit_us,
        "ms_per_query": t, "verdict": {"PASS": bad == 0 && worst <= 1.5, "bitexact": bad == 0, "within_1.5x": worst <= 1.5}});
    std::fs::write("results/jit2-engine.json", serde_json::to_string_pretty(&res).unwrap()).unwrap();
    println!("engine JIT-2: mismatches {bad}/{cases} | code {} B rendered in {:.0} µs | {}", k.exec.code_bytes, k.emit_us, res["verdict"]);
}
