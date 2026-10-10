// SPDX-License-Identifier: MIT OR Apache-2.0
//! A1 (IR-JIT-NEXT): TILE-1's register blocking rendered by OUR emitter — the batch sift the runtime owns (no dlopen, no LLVM).
//! Per row, each 32-phase block is loaded and looked up (pshufb C8/S8, abs) ONCE and applied to up to 8 queries whose int32
//! accumulators stay in ymm0..7. C and S products are added as int16 before widening (one vpmaddwd per query per block, not
//! two): exact because |T[a1][b1] + T[a2][b2]| ≤ 32,400 < 32,767 for unit phasor queries (T[a][b] = C8[a]C8[b] + S8[a]S8[b]).
//! Dims are cut into chunks (default 1,024) so the queries' bytes stay in L1 while a row tile (L2) is walked; partial sums
//! live in a small per-thread buffer between chunks. Probe 0 (an unpublished file, "D8") measured this form at
//! 0.88–0.97× the hand kernel; this module is that form as rendered code.
//! Kernel: fn(lat, rows, qr, qi, part, consts) — part = rows × 8 queries × 8 int32 lanes (256 B per row), read and written.
use super::sift::{consts, Consts};
use super::x86::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Render one chunk kernel: `q` queries (1..=8) laid out `qs` bytes apart, `dc` dims (multiple of 64), rows `stride` apart.
pub fn render(q: usize, dc: usize, qs: usize, stride: usize) -> Exec {
    assert!((1..=8).contains(&q) && dc % 64 == 0 && dc > 0);
    let mut a = Asm::default();
    // rdi lat, rsi rows, rdx qr, rcx qi, r8 part, r9 consts | r10 lat cursor, rax dim-iteration counter
    a.vmovdqu_ld(14, R9, 0); a.vmovdqu_ld(15, R9, 32);
    a.test64(RSI);
    let jz = a.jcc(false, 0);
    let row = a.b.len();
    for j in 0..q as u8 { a.vmovdqu_ld(j, R8, 32 * j as i32); }
    a.mov64(R10, RDI);
    a.zero(RAX); a.add64_i(RAX, (dc / 64) as i32);
    let dims = a.b.len();
    for blk in 0..2i32 {
        let o = 32 * blk;
        a.vmovdqu_ld(12, R10, o);
        a.vpshufb(8, 14, 12); a.vpshufb(9, 15, 12);
        a.vpabsb(10, 8); a.vpabsb(11, 9);
        for j in 0..q as u8 {
            let qo = j as i32 * qs as i32 + o;
            a.vmovdqu_ld(12, RDX, qo); a.vmovdqu_ld(13, RCX, qo);
            a.vpsignb(12, 12, 8); a.vpsignb(13, 13, 9);
            a.vpmaddubsw(12, 10, 12); a.vpmaddubsw(13, 11, 13);
            a.vpaddw(12, 12, 13);
            a.vpmaddwd_m(12, 12, R9, 64);
            a.vpaddd(j, j, 12);
        }
    }
    a.add64_i(R10, 64); a.add64_i(RDX, 64); a.add64_i(RCX, 64);
    a.dec64(RAX);
    a.jcc(true, dims);
    a.add64_i(RDX, -(dc as i32)); a.add64_i(RCX, -(dc as i32));
    for j in 0..q as u8 { a.vmovdqu_st(R8, 32 * j as i32, j); }
    a.add64_i(RDI, stride as i32); a.add64_i(R8, 256);
    a.dec64(RSI);
    a.jcc(true, row);
    let end = a.b.len(); a.patch(jz, end);
    a.vzeroupper(); a.ret();
    Exec::new(&a.b)
}

/// The runtime's batch sift: rendered chunk kernels keyed by (queries in group, chunk dims, query stride, row stride).
pub struct Rows8 { k: HashMap<(usize, usize, usize, usize), Exec>, c: Consts, pub tm: usize, pub dc: usize, pub renders: usize, pub render_us: f64, pub pool: Option<Pool> }

impl Default for Rows8 { fn default() -> Self { Rows8 { k: HashMap::new(), c: consts(), tm: 32, dc: 1024, renders: 0, render_us: 0.0, pool: None } } }

impl Rows8 {
    fn chunks(&self, d: usize) -> Vec<(usize, usize)> {
        let dc = self.dc.min(d).max(64) / 64 * 64;
        (0..d).step_by(dc).map(|c0| (c0, dc.min(d - c0))).collect()
    }
    fn ensure(&mut self, q: usize, dc: usize, qs: usize, stride: usize) {
        if !self.k.contains_key(&(q, dc, qs, stride)) {
            let t = std::time::Instant::now();
            let e = render(q, dc, qs, stride);
            self.render_us += t.elapsed().as_secs_f64() * 1e6; self.renders += 1;
            self.k.insert((q, dc, qs, stride), e);
        }
    }
    /// Score rows [0, n) of `lat` (row stride `stride`, first `d` phases) against `nq` queries given as int8 (qr, qi) planes
    /// `qs` apart. out[q * n + k]. Row tiles of `tm` rows are handed out dynamically to `threads` workers.
    pub fn sift(&mut self, lat: &[u8], n: usize, d: usize, stride: usize, qr: &[i8], qi: &[i8], nq: usize, qs: usize, out: &mut [i32], threads: usize) {
        assert!(d % 64 == 0 && d <= stride && d <= qs && out.len() >= nq * n && lat.len() >= (n.max(1) - 1) * stride + d);
        let ch = self.chunks(d);
        let groups: Vec<(usize, usize)> = (0..nq).step_by(8).map(|g| (g, 8.min(nq - g))).collect();
        for &(_, q) in &groups { for &(_, dc) in &ch { self.ensure(q, dc, qs, stride); } }
        let fs: Vec<Vec<extern "C" fn(*const u8, usize, *const i8, *const i8, *mut i32, *const u8)>> =
            groups.iter().map(|&(_, q)| ch.iter().map(|&(_, dc)| self.k[&(q, dc, qs, stride)].f6()).collect()).collect();
        let tm = self.tm.max(1); let nt = n.div_ceil(tm);
        let next = AtomicUsize::new(0);
        let (lp, rp, ip, op, cp) = (lat.as_ptr() as usize, qr.as_ptr() as usize, qi.as_ptr() as usize, out.as_mut_ptr() as usize, self.c.b.as_ptr() as usize);
        let work = || {
            let mut part = vec![0i32; tm * 64];
            loop {
                let t = next.fetch_add(1, Ordering::Relaxed);
                if t >= nt { break; }
                let (r0, rn) = (t * tm, tm.min(n - t * tm));
                for (gi, &(g0, q)) in groups.iter().enumerate() {
                    part[..rn * 64].fill(0);
                    for (ci, &(c0, _)) in ch.iter().enumerate() {
                        fs[gi][ci]((lp + r0 * stride + c0) as *const u8, rn, (rp + g0 * qs + c0) as *const i8, (ip + g0 * qs + c0) as *const i8,
                                   part.as_mut_ptr(), cp as *const u8);
                    }
                    for k in 0..rn {
                        for j in 0..q {
                            let s: i32 = part[k * 64 + j * 8..k * 64 + j * 8 + 8].iter().sum();
                            unsafe { *(op as *mut i32).add((g0 + j) * n + r0 + k) = s; }
                        }
                    }
                }
            }
        };
        if threads <= 1 || nt <= 1 { work(); }
        else if let Some(p) = &self.pool { p.run(threads, &|_| work()); }
        else { std::thread::scope(|sc| { for _ in 0..threads { sc.spawn(&work); } }); }
    }
}

/// Flatten phase-index queries into the (qr, qi) int8 planes the kernel reads (stride = d).
pub fn planes(qs: &[Vec<u8>], d: usize) -> (Vec<i8>, Vec<i8>) {
    let mut r = Vec::with_capacity(qs.len() * d); let mut i = Vec::with_capacity(qs.len() * d);
    for q in qs { let (a, b) = super::sift::query(&q[..d]); r.extend(a); i.extend(b); }
    (r, i)
}

/// Persistent workers for firing cores: a tick hands one job to `n` parked threads (plus the caller) instead of spawning
/// threads per call. Workers optionally pinned (OMR_POOL_PIN=1 → worker w on CPU w+1; the caller stays where it is).
pub struct Pool { sh: std::sync::Arc<Shared>, hs: Vec<std::thread::JoinHandle<()>>, pub n: usize }
struct Shared { m: std::sync::Mutex<(u64, usize, bool)>, cv: std::sync::Condvar, job: std::sync::atomic::AtomicPtr<()>, left: AtomicUsize, spin: bool }
type Job<'a> = &'a (dyn Fn(usize) + Sync);
impl Pool {
    pub fn new(n: usize) -> Pool {
        let spin = std::env::var("OMR_POOL_SPIN").is_ok(); let pin = std::env::var("OMR_POOL_PIN").is_ok();
        let sh = std::sync::Arc::new(Shared { m: std::sync::Mutex::new((0, 0, false)), cv: std::sync::Condvar::new(), job: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()), left: AtomicUsize::new(0), spin });
        let hs = (0..n).map(|w| { let sh = sh.clone(); std::thread::spawn(move || {
            if pin { super::pin(w + 1); }
            let mut seen = 0u64;
            loop {
                let (gn, active) = { let mut g = sh.m.lock().unwrap(); while g.0 == seen && !g.2 { g = sh.cv.wait(g).unwrap(); } if g.2 { return; } (g.0, g.1) };
                seen = gn;
                if w < active {
                    let jp = sh.job.load(Ordering::Acquire) as *const Job;
                    let f: Job = unsafe { *jp };
                    f(w + 1);
                }
                sh.left.fetch_sub(1, Ordering::AcqRel);
            }
        }) }).collect();
        Pool { sh, hs, n }
    }
    /// Run `f(worker)` on `threads` workers (0 = the caller, 1.. = pool threads) and wait for all of them.
    pub fn run(&self, threads: usize, f: &(dyn Fn(usize) + Sync)) {
        let active = threads.saturating_sub(1).min(self.n);
        if active == 0 { f(0); return; }
        let job: Job = f;
        let jp = &job as *const Job as *mut ();
        self.sh.job.store(jp, Ordering::Release);
        self.sh.left.store(self.n, Ordering::Release);
        { let mut g = self.sh.m.lock().unwrap(); g.0 += 1; g.1 = active; }
        self.sh.cv.notify_all();
        f(0);
        while self.sh.left.load(Ordering::Acquire) != 0 { if self.sh.spin { std::hint::spin_loop(); } else { std::thread::yield_now(); } }
    }
}
impl Drop for Pool {
    fn drop(&mut self) { { let mut g = self.sh.m.lock().unwrap(); g.2 = true; } self.sh.cv.notify_all(); for h in self.hs.drain(..) { let _ = h.join(); } }
}
