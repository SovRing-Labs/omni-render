// SPDX-License-Identifier: MIT OR Apache-2.0
//! P2 packed trits (4 per byte, code = t+1 in 2 bits) — Rust port of our projects/omnid/trit_pack.hpp (same layout, same
//! sign_epi8 dot, exact). The baseline every weights-as-code kernel is measured against.
use std::arch::x86_64::*;

pub fn row_bytes(n: usize) -> usize { n.div_ceil(32) * 8 }

pub fn pack(t: &[i8], rows: usize, n: usize) -> Vec<u8> {
    let rb = row_bytes(n);
    let mut v = vec![0x55u8; rb * rows];
    for r in 0..rows {
        for i in 0..n {
            let sh = 2 * (i & 3);
            let b = &mut v[r * rb + (i >> 2)];
            *b = (*b & !(3 << sh)) | (((t[r * n + i] + 1) as u8) << sh);
        }
    }
    v
}

#[inline(always)]
unsafe fn unpack32(p: *const u8) -> __m256i {
    unsafe {
        let src = _mm256_set1_epi64x((p as *const i64).read_unaligned());
        let ctl = _mm256_setr_epi8(0,0,0,0,1,1,1,1,2,2,2,2,3,3,3,3, 4,4,4,4,5,5,5,5,6,6,6,6,7,7,7,7);
        let msk = _mm256_set1_epi32(0xC0300C03u32 as i32);
        let two = _mm256_set1_epi32(0x80200802u32 as i32);
        let m = _mm256_and_si256(_mm256_shuffle_epi8(src, ctl), msk);
        _mm256_sub_epi8(_mm256_cmpeq_epi8(m, _mm256_setzero_si256()), _mm256_cmpeq_epi8(m, two))
    }
}

#[inline(always)]
unsafe fn acc_i8(a: __m256i, v: __m256i, ones: __m256i) -> __m256i {
    unsafe {
        let lo = _mm256_cvtepi8_epi16(_mm256_castsi256_si128(v));
        let hi = _mm256_cvtepi8_epi16(_mm256_extracti128_si256(v, 1));
        _mm256_add_epi32(a, _mm256_add_epi32(_mm256_madd_epi16(lo, ones), _mm256_madd_epi16(hi, ones)))
    }
}

#[inline(always)]
unsafe fn hsum(a: __m256i) -> i32 {
    unsafe {
        let mut s = _mm_add_epi32(_mm256_castsi256_si128(a), _mm256_extracti128_si256(a, 1));
        s = _mm_hadd_epi32(s, s); s = _mm_hadd_epi32(s, s); _mm_cvtsi128_si32(s)
    }
}

/// out[r] = Σ_i x[i]·t[r,i]  (batch 1; n a multiple of 32). 4 rows per pass share the activation load (p2_dot4).
#[target_feature(enable = "avx2")]
pub unsafe fn gemv_b1(p: &[u8], rows: usize, n: usize, x: &[i8], out: &mut [i32]) {
    unsafe {
        let rb = row_bytes(n); let ones = _mm256_set1_epi16(1);
        let mut r = 0;
        while r + 4 <= rows {
            let (mut a0, mut a1, mut a2, mut a3) = (_mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256(), _mm256_setzero_si256());
            let base = p.as_ptr().add(r * rb);
            for k in (0..n).step_by(32) {
                let xv = _mm256_loadu_si256(x.as_ptr().add(k) as *const __m256i); let b = k / 4;
                a0 = acc_i8(a0, _mm256_sign_epi8(xv, unpack32(base.add(b))), ones);
                a1 = acc_i8(a1, _mm256_sign_epi8(xv, unpack32(base.add(rb + b))), ones);
                a2 = acc_i8(a2, _mm256_sign_epi8(xv, unpack32(base.add(2 * rb + b))), ones);
                a3 = acc_i8(a3, _mm256_sign_epi8(xv, unpack32(base.add(3 * rb + b))), ones);
            }
            out[r] = hsum(a0); out[r + 1] = hsum(a1); out[r + 2] = hsum(a2); out[r + 3] = hsum(a3);
            r += 4;
        }
        for r in r..rows {
            let mut a = _mm256_setzero_si256(); let base = p.as_ptr().add(r * rb);
            for k in (0..n).step_by(32) {
                a = acc_i8(a, _mm256_sign_epi8(_mm256_loadu_si256(x.as_ptr().add(k) as *const __m256i), unpack32(base.add(k / 4))), ones);
            }
            out[r] = hsum(a);
        }
    }
}

/// Batch 8: x is 8 query vectors (xs[q*n + i]); out[r*8 + q]. Each 32-trit block is unpacked ONCE and applied to all 8 queries
/// (the strongest P2 form at batch 8: weight bytes and unpack amortised 8×).
#[target_feature(enable = "avx2")]
pub unsafe fn gemv_b8(p: &[u8], rows: usize, n: usize, xs: &[i8], out: &mut [i32]) {
    unsafe {
        let rb = row_bytes(n); let ones = _mm256_set1_epi16(1);
        for r in 0..rows {
            let mut a = [_mm256_setzero_si256(); 8];
            let base = p.as_ptr().add(r * rb);
            for k in (0..n).step_by(32) {
                let t = unpack32(base.add(k / 4));
                for q in 0..8 {
                    a[q] = acc_i8(a[q], _mm256_sign_epi8(_mm256_loadu_si256(xs.as_ptr().add(q * n + k) as *const __m256i), t), ones);
                }
            }
            for q in 0..8 { out[r * 8 + q] = hsum(a[q]); }
        }
    }
}

/// Scalar reference (the arbiter for every exactness check).
pub fn reference(t: &[i8], rows: usize, n: usize, x: &[i8], out: &mut [i32]) {
    for r in 0..rows { out[r] = (0..n).map(|i| x[i] as i32 * t[r * n + i] as i32).sum(); }
}
