// SPDX-License-Identifier: MIT OR Apache-2.0
//! JITW-0: a ternary head rendered into straight-line add/sub machine code — the weights are compiled away
//! (+1 → add, −1 → sub, 0 → no instruction). Field: Unrolling Ternary Neural Networks (arXiv 1909.04509), CompiledNN.
//! Two forms:
//!   B1  scalar, batch 1:   fn(x: *const i32 [n], out: *mut i32 [rows])        acc_k ±= x[i], 4 accumulators round-robin
//!   B8  AVX2, batch 8:     fn(x: *const i32 [n][8], out: *mut i32 [rows][8])  ymm_k ±= x[i][0..8], 4 accumulators per row
//! Exact (int32 sums of int8 inputs; no saturation at any n we render).
use super::x86::*;

pub struct Rendered { pub exec: Exec, pub nonzero: usize, pub emit_us: f64 }

/// Inputs per base-pointer window: rdi advances so every displacement fits disp8 (−128..127) → 5-byte vector instructions.
fn window(stride: i32) -> i32 { 256 / stride } // inputs covered by disp −128..+127

pub fn render_b1(t: &[i8], rows: usize, n: usize) -> Rendered {
    let t0 = std::time::Instant::now();
    let mut a = Asm::default();
    let acc = [RAX, RCX, RDX, R8];
    let mut nz = 0;
    for r in 0..rows {
        for &k in &acc { a.zero(k); }
        let mut j = 0;
        for i in 0..n {
            let w = t[r * n + i];
            if w == 0 { continue; }
            nz += 1;
            let d = (4 * i) as i32;
            if w > 0 { a.add_m(acc[j & 3], RDI, d) } else { a.sub_m(acc[j & 3], RDI, d) }
            j += 1;
        }
        a.add_rr(RAX, RCX); a.add_rr(RDX, R8); a.add_rr(RAX, RDX);
        a.st32(RSI, (4 * r) as i32, RAX);
    }
    a.ret();
    let us = t0.elapsed().as_secs_f64() * 1e6;
    Rendered { exec: Exec::new(&a.b), nonzero: nz, emit_us: us }
}

/// B8 with displacement windows: inputs are 32 B apart; the row walks i ascending, so rdi is re-based (lea) only when the next
/// input leaves the disp8 window — most instructions are 5 bytes (VEX2 + op + modrm + disp8) instead of 8.
pub fn render_b8(t: &[i8], rows: usize, n: usize) -> Rendered {
    let t0 = std::time::Instant::now();
    let mut a = Asm::default();
    let w = window(32) as usize; // 8 inputs per window
    let mut nz = 0;
    // r10 = x base (saved), rdi walks.
    a.b.extend_from_slice(&[0x49, 0x89, 0xFA]); // mov r10, rdi
    for r in 0..rows {
        for k in 0..4u8 { a.vpxor(k, k, k); }
        let mut cur_win = usize::MAX;
        let mut j = 0u8;
        for i in 0..n {
            let wt = t[r * n + i];
            if wt == 0 { continue; }
            nz += 1;
            let win = i / w;
            if win != cur_win {
                // lea rdi, [r10 + win*256 + 128]  (window centre so disp8 spans −128..+96)
                let disp = (win * w * 32 + 128) as i32;
                a.b.extend_from_slice(&[0x49, 0x8D, 0xBA]); a.b.extend_from_slice(&disp.to_le_bytes());
                cur_win = win;
            }
            let d = (32 * (i - win * w)) as i32 - 128;
            let k = j & 3;
            if wt > 0 { a.vpaddd_m(k, k, RDI, d) } else { a.vpsubd_m(k, k, RDI, d) }
            j += 1;
        }
        a.vpaddd(0, 0, 1); a.vpaddd(2, 2, 3); a.vpaddd(0, 0, 2);
        a.vmovdqu_st(RSI, 0, 0);
        a.b.extend_from_slice(&[0x48, 0x83, 0xC6, 0x20]); // add rsi, 32
    }
    a.vzeroupper(); a.ret();
    let us = t0.elapsed().as_secs_f64() * 1e6;
    Rendered { exec: Exec::new(&a.b), nonzero: nz, emit_us: us }
}

impl Rendered {
    pub fn run_b1(&self, x32: &[i32], out: &mut [i32]) { (self.exec.f2())(x32.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8) }
    pub fn run_b8(&self, x8x32: &[i32], out: &mut [i32]) { (self.exec.f2())(x8x32.as_ptr() as *const u8, out.as_mut_ptr() as *mut u8) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn lcg(s: &mut u64) -> u64 { *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); *s >> 33 }
    #[test]
    fn jit_ternary_exact_small() {
        let mut s = 7u64;
        for &(rows, n) in &[(1usize, 32usize), (5, 64), (17, 96), (33, 512)] {
            let t: Vec<i8> = (0..rows * n).map(|_| (lcg(&mut s) % 3) as i8 - 1).collect();
            let xs: Vec<i8> = (0..8 * n).map(|_| (lcg(&mut s) % 255) as i8 as i32 as i8).map(|v| v.max(-127)).collect();
            let mut want = vec![0i32; rows * 8];
            for q in 0..8 { let mut o = vec![0; rows]; crate::jit::p2::reference(&t, rows, n, &xs[q * n..(q + 1) * n], &mut o); for r in 0..rows { want[r * 8 + q] = o[r]; } }
            let b1 = render_b1(&t, rows, n);
            let x32: Vec<i32> = xs[..n].iter().map(|&v| v as i32).collect();
            let mut o1 = vec![0; rows]; b1.run_b1(&x32, &mut o1);
            for r in 0..rows { assert_eq!(o1[r], want[r * 8], "b1 r{r}"); }
            let b8 = render_b8(&t, rows, n);
            let mut xl = vec![0i32; n * 8]; for q in 0..8 { for i in 0..n { xl[i * 8 + q] = xs[q * n + i] as i32; } }
            let mut o8 = vec![0; rows * 8]; b8.run_b8(&xl, &mut o8);
            assert_eq!(o8, want, "b8 {rows}x{n}");
        }
    }
}
