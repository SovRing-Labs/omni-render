// SPDX-License-Identifier: LicenseRef-PolyForm-Small-Business-1.0.0 OR LicenseRef-PolyForm-Noncommercial-1.0.0
//! SHAPE station (author 10-06: "we're measuring the angles and seeing where it differs from the shape it's supposed to have …
//! the square in the square hole; if the square hole turns into a circle, it's wrong").
//! A page is a shape. Every box sits in its frame (its parent) at angles; Chrome's angles are the shape it is supposed to have,
//! ours are the shape we drew. An angle is the comparison of two values (atan2(part, frame)), so it does not move when the whole
//! frame shifts or scales: error that only flows through from elsewhere keeps every angle, and the box whose angle turned is
//! where the shape broke. Four angles per box:
//!   W  = ∠(w_n, w_p)                       width share of the frame (flows down)
//!   X  = ∠(x_n − x_p, w_p)                 offset in the frame (flows down)
//!   Y  = ∠(y_n − y_ref, w_p)               gap after the previous sibling (or the frame top) (flows sideways)
//!   H  = ∠(ext_children, h_n) or ∠(h_n, w_n) for a leaf   fit of the content in the box (flows up)
//! Deformed on axis k ⇔ |∠ours − ∠chrome| > the turn that Chrome's agreement tolerance max(2 px, 5 %) gives that part.
//! Every failing box is charged to the deformed box it inherits from (itself, a deformed descendant, an ancestor, or a
//! sibling before one of them) = blast radius. Field kin: similarity invariants in shape analysis (angles are invariant under
//! translation and uniform scale); frame invariance (math index #82); R1 geometry where items are compared.
//! `omni-render core corpus --abc shape [--all] [--top N] [--out F]`
use std::collections::{BTreeMap, HashMap};

/// ∠(part, frame): the comparison of two values as an angle.
pub fn angle(part: f32, frame: f32) -> f32 { part.atan2(frame) }
/// How far ∠(part, frame) turns when `part` moves by Chrome's agreement tolerance τ = max(2 px, 5 %).
pub fn angle_tol(part: f32, frame: f32) -> f32 { let t = 2f32.max(part.abs() * 0.05); (angle(part.abs() + t, frame.abs()) - angle(part.abs(), frame.abs())).abs() }
/// The square in the square hole: ours fits Chrome's shape on this angle.
pub fn fits(part_o: f32, frame_o: f32, part_c: f32, frame_c: f32) -> bool { (angle(part_o, frame_o) - angle(part_c, frame_c)).abs() <= angle_tol(part_c, frame_c) + 1e-4 }

/// Aliasing: snap an angle to a grid of `q` degrees (a phase grid), so sub-grid noise reads as the same angle.
pub fn snap(a: f32, q: f32) -> f32 { if q <= 0.0 { a } else { let r = q.to_radians(); (a / r).round() * r } }
/// Feathered outline: how far outside Chrome's shape a turn lies, as a soft 0→1 ramp. z = turn / tolerance; the outline is
/// `outline` tolerances thick and its edge is feathered over `feather` tolerances (linear ramp centred on the outline).
pub fn feather(turn: f32, tol: f32, outline: f32, feather: f32) -> f32 { let z = turn.abs() / tol.max(1e-6); ((z - outline) / feather.max(1e-6) + 0.5).clamp(0.0, 1.0) }

const AX: [&str; 4] = ["X", "Y", "W", "H"];
thread_local! { static LAST_E: std::cell::Cell<[f64; 4]> = const { std::cell::Cell::new([0.0; 4]) }; }

pub fn run(a: &[String]) {
    use crate::dom::{Kind, NONE};
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let top: usize = arg("--top").and_then(|v| v.parse().ok()).unwrap_or(25);
    let alias: f32 = arg("--alias").and_then(|v| v.parse().ok()).unwrap_or(0.25);     // degrees per grid step
    let outline: f32 = arg("--outline").and_then(|v| v.parse().ok()).unwrap_or(2.0);  // tolerances thick
    let fea: f32 = arg("--feather").and_then(|v| v.parse().ok()).unwrap_or(1.0);      // tolerances of soft edge
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    let t0 = std::time::Instant::now();
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let pages: Vec<_> = if a.iter().any(|x| x == "--all") { replay.iter().chain(hold.iter()).cloned().collect() } else { replay.clone() };
    let (mut n_fail, mut n_cov, mut n_def, mut n_def_ok, mut n_cause) = (0usize, 0f32, 0usize, 0usize, 0usize);
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let dump = arg("--dump"); let mut dump_rows: Vec<serde_json::Value> = Vec::new(); // every box's angle shape, ours + Chrome (reverse IFS input)
    let mut fam: BTreeMap<String, (usize, f32)> = BTreeMap::new(); // sig → (deformed boxes, failing boxes charged)
    for p in &pages {
        if dump.is_some() { crate::layout::ledger_start(); }
        let rr = super::super::rects(p);
        // the code path of every box: term + path.* markers from the ledger (author: box tree ↔ code AST, same parent-tree format)
        let mut code: HashMap<u32, std::collections::BTreeSet<String>> = HashMap::new();
        if dump.is_some() { for (n, k, v) in crate::layout::ledger_take() { code.entry(n).or_default().insert(if k == "line" { format!("L{v}") } else { k.to_string() }); } }
        let Some((c, agree, r)) = rr else { continue };
        let dom = &c.l.dom;
        let st = |n: u32| &c.l.styled.table[c.l.styled.ids[n as usize] as usize];
        let up = |mut n: u32| -> u32 { loop { n = dom.nodes[n as usize].parent; if n == NONE || r.contains_key(&n) { return n; } } };
        // comparable children / previous comparable sibling, through boxes that are not compared (inline, text)
        let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut order: Vec<u32> = r.keys().copied().collect(); order.sort();
        for &n in &order { let q = up(n); if q != NONE { kids.entry(q).or_default().push(n); } }
        let prev: HashMap<u32, u32> = kids.values().flat_map(|v| v.windows(2).map(|w| (w[1], w[0]))).collect();
        // the four angle tests
        let mut bad: HashMap<u32, Vec<(usize, f32, f32)>> = HashMap::new(); // axis, turn (deg), membership μ
        let mut ang: HashMap<u32, [[f32; 2]; 4]> = HashMap::new(); // every box: (∠ours, ∠chrome) per axis, snapped (the shape, for --dump)
        for &n in &order {
            let (o, ch) = r[&n];
            let mut hit = |k: usize, po: f32, fo: f32, pc: f32, fc: f32| {
                let (ao, ac) = (snap(angle(po, fo), alias), snap(angle(pc, fc), alias));
                ang.entry(n).or_insert([[f32::NAN; 2]; 4])[k] = [ao, ac];
                let turn = ao - ac;
                let mu = feather(turn, angle_tol(pc, fc), outline, fea);
                if mu > 0.0 { bad.entry(n).or_default().push((k, turn.to_degrees(), mu)); }
            };
            let q = up(n);
            if q != NONE {
                let (po, pc) = r[&q];
                hit(2, o[2], po[2], ch[2], pc[2]);
                hit(0, o[0] - po[0], po[2], ch[0] - pc[0], pc[2]);
                let (ro, rc) = match prev.get(&n) { Some(s) => { let (so, sc) = r[s]; (so[1] + so[3], sc[1] + sc[3]) } None => (po[1], pc[1]) };
                hit(1, o[1] - ro, po[2], ch[1] - rc, pc[2]);
            }
            match kids.get(&n) {
                Some(ks) => {
                    let ext = |i: usize| ks.iter().map(|k| { let b = if i == 0 { r[k].0 } else { r[k].1 }; b[1] + b[3] }).fold(f32::MIN, f32::max);
                    hit(3, ext(0) - o[1], o[3], ext(1) - ch[1], ch[3]);
                }
                None => hit(3, o[3], o[2], ch[3], ch[2]),
            }
        }
        // charge every failing box to the deformed box it inherits from
        let mut charge: HashMap<u32, usize> = HashMap::new();
        // nearest deformed box below each node (height flows up)
        let mut below: HashMap<u32, u32> = HashMap::new();
        // derived turns are not causes: a leaf's fit (H) follows its own width when the width is wrong (text re-wraps, scale does
        // not keep wrapping); a box's fit (H) follows a deformed box inside it (height flows up)
        let tol = |c: f32| 2f32.max(c.abs() * 0.05);
        let drop_h = |bad: &mut HashMap<u32, Vec<(usize, f32, f32)>>, n: u32| { if let Some(v) = bad.get_mut(&n) { v.retain(|x| x.0 != 3); if v.is_empty() { bad.remove(&n); } } };
        for &n in &order { if !kids.contains_key(&n) { let (o, ch) = r[&n]; if (o[2] - ch[2]).abs() > tol(ch[2]) { drop_h(&mut bad, n); } } }
        for &n in order.iter().rev() { if let Some(ks) = kids.get(&n) { if let Some(d) = ks.iter().find_map(|k| if bad.contains_key(k) { Some(*k) } else { below.get(k).copied() }) { below.insert(n, d); drop_h(&mut bad, n); } } }
        let mu = |n: u32| -> f32 { bad.get(&n).map(|v| v.iter().map(|x| x.2).fold(0.0, f32::max)).unwrap_or(0.0) };
        let mut charge: HashMap<u32, f32> = HashMap::new();
        let mut fail_here = 0;
        for &n in &order {
            if agree.contains(&n) { continue; }
            fail_here += 1;
            let (o, ch) = r[&n];
            let h_wrong = (o[3] - ch[3]).abs() > 2f32.max(ch[3].abs() * 0.05);
            // candidates in inheritance order: itself, the deformed box inside it (height), siblings before it and before each
            // ancestor (with what is inside them), the ancestors; the failing box's unit of blame is composited front to back
            let mut rem = 1.0f32;
            let mut take = |c: u32, rem: &mut f32| { let a = mu(c); if a > 0.0 { *charge.entry(c).or_default() += *rem * a; *rem *= 1.0 - a; } };
            take(n, &mut rem);
            if h_wrong { if let Some(&d) = below.get(&n) { take(d, &mut rem); } }
            let mut m = n;
            while rem > 0.01 {
                let mut s = m;
                while let Some(&b) = prev.get(&s) { take(b, &mut rem); if let Some(&d) = below.get(&b) { take(d, &mut rem); } if rem <= 0.01 { break; } s = b; }
                m = up(m); if m == NONE { break; }
                take(m, &mut rem);
            }
        }
        n_fail += fail_here; n_cov += charge.values().sum::<f32>();
        n_cause += bad.keys().filter(|&&n| mu(n) >= 0.5).count();
        n_def += bad.len(); n_def_ok += bad.keys().filter(|n| agree.contains(n)).count();
        let page = p.file_name().unwrap().to_string_lossy().to_string();
        if dump.is_some() {
            for &n in &order {
                let q = up(n); let a = ang.get(&n).copied().unwrap_or([[f32::NAN; 2]; 4]);
                let deg = |x: f32| if x.is_nan() { serde_json::Value::Null } else { serde_json::json!((x.to_degrees() * 100.0).round() / 100.0) };
                dump_rows.push(serde_json::json!({"page": p.file_name().unwrap().to_string_lossy(), "node": n, "parent": if q != NONE { q as i64 } else { -1 },
                    "prev": prev.get(&n).map(|&x| x as i64).unwrap_or(-1), "display": format!("{:?}", st(n).display), "pdisplay": if q != NONE { format!("{:?}", st(q).display) } else { "-".into() },
                    "tag": dom.tag(n), "class": dom.attr(n, "class").unwrap_or("").split_whitespace().next().unwrap_or(""), "ours": (0..4).map(|k| deg(a[k][0])).collect::<Vec<_>>(), "chrome": (0..4).map(|k| deg(a[k][1])).collect::<Vec<_>>(),
                    "mu": mu(n), "agree": agree.contains(&n), "rect_o": r[&n].0, "rect_c": r[&n].1,
                    "code": code.get(&n).map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default()}));
            }
        }
        for (n, ks) in &bad {
            let radius = charge.get(n).copied().unwrap_or(0.0);
            let q = up(*n);
            let disp = format!("{:?}", st(*n).display); let pdisp = if q != NONE { format!("{:?}", st(q).display) } else { "-".into() };
            let axes: String = ks.iter().map(|(k, _, _)| AX[*k]).collect::<Vec<_>>().join("");
            let sig = format!("{axes}|{disp}|{pdisp}");
            let e = fam.entry(sig.clone()).or_default(); e.0 += 1; e.1 += radius;
            let tag = format!("{}.{}", dom.tag(*n), dom.attr(*n, "class").unwrap_or("").split_whitespace().next().unwrap_or(""));
            let has_text = dom.children(*n).any(|k| dom.nodes[k as usize].kind == Kind::Text && !dom.text(k).trim().is_empty());
            let (o, ch) = r[n];
            rows.push(serde_json::json!({"page": page, "node": n, "tag": tag, "sig": sig, "text": has_text, "radius": (radius * 10.0).round() / 10.0, "mu": mu(*n), "agrees": agree.contains(n),
                "turn_deg": ks.iter().map(|(k, d, _)| (AX[*k], (d * 10.0).round() / 10.0)).collect::<Vec<_>>(), "ours": o, "chrome": ch}));
        }
    }
    rows.sort_by(|x, y| y["radius"].as_f64().partial_cmp(&x["radius"].as_f64()).unwrap());
    if let Some(f) = &dump { std::fs::write(f, dump_rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
    if let Some(f) = arg("--out") { std::fs::write(&f, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
    eprintln!("shape: {} pages · {n_fail} failing boxes · {} boxes inside the feather, {n_cause} past the outline (μ ≥ ½; {n_def_ok} agree on x/y/w/h) · {n_cov:.0} failing boxes charged ({:.1}%) · alias {alias}° outline {outline} feather {fea} · {:.1}s",
        pages.len(), n_def, 100.0 * n_cov as f64 / n_fail.max(1) as f64, t0.elapsed().as_secs_f64());
    let mut fv: Vec<_> = fam.into_iter().collect(); fv.sort_by(|x, y| y.1 .1.partial_cmp(&x.1 .1).unwrap());
    eprintln!("families by blast radius (sig = turned angles | display | parent display):");
    for (s, (nd, nb)) in fv.iter().take(top) { eprintln!("  {s:<32} deformed {nd:>4} · charged {nb:>7.1}"); }
    eprintln!("top deformed boxes:");
    for r in rows.iter().take(top) { eprintln!("  {:>5}  {:<44} {:<28} {} {}", r["radius"], format!("{}#{}", r["page"].as_str().unwrap_or("").chars().take(24).collect::<String>(), r["tag"].as_str().unwrap_or("").chars().take(18).collect::<String>()), r["sig"].as_str().unwrap_or(""), r["turn_deg"], if r["agrees"].as_bool() == Some(true) { "(box agrees)" } else { "" }); }
}

/// FONT-METRIC ANNEALING (author 10-06, readback confirmed: "change the formatting so the shape is perfect … if the angles are off, it
/// wastes processing power"). Per target box, our engine re-lays out its page while the box's formatting is nudged — line-height,
/// then letter-spacing, then padding (box metrics last) — with a shrinking step (coarse grid, then refined around the best: the
/// annealing schedule) until the box sits on Chrome's shape in its frame (corners within τ after the parent's displacement is
/// removed). Records which knob closed the shape, the residual, and the WORK: the layout term evaluations (term ledger) and wall
/// time of the page with every fitted formatting applied, against the page as authored.
/// `omni-render core corpus --abc anneal --targets T.jsonl [--tau 1] [--out F]`  (T rows: {"page", "node"})
pub fn anneal(a: &[String]) {
    use crate::style::{Len, FX};
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    let tau: f32 = arg("--tau").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let tf = arg("--targets").expect("--targets T.jsonl");
    let mut by_page: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for l in std::fs::read_to_string(&tf).expect("read targets").lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        by_page.entry(v["page"].as_str().unwrap_or("").to_string()).or_default().push(v["node"].as_u64().unwrap_or(0) as u32);
    }
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut pages_out: Vec<serde_json::Value> = Vec::new();
    let t0 = std::time::Instant::now();
    for (page, nodes) in &by_page {
        let p = std::path::Path::new(&dir).join(page);
        let Some(c) = super::super::cached(&p) else { continue };
        crate::style::exprs_restore(&c.exprs);
        let (b, l, fonts, ch) = (&c.b, &c.l, &c.fonts, &c.ch);
        let base_url = b.url.clone(); let dref = &l.dom;
        let sizes = |n: u32| -> Option<(u32, u32)> {
            if let Some(v) = c.sizes.borrow().get(&n) { return *v; }
            let v = if dref.tag(n) == "svg" { crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)) }
                else { crate::img_bytes(dref, n, &base_url, b).and_then(|bytes| crate::img::size(&bytes)) };
            c.sizes.borrow_mut().insert(n, v); v
        };
        let chrome = |n: u32| -> Option<[f32; 4]> { c.align.get(&n).and_then(|&j| ch[j].bounds) };
        // one layout: (rects in px, work = layout terms evaluated, seconds)
        let lay = |st: &crate::style::Styled| -> (HashMap<u32, [f32; 4]>, usize, f64) {
            let t = std::time::Instant::now();
            crate::layout::ledger_start();
            let mut ly = crate::layout::Layout::new(&l.dom, st, fonts, &sizes, 1280, 800);
            ly.run();
            let work = crate::layout::ledger_take().len();
            (ly.out.boxes.iter().map(|b| (b.0, [b.1[0] as f32 / 64.0, b.1[1] as f32 / 64.0, b.1[2] as f32 / 64.0, b.1[3] as f32 / 64.0])).collect(), work, t.elapsed().as_secs_f64())
        };
        let frame_of = |n: u32, ours: &HashMap<u32, [f32; 4]>| -> Option<u32> { let mut q = n; loop { q = dref.nodes[q as usize].parent; if q == crate::dom::NONE { return None; } if ours.contains_key(&q) && chrome(q).is_some() { return Some(q); } } };
        // shape residual of n = the five anchored triangles (tools/shape_tri.py v3): our corners mapped through each of our triangles
        // into Chrome's; per triangle the worst corner; the residual is the MEAN over the five (every view must agree for a
        // precise shape), fit = every triangle within τ
        let resid_tri = |n: u32, ours: &HashMap<u32, [f32; 4]>, rel: &(Option<u32>, Option<u32>, Option<u32>)| -> (f32, bool) {
            let (Some(o), Some(cb)) = (ours.get(&n), chrome(n)) else { return (f32::INFINITY, false) };
            let Some(q) = rel.0 else { return (f32::INFINITY, false) };
            let pcb = chrome(q).unwrap();
            let tris = |e: &dyn Fn(u32) -> Option<[f32; 4]>| -> Option<[[[f64; 2]; 3]; 5]> {
                let pr = e(q)?; let (x0, y0, w) = (pr[0] as f64, pr[1] as f64, (pr[2] as f64).max(1.0)); let x1 = x0 + w;
                let top = rel.1.and_then(|s| e(s)).map(|r| (r[1] + r[3]) as f64).unwrap_or(y0);
                let mut sb = rel.1.and_then(|s| e(s)).filter(|r| r[3] > 0.5).map(|r| [r[0] as f64, (r[1] + r[3]) as f64]).unwrap_or([x0, y0 + w]);
                if sb[1] <= y0 + 0.5 { sb = [x0, y0 + w]; }
                let mut nt = rel.2.and_then(|s| e(s)).map(|r| [(r[0] + r[2]) as f64, r[1] as f64]).unwrap_or([x1, y0 + w]);
                if nt[1] <= y0 + 0.5 { nt = [x1, y0 + w]; }
                let ybt = y0 + (pcb[3] as f64).max(1.0); let ybo = y0 + (pr[3] as f64).max(1.0); let cx = x0 + w / 2.0;
                Some([[[cx, top - w], [cx - 1.5 * w, top + w], [cx + 1.5 * w, top + w]], [[x0, y0], [x1, y0], sb], [[x1, y0], [x0, y0], nt],
                      [[x0, ybt], [x1, ybt], [x0, y0]], [[x1, ybo], [x0, ybo], [x1, y0]]])
            };
            let (Some(to), Some(tc)) = (tris(&|m| ours.get(&m).copied()), tris(&|m| chrome(m))) else { return (f32::INFINITY, false) };
            let co = |r: [f32; 4]| [[r[0] as f64, r[1] as f64], [(r[0] + r[2]) as f64, r[1] as f64], [r[0] as f64, (r[1] + r[3]) as f64], [(r[0] + r[2]) as f64, (r[1] + r[3]) as f64]];
            let (po, pc) = (co(*o), co(cb));
            let (mut sum, mut all) = (0.0f64, true); let mut ev = [0.0f64; 4];
            for k in 0..5 {
                let (a, b) = (to[k], tc[k]);
                let det = (a[0][0] - a[2][0]) * (a[1][1] - a[2][1]) - (a[1][0] - a[2][0]) * (a[0][1] - a[2][1]);
                if det.abs() < 1e-9 { return (f32::INFINITY, false); }
                let mut worst = 0.0f64; let mut pr = [[0.0f64; 2]; 4];
                for i in 0..4 {
                    let (px, py) = (po[i][0] - a[2][0], po[i][1] - a[2][1]);
                    let l1 = ((a[1][1] - a[2][1]) * px - (a[1][0] - a[2][0]) * py) / det;
                    let l2 = (-(a[0][1] - a[2][1]) * px + (a[0][0] - a[2][0]) * py) / det;
                    let l3 = 1.0 - l1 - l2;
                    let (x, y) = (l1 * b[0][0] + l2 * b[1][0] + l3 * b[2][0], l1 * b[0][1] + l2 * b[1][1] + l3 * b[2][1]);
                    worst = worst.max(((x - pc[i][0]).powi(2) + (y - pc[i][1]).powi(2)).sqrt()); pr[i] = [x, y];
                }
                sum += worst; if worst > tau as f64 { all = false; }
                ev[0] += pr[0][0] - pc[0][0]; ev[1] += pr[0][1] - pc[0][1];
                ev[2] += (pr[1][0] - pr[0][0]) - (pc[1][0] - pc[0][0]); ev[3] += (pr[2][1] - pr[0][1]) - (pc[2][1] - pc[0][1]);
            }
            LAST_E.with(|e| e.set([ev[0] / 5.0, ev[1] / 5.0, ev[2] / 5.0, ev[3] / 5.0]));
            ((sum / 5.0) as f32, all)
        };
        let (base_rects, base_work, _) = lay(&l.styled);
        let both = |m: u32| base_rects.contains_key(&m) && chrome(m).is_some();
        let up = |n: u32| -> Option<u32> { let mut q = n; loop { q = dref.nodes[q as usize].parent; if q == crate::dom::NONE { return None; } if both(q) { return Some(q); } } };
        let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut keys: Vec<u32> = base_rects.keys().copied().filter(|&m| both(m)).collect(); keys.sort();
        for &m in &keys { if let Some(q) = up(m) { kids.entry(q).or_default().push(m); } }
        let rel_of = |n: u32| -> (Option<u32>, Option<u32>, Option<u32>) {
            let q = up(n); let sib = q.and_then(|q| kids.get(&q)); let i = sib.and_then(|v| v.iter().position(|&x| x == n));
            (q, i.and_then(|i| if i > 0 { Some(sib.unwrap()[i - 1]) } else { None }), i.and_then(|i| sib.unwrap().get(i + 1).copied()))
        };
        let resid = |n: u32, ours: &HashMap<u32, [f32; 4]>| -> f32 { resid_tri(n, ours, &rel_of(n)).0 };
        let fits = |n: u32, ours: &HashMap<u32, [f32; 4]>| -> bool { resid_tri(n, ours, &rel_of(n)).1 };
        // signed fracture vector [x, y, w, h] of n (px, mean over the five triangle views) + mean residual + fit
        let evec = |n: u32, ours: &HashMap<u32, [f32; 4]>| -> ([f64; 4], f32, bool) { let (r, f) = resid_tri(n, ours, &rel_of(n)); (LAST_E.with(|e| e.get()), r, f) };
        let mut fitted = l.styled.clone();
        for &n in nodes {
            if l.styled.ids[n as usize] == 0 { continue; }
            let (e0, r0, fit0) = evec(n, &base_rects);
            if !r0.is_finite() { continue; }
            let orig = l.styled.table[l.styled.ids[n as usize] as usize].clone();
            let fs = orig.font_size.max(FX);
            let mut st = l.styled.clone();
            let slot = st.table.len(); if slot >= u16::MAX as usize { break; }
            st.table.push(orig.clone()); st.ids[n as usize] = slot as u16;
            let mut evals = 0usize;
            // 1 MAP THE VARIABLE TO THE FRACTURE: which size axis is cracked (the triangles' signed error), and only its knobs
            let cracked: Vec<usize> = [2usize, 3].into_iter().filter(|&k| e0[k].abs() > tau as f64).collect();
            let pos_only = cracked.is_empty() && !fit0;
            let knobs_for = |k: usize| -> &'static [&'static str] { if k == 2 { &["letter-spacing"] } else { &["line-height", "font-size", "padding-bottom"] } };
            // knob value in px and its setter
            let get = |s: &crate::style::Style, kn: &str| -> f64 { match kn {
                "letter-spacing" => s.letter_spacing as f64 / FX as f64,
                "font-size" => s.font_size as f64 / FX as f64,
                "line-height" => match s.line_height { Len::Px(v) => v as f64 / FX as f64, Len::Pct(p) => s.font_size as f64 * p as f64 / 100.0 / FX as f64 / 100.0, _ => s.font_size as f64 * 1.2 / FX as f64 },
                _ => match s.padding[2] { Len::Px(v) => v as f64 / FX as f64, _ => 0.0 } } };
            let set = |s: &mut crate::style::Style, kn: &str, v: f64| { let fx = (v * FX as f64).round() as i32; match kn {
                "letter-spacing" => s.letter_spacing = fx, "font-size" => s.font_size = fx.max(FX), "line-height" => s.line_height = Len::Px(fx.max(1)),
                _ => s.padding[2] = Len::Px(fx.max(0)) } };
            let mut cur = orig.clone();
            let mut cracks: Vec<serde_json::Value> = Vec::new();
            let mut sealed: Vec<serde_json::Value> = Vec::new();
            for &k in &cracked {
                let mut best_knob: Option<(&str, f64, f64)> = None; // knob, sensitivity px/px, base value
                for &kn in knobs_for(k) {
                    // 2 STRESS THE CRACK: push the knob both ways by h; the fracture must answer linearly (second difference small)
                    let v0 = get(&cur, kn); let h = if kn == "letter-spacing" || kn == "font-size" { 0.5 } else { 2.0 };
                    let mut at = |v: f64| -> [f64; 4] { let mut s2 = cur.clone(); set(&mut s2, kn, v); st.table[slot] = s2; let (rs, _, _) = lay(&st); evals += 1; evec(n, &rs).0 };
                    let (ea, ep, em) = (at(v0), at(v0 + h), at(v0 - h));
                    st.table[slot] = cur.clone();
                    let sens = (ep[k] - em[k]) / (2.0 * h);
                    let curv = ((ep[k] - ea[k]) - (ea[k] - em[k])).abs();
                    let cross = [0usize, 1, 2, 3].iter().filter(|&&j| j != k).map(|&j| ((ep[j] - em[j]) / (2.0 * h)).abs()).fold(0.0, f64::max);
                    let linear = sens.abs() > 0.05 && curv <= 0.25 * (ep[k] - em[k]).abs().max(0.5);
                    cracks.push(serde_json::json!({"axis": AX[k], "knob": kn, "e": ea[k], "e_plus": ep[k], "e_minus": em[k], "sens": sens, "curvature": curv, "cross": cross, "controls": linear}));
                    if linear && best_knob.map_or(true, |b| sens.abs() > b.1.abs()) { best_knob = Some((kn, sens, v0)); }
                }
                // 3 SEAL THE CRACK: Newton steps on the proven knob (v ← v − e / sens), sub-pixel, ≤ 6 steps
                if let Some((kn, mut sens, mut v)) = best_knob {
                    let mut e = { let mut s2 = cur.clone(); set(&mut s2, kn, v); st.table[slot] = s2; let (rs, _, _) = lay(&st); evals += 1; evec(n, &rs).0[k] };
                    for _ in 0..6 {
                        if e.abs() <= (tau as f64) / 2.0 { break; }
                        let v2 = v - e / sens;
                        let mut s2 = cur.clone(); set(&mut s2, kn, v2); st.table[slot] = s2; let (rs, _, _) = lay(&st); evals += 1;
                        let e2 = evec(n, &rs).0[k];
                        if (e2 - e).abs() > 1e-6 && (v2 - v).abs() > 1e-6 { sens = (e2 - e) / (v2 - v); }
                        if e2.abs() < e.abs() { v = v2; e = e2; } else { break; }
                    }
                    set(&mut cur, kn, v); st.table[slot] = cur.clone();
                    sealed.push(serde_json::json!({"axis": AX[k], "knob": kn, "value_px": v, "from_px": get(&orig, kn), "e_after": e}));
                }
            }
            let (rs, _, _) = lay(&st); evals += 1;
            let (e1, r1, fit1) = evec(n, &rs);
            // the sealed formatting goes into the page's fitted table for the work measure
            let fslot = fitted.table.len(); if fslot < u16::MAX as usize { fitted.table.push(cur.clone()); fitted.ids[n as usize] = fslot as u16; }
            let verdict = if fit0 { "fit already" } else if pos_only { "position fracture (not this box's formatting)" } else if fit1 { "SEALED: formatting" } else if !sealed.is_empty() { "crack found, not sealed" } else { "no formatting knob controls it (structural)" };
            rows.push(serde_json::json!({"page": page, "node": n, "tag": dref.tag(n), "font_px": fs as f64 / FX as f64, "e0": e0, "e1": e1, "resid0": r0, "resid": r1,
                "fit0": fit0, "fit": fit1, "cracked": cracked.iter().map(|&k| AX[k]).collect::<Vec<_>>(), "cracks": cracks, "sealed": sealed, "verdict": verdict, "evals": evals}));
            eprintln!("  {page:.24} n{n:<6} <{:<6}> E0 [{:+6.1} {:+6.1} {:+6.1} {:+6.1}] → [{:+6.1} {:+6.1} {:+6.1} {:+6.1}] {verdict} · {evals} layouts",
                dref.tag(n), e0[0], e0[1], e0[2], e0[3], e1[0], e1[1], e1[2], e1[3]);
        }
        // WORK: the page as authored vs with every fitted formatting applied (median of 5)
        let med = |st: &crate::style::Styled| { let mut t: Vec<f64> = Vec::new(); let mut w = 0; for _ in 0..5 { let (_, wk, s) = lay(st); t.push(s); w = wk; } t.sort_by(|a, b| a.partial_cmp(b).unwrap()); (w, t[2]) };
        let (w0, s0) = med(&l.styled); let (w1, s1) = med(&fitted);
        let (r1, _, _) = lay(&fitted);
        let agree = |rs: &HashMap<u32, [f32; 4]>| rs.iter().filter(|(n, o)| chrome(**n).map_or(false, |cb| cb[2] > 0.0 && cb[3] > 0.0 && (0..4).all(|k| (o[k] - cb[k]).abs() <= 2f32.max(cb[k].abs() * 0.05)))).count();
        let (a0, a1) = (agree(&base_rects), agree(&r1));
        eprintln!("page {page}: work {w0} → {w1} terms ({:+.1}%) · layout {:.1} → {:.1} ms · agreeing boxes {a0} → {a1} (formatting only) · base work check {base_work}",
            100.0 * (w1 as f64 - w0 as f64) / w0.max(1) as f64, s0 * 1000.0, s1 * 1000.0);
        pages_out.push(serde_json::json!({"page": page, "work0": w0, "work1": w1, "ms0": s0 * 1000.0, "ms1": s1 * 1000.0, "agree0": a0, "agree1": a1}));
    }
    let n = rows.len();
    let by: BTreeMap<String, usize> = rows.iter().fold(BTreeMap::new(), |mut m, r| { *m.entry(r["verdict"].as_str().unwrap_or("").to_string()).or_default() += 1; m });
    let knobs: BTreeMap<String, usize> = rows.iter().flat_map(|r| r["cracks"].as_array().cloned().unwrap_or_default()).filter(|c| c["controls"] == true).fold(BTreeMap::new(), |mut m, c| { *m.entry(format!("{} ← {}", c["axis"].as_str().unwrap_or(""), c["knob"].as_str().unwrap_or(""))).or_default() += 1; m });
    eprintln!("anneal (error-vector guided): {n} boxes · {by:?} · knobs proven to control a crack {knobs:?} · {:.1}s", t0.elapsed().as_secs_f64());
    if let Some(f) = arg("--out") { std::fs::write(&f, serde_json::to_string_pretty(&serde_json::json!({"boxes": rows, "pages": pages_out})).unwrap()).ok(); }
}

/// THE BROWSER AS SIMD (author 10-06 Translate 2: "run the browser as simd"). Layout as data: every box becomes a row in flat
/// arrays (struct of arrays, indexed by box), grouped into WAVEFRONTS by depth (the dependency graph D: widths flow top-down,
/// so a whole depth level is independent once the level above is done) and into homogeneous RULE batches inside each wave.
/// The block width rule (CSS 2.2 §10.3.3, block-level non-replaced in normal flow) runs as one branch-free integer kernel over
/// each batch — auto: cb − ml − mr · fixed: w + pad + border (content-box) · percent: cb × p (+ pad + border) — in exact
/// 1/64 px integers (the author's exact geometry), so LLVM vectorises it (AVX2: 8 lanes of i32). Every computed width is checked
/// bit-for-bit against the engine's own recursive layout, and the pass is timed against that layout.
/// `omni-render core corpus --abc simd [--all] [--reps N]`
pub fn simd(a: &[String]) {
    use crate::style::{Display, Len};
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    let reps: usize = arg("--reps").and_then(|v| v.parse().ok()).unwrap_or(20);
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let pages: Vec<_> = replay.iter().chain(hold.iter()).cloned().collect();
    // the SoA: one row per box that runs in the kernel (all pages together — the batch is the whole corpus wave)
    #[derive(Default)] struct Soa { info: Vec<serde_json::Value>, me: Vec<u32>, parent: Vec<u32>, depth: Vec<u16>, kind: Vec<i32>, ml: Vec<Len>, mr: Vec<Len>, w: Vec<Len>, pad: [Vec<Len>; 2], bor: Vec<i32>, bb: Vec<i32>, engine_w: Vec<i32> }
    let mut s = Soa::default();
    let mut cw: Vec<i32> = Vec::new();         // content width of EVERY box (the wave's gather source), engine-derived at the boundary
    let mut total_boxes = 0usize; let mut layout_secs = 0.0f64; let mut rule_rows = 0usize;
    for p in &pages {
        let Some(c) = super::super::cached(p) else { continue };
        crate::style::exprs_restore(&c.exprs);
        let (b, l, fonts) = (&c.b, &c.l, &c.fonts);
        let base_url = b.url.clone(); let dref = &l.dom;
        let sizes = |n: u32| -> Option<(u32, u32)> {
            if let Some(v) = c.sizes.borrow().get(&n) { return *v; }
            let v = if dref.tag(n) == "svg" { crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)) }
                else { crate::img_bytes(dref, n, &base_url, b).and_then(|bytes| crate::img::size(&bytes)) };
            c.sizes.borrow_mut().insert(n, v); v
        };
        // the engine's recursive layout, timed (median of reps) — the yardstick
        let mut ts = Vec::new(); let mut boxes: HashMap<u32, [i32; 4]> = HashMap::new();
        for _ in 0..reps.max(1) { let t = std::time::Instant::now(); let mut ly = crate::layout::Layout::new(&l.dom, &l.styled, fonts, &sizes, 1280, 800); ly.run(); ts.push(t.elapsed().as_secs_f64()); if boxes.is_empty() { boxes = ly.out.boxes.iter().map(|b| (b.0, b.1)).collect(); } }
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap()); layout_secs += ts[ts.len() / 2];
        total_boxes += boxes.len();
        let st = |n: u32| &l.styled.table[l.styled.ids[n as usize] as usize];
        let res = |v: Len, cb: i32| -> i32 { match v { Len::Px(x) => x, Len::Pct(q) => ((cb as i64 * q as i64) / 10000) as i32, _ => 0 } };
        // boundary content widths, top-down in document order (a parent precedes its children)
        let mut order: Vec<u32> = boxes.keys().copied().collect(); order.sort();
        let base = cw.len() as u32; let mut row_of: HashMap<u32, u32> = HashMap::new();
        let up = |n: u32| -> Option<u32> { let mut q = n; loop { q = dref.nodes[q as usize].parent; if q == crate::dom::NONE { return None; } if boxes.contains_key(&q) { return Some(q); } } };
        let mut depth: HashMap<u32, u16> = HashMap::new();
        for &n in &order {
            let q = up(n); let cbw = q.and_then(|q| row_of.get(&q)).map(|&r| cw[r as usize]).unwrap_or(1280 * 64);
            let y = st(n); let w = boxes[&n][2];
            let pl = res(y.padding[3], cbw) + res(y.padding[1], cbw);
            row_of.insert(n, base + cw.len() as u32 - base); cw.push(w - pl - y.border_w[1] - y.border_w[3]);
            let d = q.and_then(|q| depth.get(&q)).map(|d| d + 1).unwrap_or(0); depth.insert(n, d);
            // the rule batch: block-level, non-replaced, in normal flow, inside a block container, no min/max clamps
            let Some(q) = q else { continue };
            let py = st(q);
            let ok = matches!(y.display, Display::Block | Display::ListItem) && matches!(py.display, Display::Block | Display::ListItem)
                && y.float == 0 && y.position < 2 && y.aspect == 0 && !matches!(dref.tag(n), "img" | "svg" | "video" | "canvas" | "iframe" | "input" | "textarea" | "select" | "button" | "hr" | "table")
                && matches!(y.min_w, Len::Auto | Len::Px(0)) && matches!(y.max_w, Len::None | Len::Auto)
                && matches!(y.width, Len::Auto | Len::Px(_) | Len::Pct(_)) && !matches!(y.margin[1], Len::Pct(_)) && !matches!(y.margin[3], Len::Pct(_));
            if !ok { continue; }
            s.parent.push(row_of[&q]); s.depth.push(d);
            s.kind.push(match y.width { Len::Auto => 0, Len::Px(_) => 1, _ => 2 });
            s.ml.push(y.margin[3]); s.mr.push(y.margin[1]); s.w.push(y.width); s.pad[0].push(y.padding[3]); s.pad[1].push(y.padding[1]);
            s.bor.push(y.border_w[1] + y.border_w[3]); s.bb.push(y.border_box as i32); s.engine_w.push(w);
            rule_rows += 1;
            s.me.push(row_of[&n]);
            let pq = up(q); let pqd = pq.map(|g| format!("{:?}", st(g).display)).unwrap_or_default();
            s.info.push(serde_json::json!({"page": p.file_name().unwrap().to_string_lossy(), "node": n, "tag": dref.tag(n), "ptag": dref.tag(q), "pdisplay": format!("{:?}", py.display),
                "gpdisplay": pqd, "p_overflow": py.overflow, "p_float": py.float, "p_position": py.position, "p_width": format!("{:?}", py.width), "border_box": y.border_box,
                "ml": format!("{:?}", y.margin[3]), "mr": format!("{:?}", y.margin[1]), "width": format!("{:?}", y.width), "overflow": y.overflow, "cb_fx": cw[row_of[&q] as usize], "engine_w_fx": w}));
        }
    }
    // pack the wave: rows sorted by depth → contiguous batches; inputs resolved into flat i32 lanes once (the "compile" step)
    let n = s.parent.len();
    let mut idx: Vec<usize> = (0..n).collect(); idx.sort_by_key(|&i| s.depth[i]);
    let lane = |f: &dyn Fn(usize) -> i32| -> Vec<i32> { idx.iter().map(|&i| f(i)).collect() };
    let px = |v: Len| match v { Len::Px(x) => x, _ => 0 };
    let pct = |v: Len| match v { Len::Pct(q) => q, _ => 0 };
    let (k, ml, mr, fw, pw) = (lane(&|i| s.kind[i]), lane(&|i| px(s.ml[i])), lane(&|i| px(s.mr[i])), lane(&|i| px(s.w[i])), lane(&|i| pct(s.w[i])));
    let (ppx, ppct) = (lane(&|i| px(s.pad[0][i]) + px(s.pad[1][i])), lane(&|i| pct(s.pad[0][i]) + pct(s.pad[1][i])));
    let (bor, bb, par, me) = (lane(&|i| s.bor[i]), lane(&|i| s.bb[i]), lane(&|i| s.parent[i] as i32), lane(&|i| s.me[i] as i32));
    let waves: Vec<(usize, usize)> = { let mut v = Vec::new(); let mut st0 = 0; for j in 1..=n { if j == n || s.depth[idx[j]] != s.depth[idx[st0]] { v.push((st0, j)); st0 = j; } } v };
    let mut out = vec![0i32; n]; let mut cbv = vec![0i32; n];
    let mut kern_secs = Vec::new();
    for _ in 0..reps.max(1) {
        let mut cwv = cw.clone();
        let t = std::time::Instant::now();
        for &(a0, a1) in &waves {
            // gather: each row's containing-block width from the wave above (or the engine boundary)
            for j in a0..a1 { cbv[j] = cwv[par[j] as usize]; }
            // THE KERNEL: branch-free, exact integer, 8 lanes per AVX2 instruction after auto-vectorisation
            let (cb, o) = (&cbv[a0..a1], &mut out[a0..a1]);
            let (k, ml, mr, fw, pw, ppx, ppct, bor, bb) = (&k[a0..a1], &ml[a0..a1], &mr[a0..a1], &fw[a0..a1], &pw[a0..a1], &ppx[a0..a1], &ppct[a0..a1], &bor[a0..a1], &bb[a0..a1]);
            for j in 0..o.len() {
                let padbor = ppx[j] + ((cb[j] as i64 * ppct[j] as i64) / 10000) as i32 + bor[j];
                let add = padbor * (1 - bb[j]);
                let auto = cb[j] - ml[j] - mr[j];
                let fixed = fw[j] + add;
                let perc = ((cb[j] as i64 * pw[j] as i64) / 10000) as i32 + add;
                let w = (k[j] == 0) as i32 * auto + (k[j] == 1) as i32 * fixed + (k[j] == 2) as i32 * perc;
                o[j] = w.max(padbor);
            }
            // scatter: the computed widths become the next wave's containing blocks
            for j in 0..a1 - a0 { let pb = ppx[j] + ((cb[j] as i64 * ppct[j] as i64) / 10000) as i32 + bor[j]; cwv[me[a0 + j] as usize] = out[a0 + j] - pb; }
        }
        kern_secs.push(t.elapsed().as_secs_f64());
        std::hint::black_box(&out);
    }
    kern_secs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let exact = (0..n).filter(|&j| out[j] == s.engine_w[idx[j]]).count();
    let by_kind = |kk: i32| { let t = (0..n).filter(|&j| k[j] == kk).count(); let e = (0..n).filter(|&j| k[j] == kk && out[j] == s.engine_w[idx[j]]).count(); (e, t) };
    let ks = kern_secs[kern_secs.len() / 2];
    eprintln!("browser as SIMD: {} pages · {total_boxes} boxes · block-width rule batch {n} rows ({:.0}%) in {} depth waves · widths bit-exact vs the engine {exact}/{n} ({:.1}%) — auto {:?} fixed {:?} percent {:?}",
        pages.len(), 100.0 * rule_rows as f64 / total_boxes.max(1) as f64, waves.len(), 100.0 * exact as f64 / n.max(1) as f64, by_kind(0), by_kind(1), by_kind(2));
    eprintln!("  time: SoA wave kernel {:.3} ms for {n} widths ({:.2} ns / box) · engine recursive layout {:.1} ms for all {total_boxes} boxes ({:.0} ns / box)",
        ks * 1000.0, ks * 1e9 / n.max(1) as f64, layout_secs * 1000.0, layout_secs * 1e9 / total_boxes.max(1) as f64);
    // EXACT GEOMETRY (author 10-06 "format our browser to be exact"; Translate 3: trits, thirds without rounding). The unit is
    // 1/(2^6·3^4·5^4) px = 1/3,240,000 px: halves, thirds, quarters, fifths, ninths and every CSS percent of a 1/64 px value are
    // whole numbers in it (mixed-radix unit, math index #7), so a percent of a percent never rounds and the wave carries no drift.
    // i64 lanes (4 per AVX2 op). Each box gets a TRIT: sign(today's 1/64 px result − the exact result) ∈ {−1, 0, +1}.
    const U: i64 = 3_240_000; const FXU: i64 = U / 64; // 1/64 px = 50,625 units
    let mut ex = vec![0i64; n]; let mut cbe = vec![0i64; n];
    let (mut rem_fx, mut rem_ex) = (0usize, 0usize);
    let mut ex_secs = Vec::new();
    for rep in 0..reps.max(1) {
        let mut cwe: Vec<i64> = cw.iter().map(|&v| v as i64 * FXU).collect();
        let t = std::time::Instant::now();
        for &(a0, a1) in &waves {
            for j in a0..a1 { cbe[j] = cwe[par[j] as usize]; }
            let (cb, o) = (&cbe[a0..a1], &mut ex[a0..a1]);
            let (k, ml, mr, fw, pw, ppx, ppct, bor, bb) = (&k[a0..a1], &ml[a0..a1], &mr[a0..a1], &fw[a0..a1], &pw[a0..a1], &ppx[a0..a1], &ppct[a0..a1], &bor[a0..a1], &bb[a0..a1]);
            for j in 0..o.len() {
                // percent p is stored ×100 (p·100 / 10000 = p / 100): cb·p/10000 is exact in U whenever cb is (U carries 2^4·5^4)
                let padbor = ppx[j] as i64 * FXU + cb[j] * ppct[j] as i64 / 10000 + bor[j] as i64 * FXU;
                let add = padbor * (1 - bb[j] as i64);
                let w = (k[j] == 0) as i64 * (cb[j] - (ml[j] + mr[j]) as i64 * FXU) + (k[j] == 1) as i64 * (fw[j] as i64 * FXU + add)
                      + (k[j] == 2) as i64 * (cb[j] * pw[j] as i64 / 10000 + add);
                o[j] = w.max(padbor);
            }
            for j in 0..a1 - a0 { let pb = ppx[j] as i64 * FXU + cb[j] * ppct[j] as i64 / 10000 + bor[j] as i64 * FXU; cwe[me[a0 + j] as usize] = ex[a0 + j] - pb; }
        }
        ex_secs.push(t.elapsed().as_secs_f64());
        if rep == 0 {
            // remainders: divisions that do not land on the grid (1/64 px today; the exact unit)
            for j in 0..n { if pw[j] != 0 || ppct[j] != 0 {
                let cfx = cbv[j] as i64; if (cfx * pw[j] as i64) % 10000 != 0 || (cfx * ppct[j] as i64) % 10000 != 0 { rem_fx += 1; }
                let cex = cbe[j]; if (cex * pw[j] as i64) % 10000 != 0 || (cex * ppct[j] as i64) % 10000 != 0 { rem_ex += 1; } } }
        }
    }
    ex_secs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut trit = [0usize; 3]; let mut drift = 0f64; let mut maxd = 0f64;
    for j in 0..n { let d = out[j] as i64 * FXU - ex[j]; trit[(d.signum() + 1) as usize] += 1; let px = d.abs() as f64 / U as f64; drift += px; if px > maxd { maxd = px; } }
    let exact_vs_engine = (0..n).filter(|&j| ex[j] % FXU == 0 && (ex[j] / FXU) as i32 == s.engine_w[idx[j]]).count();
    eprintln!("  EXACT geometry (unit 1/3,240,000 px, i64 lanes): {:.3} ms · divisions off the grid: 1/64 px {rem_fx} → exact unit {rem_ex} · trits (1/64 px − exact) −1 {} · 0 {} · +1 {} · total drift {drift:.3} px, max {maxd:.4} px · exact widths that are whole 1/64 px AND equal the engine: {exact_vs_engine}",
        ex_secs[ex_secs.len() / 2] * 1000.0, trit[0], trit[1], trit[2]);
    if let Some(f) = arg("--misses") {
        let in_batch: std::collections::HashSet<i32> = me.iter().copied().collect();
        let txt: String = (0..n).filter(|&j| out[j] != s.engine_w[idx[j]]).map(|j| { let mut v = s.info[idx[j]].clone();
            v["simd_fx"] = serde_json::json!(out[j]); v["diff_px"] = serde_json::json!((s.engine_w[idx[j]] - out[j]) as f64 / 64.0); v["kind"] = serde_json::json!(k[j]);
            v["parent_in_batch"] = serde_json::json!(in_batch.contains(&par[j])); v["cb_wave_fx"] = serde_json::json!(cbv[j]); v.to_string() + "\n" }).collect();
        std::fs::write(&f, txt).ok();
    }
    let miss: Vec<_> = (0..n).filter(|&j| out[j] != s.engine_w[idx[j]]).take(8).map(|j| (k[j], out[j] as f32 / 64.0, s.engine_w[idx[j]] as f32 / 64.0)).collect();
    eprintln!("  first mismatches (kind, simd px, engine px): {miss:?}");
    let off1 = (0..n).filter(|&j| (out[j] - s.engine_w[idx[j]]).abs() == 1).count();
    let big: Vec<_> = (0..n).filter(|&j| (out[j] - s.engine_w[idx[j]]).abs() > 1).take(10).map(|j| (k[j], out[j] as f32 / 64.0, s.engine_w[idx[j]] as f32 / 64.0, cbv[j] as f32 / 64.0)).collect();
    eprintln!("  mismatches off by exactly 1/64 px: {off1} · larger: {} e.g. (kind, simd, engine, cb) {big:?}", n - exact - off1);
}

/// EULER / LINE-COUNT SPLIT (author 10-06 "triangle torus"; Euler characteristic as a counting invariant beside the angles): every
/// failing box gets its number of text lines in ours (distinct line positions of our text runs inside its box) and in Chrome
/// (distinct line positions of Chrome's DOMSnapshot textBoxes inside its Chrome box). A box whose line count differs is
/// TOPOLOGICAL (it wrapped differently); equal counts = METRIC (sizes / angles). `--out F` rows: page, node, lines_o, lines_c,
/// w_ok, h_ok, x/y ok. `omni-render core corpus --abc lines [--all] [--out F]`
pub fn lines(a: &[String]) {
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let pages: Vec<_> = if a.iter().any(|x| x == "--all") { replay.iter().chain(hold.iter()).cloned().collect() } else { replay.clone() };
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let tol = |c: f32| 2f32.max(c.abs() * 0.05);
    for p in &pages {
        let Some(c) = super::super::cached(p) else { continue };
        crate::style::exprs_restore(&c.exprs);
        let (b, l, fonts) = (&c.b, &c.l, &c.fonts);
        let base_url = b.url.clone(); let dref = &l.dom;
        let sizes = |n: u32| -> Option<(u32, u32)> {
            if let Some(v) = c.sizes.borrow().get(&n) { return *v; }
            let v = if dref.tag(n) == "svg" { crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)) }
                else { crate::img_bytes(dref, n, &base_url, b).and_then(|bytes| crate::img::size(&bytes)) };
            c.sizes.borrow_mut().insert(n, v); v
        };
        let mut ly = crate::layout::Layout::new(&l.dom, &l.styled, fonts, &sizes, 1280, 800);
        ly.run();
        // our text runs: (x, y) in px
        let ours_text: Vec<(f32, f32)> = ly.out.items.iter().filter_map(|it| if let crate::layout::Item::Text { x, y, .. } = it { Some((*x as f32 / 64.0, *y as f32 / 64.0)) } else { None }).collect();
        // Chrome's line fragments: textBoxes bounds (centre)
        let snap: serde_json::Value = match std::fs::read(p.join("chrome-snap.json")).ok().and_then(|bts| serde_json::from_slice(&bts).ok()) { Some(v) => v, None => continue };
        let tb: Vec<(f32, f32)> = snap["snap"]["documents"][0]["textBoxes"]["bounds"].as_array().map(|v| v.iter().filter_map(|b| { let b = b.as_array()?; Some((b[0].as_f64()? as f32 + b[2].as_f64()? as f32 / 2.0, b[1].as_f64()? as f32 + b[3].as_f64()? as f32 / 2.0)) }).collect()).unwrap_or_default();
        let count = |pts: &[(f32, f32)], r: [f32; 4]| -> usize {
            let mut ys: Vec<f32> = pts.iter().filter(|(x, y)| *x >= r[0] - 1.0 && *x <= r[0] + r[2] + 1.0 && *y >= r[1] - 1.0 && *y <= r[1] + r[3] + 1.0).map(|q| q.1).collect();
            ys.sort_by(|a, b| a.partial_cmp(b).unwrap()); let mut n = 0; let mut last = f32::MIN;
            for y in ys { if y - last > 3.0 { n += 1; last = y; } } n
        };
        let Some((_, agree, r)) = super::super::rects(p) else { continue };
        let with_agree = a.iter().any(|x| x == "--agree-too");
        for (&n, &(o, ch)) in &r {
            if agree.contains(&n) && !with_agree { continue; }
            let (lo, lc) = (count(&ours_text, o), count(&tb, ch));
            // instrument check: nearest of our text runs to the box (dy of the closest run whose x lies in the box)
            let near: Option<f32> = if lo == 0 && lc > 0 { ours_text.iter().filter(|(x, _)| *x >= o[0] - 1.0 && *x <= o[0] + o[2] + 1.0).map(|(_, y)| if *y < o[1] { *y - o[1] } else { *y - (o[1] + o[3]) }).min_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap()) } else { None };
            rows.push(serde_json::json!({"page": p.file_name().unwrap().to_string_lossy(), "node": n, "tag": dref.tag(n), "lines_o": lo, "lines_c": lc,
                "agree": agree.contains(&n), "near_dy": near, "w_ok": (o[2] - ch[2]).abs() <= tol(ch[2]), "h_ok": (o[3] - ch[3]).abs() <= tol(ch[3]), "x_ok": (o[0] - ch[0]).abs() <= 2.0, "y_ok": (o[1] - ch[1]).abs() <= 2.0}));
        }
    }
    let n = rows.len();
    let topo = rows.iter().filter(|r| r["lines_o"] != r["lines_c"]).count();
    let topo_wok = rows.iter().filter(|r| r["lines_o"] != r["lines_c"] && r["w_ok"] == true).count();
    eprintln!("lines: {} pages · {n} failing boxes · TOPOLOGICAL (line count differs) {topo} ({:.0}%) — of them width right {topo_wok} (wrap from text measure) · METRIC {}", pages.len(), 100.0 * topo as f64 / n.max(1) as f64, n - topo);
    if let Some(f) = arg("--out") { std::fs::write(&f, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
}

/// COUNTERFACTUAL CASCADE (doctor B2; author 10-06 "if we change the roots we'll see a cascade — whether we have to adapt or it adapts to
/// us"): the target boxes (roots) are laid out at Chrome's own size (layout::INJECT), the page re-laid out, and every OTHER box
/// compared before / after: gained = the cascade adapts to us (boxes that were only carried), lost = we must adapt (boxes whose
/// agreement depended on the root being wrong — a compensating error surfaces). `omni-render core corpus --abc inject --targets T.jsonl`
pub fn inject(a: &[String]) {
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let mut by_page: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for l in std::fs::read_to_string(arg("--targets").expect("--targets")).expect("read").lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        by_page.entry(v["page"].as_str().unwrap_or("").into()).or_default().push(v["node"].as_u64().unwrap_or(0) as u32);
    }
    let (mut g_all, mut l_all, mut fixed_roots, mut n_roots) = (0usize, 0usize, 0usize, 0usize);
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for (page, nodes) in &by_page {
        let p = std::path::Path::new(&dir).join(page);
        crate::layout::inject_set(HashMap::new());
        let Some((_, a0, r0)) = super::super::rects(&p) else { continue };
        let inj: HashMap<u32, (i32, i32)> = nodes.iter().filter_map(|n| r0.get(n).map(|(_, c)| (*n, ((c[2] * 64.0).round() as i32, (c[3] * 64.0).round() as i32)))).collect();
        crate::layout::inject_set(inj.clone());
        let r1 = super::super::rects(&p);
        crate::layout::inject_set(HashMap::new());
        let Some((_, a1, _)) = r1 else { continue };
        let roots: std::collections::HashSet<u32> = inj.keys().copied().collect();
        let gained = a1.iter().filter(|n| !a0.contains(n) && !roots.contains(n)).count();
        let lost = a0.iter().filter(|n| !a1.contains(n) && !roots.contains(n)).count();
        let rf = roots.iter().filter(|n| a1.contains(n)).count();
        g_all += gained; l_all += lost; fixed_roots += rf; n_roots += roots.len();
        rows.push(serde_json::json!({"page": page, "roots": roots.len(), "roots_agree_after": rf, "gained": gained, "lost": lost}));
        eprintln!("  {page:.34} roots {:3} (agree after {rf:3}) · cascade gained {gained:4} · lost {lost:4}", roots.len());
    }
    eprintln!("inject: {n_roots} roots set to Chrome's size · roots then agree {fixed_roots} · OTHER boxes: gained {g_all} (adapt to us) · lost {l_all} (we adapt)");
    if let Some(f) = arg("--out") { std::fs::write(&f, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).ok(); }
}

/// T25-CORE-JIT step 1 (author 10-06 "jit compile the 25t … live firing core"; an unpublished plan): the T25 core
/// inside the engine over every page's boxes, in the same pass that lays them out — residues as one 32-byte row per box, the
/// AVX2 twist kernel (checked against the scalar one), CRT decode to exact errors, roots, triangle keys, families.
/// `--out F.jsonl` writes every box's twist and every root's key for tools/t25_check.py (bit-exact vs the Python golden).
pub fn t25(a: &[String]) {
    use crate::jit::t25 as T;
    use crate::dom::NONE;
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    let t0 = std::time::Instant::now();
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let pages: Vec<_> = if a.iter().any(|x| x == "--all") { replay.iter().chain(hold.iter()).cloned().collect() } else { replay.clone() };
    let pr = T::Program::t25();
    let avx2 = is_x86_feature_detected!("avx2");
    // step 2: the rendered core (once per program; its code cached by the program hash)
    let rk = if avx2 { Some(T::render(&pr)) } else { None };
    let (mut t_jit, mut jit_bad_res, mut jit_bad_tw) = (0f64, 0usize, 0usize);
    let (mut n_box, mut n_root, mut bad_kernel, mut t_res, mut t_kernel, mut t_dec) = (0usize, 0usize, 0usize, 0f64, 0f64, 0f64);
    let mut fam: HashMap<(String, String, [i8; 4]), usize> = HashMap::new();
    let mut out: Vec<String> = Vec::new();
    for p in &pages {
        let Some((c, agree, r)) = super::super::rects(p) else { continue };
        let dom = &c.l.dom;
        let st = |n: u32| &c.l.styled.table[c.l.styled.ids[n as usize] as usize];
        let up = |mut n: u32| -> u32 { loop { n = dom.nodes[n as usize].parent; if n == NONE || r.contains_key(&n) { return n; } } };
        let mut order: Vec<u32> = r.keys().copied().collect(); order.sort();
        let idx: HashMap<u32, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let par: Vec<u32> = order.iter().map(|&n| up(n)).collect();
        // RES: every box → its T25 point (ours, Chrome), one 32-byte row each
        let t = std::time::Instant::now();
        let fr = |q: u32, which: usize| if q != NONE { Some(if which == 0 { r[&q].0 } else { r[&q].1 }) } else { None };
        let ro: Vec<[u8; 32]> = order.iter().zip(&par).map(|(&n, &q)| pr.residues(&T::coords(r[&n].0, fr(q, 0)))).collect();
        let rc: Vec<[u8; 32]> = order.iter().zip(&par).map(|(&n, &q)| pr.residues(&T::coords(r[&n].1, fr(q, 1)))).collect();
        t_res += t.elapsed().as_secs_f64();
        // TWIST: all 25 rings of every box (AVX2), checked against the scalar reference
        let mut d = vec![[0u8; 32]; order.len()];
        let t = std::time::Instant::now();
        if avx2 { unsafe { T::twist_avx2(&ro, &rc, &pr.mods, &mut d) } } else { T::twist_scalar(&ro, &rc, &pr.mods, &mut d) }
        t_kernel += t.elapsed().as_secs_f64();
        let mut ds = vec![[0u8; 32]; order.len()];
        T::twist_scalar(&ro, &rc, &pr.mods, &mut ds);
        bad_kernel += d.iter().zip(&ds).filter(|(x, y)| x != y).count();
        // RENDERED: the same RES (ours) + TWIST (vs Chrome's plate) as one generated kernel — must equal the rows above
        if let Some(rk) = &rk {
            let co: Vec<i64> = order.iter().zip(&par).flat_map(|(&n, &q)| T::coords(r[&n].0, fr(q, 0))).collect();
            let mut res = vec![[0u8; 32]; order.len()]; let mut tw = vec![[0u8; 32]; order.len()];
            let t = std::time::Instant::now();
            (rk.kernel())(co.as_ptr(), order.len(), res.as_mut_ptr() as *mut u8, rc.as_ptr() as *const u8, tw.as_mut_ptr() as *mut u8, pr.mods.as_ptr());
            t_jit += t.elapsed().as_secs_f64();
            jit_bad_res += res.iter().zip(&ro).filter(|(x, y)| x != y).count();
            jit_bad_tw += tw.iter().zip(&d).filter(|(x, y)| x != y).count();
        }
        // DECODE: CRT per group → exact errors (1/64 px)
        let t = std::time::Instant::now();
        let e: Vec<[i64; 5]> = d.iter().map(|x| pr.decode(x)).collect();
        t_dec += t.elapsed().as_secs_f64();
        let page = p.file_name().unwrap().to_string_lossy().to_string();
        for (i, &n) in order.iter().enumerate() {
            n_box += 1;
            let q = par[i];
            let root = !agree.contains(&n) && q != NONE && agree.contains(&q);
            let key = if root {
                n_root += 1;
                let k = (format!("{:?}", st(q).display), format!("{:?}", st(n).display), T::tri_trits(&e[i], &e[idx[&q]], r[&n].1));
                *fam.entry(k.clone()).or_default() += 1;
                Some(k)
            } else { None };
            if arg("--out").is_some() {
                out.push(serde_json::json!({"page": page, "node": n, "twist": e[i], "root": root,
                    "key": key.map(|k| serde_json::json!([k.0, k.1, k.2[0], k.2[1], k.2[2], k.2[3]]))}).to_string());
            }
        }
    }
    if let Some(f) = arg("--out") { std::fs::write(&f, out.join("\n") + "\n").ok(); }
    let mut fv: Vec<_> = fam.into_iter().collect(); fv.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    eprintln!("t25 core · program {:016x} ({} rings) · {} pages · {n_box} boxes · kernel {} (avx2 vs scalar mismatches {bad_kernel}) · {n_root} roots → {} triangle families",
        crate::jit::ir::hash(&pr.bytes()), pr.groups.iter().map(|g| g.len()).sum::<usize>(), pages.len(), if avx2 { "avx2" } else { "scalar" }, fv.len());
    eprintln!("  µs: residues {:.0} · twist kernel {:.0} ({:.1} ns/box) · decode {:.0} · whole station incl. layout {:.2} s",
        t_res * 1e6, t_kernel * 1e6, t_kernel * 1e9 / n_box.max(1) as f64, t_dec * 1e6, t0.elapsed().as_secs_f64());
    if let Some(rk) = &rk {
        eprintln!("  rendered core: {} bytes of x86 in {:.0} µs (program {:016x}) · RES+TWIST {:.0} µs = {:.1} ns/box · vs step 1 rows: residues {} / twists {} mismatches",
            rk.code.len(), rk.render_us, rk.hash, t_jit * 1e6, t_jit * 1e9 / n_box.max(1) as f64, jit_bad_res, jit_bad_tw);
        if let Some(f) = arg("--code") { std::fs::write(&f, &rk.code).ok(); }
    }
    for (k, v) in fv.iter().take(6) { eprintln!("  {v:4} ({}, {}, {}, {}, {}, {})", k.0, k.1, k.2[0], k.2[1], k.2[2], k.2[3]); }
}

/// T25 LIVE (author 10-06: "render it in our trit renderer/browser … viewable running jit", "the 25t ring system with the core in the
/// center"): every frame the RENDERED T25 core (generated x86) fires over all of a page's boxes against Chrome's plate, and the
/// engine itself lays out and rasters the frame from the core's live output — 25 rings around the core (Chrome's point bright,
/// ours white, the rings that twist marked), the program's actual bytes per ring block, the live timing. Frames → OUT/frameNNN.png.
pub fn t25live(a: &[String]) {
    use crate::jit::t25 as T;
    use crate::dom::NONE;
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    let frames: usize = arg("--frames").and_then(|v| v.parse().ok()).unwrap_or(36);
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "results/t25live".into()));
    let want = arg("--page").unwrap_or_else(|| "theguardian".into());
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let p = replay.iter().chain(hold.iter()).find(|p| p.to_string_lossy().contains(&want)).cloned().expect("--page");
    let Some((c, agree, r)) = super::super::rects(&p) else { return };
    let dom = &c.l.dom;
    let up = |mut n: u32| -> u32 { loop { n = dom.nodes[n as usize].parent; if n == NONE || r.contains_key(&n) { return n; } } };
    let mut order: Vec<u32> = r.keys().copied().collect(); order.sort();
    let par: Vec<u32> = order.iter().map(|&n| up(n)).collect();
    let pr = T::Program::t25();
    let rk = T::render(&pr);
    let fr = |q: u32, w: usize| if q != NONE { Some(if w == 0 { r[&q].0 } else { r[&q].1 }) } else { None };
    // the PLATE: Chrome's points, built once (the codebook the core fires against)
    let plate: Vec<[u8; 32]> = order.iter().zip(&par).map(|(&n, &q)| pr.residues(&T::coords(r[&n].1, fr(q, 1)))).collect();
    let co: Vec<i64> = order.iter().zip(&par).flat_map(|(&n, &q)| T::coords(r[&n].0, fr(q, 0))).collect();
    let roots: Vec<usize> = (0..order.len()).filter(|&i| !agree.contains(&order[i]) && par[i] != NONE && agree.contains(&par[i])).collect();
    let good: Vec<usize> = (0..order.len()).filter(|&i| agree.contains(&order[i]) && r[&order[i]].1[2] > 40.0).collect();
    let (p25, _) = { let mut z = [0u8; 32]; z[0] = 0; let (_, big) = T::point_u128(&pr, &z); (big, 0) };
    std::fs::create_dir_all(&out).ok();
    let page = p.file_name().unwrap().to_string_lossy().to_string();
    let col = ["#f5a623", "#2ec4d6", "#a78bfa", "#4ade80", "#f472b6"];
    let dim = ["#5a4220", "#1d5a63", "#463b72", "#1d6038", "#6a2a4a"];
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut total_us = 0f64;
    for f in 0..frames {
        // FIRE: the generated kernel over every box of the page, this frame
        let mut res = vec![[0u8; 32]; order.len()]; let mut tw = vec![[0u8; 32]; order.len()];
        let t = std::time::Instant::now();
        (rk.kernel())(co.as_ptr(), order.len(), res.as_mut_ptr() as *mut u8, plate.as_ptr() as *const u8, tw.as_mut_ptr() as *mut u8, pr.mods.as_ptr());
        let us = t.elapsed().as_secs_f64() * 1e6; total_us += us;
        let twisted_boxes = tw.iter().filter(|x| x.iter().any(|&v| v != 0)).count();
        let within = order.iter().filter(|n| agree.contains(n)).count();
        let i = if f % 4 == 3 && !good.is_empty() { good[(f / 4) % good.len()] } else if !roots.is_empty() { roots[(f - f / 4) % roots.len()] } else { f % order.len() };
        let n = order[i];
        let e = pr.decode(&tw[i]);
        let (po, _) = T::point_u128(&pr, &res[i]); let (pc, _) = T::point_u128(&pr, &plate[i]);
        let mut h = String::new();
        h.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><title>T25 live core</title></head><body style=\"margin:0;background:#0b0b10;color:#d8d8e0;font-family:DejaVu Sans Mono,monospace;font-size:11px;line-height:1.35\"><div style=\"position:relative;width:1280px;height:800px\">");
        // the ring system: 25 rings, groups x0 · y0 · w · h · ry from the centre out; core in the middle
        let (cx, cy, r0, dr) = (392.0f64, 410.0f64, 122.0f64, 10.6f64);
        let mut k = 0;
        for (g, ms) in pr.groups.iter().enumerate() {
            for &m in ms {
                let rad = r0 + dr * k as f64;
                let ring_tw = tw[i][k] != 0;
                for j in 0..m as usize {
                    let a = std::f64::consts::TAU * j as f64 / m as f64 - std::f64::consts::FRAC_PI_2;
                    let (x, y) = (cx + rad * a.cos(), cy + rad * a.sin());
                    let (sz, c) = if j == plate[i][k] as usize { (7.0, col[g]) } else if j == res[i][k] as usize { (7.0, "#ffffff") } else { (2.0, if ring_tw { col[g] } else { dim[g] }) };
                    h.push_str(&format!("<div style=\"position:absolute;left:{:.1}px;top:{:.1}px;width:{sz}px;height:{sz}px;background:{c}\"></div>", x - sz / 2.0, y - sz / 2.0));
                }
                k += 1;
            }
        }
        // the core: the firing kernel at the centre
        h.push_str(&format!("<div style=\"position:absolute;left:{}px;top:{}px;width:196px;text-align:center\">\
            <div style=\"color:#fff;font-weight:bold;font-size:13px\">RENDERED CORE</div><div>x86 · {} B · {} rings</div>\
            <div style=\"color:#fff;margin-top:4px\">pass {}</div><div>{} boxes in {:.0} µs</div><div>{:.1} ns / box</div>\
            <div style=\"margin-top:4px;color:#4ade80\">{} exact to 1/64 px</div><div style=\"color:#2ec4d6\">{} within 2 px</div><div style=\"color:#f472b6\">{} off</div></div>",
            cx as i32 - 98, cy as i32 - 60, rk.code.len(), k, f + 1, order.len(), us, us * 1e3 / order.len() as f64, order.len() - twisted_boxes, within, order.len() - within));
        // the panel: what the core holds and what it just did
        let tag = dom.tag(n); let cls = dom.attr(n, "class").unwrap_or("");
        let (o, ch) = r[&n];
        h.push_str(&format!("<div style=\"position:absolute;left:24px;top:12px;font-size:18px;font-weight:bold;color:#fff\">T²⁵ · the rendered core firing in the ring system</div>\
            <div style=\"position:absolute;left:24px;top:36px;width:740px;color:#9a9ab0\">{} · every frame the generated x86 reduces every box to its T²⁵ point and twists all 25 rings against Chrome's plate at once (AVX2). Bright = Chrome · white = ours.</div>", esc(&page[..page.len().min(40)])));
        let mut panel = format!("<div style=\"position:absolute;left:800px;top:12px;width:466px\">\
            <div style=\"color:#fff;font-weight:bold\">program {:016x} · rendered in {:.0} µs</div>\
            <div style=\"color:#9a9ab0\">holds: P25 = {} (2^126.45, one u128)</div>\
            <div style=\"margin-top:6px;color:#fff\">box &lt;{}{}&gt; node {}</div>\
            <div>Chrome [{:.1}, {:.1}, {:.1}, {:.1}]</div><div>ours&nbsp;&nbsp; [{:.1}, {:.1}, {:.1}, {:.1}]</div>\
            <div style=\"color:#9a9ab0\">Chrome point {}</div><div style=\"color:#9a9ab0\">ours&nbsp;&nbsp; point {}</div><div style=\"margin-top:4px\">",
            rk.hash, rk.render_us, p25, esc(tag), if cls.is_empty() { String::new() } else { format!(".{}", esc(&cls[..cls.len().min(24)])) }, n,
            ch[0], ch[1], ch[2], ch[3], o[0], o[1], o[2], o[3], pc, po);
        for g in 0..5 { panel.push_str(&format!("<span style=\"color:{}\">{} {:+.2}px</span>&nbsp; ", col[g], T::NAMES[g], e[g] as f64 / 64.0)); }
        panel.push_str("</div><div style=\"margin-top:8px;color:#fff\">the program (its bytes, as rendered):</div>");
        let mut kk = 0;
        for (g, ms) in pr.groups.iter().enumerate() {
            for &m in ms {
                let b0 = rk.ring_at[kk]; let b1 = rk.ring_at.get(kk + 1).copied().unwrap_or(rk.tail_at);
                let bytes: String = rk.code[b0..b1].iter().take(14).map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ");
                let lit = tw[i][kk] != 0;
                panel.push_str(&format!("<div style=\"color:{};{}\">{:04x} m={:<3} {}…</div>", if lit { "#fff" } else { col[g] }, if lit { format!("background:{}", dim[g]) } else { String::new() }, b0, m, bytes));
                kk += 1;
            }
        }
        let tail: String = rk.code[rk.tail_at..(rk.tail_at + 14).min(rk.code.len())].iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ");
        panel.push_str(&format!("<div style=\"color:#fff\">{:04x} AVX2 vpsubb·vpcmpgtb·vpand·vpaddb {}…</div></div>", rk.tail_at, tail));
        h.push_str(&panel);
        h.push_str("</div></body></html>");
        // draw it with OUR engine: parse → style → layout → raster → PNG
        let fd = out.join(format!("frame{f:03}"));
        std::fs::create_dir_all(&fd).ok();
        std::fs::write(fd.join("page.html"), &h).ok();
        let url = format!("file://{}", std::fs::canonicalize(fd.join("page.html")).unwrap().display());
        std::fs::write(fd.join("manifest.json"), serde_json::json!({"url": url, "final_url": url, "res": {}}).to_string()).ok();
        let b = crate::page::Bundle::open(&fd).expect("bundle");
        let l = crate::page::load(&b, (1280.0, 800.0));
        let fonts = crate::font::Fonts::new();
        let sizes = |_n: u32| -> Option<(u32, u32)> { None };
        let mut lay = crate::layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
        lay.run();
        let items = std::mem::take(&mut lay.out.items);
        drop(lay);
        let imgf = |_n: u32, _m: u8, _w: i32, _h: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> { None };
        let mut store = crate::raster::Store::default();
        let vp = crate::raster::render(&items, &fonts, &imgf, 1280, 0, 800, &mut store);
        let img = crate::raster::compose(&vp, &store, 1280);
        let file = std::fs::File::create(out.join(format!("frame{f:03}.png"))).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 1280, 800);
        enc.set_color(png::ColorType::Rgb); enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        let data: Vec<u8> = img[..1280 * 800].iter().flat_map(|&c| [(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8]).collect();
        wr.write_image_data(&data).unwrap();
    }
    eprintln!("t25 live · {page} · {} boxes · {frames} frames · core fired {frames}× in {:.0} µs total ({:.0} µs/pass) · rendered core {} B → {}",
        order.len(), total_us, total_us / frames as f64, rk.code.len(), out.display());
}

/// PRISTINE (author 10-06: "our goal in formatting pages for our ternary browser is not to match Chrome, it's to make the code on the pages
/// mathematically pristine"): every box judged by its OWN page's geometry — no Chrome. A box is pristine when each triangle it forms
/// with its frame and its previous sibling closes cleanly: either exactly on a relation or clearly away from it, never a near-miss.
///   overflow    — escapes its frame's border box by > ½ px
///   near_flush  — an edge within (0, 1] px of its frame's same edge (almost flush, not flush)
///   near_align  — left edge within (0, 1] px of its previous sibling's (almost aligned, not aligned)
///   near_touch  — vertical gap/overlap to its previous sibling within (0, 1) px (almost touching, not touching)
///   near_equal  — width within (1/64, 1] px of its previous sibling's (almost equal, not equal) — v1 (v0's "near an exact fraction of
///                 the frame" flagged coincidences: the splits it named close exactly, e.g. Wikipedia's 10 cells sum to their row to 1/64 px)
///   uneven_gap  — in a row of ≥ 3 siblings sharing a top, two consecutive gaps differ by (1/64, 1] px (almost even spacing, not even)
/// overflow is judged only where the frame does not clip (overflow visible) and the box is in flow (not absolute / fixed).
/// `--out F.jsonl` writes every defect (page, node, kind, Δ px, display pair).
pub fn pristine(a: &[String]) {
    use crate::dom::NONE;
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.first().filter(|x| !x.starts_with("--")).cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let (replay, hold, _) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let pages: Vec<_> = if a.iter().any(|x| x == "--all") { replay.iter().chain(hold.iter()).cloned().collect() } else { replay.clone() };
    const KINDS: [&str; 6] = ["overflow", "near_flush", "near_align", "near_touch", "near_equal", "uneven_gap"];
    let (mut n_box, mut n_clean) = (0usize, 0usize);
    let mut by_kind = [0usize; 6];
    let mut fam: HashMap<(String, String, &'static str), usize> = HashMap::new();
    let mut per_page: Vec<(String, usize, usize)> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    let t0 = std::time::Instant::now();
    for p in &pages {
        let Some((c, _agree, r)) = super::super::rects(p) else { continue };
        let dom = &c.l.dom;
        let st = |n: u32| &c.l.styled.table[c.l.styled.ids[n as usize] as usize];
        let up = |mut n: u32| -> u32 { loop { n = dom.nodes[n as usize].parent; if n == NONE || r.contains_key(&n) { return n; } } };
        let mut order: Vec<u32> = r.keys().copied().collect(); order.sort();
        let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
        for &n in &order { let q = up(n); if q != NONE { kids.entry(q).or_default().push(n); } }
        let prev: HashMap<u32, u32> = kids.values().flat_map(|v| v.windows(2).map(|w| (w[1], w[0]))).collect();
        let page = p.file_name().unwrap().to_string_lossy().to_string();
        let (mut pb, mut pc) = (0usize, 0usize);
        for &n in &order {
            let q = up(n); if q == NONE { continue; }
            let b = r[&n].0; let f = r[&q].0;
            if b[2] <= 0.0 || b[3] <= 0.0 { continue; }
            n_box += 1; pb += 1;
            let mut hit: Vec<(usize, f32)> = Vec::new();
            // a near-miss is SUB-PIXEL: (0, 1) px off a relation — a whole pixel is a border or a deliberate offset, exact by rule
            let near = |d: f32| d.abs() > 1e-3 && d.abs() < 1.0 - 1e-3;
            // overflow (left / right / top; a box taller than its frame is often intended, so the bottom is not judged)
            let ov = [(f[0] - b[0]), (b[0] + b[2]) - (f[0] + f[2]), (f[1] - b[1])].into_iter().fold(0f32, f32::max);
            if ov > 0.5 && st(q).overflow == 0 && !matches!(st(n).position, 2 | 3) { hit.push((0, ov)); }
            // flush is judged against the frame's CONTENT box (border + padding), where children are placed
            let fs = st(q); let fx = |v: f32| v / 64.0;
            let pl = fx(crate::layout::res0(fs.padding[3], (f[2] * 64.0) as i32) as f32) + fx(fs.border_w[3] as f32);
            let pr = fx(crate::layout::res0(fs.padding[1], (f[2] * 64.0) as i32) as f32) + fx(fs.border_w[1] as f32);
            let dl = b[0] - (f[0] + pl); let dr = (f[0] + f[2] - pr) - (b[0] + b[2]);
            if near(dl) { hit.push((1, dl)); } else if near(dr) { hit.push((1, dr)); }
            if let Some(&s) = prev.get(&n) {
                let sb = r[&s].0;
                let da = b[0] - sb[0]; if near(da) { hit.push((2, da)); }
                let gap = b[1] - (sb[1] + sb[3]); if gap.abs() > 1e-3 && gap.abs() < 1.0 { hit.push((3, gap)); }
            }
            if let Some(&s) = prev.get(&n) {
                let sb = r[&s].0; let dw = b[2] - sb[2];
                if dw.abs() > 1.0 / 64.0 + 1e-4 && dw.abs() <= 1.0 { hit.push((4, dw)); }
                // even spacing: this box, its previous sibling and the one before it, all on one top line
                if let Some(&s2) = prev.get(&s) {
                    let s2b = r[&s2].0;
                    if (b[1] - sb[1]).abs() < 1e-3 && (sb[1] - s2b[1]).abs() < 1e-3 && b[0] > sb[0] && sb[0] > s2b[0] {
                        let g1 = sb[0] - (s2b[0] + s2b[2]); let g2 = b[0] - (sb[0] + sb[2]);
                        let dg = g2 - g1; if dg.abs() > 1.0 / 64.0 + 1e-4 && dg.abs() <= 1.0 { hit.push((5, dg)); }
                    }
                }
            }
            if hit.is_empty() { n_clean += 1; pc += 1; continue; }
            for &(k, d) in &hit {
                by_kind[k] += 1;
                *fam.entry((format!("{:?}", st(q).display), format!("{:?}", st(n).display), KINDS[k])).or_default() += 1;
                if arg("--out").is_some() { out.push(serde_json::json!({"page": page, "node": n, "tag": dom.tag(n), "kind": KINDS[k], "d": d, "frame": q, "rect": b}).to_string()); }
            }
        }
        per_page.push((page, pb, pc));
    }
    if let Some(f) = arg("--out") { std::fs::write(&f, out.join("\n") + "\n").ok(); }
    eprintln!("PRISTINE (own geometry, no Chrome) · {} pages · {n_box} boxes · pristine {n_clean} = {:.1} % · {:.1} s",
        pages.len(), n_clean as f64 * 100.0 / n_box.max(1) as f64, t0.elapsed().as_secs_f64());
    for k in 0..6 { eprintln!("  {:11} {:6} boxes", KINDS[k], by_kind[k]); }
    let mut fv: Vec<_> = fam.into_iter().collect(); fv.sort_by(|x, y| y.1.cmp(&x.1));
    eprintln!("  largest defect families (frame display, box display, kind):");
    for ((pd, d, k), v) in fv.iter().take(10) { eprintln!("    {v:6}  {pd} > {d}  {k}"); }
    per_page.sort_by(|x, y| ((x.2 as f64) / (x.1.max(1) as f64)).partial_cmp(&((y.2 as f64) / (y.1.max(1) as f64))).unwrap());
    eprintln!("  least pristine pages:");
    for (pg, b, c) in per_page.iter().take(6) { eprintln!("    {:5.1} %  {:5} boxes  {}", *c as f64 * 100.0 / (*b).max(1) as f64, b, &pg[..pg.len().min(48)]); }
}

/// E1 verify (projects/doctor-core/03-evolve-t25/PLAN.md): the T25 core FIRING inside every layout pass, checked against the
/// station. For the base program and base + `--clause` (the patch toggle), every replay + holdout page is scored by the station
/// (`score_full`, whose `lay.run()` fires the core with the page's armed plate). Checks: core agreement == station agreement on
/// every page (mismatch count), core lost / gained == station lost / gained (the gates' deltas), family deltas, fire share of
/// layout time. Marks (program sig + kernel hash per firing) → `--marks F` (default results/t25fire.marks.jsonl).
/// `omni-render core corpus --abc t25fire [--program P] [--clause 'patch.K[Ctx]=-1'] [--marks F]`
pub fn t25fire(a: &[String]) {
    use crate::jit::t25 as T;
    use crate::dom::Kind;
    use crate::style::Display;
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let dir = a.iter().skip_while(|x| *x != "corpus").next().cloned().unwrap_or_else(|| "corpus".into());
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    let (replay, hold, ysha) = super::super::frozen_sets(&dir, &format!("{}.sets.json", prog.trim_end_matches(".json")));
    let pages: Vec<std::path::PathBuf> = replay.iter().chain(hold.iter()).cloned().collect();
    let base = crate::layout::Program::load(&prog).map();
    let mut progs = vec![("base".to_string(), base.clone())];
    if let Some(cl) = arg("--clause") {
        let (kc, w) = cl.rsplit_once('=').expect("key[ctx]=w"); let w: i8 = w.parse().expect("ternary weight");
        let (key, ctx) = kc.split_once('[').map(|(k, c)| (k.to_string(), c.trim_end_matches(']').to_string())).expect("key[ctx]");
        let mut cand = base.clone(); cand.entry(format!("k:{key}")).or_default().insert(ctx, vec![w]);
        progs.push((cl, cand));
    }
    let plate = |p: &std::path::Path| -> Option<T::Plate> {
        let c = super::super::cached_pub(p)?;
        let l = &c.l;
        let mut chrome = HashMap::new();
        for (&n, &j) in &c.align {
            if l.dom.nodes[n as usize].kind != Kind::Element || l.dom.tag(n).starts_with("::") { continue; }
            if l.styled.table[l.styled.ids[n as usize] as usize].display == Display::Inline { continue; }
            let Some(cb) = c.ch[j].bounds else { continue };
            if cb[2] <= 0.0 || cb[3] <= 0.0 { continue; }
            chrome.insert(n, (cb, [T::band(cb[0]), T::band(cb[1]), T::band(cb[2]), T::band(cb[3])]));
        }
        Some(T::Plate { page: p.file_name().unwrap().to_string_lossy().to_string(), chrome })
    };
    let marks_path = arg("--marks").unwrap_or_else(|| "results/t25fire.marks.jsonl".into());
    let mut marks = String::new();
    // per program: page → (station agree, core firing)
    let mut runs: Vec<HashMap<std::path::PathBuf, (std::collections::HashSet<u32>, T::Fired)>> = Vec::new();
    for (name, m) in &progs {
        crate::layout::set_program(crate::layout::Program::from_map(m.clone()));
        // the program's hash (sorted map → bytes → ir::hash): marks carry WHICH program the core fired under
        let sig = { let b: BTreeMap<&String, BTreeMap<&String, &Vec<i8>>> = m.iter().map(|(k, v)| (k, v.iter().collect())).collect();
                    format!("{:016x}", crate::jit::ir::hash(serde_json::to_string(&b).unwrap().as_bytes())) };
        let mut run = HashMap::new();
        let (mut mism, mut fus, mut lus, mut nb) = (0usize, 0f64, 0f64, 0usize);
        for p in &pages {
            T::arm(plate(p));
            let st = super::super::score_full_pub(p);
            let fired = T::take_last();
            T::arm(None);
            let (Some((agree, _)), Some(f)) = (st, fired) else { continue };
            mism += agree.symmetric_difference(&f.agree).count();
            fus += f.fire_us; lus += f.layout_us; nb += f.boxes;
            marks.push_str(&serde_json::json!({"prog": name, "sig": sig, "page": f.page, "kernel": format!("{:016x}", f.kernel), "boxes": f.boxes,
                "agree": f.agree.len(), "roots": f.roots, "families": f.fam.len(), "fire_us": f.fire_us, "layout_us": f.layout_us}).to_string());
            marks.push('\n');
            run.insert(p.clone(), (agree, f));
        }
        let ag: usize = run.values().map(|x| x.1.agree.len()).sum();
        let st_ag: usize = run.values().map(|x| x.0.len()).sum();
        let fam: usize = { let mut s = std::collections::HashSet::new(); for x in run.values() { for k in x.1.fam.keys() { s.insert(k.clone()); } } s.len() };
        eprintln!("t25fire [{name}] sig {sig} · {} pages · {nb} boxes · core agree {ag} == station {st_ag} · mismatches {mism} · roots {} · {fam} families · fire {:.0} µs / layout {:.0} µs = {:.2} % ({:.1} ns/box)",
            run.len(), run.values().map(|x| x.1.roots).sum::<usize>(), fus, lus, 100.0 * fus / lus.max(1.0), fus * 1e3 / nb.max(1) as f64);
        runs.push(run);
    }
    std::fs::write(&marks_path, &marks).ok();
    if runs.len() == 2 {
        let (b, c) = (&runs[0], &runs[1]);
        let (mut cl, mut cg, mut sl, mut sg) = ([0usize; 2], [0usize; 2], [0usize; 2], [0usize; 2]);
        let mut fdelta: HashMap<(String, String, [i8; 4]), i64> = HashMap::new();
        for p in &pages {
            let (Some(x), Some(y)) = (b.get(p), c.get(p)) else { continue };
            let h = hold.contains(p) as usize;
            cl[h] += x.1.agree.difference(&y.1.agree).count(); cg[h] += y.1.agree.difference(&x.1.agree).count();
            sl[h] += x.0.difference(&y.0).count(); sg[h] += y.0.difference(&x.0).count();
            for (k, v) in &x.1.fam { *fdelta.entry(k.clone()).or_default() -= *v as i64; }
            for (k, v) in &y.1.fam { *fdelta.entry(k.clone()).or_default() += *v as i64; }
        }
        let same = cl == sl && cg == sg;
        eprintln!("t25fire toggle {}: core lost {}/{} gained {}/{} · station lost {}/{} gained {}/{} → {} (sets {})", progs[1].0, cl[0], cl[1], cg[0], cg[1], sl[0], sl[1], sg[0], sg[1],
            if same { "EQUAL" } else { "DIFFERENT" }, &ysha[..12]);
        let mut fv: Vec<_> = fdelta.into_iter().filter(|x| x.1 != 0).collect(); fv.sort_by(|x, y| y.1.abs().cmp(&x.1.abs()).then(x.0.cmp(&y.0)));
        for (k, v) in fv.iter().take(8) { eprintln!("  family {v:+4} ({}, {}, {}, {}, {}, {})", k.0, k.1, k.2[0], k.2[1], k.2[2], k.2[3]); }
        println!("{}", serde_json::json!({"clause": progs[1].0, "core": {"lost": cl, "gained": cg}, "station": {"lost": sl, "gained": sg}, "equal": same}));
    }
}

/// Lay out one page bundle with no oracle (any new site: page.html + res/ + manifest.json; Chrome snapshot not needed).
fn lay_only(p: &std::path::Path) -> Option<()> {
    let b = crate::page::Bundle::open(p)?;
    let l = crate::page::load(&b, (1280.0, 800.0));
    let fonts = crate::font::Fonts::new();
    for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
    let base = b.url.clone(); let dref = &l.dom;
    let sizes = |n: u32| -> Option<(u32, u32)> {
        if dref.tag(n) == "svg" { crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)) }
        else { crate::img_bytes(dref, n, &base, &b).and_then(|bytes| crate::img::size(&bytes)) }
    };
    let mut lay = crate::layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
    lay.run();
    Some(())
}

/// PRISTINE MAP (doctor-core S1/M1–M2, author 10-07): the doctor's FIRST-LOAD procedure for any page — wake the T25 core with the
/// pristine plate (the page's own exact geometry), lay the page out once, take the map: every box not pristine, its exact
/// per-axis error (T25 twist → CRT), the relation it misses, its family key. No Chrome needed.
/// `omni-render core corpus --abc t25map [--all] [--page DIR] [--chrome-set] [--out F.jsonl] [--json F]`
///   --page DIR    one bundle (a newly added site); otherwise the corpus yardstick pages (replay; --all adds holdout)
///   --chrome-set  restrict to the boxes Chrome also has (the `--abc pristine` station's set) — the equality check
pub fn t25map(a: &[String]) {
    use crate::jit::t25 as T;
    use crate::dom::Kind;
    use crate::style::Display;
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let pages: Vec<std::path::PathBuf> = if let Some(d) = arg("--page") { vec![d.into()] } else if let Some(d) = arg("--dir") { super::fix::bundles(&d) } else {
        let (replay, hold, _) = super::super::frozen_sets("corpus", &format!("{}.sets.json", prog.trim_end_matches(".json")));
        if a.iter().any(|x| x == "--all") { replay.iter().chain(hold.iter()).cloned().collect() } else { replay }
    };
    let chrome_set = a.iter().any(|x| x == "--chrome-set");
    let t0 = std::time::Instant::now();
    let (mut nb, mut np, mut bk, mut fus, mut lus) = (0usize, 0usize, [0usize; 6], 0f64, 0f64);
    let mut nov = 0usize;
    let mut fam: HashMap<(String, String, &'static str, [i8; 4]), usize> = HashMap::new();
    let mut out = String::new();
    let mut per_page = Vec::new();
    for p in &pages {
        let page = p.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        let only = if chrome_set {
            let Some(c) = super::super::cached_pub(p) else { continue };
            let l = &c.l;
            Some(c.align.iter().filter(|&(&n, &j)| l.dom.nodes[n as usize].kind == Kind::Element && !l.dom.tag(n).starts_with("::")
                && l.styled.table[l.styled.ids[n as usize] as usize].display != Display::Inline
                && c.ch[j].bounds.is_some_and(|cb| cb[2] > 0.0 && cb[3] > 0.0)).map(|(&n, _)| n).collect())
        } else { None };
        T::arm_pristine(Some(T::PristinePlate { page: page.clone(), only, wake: "first_load" }));
        let laid = if chrome_set { super::super::score_full_pub(p).map(|_| ()) } else { lay_only(p) };
        let m = T::take_map();
        T::arm_pristine(None);
        let (Some(()), Some(m)) = (laid, m) else { eprintln!("  t25map: {page}: cannot lay out"); continue };
        nb += m.boxes; np += m.pristine; fus += m.fire_us; lus += m.layout_us; nov += m.overlaps;
        for k in 0..6 { bk[k] += m.by_kind[k]; }
        for d in &m.defects {
            *fam.entry(d.key.clone()).or_default() += 1;
            out.push_str(&serde_json::json!({"page": page, "node": d.node, "frame": d.frame, "hits": d.hits.iter().map(|h| (T::KINDS[h.0], h.1)).collect::<Vec<_>>(),
                "err": d.err, "key": [&d.key.0, &d.key.1, d.key.2, d.key.3[0], d.key.3[1], d.key.3[2], d.key.3[3]]}).to_string()); out.push('\n');
        }
        per_page.push(serde_json::json!({"page": page, "boxes": m.boxes, "pristine": m.pristine, "defects": m.defects.len(), "overlaps": m.overlaps, "fire_us": m.fire_us, "layout_us": m.layout_us, "kernel": format!("{:016x}", m.kernel)}));
    }
    if let Some(f) = arg("--out") { std::fs::write(&f, &out).ok(); }
    let nf = fam.len();
    eprintln!("t25map (PRISTINE plate, wake=first_load{}) · {} pages · {nb} boxes · pristine {np} = {:.1} % · defects {} · {nf} families · sibling overlaps {nov} · fire {:.0} µs / layout {:.0} µs = {:.2} % · {:.1} s",
        if chrome_set { ", chrome set" } else { ", all boxes" }, per_page.len(), np as f64 * 100.0 / nb.max(1) as f64, nb - np, fus, lus, 100.0 * fus / lus.max(1.0), t0.elapsed().as_secs_f64());
    for k in 0..6 { eprintln!("  {:11} {:6} boxes", T::KINDS[k], bk[k]); }
    let mut fv: Vec<_> = fam.into_iter().collect(); fv.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    for (k, v) in fv.iter().take(10) { eprintln!("    {v:6}  {} > {}  {}  {:?}", k.0, k.1, k.2, k.3); }
    if let Some(f) = arg("--json") {
        std::fs::write(&f, serde_json::json!({"boxes": nb, "pristine": np, "by_kind": T::KINDS.iter().zip(bk).map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>(),
            "families": nf, "top_families": fv.iter().take(40).map(|(k, v)| serde_json::json!([k.0, k.1, k.2, k.3, v])).collect::<Vec<_>>(), "pages": per_page}).to_string()).ok();
    }
}
