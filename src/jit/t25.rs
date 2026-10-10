// SPDX-License-Identifier: LicenseRef-PolyForm-Small-Business-1.0.0 OR LicenseRef-PolyForm-Noncommercial-1.0.0
//! T25-CORE-JIT (author 10-06: "jit compile the 25t and render it in our trit renderer/browser … as live firing core"; plan
//! an unpublished plan). The doctor's torus as a core INSIDE the engine: every layout box is one point
//! on T25 (25 coprime rings 3…101 in five groups of five — x0, y0, w, h, ry = place in its frame), ours vs Chrome is a pair of
//! points, the per-ring residue gap is an angle, the CRT over a group's five gaps is that coordinate's exact error (1/64 px).
//! Every residue is < 128, so a box's whole T25 point is 25 bytes = ONE 32-byte AVX2 row: the twist of every ring of a box is
//! one vpsubb + vpcmpgtb + vpand + vpaddb. The PROGRAM is the moduli (the tier): its bytes hash to the rendered kernel.
//! Reference (golden): tools/box_t25.py, t25_ifs.py, t25_families.py — this core must equal them bit for bit (step 1).

/// The five coordinate groups of T25 (box_t25.py `G`), in lane order.
pub const T25: [[u8; 5]; 5] = [[3, 29, 41, 59, 101], [5, 31, 37, 67, 89], [7, 23, 43, 61, 97], [11, 19, 47, 71, 83], [13, 17, 53, 73, 79]];
pub const NAMES: [&str; 5] = ["x0", "y0", "w", "h", "ry"];
pub const U: f64 = 64.0;

/// A tier = which rings carry which coordinate. Lanes 0..25 = the groups flattened; lanes 25..32 are zero (inert).
pub struct Program { pub groups: Vec<Vec<u8>>, pub mods: [u8; 32], pub off: [i64; 5], pub prod: [u64; 5], coef: Vec<Vec<u64>> }

impl Program {
    pub fn t25() -> Program { Program::new(T25.iter().map(|g| g.to_vec()).collect()) }
    pub fn new(groups: Vec<Vec<u8>>) -> Program {
        let mut mods = [0u8; 32];
        let mut k = 0;
        for g in &groups { for &m in g { mods[k] = m; k += 1; } }
        let prod: Vec<u64> = groups.iter().map(|g| g.iter().map(|&m| m as u64).product()).collect();
        // CRT basis per group: Mi · (Mi⁻¹ mod m), Mi = P / m
        let coef = groups.iter().zip(&prod).map(|(g, &p)| g.iter().map(|&m| { let m = m as u64; let mi = p / m; mi * inv(mi % m, m) % p }).collect()).collect();
        // offsets keep every coordinate non-negative on its group (box_t25.py OFF): x0/y0 from −16384 px, ry centred
        let off = [16384 * 64, 16384 * 64, 0, 0, (prod[4] / 2) as i64];
        Program { groups, mods, off, prod: [prod[0], prod[1], prod[2], prod[3], prod[4]], coef }
    }
    /// The program as bytes (its hash keys the rendered kernel).
    pub fn bytes(&self) -> Vec<u8> { self.groups.iter().flat_map(|g| std::iter::once(g.len() as u8).chain(g.iter().copied())).collect() }
    /// Coordinates (1/64 px) → the box's T25 point as residues, one lane per ring.
    pub fn residues(&self, c: &[i64; 5]) -> [u8; 32] {
        let mut r = [0u8; 32];
        let mut k = 0;
        for (g, ms) in self.groups.iter().enumerate() { for &m in ms { r[k] = (c[g] + self.off[g]).rem_euclid(m as i64) as u8; k += 1; } }
        r
    }
    /// The twist (per-ring gaps) → five exact errors, 1/64 px, centred (box_t25.py `errors`).
    pub fn decode(&self, d: &[u8; 32]) -> [i64; 5] {
        let mut e = [0i64; 5];
        let mut k = 0;
        for (g, ms) in self.groups.iter().enumerate() {
            let p = self.prod[g] as u128;
            let mut s: u128 = 0;
            for (i, _) in ms.iter().enumerate() { s += d[k] as u128 * self.coef[g][i] as u128; k += 1; }
            let v = (s % p) as i64;
            e[g] = if v as u128 > p / 2 { v - p as i64 } else { v };
        }
        e
    }
}

fn inv(a: u64, m: u64) -> u64 { (1..m).find(|&x| a * x % m == 1).unwrap_or(0) }

/// Python's round() (ties to even) on the 1/64 grid, so coordinates equal the reference's.
pub fn q(v: f64) -> i64 { (v * U).round_ties_even() as i64 }

/// box_t25.py `coords`: x0, y0, w, h, and ry = y − frame y.
pub fn coords(r: [f32; 4], frame: Option<[f32; 4]>) -> [i64; 5] {
    let f = |x: f32| x as f64;
    [q(f(r[0])), q(f(r[1])), q(f(r[2])), q(f(r[3])), q(f(r[1]) - frame.map(|p| f(p[1])).unwrap_or(0.0))]
}

/// Scalar twist (reference): per ring (ours − Chrome) mod m.
pub fn twist_scalar(ro: &[[u8; 32]], rc: &[[u8; 32]], mods: &[u8; 32], out: &mut [[u8; 32]]) {
    for i in 0..ro.len() { for k in 0..32 { let m = mods[k] as i16; out[i][k] = if m == 0 { 0 } else { (ro[i][k] as i16 - rc[i][k] as i16).rem_euclid(m) as u8 }; } }
}

/// AVX2 twist: one row per box, all 25 rings at once — d = o − c; d += m where d < 0. The hand kernel step 2 renders.
#[target_feature(enable = "avx2")]
pub unsafe fn twist_avx2(ro: &[[u8; 32]], rc: &[[u8; 32]], mods: &[u8; 32], out: &mut [[u8; 32]]) {
    use std::arch::x86_64::*;
    let m = _mm256_loadu_si256(mods.as_ptr() as *const __m256i);
    let z = _mm256_setzero_si256();
    for i in 0..ro.len() {
        let a = _mm256_loadu_si256(ro[i].as_ptr() as *const __m256i);
        let b = _mm256_loadu_si256(rc[i].as_ptr() as *const __m256i);
        let d = _mm256_sub_epi8(a, b);
        let neg = _mm256_cmpgt_epi8(z, d);
        let d = _mm256_add_epi8(d, _mm256_and_si256(neg, m));
        _mm256_storeu_si256(out[i].as_mut_ptr() as *mut __m256i, d);
    }
}

/// The triangle key (t25_families.py `tri_key`): display pair + one trit per coordinate of the triangle twist ΔB − ΔP,
/// 0 inside the agreement tolerance (2 px, or 5 % of Chrome's size for w / h).
pub fn tri_trits(eb: &[i64; 5], ep: &[i64; 5], chrome: [f32; 4]) -> [i8; 4] {
    let mut t = [0i8; 4];
    for g in 0..4 {
        let rel = (eb[g] - ep[g]) as f64 / U;
        let tol = if g >= 2 { 2f64.max(0.05 * (chrome[g] as f64).abs()) } else { 2.0 };
        t[g] = if rel.abs() <= tol { 0 } else if rel > 0.0 { 1 } else { -1 };
    }
    t
}

/// The RENDERED core (step 2): the program's rings compiled into straight-line x86 — per box, per ring, the coordinate plus its
/// group offset, reduced by the ring's magic number (q = x·⌈2⁴⁰/m⌉ ≫ 40, r = x − q·m: exact for 0 ≤ x < 2³⁶, no divide), one byte
/// each into the box's 32-byte row; then the whole row is twisted against Chrome's plate in AVX2 (vpsubb, vpcmpgtb, vpand,
/// vpaddb). The moduli and magics are immediates in the code — changing the tier re-renders it (JITW-0's "weights compiled away").
/// Entry: fn(coords: *const i64 [n×5], n, res_out: *mut u8 [n×32], plate: *const u8 [n×32], twist_out: *mut u8 [n×32], mods: *const u8 [32]).
pub struct Rendered { pub exec: super::x86::Exec, pub code: Vec<u8>, pub hash: u64, pub render_us: f64, pub ring_at: Vec<usize>, pub tail_at: usize }
pub type Kernel = extern "C" fn(*const i64, usize, *mut u8, *const u8, *mut u8, *const u8);

pub fn render(pr: &Program) -> Rendered {
    use super::x86::*;
    let t = std::time::Instant::now();
    let mut a = Asm::default();
    a.test64(RSI);
    let to_end = a.jcc(false, 0);
    a.vmovdqu_ld(15, R9, 0);       // ymm15 = the moduli, one per lane
    a.vpxor(14, 14, 14);           // ymm14 = 0
    let top = a.b.len();
    let mut ring_at = Vec::new();
    let mut k = 0i32;
    for (g, ms) in pr.groups.iter().enumerate() {
        for &m in ms {
            let magic = ((1u128 << 40) + m as u128 - 1) / m as u128;
            ring_at.push(a.b.len());
            a.ld64(RAX, RDI, 8 * g as i32);
            a.add64_i(RAX, pr.off[g] as i32);
            a.mov64(R10, RAX);
            a.movabs(R11, magic as u64);
            a.imul64(R10, R11);
            a.shr64_i(R10, 40);
            a.imul64_i(R10, R10, m as i32);
            a.sub64(RAX, R10);
            a.st8(RDX, k, RAX);
            k += 1;
        }
    }
    let tail_at = a.b.len();
    a.vmovdqu_ld(0, RDX, 0);       // ours: the box's T25 point
    a.vmovdqu_ld(1, RCX, 0);       // Chrome's point from the plate
    a.vpsubb(0, 0, 1);
    a.vpcmpgtb(2, 14, 0);          // lanes that went negative
    a.vpand(2, 2, 15);
    a.vpaddb(0, 0, 2);             // + m there: the twist, every ring at once
    a.vmovdqu_st(R8, 0, 0);
    a.add64_i(RDI, 8 * pr.groups.len() as i32); a.add64_i(RDX, 32); a.add64_i(RCX, 32); a.add64_i(R8, 32);
    a.dec64(RSI);
    a.jcc(true, top);
    let end = a.b.len(); a.patch(to_end, end);
    a.vzeroupper(); a.ret();
    let exec = Exec::new(&a.b);
    Rendered { exec, hash: super::ir::hash(&pr.bytes()), render_us: t.elapsed().as_secs_f64() * 1e6, code: a.b, ring_at, tail_at }
}

impl Rendered {
    pub fn kernel(&self) -> Kernel { unsafe { std::mem::transmute::<*const u8, Kernel>(self.exec.ptr()) } }
}

/// The whole point as ONE integer on T25 (Garner's mixed-radix CRT over all 25 rings; every partial < P25 < 2¹²⁷, u128-exact).
pub fn point_u128(pr: &Program, r: &[u8; 32]) -> (u128, u128) {
    let mut x: u128 = 0; let mut big: u128 = 1; let mut k = 0;
    for g in &pr.groups { for &m in g {
        let m = m as u128;
        let t = ((r[k] as u128 + m - x % m) % m) * inv((big % m) as u64, m as u64) as u128 % m;
        x += big * t; big *= m; k += 1;
    } }
    (x, big)
}

/// `omni-render t25-bus-bench [N]` (WIRE-ALL B1; author 10-07: "load the t25 and the t25 jit into renderer and speedtest … this may be
/// the one event bus"). The RENDERED core above, unchanged, used as an event bus: an event = 5 fields, one per coordinate group
/// (clock · token · generator plate · level×stage · position in row) → its 25 residues = its T25 address; the plate row = the
/// PRECOMPUTED expected event, so twist 0 on all 25 rings = confirmed and a nonzero ring = where the event deviates; dispatch =
/// ledger[ring][residue] += 1 on every ring (CRT jump, nothing scans — law R14). Timed batched (one call per N events), per token
/// (one call per event = the token-by-token case) and the whole bus op per token (encode + confirm + dispatch), vs the scalar
/// reference. Verified on every event: residues == Program::residues, twist == twist_scalar; 1 % of plate rows are damaged
/// (expected token + 1) and must be the events flagged — so the check can fail.
pub fn bus_bench(n: usize) {
    let pr = Program::t25(); let rd = render(&pr); let kern = rd.kernel();
    let mut s = 0x9E37_79B9_7F4A_7C15u64; let mut rnd = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s };
    let mut ev = vec![0i64; n * 5];
    for i in 0..n { ev[5 * i] = i as i64; ev[5 * i + 1] = (rnd() % 8192) as i64; ev[5 * i + 2] = (rnd() % 37) as i64;
                    ev[5 * i + 3] = (rnd() % 41) as i64; ev[5 * i + 4] = (i % 512) as i64; }
    let damaged: Vec<bool> = (0..n).map(|i| i % 100 == 37).collect();
    let plate: Vec<[u8; 32]> = (0..n).map(|i| { let mut c = [ev[5 * i], ev[5 * i + 1], ev[5 * i + 2], ev[5 * i + 3], ev[5 * i + 4]];
        if damaged[i] { c[1] += 1; } pr.residues(&c) }).collect();
    let (mut res, mut tw) = (vec![[0u8; 32]; n], vec![[0u8; 32]; n]);
    let call = |e: &[i64], m: usize, r: &mut [[u8; 32]], p: &[[u8; 32]], t: &mut [[u8; 32]]|
        kern(e.as_ptr(), m, r.as_mut_ptr() as *mut u8, p.as_ptr() as *const u8, t.as_mut_ptr() as *mut u8, pr.mods.as_ptr());
    // batched: one call over all N events (best of 5)
    let mut batched = f64::MAX;
    for _ in 0..5 { let t = std::time::Instant::now(); call(&ev, n, &mut res, &plate, &mut tw); batched = batched.min(t.elapsed().as_secs_f64()); }
    // per token: one call per event
    let mut per = f64::MAX;
    for _ in 0..3 { let t = std::time::Instant::now();
        for i in 0..n { call(&ev[5 * i..5 * i + 5], 1, &mut res[i..i + 1], &plate[i..i + 1], &mut tw[i..i + 1]); }
        per = per.min(t.elapsed().as_secs_f64()); }
    // the whole bus op per token: encode (rendered) + confirm (twist row == 0) + dispatch (ledger[ring][residue] += 1, 25 rings)
    let mut ledger = vec![[0u32; 128]; 25]; let (mut confirmed, mut flagged) = (0usize, 0usize); let mut flag_ok = true;
    let t = std::time::Instant::now();
    for i in 0..n {
        call(&ev[5 * i..5 * i + 5], 1, &mut res[i..i + 1], &plate[i..i + 1], &mut tw[i..i + 1]);
        let w = unsafe { std::ptr::read_unaligned(tw[i].as_ptr() as *const [u64; 4]) };
        if (w[0] | w[1] | w[2] | w[3]) == 0 { confirmed += 1; } else { flagged += 1; flag_ok &= damaged[i]; }
        for k in 0..25 { ledger[k][res[i][k] as usize] += 1; }
    }
    let bus = t.elapsed().as_secs_f64();
    // scalar reference (same work without the rendered code) + exactness on every event
    let (mut rs, mut ts) = (vec![[0u8; 32]; n], vec![[0u8; 32]; n]);
    let t = std::time::Instant::now();
    for i in 0..n { rs[i] = pr.residues(&[ev[5 * i], ev[5 * i + 1], ev[5 * i + 2], ev[5 * i + 3], ev[5 * i + 4]]); }
    twist_scalar(&rs, &plate, &pr.mods, &mut ts);
    let scalar = t.elapsed().as_secs_f64();
    let bad_r = (0..n).filter(|&i| res[i] != rs[i]).count(); let bad_t = (0..n).filter(|&i| tw[i] != ts[i]).count();
    let n_dmg = damaged.iter().filter(|&&d| d).count();
    let total: u64 = ledger.iter().flat_map(|l| l.iter()).map(|&c| c as u64).sum();
    let ok = bad_r == 0 && bad_t == 0 && flag_ok && flagged == n_dmg && confirmed == n - n_dmg && total == 25 * n as u64;
    let ns = |s: f64| s * 1e9 / n as f64;
    println!("{}", serde_json::json!({"events": n, "code_bytes": rd.code.len(), "render_us": rd.render_us,
        "batched_ns_per_event": ns(batched), "per_token_call_ns": ns(per), "bus_op_ns_per_token": ns(bus), "scalar_ns_per_event": ns(scalar),
        "events_per_s": {"batched": (n as f64 / batched) as u64, "per_token": (n as f64 / per) as u64, "bus_op": (n as f64 / bus) as u64},
        "residue_mismatches": bad_r, "twist_mismatches": bad_t, "confirmed": confirmed, "flagged": flagged, "damaged_plate_rows": n_dmg,
        "ledger_total": total, "load": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().split_whitespace().take(3).collect::<Vec<_>>()}));
    println!("T25-BUS-BENCH {}", if ok { "PASS" } else { "FAIL" });
}

// ── E1 (projects/doctor-core/03-evolve-t25/PLAN.md; T25-CORE-JIT step 4; author 10-07 "correct, that is step 1"): the T25 doctor core
// FIRES on every layout pass. A PLATE (the oracle's points + each box's agreement band) is armed for a document; at the end of
// `Layout::run` the rendered kernel fires over every comparable box: residues + twist vs the plate, CRT decode → exact errors,
// agreement = every axis error inside its band, roots (wrong box, agreeing frame), triangle keys → the live family queue.
// The band is exact: the integer interval of our 1/64-px coordinate for which the station's float rule (|o − c| ≤ max(2, 5 %·|c|))
// holds, found by evaluating that same rule, shifted by Chrome's point — so the core's agreement equals the station's bit for bit.
use std::collections::{HashMap, HashSet};

pub struct Plate { pub page: String, pub chrome: HashMap<u32, ([f32; 4], [[i64; 2]; 4])> }
pub struct Fired { pub page: String, pub kernel: u64, pub boxes: usize, pub agree: HashSet<u32>, pub roots: usize,
                   pub fam: HashMap<(String, String, [i8; 4]), usize>, pub fire_us: f64, pub layout_us: f64 }

thread_local! {
    static PLATE: std::cell::RefCell<Option<Plate>> = const { std::cell::RefCell::new(None) };
    static CORE: std::cell::RefCell<Option<(Program, Option<Rendered>)>> = const { std::cell::RefCell::new(None) };
    static LAST: std::cell::RefCell<Option<Fired>> = const { std::cell::RefCell::new(None) };
}

/// Arm (Some) or disarm (None) the plate for the next layout passes on this thread.
pub fn arm(p: Option<Plate>) { PLATE.with(|x| *x.borrow_mut() = p); }
/// The last firing's result (taken).
pub fn take_last() -> Option<Fired> { LAST.with(|x| x.borrow_mut().take()) }

/// Agreement band of one Chrome coordinate, in error units (ours − Chrome's 1/64 point).
pub fn band(c: f32) -> [i64; 2] {
    let ok = |oi: i64| ((oi as f32 / 64.0) - c).abs() <= 2.0f32.max(c.abs() * 0.05);
    let tol = 2.0f64.max(c.abs() as f64 * 0.05);
    let (a, b) = (((c as f64 - tol) * U).floor() as i64, ((c as f64 + tol) * U).ceil() as i64);
    let mut lo = a - 4; while lo <= b + 4 && !ok(lo) { lo += 1; }
    let mut hi = b + 4; while hi >= lo && !ok(hi) { hi -= 1; }
    let qc = q(c as f64);
    [lo - qc, hi - qc]
}

/// Called at the end of `Layout::run`: fires the core when a plate is armed (otherwise one thread-local check).
pub fn fire(lay: &crate::layout::Layout, layout_us: f64) {
    PLATE.with(|pl| {
        let pl = pl.borrow();
        let Some(plate) = pl.as_ref() else { return };
        let t = std::time::Instant::now();
        let dom = lay.dom; let none = crate::dom::NONE;
        let mut ours: HashMap<u32, [i32; 4]> = HashMap::with_capacity(lay.out.boxes.len());
        for b in &lay.out.boxes { ours.insert(b.0, b.1); }
        let mut order: Vec<u32> = plate.chrome.keys().copied().filter(|n| ours.contains_key(n)).collect(); order.sort();
        let idx: HashMap<u32, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let par: Vec<u32> = order.iter().map(|&n| { let mut m = n; loop { m = dom.nodes[m as usize].parent; if m == none || idx.contains_key(&m) { return m; } } }).collect();
        let co: Vec<i64> = order.iter().zip(&par).flat_map(|(&n, &p)| { let o = ours[&n]; let fy = if p != none { ours[&p][1] } else { 0 };
            [o[0] as i64, o[1] as i64, o[2] as i64, o[3] as i64, (o[1] - fy) as i64] }).collect();
        CORE.with(|c| {
            let mut c = c.borrow_mut();
            let (pr, rk) = c.get_or_insert_with(|| { let pr = Program::t25(); let rk = if is_x86_feature_detected!("avx2") { Some(render(&pr)) } else { None }; (pr, rk) });
            let rc: Vec<[u8; 32]> = order.iter().zip(&par).map(|(&n, &p)| pr.residues(&coords(plate.chrome[&n].0, if p != none { Some(plate.chrome[&p].0) } else { None }))).collect();
            let mut res = vec![[0u8; 32]; order.len()]; let mut tw = vec![[0u8; 32]; order.len()];
            match rk {
                Some(rk) => (rk.kernel())(co.as_ptr(), order.len(), res.as_mut_ptr() as *mut u8, rc.as_ptr() as *const u8, tw.as_mut_ptr() as *mut u8, pr.mods.as_ptr()),
                None => { for (i, r) in res.iter_mut().enumerate() { let c5 = [co[5 * i], co[5 * i + 1], co[5 * i + 2], co[5 * i + 3], co[5 * i + 4]]; *r = pr.residues(&c5); }
                          twist_scalar(&res, &rc, &pr.mods, &mut tw); }
            }
            let e: Vec<[i64; 5]> = tw.iter().map(|d| pr.decode(d)).collect();
            let ok: Vec<bool> = order.iter().enumerate().map(|(i, n)| { let b = &plate.chrome[n].1; (0..4).all(|k| e[i][k] >= b[k][0] && e[i][k] <= b[k][1]) }).collect();
            let disp = |n: u32| format!("{:?}", lay.st.table[lay.st.ids[n as usize] as usize].display);
            let (mut roots, mut fam) = (0usize, HashMap::new());
            for i in 0..order.len() {
                let p = par[i];
                if !ok[i] && p != none && ok[idx[&p]] {
                    roots += 1;
                    *fam.entry((disp(p), disp(order[i]), tri_trits(&e[i], &e[idx[&p]], plate.chrome[&order[i]].0))).or_insert(0usize) += 1;
                }
            }
            let agree: HashSet<u32> = order.iter().zip(&ok).filter(|x| *x.1).map(|x| *x.0).collect();
            let kernel = rk.as_ref().map_or(0, |r| r.hash);
            LAST.with(|l| *l.borrow_mut() = Some(Fired { page: plate.page.clone(), kernel, boxes: order.len(), agree, roots, fam,
                fire_us: t.elapsed().as_secs_f64() * 1e6, layout_us }));
        });
    });
}

// ── PRISTINE MAP (doctor-core 03-evolve-t25 S1/M1, author 10-07): the core fires against the page's OWN exact geometry — no oracle.
// The pristine plate is derived inside the core from the laid-out boxes: each sub-pixel near-miss of a geometric relation
// (the `--abc pristine` rules: inside the frame, flush to the frame's content box, aligned with / touching / equal to / evenly
// spaced after the previous sibling) names the exact coordinate the box should have. The box's T25 point vs its pristine point
// → twist → CRT decode = the exact error per axis; the relation that set it = the defect kind; (frame display, box display,
// kind, exact sign trits vs the frame) = the family key (reuse / learning, not needed to find the boxes).
// DORMANT by default: armed on first load, on a wake trigger, and for every pass while repairing (author 10-07).
pub const KINDS: [&str; 6] = ["overflow", "near_flush", "near_align", "near_touch", "near_equal", "uneven_gap"];
pub struct PristinePlate { pub page: String, pub only: Option<HashSet<u32>>, pub wake: &'static str }
pub struct Defect { pub node: u32, pub frame: u32, pub hits: Vec<(usize, i64)>, pub err: [i64; 5], pub key: (String, String, &'static str, [i8; 4]),
                   pub rect: [i64; 4], pub frect: [i64; 4] }
pub struct Map { pub page: String, pub wake: &'static str, pub kernel: u64, pub boxes: usize, pub pristine: usize, pub by_kind: [usize; 6],
                 pub defects: Vec<Defect>, pub clean: Vec<u32>, pub overlaps: usize, pub fire_us: f64, pub layout_us: f64 }

thread_local! {
    static PPLATE: std::cell::RefCell<Option<PristinePlate>> = const { std::cell::RefCell::new(None) };
    static LASTMAP: std::cell::RefCell<Option<Map>> = const { std::cell::RefCell::new(None) };
}
/// Wake the doctor for the next layout pass(es) on this thread (None = dormant).
pub fn arm_pristine(p: Option<PristinePlate>) { PPLATE.with(|x| *x.borrow_mut() = p); }
pub fn take_map() -> Option<Map> { LASTMAP.with(|x| x.borrow_mut().take()) }

pub fn fire_pristine(lay: &crate::layout::Layout, layout_us: f64) {
    PPLATE.with(|pl| {
        let pl = pl.borrow();
        let Some(plate) = pl.as_ref() else { return };
        let t = std::time::Instant::now();
        let dom = lay.dom; let none = crate::dom::NONE;
        let st = |n: u32| &lay.st.table[lay.st.ids[n as usize] as usize];
        let mut ours: HashMap<u32, [i32; 4]> = HashMap::with_capacity(lay.out.boxes.len());
        for b in &lay.out.boxes { ours.insert(b.0, b.1); }
        let mut order: Vec<u32> = match &plate.only {
            Some(s) => s.iter().copied().filter(|n| ours.contains_key(n)).collect(),
            None => ours.keys().copied().filter(|&n| dom.nodes[n as usize].kind == crate::dom::Kind::Element && !dom.tag(n).starts_with("::")
                && st(n).display != crate::style::Display::Inline).collect(),
        };
        order.sort();
        let inset: HashSet<u32> = order.iter().copied().collect();
        let up = |n: u32| { let mut m = n; loop { m = dom.nodes[m as usize].parent; if m == none || inset.contains(&m) { return m; } } };
        let par: HashMap<u32, u32> = order.iter().map(|&n| (n, up(n))).collect();
        let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
        for &n in &order { let q = par[&n]; if q != none { kids.entry(q).or_default().push(n); } }
        let prev: HashMap<u32, u32> = kids.values().flat_map(|v| v.windows(2).map(|w| (w[1], w[0]))).collect();
        let near = |d: i64| (1..=63).contains(&d.abs());
        let i = |v: i32| v as i64;
        let (mut boxes, mut pristine, mut by_kind) = (0usize, 0usize, [0usize; 6]);
        let mut clean: Vec<u32> = Vec::new();
        let mut rows: Vec<(u32, u32, Vec<(usize, i64)>, [i64; 5], [i64; 5])> = Vec::new(); // node, frame, hits, ours, target (coords)
        for &n in &order {
            let q = par[&n]; if q == none { continue; }
            let b = ours[&n]; let f = ours[&q];
            if b[2] <= 0 || b[3] <= 0 { continue; }
            boxes += 1;
            let (bx, by, bw, bh) = (i(b[0]), i(b[1]), i(b[2]), i(b[3])); let (fx, fy, fw) = (i(f[0]), i(f[1]), i(f[2]));
            let mut tg = [bx, by, bw, bh];
            let mut set = [false; 4];
            let mut put = |k: usize, v: i64, tg: &mut [i64; 4]| if !set[k] { tg[k] = v; set[k] = true; };
            let mut hits: Vec<(usize, i64)> = Vec::new();
            let (ovl, ovr, ovt) = (fx - bx, (bx + bw) - (fx + fw), fy - by);
            let ov = ovl.max(ovr).max(ovt).max(0);
            if ov > 32 && st(q).overflow == 0 && !matches!(st(n).position, 2 | 3) {
                hits.push((0, ov));
                if ovl == ov { put(0, fx, &mut tg) } else if ovr == ov { put(0, fx + fw - bw, &mut tg) } else { put(1, fy, &mut tg) }
            }
            let fs = st(q);
            let pl = crate::layout::res0(fs.padding[3], f[2]) as i64 + fs.border_w[3] as i64;
            let pr = crate::layout::res0(fs.padding[1], f[2]) as i64 + fs.border_w[1] as i64;
            let dl = bx - (fx + pl); let dr = (fx + fw - pr) - (bx + bw);
            if near(dl) { hits.push((1, dl)); put(0, fx + pl, &mut tg) } else if near(dr) { hits.push((1, dr)); put(0, bx + dr, &mut tg) }
            if let Some(&s) = prev.get(&n) {
                let sb = ours[&s]; let (sx, sy, sw, sh) = (i(sb[0]), i(sb[1]), i(sb[2]), i(sb[3]));
                let da = bx - sx; if near(da) { hits.push((2, da)); put(0, sx, &mut tg) }
                let gap = by - (sy + sh); if near(gap) { hits.push((3, gap)); put(1, sy + sh, &mut tg) }
                let dw = bw - sw; if (2..=64).contains(&dw.abs()) { hits.push((4, dw)); put(2, sw, &mut tg) }
                if let Some(&s2) = prev.get(&s) {
                    let s2b = ours[&s2]; let (s2x, s2y, s2w) = (i(s2b[0]), i(s2b[1]), i(s2b[2]));
                    if by == sy && sy == s2y && bx > sx && sx > s2x {
                        let g1 = sx - (s2x + s2w); let g2 = bx - (sx + sw);
                        let dg = g2 - g1; if (2..=64).contains(&dg.abs()) { hits.push((5, dg)); put(0, sx + sw + g1, &mut tg) }
                    }
                }
            }
            if hits.is_empty() { pristine += 1; clean.push(n); continue; }
            for h in &hits { by_kind[h.0] += 1; }
            rows.push((n, q, hits, [bx, by, bw, bh, by - i(f[1])], [tg[0], tg[1], tg[2], tg[3], tg[1] - i(f[1])]));
        }
        // OVERLAP guard (agent check 10-07): the six relations never test siblings against each other, so a fix that pulls a box
        // "inside its frame" could stack it on a neighbour and still score pristine. Count in-flow sibling pairs whose boxes
        // intersect (both axes > 0; floats and abs/fixed excluded — they overlap by design); a fix may not raise it.
        let mut overlaps = 0usize;
        for v in kids.values() {
            let mut r: Vec<[i64; 4]> = v.iter().filter(|&&n| st(n).float == 0 && !matches!(st(n).position, 2 | 3)).map(|n| ours[n]).filter(|b| b[2] > 0 && b[3] > 0)
                .map(|b| [b[0] as i64, b[1] as i64, (b[0] + b[2]) as i64, (b[1] + b[3]) as i64]).collect();
            r.sort();
            for i in 0..r.len() { for j in i + 1..r.len() { if r[j][0] >= r[i][2] { break; } if r[j][1] < r[i][3] && r[i][1] < r[j][3] { overlaps += 1; } } }
        }
        CORE.with(|c| {
            let mut c = c.borrow_mut();
            let (pr, rk) = c.get_or_insert_with(|| { let pr = Program::t25(); let rk = if is_x86_feature_detected!("avx2") { Some(render(&pr)) } else { None }; (pr, rk) });
            let co: Vec<i64> = rows.iter().flat_map(|r| r.3).collect();
            let plate_rows: Vec<[u8; 32]> = rows.iter().map(|r| pr.residues(&r.4)).collect();
            let mut res = vec![[0u8; 32]; rows.len()]; let mut tw = vec![[0u8; 32]; rows.len()];
            match rk {
                Some(rk) if !rows.is_empty() => (rk.kernel())(co.as_ptr(), rows.len(), res.as_mut_ptr() as *mut u8, plate_rows.as_ptr() as *const u8, tw.as_mut_ptr() as *mut u8, pr.mods.as_ptr()),
                _ => { for (k, r) in res.iter_mut().enumerate() { *r = pr.residues(&rows[k].3); } twist_scalar(&res, &plate_rows, &pr.mods, &mut tw); }
            }
            let err: Vec<[i64; 5]> = tw.iter().map(|d| pr.decode(d)).collect();
            let at: HashMap<u32, usize> = rows.iter().enumerate().map(|(k, r)| (r.0, k)).collect();
            let disp = |n: u32| format!("{:?}", st(n).display);
            let defects: Vec<Defect> = rows.iter().enumerate().map(|(k, r)| {
                let fe = at.get(&r.1).map(|&j| err[j]).unwrap_or([0; 5]);
                let mut tr = [0i8; 4]; for g in 0..4 { tr[g] = (err[k][g] - fe[g]).signum() as i8; }
                let (b, f) = (ours[&r.0], ours[&r.1]);
                Defect { node: r.0, frame: r.1, hits: r.2.clone(), err: err[k], key: (disp(r.1), disp(r.0), KINDS[r.2[0].0], tr),
                    rect: [b[0] as i64, b[1] as i64, b[2] as i64, b[3] as i64], frect: [f[0] as i64, f[1] as i64, f[2] as i64, f[3] as i64] }
            }).collect();
            let kernel = rk.as_ref().map_or(0, |r| r.hash);
            LASTMAP.with(|l| *l.borrow_mut() = Some(Map { page: plate.page.clone(), wake: plate.wake, kernel, boxes, pristine, by_kind, defects, clean, overlaps,
                fire_us: t.elapsed().as_secs_f64() * 1e6, layout_us }));
        });
    });
}
