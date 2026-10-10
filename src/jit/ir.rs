// SPDX-License-Identifier: MIT OR Apache-2.0
//! Omniglyph-IR v0 — one program, lowered by the runtime into rendered machine code with a schedule chosen for THIS cpu.
//! v0 ops (array + ring + ternary): SIFT d (phase sift over a lattice, optional prefix of a wider row), TOPK k, RESCORE d
//! (exact sift on the survivors only), TERNARY head (rendered add/sub head). A program is bytes → its hash keys the rendered
//! kernels, so changing the program re-renders only what changed (CORTEX-2's program vector, taken to machine code).
//! Schedule v0 = loop tiling of the (query × row) nest: rows are strip-mined into tiles that fit L2 and the query loop is moved
//! inside (TILE-1's rule; the polyhedral model's rectangular tiling of a 2-D iteration space — dependence-free, so legal).
use super::sift;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op { Sift { d: u16 }, TopK { k: u16 }, Rescore { d: u16 } }

pub fn encode(p: &[Op]) -> Vec<u8> {
    let mut b = vec![];
    for o in p { match *o { Op::Sift { d } => { b.push(1); b.extend(d.to_le_bytes()) } Op::TopK { k } => { b.push(2); b.extend(k.to_le_bytes()) } Op::Rescore { d } => { b.push(3); b.extend(d.to_le_bytes()) } } }
    b
}
pub fn hash(b: &[u8]) -> u64 { b.iter().fold(0xcbf29ce484222325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100000001b3)) }

/// The runtime's code cache: (d, stride) → rendered sift.
#[derive(Default)]
pub struct Cache { pub k: HashMap<(usize, usize), sift::Sift>, pub renders: usize, pub render_us: f64 }
impl Cache {
    pub fn get(&mut self, d: usize, stride: usize) -> &sift::Sift {
        if !self.k.contains_key(&(d, stride)) {
            let t = Instant::now(); let s = sift::render_strided(d, stride); self.render_us += t.elapsed().as_secs_f64() * 1e6; self.renders += 1;
            self.k.insert((d, stride), s);
        }
        &self.k[&(d, stride)]
    }
}

pub struct Lattice<'a> { pub rows: &'a [u8], pub n: usize, pub stride: usize }

/// Run a program over a batch of queries. `tile_rows` = 0 → untiled (query outer); else row tiles of that size (query inner).
/// Returns per query: final scores of the survivors as (row, score), best first.
pub fn run(p: &[Op], lat: &Lattice, queries: &[Vec<u8>], cache: &mut Cache, tile_rows: usize, threads: usize) -> Vec<Vec<(u32, i32)>> {
    let mut cand: Vec<Option<Vec<(u32, i32)>>> = vec![None; queries.len()];
    let mut scores: Vec<Vec<i32>> = vec![];
    for op in p {
        match *op {
            Op::Sift { d } => {
                let k = cache.get(d as usize, lat.stride) as *const sift::Sift;
                let k = unsafe { &*k };
                let qs: Vec<(Vec<i8>, Vec<i8>)> = queries.iter().map(|q| sift::query(&q[..d as usize])).collect();
                scores = vec![vec![0i32; lat.n]; queries.len()];
                if tile_rows == 0 {
                    for (qi, (r, m)) in qs.iter().enumerate() { k.run(lat.rows, r, m, &mut scores[qi], threads); }
                } else {
                    // tiles of rows; threads take tiles; inside a tile every query passes over the L2-resident rows
                    let nt = lat.n.div_ceil(tile_rows);
                    let sp: Vec<usize> = scores.iter_mut().map(|s| s.as_mut_ptr() as usize).collect();
                    let f = k.exec.f6(); let cp = k.consts.b.as_ptr() as usize; let st = lat.stride;
                    std::thread::scope(|sc| {
                        for t in 0..threads.max(1) {
                            let (sp, qs) = (&sp, &qs);
                            sc.spawn(move || {
                                let mut ti = t;
                                while ti < nt {
                                    let r0 = ti * tile_rows; let rn = tile_rows.min(lat.n - r0);
                                    for (qi, (r, m)) in qs.iter().enumerate() {
                                        f(lat.rows[r0 * st..].as_ptr(), rn, r.as_ptr(), m.as_ptr(), (sp[qi] as *mut i32).wrapping_add(r0), cp as *const u8);
                                    }
                                    ti += threads.max(1);
                                }
                            });
                        }
                    });
                }
                for c in cand.iter_mut() { *c = None; }
            }
            Op::TopK { k } => {
                for (qi, c) in cand.iter_mut().enumerate() {
                    let src: Vec<(u32, i32)> = match c.take() { Some(v) => v, None => scores[qi].iter().enumerate().map(|(i, &s)| (i as u32, s)).collect() };
                    let mut v = src; let kk = (k as usize).min(v.len());
                    v.select_nth_unstable_by(kk.saturating_sub(1), |a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    v.truncate(kk); v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    *c = Some(v);
                }
            }
            Op::Rescore { d } => {
                let k = cache.get(d as usize, lat.stride) as *const sift::Sift;
                let k = unsafe { &*k };
                for (qi, c) in cand.iter_mut().enumerate() {
                    let (r, m) = sift::query(&queries[qi][..d as usize]);
                    let mut one = [0i32; 1];
                    if let Some(v) = c.as_mut() { for e in v.iter_mut() { k.run(&lat.rows[e.0 as usize * lat.stride..], &r, &m, &mut one, 1); e.1 = one[0]; } v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))); }
                }
            }
        }
    }
    cand.into_iter().enumerate().map(|(qi, c)| c.unwrap_or_else(|| scores[qi].iter().enumerate().map(|(i, &s)| (i as u32, s)).collect())).collect()
}

/// `omni-render ir-check`: IR-lowered kernels vs the hand AVX2 batch kernel (bit-identical), tiled vs untiled schedules,
/// a cascade program (prefix SIFT → TOPK → RESCORE, no copy) vs exact, and re-render cost on program change.
pub fn main() {
    use serde_json::json;
    let hand = sift::Hand::load().expect("libomniring_avx2.so");
    let (n, d, nq) = (8738usize, 5120usize, 16usize);
    let mut s = 11u64;
    let mut rnd = || { s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (s >> 33) as u32 };
    let lat: Vec<u8> = (0..n * d).map(|_| (rnd() & 15) as u8).collect();
    // queries: noisy copies of stored rows (30% of phases resampled) — a recall workload
    let qs: Vec<Vec<u8>> = (0..nq).map(|i| { let r = (i * 541) % n; lat[r * d..(r + 1) * d].iter().map(|&p| if rnd() % 10 < 3 { (rnd() & 15) as u8 } else { p }).collect() }).collect();
    let l = Lattice { rows: &lat, n, stride: d };
    let mut cache = Cache::default();
    // exactness: full-score program vs hand batch kernel
    let exact = [Op::Sift { d: d as u16 }];
    let mut hb = vec![0i32; nq * n];
    let flat: Vec<u8> = qs.concat();
    let hand_batch = |out: &mut Vec<i32>| unsafe {
        let h = libc::dlopen(c"packages/omni-ring/omni_ring/libomniring_avx2.so".as_ptr(), libc::RTLD_NOW);
        let f: unsafe extern "C" fn(*const u8, usize, usize, *const u8, usize, *mut i32) = std::mem::transmute(libc::dlsym(h, c"omniring_sift_batch".as_ptr()));
        f(lat.as_ptr(), n, d, flat.as_ptr(), nq, out.as_mut_ptr());
    };
    hand_batch(&mut hb);
    let l2 = 256 * 1024; let tile = (l2 / 2 / d).max(8);   // half of L2 for the rows, the rest for queries/out (TILE-1 rule)
    let mut bad = 0;
    for (name, tr, th) in [("untiled", 0usize, 1usize), ("tiled", tile, 1), ("tiled", tile, 4)] {
        let o = run(&exact, &l, &qs, &mut cache, tr, th);
        for (qi, v) in o.iter().enumerate() { for &(r, sc) in v { if sc != hb[qi * n + r as usize] { bad += 1; } } }
        let _ = name;
    }
    let mut t = serde_json::Map::new();
    let reps = 5;
    let tm = |f: &mut dyn FnMut()| { f(); let mut b = f64::MAX; for _ in 0..reps { let t0 = Instant::now(); f(); b = b.min(t0.elapsed().as_secs_f64() * 1e3); } b };
    for th in [1usize, 4] {
        hand.set_threads(th as i32);
        let h = tm(&mut || hand_batch(&mut hb));
        let u = tm(&mut || { run(&exact, &l, &qs, &mut cache, 0, th); });
        let ti = tm(&mut || { run(&exact, &l, &qs, &mut cache, tile, th); });
        println!("batch {nq} x {n} x {d}, {th} thr: hand batch {h:.1} ms | IR untiled {u:.1} ms | IR tiled ({tile} rows/tile) {ti:.1} ms  (tiled/untiled {:.2}x, tiled/hand {:.2}x)", u / ti, ti / h);
        t.insert(format!("t{th}"), json!({"hand_batch_ms": h, "ir_untiled_ms": u, "ir_tiled_ms": ti, "tiled_speedup": u / ti, "tiled_over_hand": ti / h}));
    }
    hand.set_threads(8);
    // cascade program: prefix 1,024 of the 5,120-wide rows (strided, in place) → top-64 → exact rescore → top-1
    let casc = [Op::Sift { d: 1024 }, Op::TopK { k: 64 }, Op::Rescore { d: d as u16 }, Op::TopK { k: 1 }];
    let full = [Op::Sift { d: d as u16 }, Op::TopK { k: 1 }];
    let r0 = cache.renders;
    let oc = run(&casc, &l, &qs, &mut cache, tile, 4); let of = run(&full, &l, &qs, &mut cache, tile, 4);
    let agree = oc.iter().zip(&of).filter(|(a, b)| a[0] == b[0]).count();
    let truth = oc.iter().enumerate().filter(|(i, v)| v[0].0 as usize == (i * 541) % n).count();
    let tc = tm(&mut || { run(&casc, &l, &qs, &mut cache, tile, 4); }); let tf = tm(&mut || { run(&full, &l, &qs, &mut cache, tile, 4); });
    println!("cascade (prefix 1024 → top-64 → rescore 5120): top-1 == exact {agree}/{nq}, == stored row {truth}/{nq} | {tc:.1} ms vs exact {tf:.1} ms ({:.2}x)", tf / tc);
    // program change → re-render only new kernels
    let progs = [encode(&exact), encode(&casc), encode(&[Op::Sift { d: 2048 }, Op::TopK { k: 8 }])];
    let before = cache.renders; let us0 = cache.render_us;
    let _ = run(&[Op::Sift { d: 2048 }, Op::TopK { k: 8 }], &l, &qs[..1], &mut cache, 0, 1);
    let rerender_us = cache.render_us - us0;
    println!("program change: hashes {:x?} | new kernels rendered {} in {:.0} µs total ({} renders this run; cascade added {})",
        progs.iter().map(|p| hash(p)).collect::<Vec<_>>(), cache.renders - before, rerender_us, cache.renders, cache.renders.saturating_sub(r0));
    let res = json!({"lattice": [n, d], "batch": nq, "tile_rows": tile, "mismatches_vs_hand_batch": bad, "ms": t,
        "cascade": {"program": format!("{casc:?}"), "top1_agree_exact": agree, "top1_true": truth, "of": nq, "ms": tc, "exact_ms": tf, "speedup": tf / tc},
        "rerender": {"new_kernels": cache.renders - before, "us": rerender_us, "total_renders": cache.renders, "total_render_us": cache.render_us},
        "verdict": {"bitexact": bad == 0}});
    std::fs::write("results/ir0.json", serde_json::to_string_pretty(&res).unwrap()).unwrap();
    println!("IR-0: mismatches vs hand batch kernel {bad} -> results/ir0.json");
}
