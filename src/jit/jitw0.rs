// SPDX-License-Identifier: MIT OR Apache-2.0
//! JITW-0 bench: ternary heads rendered as add/sub code vs the P2 kernel, golden (rg1 arm A trained rows) + random rows,
//! sizes 4 KB → 4 MB of int8 trits. Pre-registered in an unpublished plan step 1.
use super::{p2, pin, ternary, time_ns};
use serde_json::json;

fn lcg(s: &mut u64) -> u64 { *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); *s >> 33 }

/// Activations as the engines quantise them: |x| ≤ 127 (g = 127/max|x|), so sign_epi8 never meets −128.
fn acts(s: &mut u64, n: usize) -> Vec<i8> { (0..n).map(|_| ((lcg(s) % 255) as i32 - 127) as i8).collect() }

fn load_golden(path: &str) -> Vec<(usize, usize, Vec<i8>)> {
    let Ok(b) = std::fs::read(path) else { return vec![] };
    assert_eq!(&b[..4], b"TRIT");
    let cnt = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    let mut o = 8; let mut v = Vec::new();
    for _ in 0..cnt {
        let r = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize; let c = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize; o += 8;
        v.push((r, c, b[o..o + r * c].iter().map(|&x| x as i8).collect())); o += r * c;
    }
    v
}

/// One head through every form: returns (mismatches, json row).
fn run(name: &str, t: &[i8], rows: usize, n: usize, seed: u64, timed: bool) -> (usize, serde_json::Value) {
    let mut s = seed;
    let xs = acts(&mut s, 8 * n);
    // reference (scalar) for 8 queries, laid out [r][q]
    let mut want = vec![0i32; rows * 8];
    for q in 0..8 { let mut o = vec![0; rows]; p2::reference(t, rows, n, &xs[q * n..(q + 1) * n], &mut o); for r in 0..rows { want[r * 8 + q] = o[r]; } }
    let pk = p2::pack(t, rows, n);
    let (mut o1, mut o8) = (vec![0i32; rows], vec![0i32; rows * 8]);
    let mut bad = 0;
    unsafe { p2::gemv_b1(&pk, rows, n, &xs[..n], &mut o1) };
    bad += (0..rows).filter(|&r| o1[r] != want[r * 8]).count();
    unsafe { p2::gemv_b8(&pk, rows, n, &xs, &mut o8) };
    bad += o8.iter().zip(&want).filter(|(a, b)| a != b).count();
    let b1 = ternary::render_b1(t, rows, n);
    let x32: Vec<i32> = xs[..n].iter().map(|&v| v as i32).collect();
    let mut j1 = vec![0i32; rows]; b1.run_b1(&x32, &mut j1);
    bad += (0..rows).filter(|&r| j1[r] != want[r * 8]).count();
    let b8 = ternary::render_b8(t, rows, n);
    let mut xl = vec![0i32; n * 8]; for q in 0..8 { for i in 0..n { xl[i * 8 + q] = xs[q * n + i] as i32; } }
    let mut j8 = vec![0i32; rows * 8]; b8.run_b8(&xl, &mut j8);
    bad += j8.iter().zip(&want).filter(|(a, b)| a != b).count();
    let w = rows * n;
    let mut row = json!({"name": name, "rows": rows, "n": n, "weights": w, "int8_bytes": w, "p2_bytes": pk.len(),
        "nonzero": b8.nonzero, "code_b1_bytes": b1.exec.code_bytes, "code_b8_bytes": b8.exec.code_bytes,
        "emit_b1_us": b1.emit_us, "emit_b8_us": b8.emit_us, "mismatches": bad});
    if timed {
        let (reps, ms) = (5, 40.0);
        let p2b1 = time_ns(reps, ms, || unsafe { p2::gemv_b1(&pk, rows, n, &xs[..n], &mut o1) });
        let p2b8 = time_ns(reps, ms, || unsafe { p2::gemv_b8(&pk, rows, n, &xs, &mut o8) });
        let jb1 = time_ns(reps, ms, || b1.run_b1(&x32, &mut j1));
        let jb8 = time_ns(reps, ms, || b8.run_b8(&xl, &mut j8));
        // per row per query
        let (pr1, pr8, jr1, jr8) = (p2b1 / rows as f64, p2b8 / rows as f64 / 8.0, jb1 / rows as f64, jb8 / rows as f64 / 8.0);
        row["ns_row_query"] = json!({"p2_b1": pr1, "p2_b8": pr8, "jit_b1": jr1, "jit_b8": jr8});
        row["jit_over_p2"] = json!({"b1": pr1 / jr1, "b8": pr8 / jr8});
        row["gweights_per_s"] = json!({"p2_b1": w as f64 / p2b1, "p2_b8": 8.0 * w as f64 / p2b8, "jit_b1": w as f64 / jb1, "jit_b8": 8.0 * w as f64 / jb8});
        println!("{:<14} {:>9} w  code b8 {:>9} B ({:>4.1} B/nz)  ns/row/q  P2 b1 {:>8.1}  JIT b1 {:>8.1}  | P2 b8 {:>8.1}  JIT b8 {:>8.1}  JIT/P2 b8 {:>5.2}x  b1 {:>5.2}x  bad {}",
            name, w, b8.exec.code_bytes, b8.exec.code_bytes as f64 / b8.nonzero.max(1) as f64, pr1, jr1, pr8, jr8, pr8 / jr8, pr1 / jr1, bad);
    }
    (bad, row)
}

pub fn main(golden: &str) {
    pin(2);
    let mut out = json!({"cpu": "i5-8365U (L1i 32 KB, L2 256 KB/core, L3 6 MB)", "pinned_cpu": 2, "threads": 1});
    // golden: trained rows (exactness on every matrix + timing on the largest shapes)
    let g = load_golden(golden);
    let (mut gbad, mut grows) = (0, vec![]);
    for (k, (r, c, t)) in g.iter().enumerate() {
        let (b, row) = run(&format!("rg1A.m{k:02}"), t, *r, *c, 1000 + k as u64, k < 4);
        gbad += b; grows.push(row);
    }
    let zf = g.iter().map(|(_, _, t)| t.iter().filter(|&&v| v == 0).count()).sum::<usize>() as f64 / g.iter().map(|x| x.2.len()).sum::<usize>().max(1) as f64;
    println!("golden: {} trained matrices, mismatches {gbad}, zero fraction {zf:.4}", g.len());
    out["golden"] = json!({"source": "rg1 arm A (cal3.LM RESIDUE, BitLinear absmean)", "matrices": g.len(), "mismatches": gbad, "zero_fraction": zf, "rows": grows});
    // sizes: 4 KB → 4 MB int8 trits, n = 512 columns, zero fraction = golden's
    let mut s = 99u64; let mut srows = vec![]; let mut sbad = 0;
    let n = 512;
    let mut w = 4096;
    while w <= 4 << 20 {
        let rows = w / n;
        let t: Vec<i8> = (0..w).map(|_| { let u = (lcg(&mut s) % 1_000_000) as f64 / 1e6; if u < zf { 0 } else if u < zf + (1.0 - zf) / 2.0 { 1 } else { -1 } }).collect();
        let (b, row) = run(&format!("rand.{}KB", w / 1024), &t, rows, n, w as u64, true);
        sbad += b; srows.push(row);
        w *= 2;
    }
    // crossover: first size where JIT b8 stops beating P2 b8
    let cross = srows.iter().find(|r| r["jit_over_p2"]["b8"].as_f64().unwrap_or(0.0) < 1.0).map(|r| r["weights"].clone());
    out["sizes"] = json!({"n": n, "mismatches": sbad, "rows": srows, "b8_crossover_weights": cross});
    std::fs::create_dir_all("results").ok();
    std::fs::write("results/jitw0.json", serde_json::to_string_pretty(&out).unwrap()).unwrap();
    println!("total mismatches {} | b8 crossover at {:?} weights | -> results/jitw0.json", gbad + sbad, out["sizes"]["b8_crossover_weights"]);
}
