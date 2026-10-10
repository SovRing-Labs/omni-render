// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime half of the rendering engine: code rendered into memory at run time (VOCAB "render").
pub mod ir;
pub mod jitw0;
pub mod p2;
pub mod rows8;
pub mod sift;
#[cfg(feature = "t25")] pub mod t25; // PolyForm side (see LICENSING.md); off by default
#[cfg(not(feature = "t25"))] #[path = "t25_stub.rs"] pub mod t25; // MIT OR Apache-2.0: no-op hooks so Layout::run builds without the T25 core
pub mod ternary;
pub mod x86;

/// Pin the calling thread to one CPU (benchmarks pin threads explicitly).
pub fn pin(cpu: usize) {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// Best-of-`reps` nanoseconds per call of `f`, each rep running ≥ `min_ms` of back-to-back calls.
pub fn time_ns(reps: usize, min_ms: f64, mut f: impl FnMut()) -> f64 {
    f();
    let mut best = f64::MAX;
    for _ in 0..reps {
        let t = std::time::Instant::now();
        let mut k = 0u64;
        while t.elapsed().as_secs_f64() * 1e3 < min_ms { f(); k += 1; }
        best = best.min(t.elapsed().as_secs_f64() * 1e9 / k as f64);
    }
    best
}
