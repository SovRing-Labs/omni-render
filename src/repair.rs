// SPDX-License-Identifier: LicenseRef-PolyForm-Small-Business-1.0.0 OR LicenseRef-PolyForm-Noncommercial-1.0.0
//! repair — the self-repair core (author 10-04: "the core works on the problems one at a time, comparing the error to the
//! proof, trying combination after combination until complete, then moving on; as it keeps succeeding it learns
//! patterns and gets better, like the sieve").
//! Problems = origin mismatches (the box is wrong, its parent agrees with Chrome) grouped by signature. Combinations =
//! ternary weights on named layout terms per context (the engine's program vector, `layout::Program`). Proof = Chrome's
//! boxes. Each move is measured on the problem's pages (layout only); kept only if the problem shrinks and no page gets
//! worse; every success goes into a codebook so the next similar problem tries the move that worked first.
use crate::layout::{set_program, Program, POINTS};
#[path = "doctor.rs"] pub mod doctor; // DOCTOR core (src/doctor.rs), declared here so main.rs stays untouched
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Agreement of one page under the current program: (agreeing comparable boxes, comparable boxes, wrong origins as (node, signature)).
fn score(p: &Path) -> Option<(HashSet<u32>, usize, Vec<(u32, String, f32)>)> { score_full(p).map(|f| (f.agree, f.total, f.origins.into_iter().map(|o| (o.n, o.sig, o.err)).collect())) }

thread_local! { static LEAN: std::cell::RefCell<Option<std::rc::Rc<HashSet<u32>>>> = const { std::cell::RefCell::new(None) }; }
/// Score in lean mode: only `watch` boxes get errors, no origins (screens).
fn score_lean(p: &Path, watch: std::rc::Rc<HashSet<u32>>) -> Option<Full> { LEAN.with(|l| *l.borrow_mut() = Some(watch)); let f = score_full(p); LEAN.with(|l| *l.borrow_mut() = None); f }

struct Origin { n: u32, parent: u32, sig: String, axis: usize, err: f32, gp: u32, gpd: String, ours: f32 }
struct Full { agree: HashSet<u32>, total: usize, origins: Vec<Origin>, diff: HashMap<u32, [f32; 4]> }

/// One page's program-independent work, done once per worker thread (WebKit CSS JIT lesson: move the expensive generic part
/// out of the repeated path). Style never depends on the program vector, so parse + cascade + web fonts + Chrome boxes +
/// alignment + image sizes are reused by every screen; only layout re-runs. `--split` measured load = 70% of a screen.
pub(crate) struct Cached { exprs: crate::style::ExprTable, b: crate::page::Bundle, l: crate::page::Loaded, fonts: crate::font::Fonts, ch: Vec<crate::verify::ChromeEl>, align: HashMap<u32, usize>, sizes: std::cell::RefCell<HashMap<u32, Option<(u32, u32)>>> }
thread_local! { static PAGES: std::cell::RefCell<HashMap<PathBuf, std::rc::Rc<Cached>>> = std::cell::RefCell::new(HashMap::new()); }
fn cached(p: &Path) -> Option<std::rc::Rc<Cached>> {
    if let Some(c) = PAGES.with(|m| m.borrow().get(p).cloned()) { return Some(c); }
    let b = crate::page::Bundle::open(p)?;
    let ch = crate::verify::chrome_elements(&p.join("chrome-snap.json"))?;
    let l = crate::page::load(&b, (1280.0, 800.0));
    let fonts = crate::font::Fonts::new();
    for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
    let align = crate::verify::align(&l.dom, &ch).into_iter().collect();
    let c = std::rc::Rc::new(Cached { exprs: crate::style::exprs_save(), b, l, fonts, ch, align, sizes: Default::default() });
    PAGES.with(|m| m.borrow_mut().insert(p.to_path_buf(), c.clone()));
    Some(c)
}

thread_local! { static RECTS: std::cell::RefCell<Option<HashMap<u32, ([f32; 4], [f32; 4])>>> = const { std::cell::RefCell::new(None) }; }
/// Both engines' rectangles for every comparable box: node → (ours, Chrome), x/y/w/h in px (the SHAPE station's input).
pub(crate) fn rects(p: &Path) -> Option<(std::rc::Rc<Cached>, HashSet<u32>, HashMap<u32, ([f32; 4], [f32; 4])>)> {
    RECTS.with(|r| *r.borrow_mut() = Some(HashMap::new()));
    let f = score_full(p);
    let m = RECTS.with(|r| r.borrow_mut().take()).unwrap_or_default();
    Some((cached(p)?, f?.agree, m))
}

/// (agree set, per-box error x/y/w/h) for the doctor's gate display.
/// The agreeing set keyed by STABLE ids: a node's index among the document's real nodes (generated ::before / ::after
/// nodes and their contents excluded). A style change that adds or drops a pseudo-element shifts every later arena id —
/// `:has()` on ebay moved 2 ids per card and the id-keyed binary gate read 32 lost / 32 gained for an unchanged page.
pub(crate) fn score_stable(p: &Path) -> Option<HashSet<u32>> {
    let f = score_full(p)?;
    let c = cached(p)?;
    let d = &c.l.dom;
    let mut pseudo = vec![false; d.nodes.len()]; let mut map = vec![u32::MAX; d.nodes.len()]; let mut k = 0u32;
    for i in 0..d.nodes.len() {
        let par = d.nodes[i].parent;
        pseudo[i] = d.tag(i as u32).starts_with("::") || (par != crate::dom::NONE && (par as usize) < i && pseudo[par as usize]);
        if !pseudo[i] { map[i] = k; k += 1; }
    }
    Some(f.agree.into_iter().filter(|&n| map[n as usize] != u32::MAX).map(|n| map[n as usize]).collect())
}

pub(crate) fn score_full_pub(p: &Path) -> Option<(HashSet<u32>, HashMap<u32, [f32; 4]>)> { score_full(p).map(|f| (f.agree, f.diff)) }
pub(crate) fn cached_pub(p: &Path) -> Option<std::rc::Rc<Cached>> { cached(p) }
fn score_full(p: &Path) -> Option<Full> {
    use crate::dom::{Kind, NONE};
    use crate::style::Display;
    let c = cached(p)?;
    crate::style::exprs_restore(&c.exprs); // calc() tables are per document
    let (b, l, fonts, ch) = (&c.b, &c.l, &c.fonts, &c.ch);
    let dref = &l.dom; let base = b.url.clone();
    let sizes = |n: u32| -> Option<(u32, u32)> {
        if let Some(v) = c.sizes.borrow().get(&n) { return *v; }
        let v = if dref.tag(n) == "svg" { crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)) }
            else { crate::img_bytes(dref, n, &base, b).and_then(|bytes| crate::img::size(&bytes)) };
        c.sizes.borrow_mut().insert(n, v); v
    };
    let mut lay = crate::layout::Layout::new(&l.dom, &l.styled, fonts, &sizes, 1280, 800);
    lay.run();
    let ours: HashMap<u32, [i32; 4]> = lay.out.boxes.iter().map(|b| (b.0, b.1)).collect();
    let align = &c.align;
    let st = |n: u32| &l.styled.table[l.styled.ids[n as usize] as usize];
    let el = |n: u32| l.dom.nodes[n as usize].kind == Kind::Element && !l.dom.tag(n).starts_with("::");
    let agree = |n: u32| -> Option<(bool, [f32; 4], [f32; 4])> {
        let o = ours.get(&n)?; let cb = align.get(&n).and_then(|&j| ch[j].bounds)?;
        if cb[2] <= 0.0 || cb[3] <= 0.0 { return None; }
        let o = [o[0] as f32 / 64.0, o[1] as f32 / 64.0, o[2] as f32 / 64.0, o[3] as f32 / 64.0];
        Some(((0..4).all(|k| (o[k] - cb[k]).abs() <= 2.0f32.max(cb[k].abs() * 0.05)), o, cb))
    };
    let (mut good, mut total) = (HashSet::new(), 0);
    let mut origins = Vec::new(); let mut diff = HashMap::new();
    // fused screen pass (HyPer's data-centric idea, clean-room): a screen needs only the agreeing set and the watched boxes'
    // errors, so it skips the full diff map and origin clustering in the same loop
    let lean = LEAN.with(|l| l.borrow().clone());
    for &n in ours.keys() {
        if !el(n) || st(n).display == Display::Inline { continue; }
        let Some((ok, o, cb)) = agree(n) else { continue };
        total += 1;
        RECTS.with(|r| if let Some(m) = r.borrow_mut().as_mut() { m.insert(n, (o, cb)); });
        if lean.as_ref().map_or(true, |w| w.contains(&n)) { diff.insert(n, [o[0] - cb[0], o[1] - cb[1], o[2] - cb[2], o[3] - cb[3]]); }
        if ok { good.insert(n); continue; }
        if lean.is_some() { continue; }
        let p = l.dom.nodes[n as usize].parent;
        if p == NONE || !el(p) || agree(p).map(|x| x.0) != Some(true) { continue; }
        let kind = if (o[2] - cb[2]).abs() > 2.0f32.max(cb[2] * 0.05) { if o[2] > cb[2] { "w+" } else { "w-" } } else if (o[3] - cb[3]).abs() > 2.0f32.max(cb[3] * 0.05) { if o[3] > cb[3] { "h+" } else { "h-" } } else if (o[0] - cb[0]).abs() > 2.0 { "x" } else { "y" };
        let has_text = l.dom.children(n).any(|c| l.dom.nodes[c as usize].kind == Kind::Text && !l.dom.text(c).trim().is_empty());
        let k = match kind { "w+" | "w-" => 2, "h+" | "h-" => 3, "x" => 0, _ => 1 };
        let gp = l.dom.nodes[p as usize].parent;
        let gpd = if gp != NONE && el(gp) { format!("{:?}", st(gp).display) } else { String::new() };
        origins.push(Origin { gp, gpd, n, parent: p, sig: format!("{kind}|{:?}|{:?}|{}", st(n).display, st(p).display, if has_text { "txt" } else { "-" }), axis: k, err: o[k] - cb[k], ours: o[k] });
    }
    Some(Full { agree: good, total, origins, diff })
}

#[derive(Clone)]
struct Move { point: String, ctx: String, term: usize, w: i8 }
type Prog = HashMap<String, HashMap<String, Vec<i8>>>;

fn apply(base: &Prog, m: &Move) -> Prog {
    let mut p = base.clone();
    let e = p.entry(m.point.clone()).or_default().entry(m.ctx.clone()).or_insert_with(|| vec![1; Program::terms(&m.point)]);
    e[m.term] = m.w;
    p
}
fn term_name(m: &Move) -> String { if let Some(k) = m.point.strip_prefix("k:") { return k.to_string(); } POINTS.iter().find(|p| p.0 == m.point).map(|p| p.1[m.term].to_string()).unwrap_or_else(|| "?".into()) }

/// Frozen yardstick (reason-gen `frozen_set` form): the page list split into replay (the core learns from these) and holdout
/// (canary: never a problem source, only gated), pinned by the sha256 of each page's Chrome snapshot on first use, so later
/// corpus or engine changes cannot move it. A page whose snapshot changed is dropped from the yardstick, never re-graded.
fn frozen_sets(dir: &str, path: &str) -> (Vec<PathBuf>, Vec<PathBuf>, String) {
        let sha = |p: &Path| std::fs::read(p.join("chrome-snap.json")).ok().map(|b| hex(&b));
    if let Ok(t) = std::fs::read_to_string(path) {
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(&t) {
            let take = |k: &str| -> Vec<PathBuf> { j[k].as_array().map(|a| a.iter().filter_map(|e| { let p = Path::new(dir).join(e["page"].as_str()?); (sha(&p).as_deref() == e["sha"].as_str()).then_some(p) }).collect()).unwrap_or_default() };
            return (take("replay"), take("holdout"), hex(t.as_bytes()));
        }
    }
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists() && p.join("chrome-snap.json").exists()).collect();
    pages.sort();
    // every 4th page (by name) is holdout: deterministic, spread across the corpus
    let (mut rep, mut hold) = (Vec::new(), Vec::new());
    for (i, p) in pages.into_iter().enumerate() { if i % 4 == 3 { hold.push(p) } else { rep.push(p) } }
    let ent = |v: &Vec<PathBuf>| v.iter().map(|p| serde_json::json!({"page": p.file_name().unwrap().to_string_lossy(), "sha": sha(p)})).collect::<Vec<_>>();
    let body = serde_json::to_string_pretty(&serde_json::json!({"replay": ent(&rep), "holdout": ent(&hold)})).unwrap();
    std::fs::write(path, &body).ok();
    (rep, hold, hex(body.as_bytes()))
}

fn hex(b: &[u8]) -> String { ring::digest::digest(&ring::digest::SHA256, b).as_ref().iter().map(|x| format!("{x:02x}")).collect() }
fn prog_sha(p: &Prog) -> String {
        let mut pts: Vec<_> = p.iter().map(|(k, m)| { let mut c: Vec<_> = m.iter().collect(); c.sort(); (k, c) }).collect(); pts.sort();
    hex(format!("{pts:?}").as_bytes())[..16].to_string()
}
fn move_key(m: &Move) -> String { format!("{}[{}].{}={}", m.point, m.ctx, m.term, m.w) }
fn name(p: &Path) -> String { p.file_name().unwrap().to_string_lossy().to_string() }
fn now() -> String { let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(); format!("{s}") }

/// Abstain (reason-gen form: an open problem names the missing piece). Re-lays out up to 3 instances with the derivation
/// recorder on the failing box and reports: the error (median signed px on the failing axis), the layout stages the box went
/// through, and which of those stages have NO decision point in the program vector — that stage is where a term is missing.
fn abstain_card(sig: &str, inst: &[(PathBuf, u32, f32)]) -> serde_json::Value {
    let mut d: Vec<f32> = inst.iter().map(|x| x.2).collect(); d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = d[d.len() / 2];
    let mut stages: HashMap<&'static str, usize> = HashMap::new(); let mut samples = Vec::new();
    let mut seen = HashSet::new();
    for (p, n, _) in inst.iter().filter(|x| seen.insert(x.0.clone())).take(3) {
        crate::why::watch(*n); score(p); let log = crate::why::take(); crate::why::watch(u32::MAX);
        let mut st: Vec<&str> = log.iter().map(|x| x.0).collect(); st.dedup();
        for s in &st { *stages.entry(s).or_default() += 1; }
        samples.push(serde_json::json!({"page": name(p), "node": n, "stages": st, "last": log.last().map(|x| x.1.clone())}));
    }
    // stage → the program points that already cover it
    let covered = |s: &str| -> &'static [&'static str] { match s { "intrinsic" => &["intrinsic.inline"], "flow" | "block" => &["flow.margins"], "flex" => &["flex.main"], _ => &[] } };
    let mut st: Vec<_> = stages.into_iter().collect(); st.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let missing: Vec<String> = st.iter().filter(|(s, _)| *s != "box" && covered(s).is_empty()).map(|(s, _)| s.to_string()).collect();
    serde_json::json!({"sig": sig, "instances": inst.len(), "median_err_px": med, "stages": st.iter().map(|(s, c)| format!("{s}×{c}")).collect::<Vec<_>>(),
        "missing": if st.iter().all(|(s, _)| *s == "box") { format!("no decision point and no recorder in the {} layout path (parent {})", sig.split('|').nth(1).unwrap_or("?"), sig.split('|').nth(2).unwrap_or("?")) } else if missing.is_empty() { "a term inside a covered stage".to_string() } else { format!("no decision point in stage(s) {}", missing.join(", ")) }, "samples": samples})
}

/// `omni-render core [corpus] [--program omr.program.json] [--max N] [--gates K]`
/// Ring 0 (never rewritten by the core): Chrome's boxes, `score`, the gate. The core rewrites only the program vector.
/// Gate (reason-gen form): every box that agreed with Chrome still agrees, on every replay AND holdout page; the problem gains.
/// Kept → program saved + history entry (from/to program sha, yardstick sha, boxes before/after — the rollback breadcrumb)
/// + codebook. Rejected → quarantine with the killing page and box (negative knowledge: skipped while the program is the same).
pub fn run(args: &[String]) {
    if args.iter().any(|a| a == "--abc") { return doctor::abc::cli(args); }
    if args.iter().any(|a| a == "--sections" || a == "--donors") { return doctor::run(args); }
    if args.iter().any(|a| a == "--patch-gate") { return doctor::patch_gate(args); }
    if args.iter().any(|a| a == "--patch-search") { return doctor::patch_search(args); }
    if args.iter().any(|a| a == "--missing") { return doctor::missing(args); }
    if args.iter().any(|a| a == "--index") { return doctor::index(args); }
    if args.iter().any(|a| a == "--agree-snapshot" || a == "--binary-gate") { return doctor::binary_gate(args); }
    if args.iter().any(|a| a == "--split") {
        // where a screen's time goes: bundle open + parse + cascade (load) vs layout, per page, one thread
        let dir = args.first().cloned().unwrap_or_else(|| "corpus".into());
        let mut ps: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect(); ps.sort();
        let (mut lo, mut la) = (0.0, 0.0);
        for p in &ps { if let Some(v) = watch_page(p, false) { lo += v["load_ms"].as_f64().unwrap_or(0.0); la += v["layout_ms"].as_f64().unwrap_or(0.0); } }
        eprintln!("split over {} pages: load (open+parse+cascade+fonts) {lo:.0} ms · layout {la:.0} ms · load share {:.0}%", ps.len(), 100.0 * lo / (lo + la));
        for pass in 1..=3 { let t = std::time::Instant::now(); let a: usize = ps.iter().filter_map(|p| score_full(p)).map(|f| f.agree.len()).sum();
            eprintln!("score pass {pass} (page cache {}): {:.0} ms, {a} agreeing boxes", if pass == 1 { "cold" } else { "warm" }, t.elapsed().as_secs_f64() * 1e3); }
        { let t = std::time::Instant::now(); let a: usize = ps.iter().filter_map(|p| score_lean(p, std::rc::Rc::new(HashSet::new()))).map(|f| f.agree.len()).sum(); eprintln!("lean pass (fused): {:.0} ms, {a} agreeing boxes", t.elapsed().as_secs_f64() * 1e3); }
        if let Ok(st) = std::fs::read_to_string("/proc/self/status") { eprintln!("{}", st.lines().find(|l| l.starts_with("VmRSS")).unwrap_or("")); }
        return;
    }
    if args.iter().any(|a| a == "--check-compiled") {
        // the compiled knob lookup must equal the interpreter (knob_at) for every (owner box, key) on every page
        let dir = args.first().cloned().unwrap_or_else(|| "corpus".into());
        let prog = Program::load(&args.iter().position(|a| a == "--program").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "omr.program.json".into()));
        let mut ps: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect(); ps.sort();
        let keys: Vec<String> = prog.clauses().into_iter().map(|c| c.0).collect::<HashSet<_>>().into_iter().collect();
        let (mut n_eq, mut n_diff) = (0usize, 0usize);
        for p in &ps { let Some(b) = crate::page::Bundle::open(p) else { continue }; let l = crate::page::load(&b, (1280.0, 800.0));
            for n in 0..l.dom.nodes.len() as u32 { if l.dom.nodes[n as usize].kind != crate::dom::Kind::Element { continue; }
                let disp = crate::layout::node_feature(&l.dom, &l.styled, n, "disp"); let ctx = if l.dom.quirks { format!("{disp}@q") } else { disp.clone() };
                for k in &keys { let a = prog.knob_at(k, &ctx, &|f| crate::layout::node_feature(&l.dom, &l.styled, n, f));
                    let b2 = prog.knob_fast(k, &disp, l.dom.quirks, &|i| crate::layout::node_feature(&l.dom, &l.styled, n, crate::layout::FEATURES[i]));
                    let mut codes = vec![crate::layout::intern(&ctx), crate::layout::intern(&disp), crate::layout::intern(if l.dom.quirks { "1" } else { "0" })];
                    for f in crate::layout::FEATURES { codes.push(crate::layout::intern(&crate::layout::node_feature(&l.dom, &l.styled, n, f))); }
                    let (b3, b4) = (prog.knob_bits(k, &codes), prog.knob_jit(k, &codes));
                    if a == b2 && a == b3 && a == b4 { n_eq += 1 } else { n_diff += 1; if n_diff <= 5 { eprintln!("  DIFF {}#{n} {k}: interpreter {a} compiled {b2} bits {b3} stencil {b4}", name(p)); } } } } }
        eprintln!("interpreter = compiled = bitmask = stencil JIT: {n_eq} identical, {n_diff} different ({} keys, {} pages)", keys.len(), ps.len());
        return;
    }
    if args.iter().any(|a| a == "--verify-program") {
        let path = args.iter().position(|a| a == "--program").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "omr.program.json".into());
        let signing = args.iter().any(|a| a == "--sign-to");
        let pr = Program::load_checked(&path, !signing); let cl = pr.clauses();
        eprintln!("program {path}: {} knob clauses loaded, {} known keys in the engine source", cl.len(), crate::layout::known_keys().len());
        if let Some(out) = args.iter().position(|a| a == "--sign-to").and_then(|i| args.get(i + 1)) { pr.save(out); eprintln!("signed copy → {out}"); }
        return;
    }
    if args.iter().any(|a| a == "--challenge") { return run_challenge(args); }
    if args.iter().any(|a| a == "--fire") { return run_fire(args); }
    if args.iter().any(|a| a == "--screen") { return run_screen(args); }
    let dir = args.first().cloned().unwrap_or_else(|| "corpus".into());
    let mut prog_path = "omr.program.json".to_string(); let mut max_problems = 20usize; let mut gates = 4usize;
    let mut i = 1; while i < args.len() { match args[i].as_str() { "--program" => { prog_path = args[i + 1].clone(); i += 2; } "--max" => { max_problems = args[i + 1].parse().unwrap_or(20); i += 2; } "--gates" => { gates = args[i + 1].parse().unwrap_or(4); i += 2; } _ => i += 1 } }
    let stem = prog_path.trim_end_matches(".json").to_string();
    let mut prog: Prog = Program::load(&prog_path).map();
    let set = |m: &Prog| set_program(Program::from_map(m.clone()));
    set(&prog);
    let (replay, holdout, ysha) = frozen_sets(&dir, &format!("{stem}.sets.json"));
    let pages: Vec<PathBuf> = replay.iter().chain(&holdout).cloned().collect();
    let t0 = std::time::Instant::now();
    let mut base: HashMap<PathBuf, HashSet<u32>> = HashMap::new(); let mut total0 = 0;
    let mut problems: HashMap<String, Vec<(PathBuf, u32, f32)>> = HashMap::new();
    for p in &pages { if let Some((g, t, o)) = score(p) { base.insert(p.clone(), g); total0 += t; if replay.contains(p) { for (n, sig, d) in o { problems.entry(sig).or_default().push((p.clone(), n, d)); } } } }
    let count = |b: &HashMap<PathBuf, HashSet<u32>>, ps: &[PathBuf]| -> usize { ps.iter().filter_map(|p| b.get(p)).map(|s| s.len()).sum() };
    let (r0, h0) = (count(&base, &replay), count(&base, &holdout));
    let mut queue: Vec<(String, Vec<(PathBuf, u32, f32)>)> = problems.into_iter().collect();
    queue.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    eprintln!("core: yardstick {} replay + {} holdout pages (sets {}) · agree replay {r0} holdout {h0} of {total0} · {} problems · {:.1}s", replay.len(), holdout.len(), &ysha[..12], queue.len(), t0.elapsed().as_secs_f64());
    let book_path = format!("{stem}.codebook.jsonl");
    let mut book: Vec<(String, Move, usize)> = std::fs::read_to_string(&book_path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).map(|j| (j["sig"].as_str().unwrap_or("").to_string(), Move { point: j["point"].as_str().unwrap_or("intrinsic.inline").to_string(), ctx: j["ctx"].as_str().unwrap_or("*").to_string(), term: j["term"].as_u64().unwrap_or(0) as usize, w: j["w"].as_i64().unwrap_or(1) as i8 }, j["gain"].as_u64().unwrap_or(0) as usize)).collect();
    let quar_path = format!("{stem}.quarantine.jsonl");
    // quarantine is keyed by (move, program it was tried on): a move that broke a page stays skipped until the program changes
    let mut quarantined: HashSet<(String, String)> = std::fs::read_to_string(&quar_path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).map(|j| (j["key"].as_str().unwrap_or("").to_string(), j["program"].as_str().unwrap_or("").to_string())).collect();
    use std::io::Write;
    let app = |path: &str| std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    let (mut log, mut quar, mut hist) = (app(&book_path), app(&quar_path), app(&format!("{stem}.history.jsonl")));
    let mut open_f = std::fs::File::create(format!("{stem}.open.jsonl")).unwrap();
    let (mut solved, mut tries_total, mut skipped) = (0usize, 0usize, 0usize);
    for (sig, inst) in queue.iter().take(max_problems) {
        let pset: Vec<PathBuf> = { let mut v: Vec<PathBuf> = inst.iter().map(|x| x.0.clone()).collect::<HashSet<_>>().into_iter().collect(); v.sort(); v };
        let parts: Vec<&str> = sig.split('|').collect();
        let mut cands: Vec<Move> = Vec::new();
        for (point, names) in POINTS {
            let c1 = parts.get(1).copied().unwrap_or("*"); let c2 = parts.get(2).copied().unwrap_or("*");
            let (q1, q2) = (format!("{c1}@q"), format!("{c2}@q"));
            for ctx in [c1, c2, q1.as_str(), q2.as_str()] {
                let cur = prog.get(*point).and_then(|m| m.get(ctx)).cloned().unwrap_or_else(|| vec![1; names.len()]);
                for t in 0..names.len() { for w in [0i8, -1, 1] { if w != cur[t] { cands.push(Move { point: point.to_string(), ctx: ctx.to_string(), term: t, w }); } } }
            }
        }
        let resonance = |m: &Move| -> usize { book.iter().filter(|(_, bm, _)| bm.point == m.point && bm.ctx == m.ctx && bm.term == m.term && bm.w == m.w).map(|(s, _, g)| { let sp: Vec<&str> = s.split('|').collect(); sp.iter().zip(&parts).filter(|(a, b)| a == b).count() * (1 + g.min(&50)) }).sum() };
        cands.sort_by_key(|m| std::cmp::Reverse(resonance(m)));
        let psha = prog_sha(&prog);
        let before: usize = pset.iter().map(|p| base[p].len()).sum();
        // local pass: every move that grows the problem's pages without losing one agreeing box there, ranked by gain
        let mut local: Vec<(Move, usize)> = Vec::new(); let mut tries = 0;
        for m in &cands {
            if quarantined.contains(&(move_key(m), psha.clone())) { skipped += 1; continue; }
            tries += 1;
            set(&apply(&prog, m));
            let mut ok = true; let mut after = 0;
            for p in &pset { if let Some((g, _, _)) = score(p) { if !base[p].is_subset(&g) { ok = false; break; } after += g.len(); } }
            if ok && after > before { local.push((m.clone(), after - before)); if resonance(m) > 0 { break; } }
        }
        set(&prog);
        local.sort_by(|a, b| b.1.cmp(&a.1));
        let mut done = false; let mut rejected = Vec::new();
        // gate pass: best local move first; a rejected one goes to quarantine and the next is gated (up to --gates)
        for (m, local_gain) in local.iter().take(gates) {
            tries += 1;
            let trial = apply(&prog, m); set(&trial);
            let mut newb: HashMap<PathBuf, HashSet<u32>> = HashMap::new(); let mut killer: Option<(String, u32)> = None;
            for p in &pages { if let Some((g, _, _)) = score(p) { if let Some(&n) = base[p].difference(&g).next() { killer = Some((name(p), n)); break; } newb.insert(p.clone(), g); } }
            if let Some((kp, kn)) = killer {
                writeln!(quar, "{}", serde_json::json!({"ts": now(), "sig": sig, "key": move_key(m), "program": psha, "term_name": term_name(m), "local_gain": local_gain, "reason": "an agreeing box stopped agreeing", "killing_page": kp, "killing_node": kn, "sets": ysha })).ok();
                quarantined.insert((move_key(m), psha.clone()));
                rejected.push(format!("{} (+{local_gain}, breaks {kp}#{kn})", move_key(m)));
                continue;
            }
            let (r1, h1) = (count(&newb, &replay), count(&newb, &holdout));
            let (rb, hb) = (count(&base, &replay), count(&base, &holdout));
            writeln!(hist, "{}", serde_json::json!({"ts": now(), "sig": sig, "move": move_key(m), "term_name": term_name(m), "from": psha, "to": prog_sha(&trial), "sets": ysha, "replay": [rb, r1], "holdout": [hb, h1], "status": "PROPOSED", "gate": "every agreeing box kept, replay + holdout" })).ok();
            let gain = (r1 + h1) - (rb + hb);
            writeln!(log, "{}", serde_json::json!({ "sig": sig, "point": m.point, "ctx": m.ctx, "term": m.term, "w": m.w, "gain": gain, "tries": tries, "term_name": term_name(m) })).ok();
            book.push((sig.clone(), m.clone(), gain));
            prog = trial; base = newb;
            Program::from_map(prog.clone()).save(&prog_path);
            solved += 1; done = true;
            eprintln!("  solved   {sig:40} ({} inst) {}[{}].{}={} after {tries} tries: replay +{} holdout +{}", inst.len(), m.point, m.ctx, term_name(m), m.w, r1 - rb, h1 - hb);
            break;
        }
        set(&prog);
        tries_total += tries;
        if !done {
            if rejected.is_empty() {
                let card = abstain_card(sig, inst);
                eprintln!("  open     {sig:40} ({} inst): no term reaches it ({tries} tries) · missing piece: {}", inst.len(), card["missing"]);
                writeln!(open_f, "{}", card).ok();
            }
            else { eprintln!("  rejected {sig:40} ({} inst): {}", inst.len(), rejected.join(" · ")); }
        }
    }
    let (r1, h1) = (count(&base, &replay), count(&base, &holdout));
    eprintln!("core: {solved} solved · {tries_total} tries ({skipped} skipped by quarantine) · replay {r0} → {r1} · holdout {h0} → {h1} · {:.1}s", t0.elapsed().as_secs_f64());
}

// ── v2 stations: CAUSAL (causal.dag) + FACTORIAL (science.factorial), ported from bin/reason-gen ─────────────────────────
// science.factorial: intervene, never read correlations; required runs per arm = required_n(δ, σ). Layout is deterministic
// (σ = 0), so one intervention per setting decides its effect exactly — the screen is one run per (knob, level), shared by
// EVERY problem at once (the runs cover all replay + holdout pages, so each run is also its own ring-0 gate).
// causal.dag: the effect on a box is DIRECT only when its parent box did not move in that run (the parent is the backdoor
// path: block it by conditioning on it); a box whose parent moved is confounded → the cause sits higher (climb).

struct Run { m: Move, pages: HashMap<PathBuf, (Vec<u32>, Vec<u32>, HashMap<u32, [f32; 4]>)> } // gained, lost, watched diffs

fn screen(pages: &[PathBuf], prog: &Prog, knobs: &[Move], watch: &HashMap<PathBuf, HashSet<u32>>, base: &HashMap<PathBuf, HashSet<u32>>, workers: usize) -> Vec<Run> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= knobs.len() { break; }
                set_program(Program::from_map(apply(prog, &knobs[i])));
                let mut r = Run { m: knobs[i].clone(), pages: HashMap::new() };
                for p in pages {
                    let Some(f) = score_full(p) else { continue };
                    let b = &base[p];
                    let gained: Vec<u32> = f.agree.difference(b).copied().collect();
                    let lost: Vec<u32> = b.difference(&f.agree).copied().collect();
                    let w: HashMap<u32, [f32; 4]> = watch.get(p).map(|ws| ws.iter().filter_map(|n| f.diff.get(n).map(|d| (*n, *d))).collect()).unwrap_or_default();
                    r.pages.insert(p.clone(), (gained, lost, w));
                }
                if (i + 1) % 20 == 0 { eprintln!("    screen {}/{}", i + 1, knobs.len()); }
                out.lock().unwrap().push(r);
            });
        }
    });
    let mut v = out.into_inner().unwrap(); v.sort_by_key(|r| move_key(&r.m)); v
}

/// `omni-render core corpus --screen [--program P] [--workers 3]` — the v2 beat: screen once, then per problem
/// SIEVE (codebook) → CAUSAL (direct effects from the screen) → GATE (real relayout of every page) → COMMIT | quarantine | ABSTAIN.
/// Every problem leaves a trace in reason-gen's 16-op ISA (`<program>.traces.jsonl`): the rows of a future layout.repair generator.
pub fn run_screen(args: &[String]) {
    let dir = args.first().cloned().unwrap_or_else(|| "corpus".into());
    let mut prog_path = "omr.program.json".to_string(); let mut workers = 3usize; let mut max_problems = 63usize;
    let mut i = 1; while i < args.len() { match args[i].as_str() { "--program" => { prog_path = args[i + 1].clone(); i += 2; } "--workers" => { workers = args[i + 1].parse().unwrap_or(3); i += 2; } "--max" => { max_problems = args[i + 1].parse().unwrap_or(63); i += 2; } _ => i += 1 } }
    let stem = prog_path.trim_end_matches(".json").to_string();
    let mut prog: Prog = Program::load(&prog_path).map();
    set_program(Program::from_map(prog.clone()));
    let (replay, holdout, ysha) = frozen_sets(&dir, &format!("{stem}.sets.json"));
    let pages: Vec<PathBuf> = replay.iter().chain(&holdout).cloned().collect();
    let t0 = std::time::Instant::now();
    let mut base: HashMap<PathBuf, HashSet<u32>> = HashMap::new();
    let mut problems: HashMap<String, Vec<(PathBuf, u32, u32, usize, f32)>> = HashMap::new(); // page, node, parent, axis, err
    let mut watch: HashMap<PathBuf, HashSet<u32>> = HashMap::new();
    let mut bdiff: HashMap<PathBuf, HashMap<u32, [f32; 4]>> = HashMap::new();
    let mut led: HashMap<PathBuf, HashMap<u32, HashMap<&'static str, i32>>> = HashMap::new(); // page → node → term → Σ value
    for p in &pages { crate::layout::ledger_start(); let f = score_full(p); let lg = crate::layout::ledger_take();
        { let m = led.entry(p.clone()).or_default(); for (n, k, v) in lg { *m.entry(n).or_default().entry(k).or_insert(0) += v; } }
        if let Some(f) = f { base.insert(p.clone(), f.agree); bdiff.insert(p.clone(), f.diff); if replay.contains(p) { for o in f.origins { let w = watch.entry(p.clone()).or_default(); w.insert(o.n); w.insert(o.parent); problems.entry(o.sig).or_default().push((p.clone(), o.n, o.parent, o.axis, o.err)); } } } }
    let count = |b: &HashMap<PathBuf, HashSet<u32>>, ps: &[PathBuf]| -> usize { ps.iter().filter_map(|p| b.get(p)).map(|s| s.len()).sum() };
    let (r0, h0) = (count(&base, &replay), count(&base, &holdout));
    let mut queue: Vec<_> = problems.into_iter().collect();
    queue.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    queue.truncate(max_problems);
    // knobs: every point × term × alternative weight, in every context the problems name (box display, parent display)
    let mut ctxs: Vec<String> = queue.iter().flat_map(|(s, _)| { let p: Vec<&str> = s.split('|').collect(); vec![p[1].to_string(), p[2].to_string()] }).collect::<HashSet<_>>().into_iter().collect();
    ctxs.sort();
    let use_points = args.iter().any(|a| a == "--points");
    let topk: usize = args.iter().position(|a| a == "--topk").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(6);
    let mut knobs: Vec<Move> = Vec::new();
    // M1 lanes: for every problem, every ledger term on its boxes (or their parents) is a knob; its effect at w ∈ {0, −1} is
    // PREDICTED from the term value (additive: Δ = −v·(1−w)) for all instances at once, without relayout. Only the top-k
    // predicted knobs per problem are re-laid out (the screen), which also measures cascades and is its own gate.
    let mut predicted: HashMap<String, Vec<(Move, usize, f32)>> = HashMap::new();
    for (sig, inst) in &queue {
        let parts: Vec<&str> = sig.split('|').collect();
        let mut cand: HashMap<(String, String, i8), (usize, f32)> = HashMap::new();
        for (p, n, par, _ax, e) in inst {
            let tol = 2.0f32.max(e.abs() * 0.0); let _ = tol;
            let Some(pm) = led.get(p) else { continue };
            for (node, ctx, own) in [(*n, parts[1], true), (*par, parts[2], false)] {
                let Some(terms) = pm.get(&node) else { continue };
                for (k, &v) in terms {
                    if v == 0 { continue; }
                    for w in [0i8, -1] {
                        let delta = -(v as f32 / 64.0) * (1 - w) as f32;
                        let after = e + delta;
                        let fixes = own && after.abs() <= 2.0;
                        let gain = if own { (e.abs() - after.abs()).max(0.0) } else { 0.0 };
                        let c = cand.entry((format!("k:{k}"), ctx.to_string(), w)).or_insert((0, 0.0));
                        if fixes { c.0 += 1; } c.1 += gain + if !own { 0.01 } else { 0.0 };
                    }
                }
            }
        }
        let mut v: Vec<(Move, usize, f32)> = cand.into_iter().map(|((pt, ctx, w), (f, g))| (Move { point: pt, ctx, term: 0, w }, f, g)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.partial_cmp(&a.2).unwrap()).then(move_key(&a.0).cmp(&move_key(&b.0))));
        v.truncate(topk);
        for (m, _, _) in &v { if !knobs.iter().any(|k: &Move| move_key(k) == move_key(m)) { knobs.push(m.clone()); } }
        predicted.insert(sig.clone(), v);
    }
    if use_points { for (point, names) in POINTS { for c in &ctxs { let cur = prog.get(*point).and_then(|m| m.get(c)).cloned().unwrap_or_else(|| vec![1; names.len()]); for t in 0..names.len() { for w in [0i8, -1, 1] { if w != cur[t] { knobs.push(Move { point: point.to_string(), ctx: c.clone(), term: t, w }); } } } } } }
    eprintln!("core v2: yardstick {} replay + {} holdout (sets {}) · agree replay {r0} holdout {h0} · {} problems · {} contexts → {} interventions · {:.1}s", replay.len(), holdout.len(), &ysha[..12], queue.len(), ctxs.len(), knobs.len(), t0.elapsed().as_secs_f64());
    let runs = screen(&pages, &prog, &knobs, &watch, &base, workers);
    set_program(Program::from_map(prog.clone()));
    eprintln!("core v2: screen done, {} runs · {:.1}s", runs.len(), t0.elapsed().as_secs_f64());
    use std::io::Write;
    let app = |path: &str| std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    let (mut book, mut quar, mut hist, mut tr) = (app(&format!("{stem}.codebook.jsonl")), app(&format!("{stem}.quarantine.jsonl")), app(&format!("{stem}.history.jsonl")), app(&format!("{stem}.traces.jsonl")));
    let mut open_f = std::fs::File::create(format!("{stem}.open.jsonl")).unwrap();
    let mut changed_since_screen = false; let (mut solved, mut gates_run) = (0, 0);
    for (sig, inst) in &queue {
        let mut t: Vec<serde_json::Value> = Vec::new();
        let mut step = |op: &str, a: String, r: serde_json::Value| t.push(serde_json::json!({"op": op, "a": a, "r": r}));
        let mut errs: Vec<f32> = inst.iter().map(|x| x.4).collect(); errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        step("GOAL", format!("make the {} origin boxes of {sig} agree with Chrome, losing no agreeing box", inst.len()), serde_json::json!(null));
        step("OBSERVE", "residual ours − Chrome on the failing axis (median px)".into(), serde_json::json!(errs[errs.len() / 2]));
        step("PREDICT", "lanes: ledger terms on these boxes, effect at w 0/−1 from the term value, top-k".into(), serde_json::json!(predicted.get(sig).map(|v| v.iter().map(|(m, f, g)| format!("{} predicts {f} fixed, {g:.0} px closed", move_key(m))).collect::<Vec<_>>())));
        step("INTERVENE", "one run per selected knob over every page (deterministic: required runs per arm = 1)".into(), serde_json::json!(runs.len()));
        // CAUSAL: per run, instances fixed with the parent still (direct), instances moved only via the parent (confounded)
        let mut rows: Vec<(usize, usize, usize, usize, f32)> = Vec::new(); // run idx, direct fixes, confounded, lost boxes, |err| reduction share
        for (k, r) in runs.iter().enumerate() {
            let (mut direct, mut conf, mut red) = (0, 0, 0.0f32);
            for (p, n, par, ax, e) in inst {
                let Some((gained, _, w)) = r.pages.get(p) else { continue };
                let pmoved = match (w.get(par), bdiff[p].get(par)) { (Some(d), Some(b)) => d.iter().zip(b).any(|(x, y)| (x - y).abs() > 0.5), _ => false };
                let nd = w.get(n).map(|d| d[*ax]).unwrap_or(*e);
                if gained.contains(n) { if pmoved { conf += 1 } else { direct += 1 } }
                if e.abs() > 0.0 { red += ((e.abs() - nd.abs()) / e.abs()).clamp(-1.0, 1.0); }
            }
            let lost: usize = r.pages.values().map(|x| x.1.len()).sum();
            if direct > 0 || conf > 0 || red > 0.25 * inst.len() as f32 { rows.push((k, direct, conf, lost, red / inst.len() as f32)); }
        }
        // rank: direct fixes first, then fewer losses
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)).then(b.4.partial_cmp(&a.4).unwrap()));
        let clean: Vec<_> = rows.iter().filter(|x| x.1 > 0 && x.3 == 0).collect();
        step("DERIVE", "direct causes (fix instances, parent unmoved, 0 boxes lost)".into(), serde_json::json!(clean.iter().take(5).map(|x| format!("{} fixes {} ", move_key(&runs[x.0].m), x.1)).collect::<Vec<_>>()));
        let mut done = false;
        for x in clean.iter().take(3) {
            let m = &runs[x.0].m;
            // GATE: when the program changed since the screen, re-run the move for real on every page
            let mut newb = base.clone(); let mut ok = true; let mut killer = None;
            if changed_since_screen {
                gates_run += 1;
                set_program(Program::from_map(apply(&prog, m)));
                for p in &pages { if let Some(f) = score_full(p) { if let Some(&n) = base[p].difference(&f.agree).next() { ok = false; killer = Some((name(p), n)); break; } newb.insert(p.clone(), f.agree); } }
                set_program(Program::from_map(prog.clone()));
            } else { for (p, (g, _, _)) in &runs[x.0].pages { newb.get_mut(p).unwrap().extend(g.iter().copied()); } }
            if !ok {
                let (kp, kn) = killer.unwrap();
                writeln!(quar, "{}", serde_json::json!({"ts": now(), "sig": sig, "key": move_key(m), "program": prog_sha(&prog), "term_name": term_name(m), "reason": "an agreeing box stopped agreeing (re-gate after earlier commits)", "killing_page": kp, "killing_node": kn, "sets": ysha})).ok();
                step("CHECK", format!("gate {}", move_key(m)), serde_json::json!(format!("fail: {kp}#{kn}"))); continue;
            }
            let (rb, hb) = (count(&base, &replay), count(&base, &holdout)); let (r1, h1) = (count(&newb, &replay), count(&newb, &holdout));
            if r1 + h1 <= rb + hb { step("CHECK", format!("gate {}", move_key(m)), serde_json::json!("no gain on the current program (already applied or superseded)")); continue; }
            let trial = apply(&prog, m);
            writeln!(hist, "{}", serde_json::json!({"ts": now(), "sig": sig, "move": move_key(m), "term_name": term_name(m), "from": prog_sha(&prog), "to": prog_sha(&trial), "sets": ysha, "replay": [rb, r1], "holdout": [hb, h1], "status": "PROPOSED", "gate": "every agreeing box kept, replay + holdout", "by": "core v2 causal screen"})).ok();
            writeln!(book, "{}", serde_json::json!({"sig": sig, "point": m.point, "ctx": m.ctx, "term": m.term, "w": m.w, "gain": (r1 + h1) - (rb + hb), "tries": 1, "term_name": term_name(m)})).ok();
            step("CHECK", format!("gate {}", move_key(m)), serde_json::json!(format!("pass: replay {rb}→{r1} holdout {hb}→{h1}")));
            step("EMIT", "commit (PROPOSED)".into(), serde_json::json!(move_key(m)));
            eprintln!("  solved   {sig:40} ({} inst) {}[{}].{}={} direct {} : replay +{} holdout +{}", inst.len(), m.point, m.ctx, term_name(m), m.w, x.1, r1 - rb, h1 - hb);
            prog = trial; base = newb; changed_since_screen = true; solved += 1;
            Program::from_map(prog.clone()).save(&prog_path);
            done = true; break;
        }
        if !done {
            let lossy: Vec<_> = rows.iter().filter(|x| x.1 > 0 && x.3 > 0).take(3).collect();
            let conf: Vec<_> = rows.iter().filter(|x| x.1 == 0 && x.2 > 0).take(3).collect();
            let partial: Vec<_> = rows.iter().filter(|x| x.1 == 0 && x.2 == 0).take(3).collect();
            for x in &lossy {
                let (kp, kn) = runs[x.0].pages.iter().find(|(_, v)| !v.1.is_empty()).map(|(p, v)| (name(p), v.1[0])).unwrap();
                writeln!(quar, "{}", serde_json::json!({"ts": now(), "sig": sig, "key": move_key(&runs[x.0].m), "program": prog_sha(&prog), "term_name": term_name(&runs[x.0].m), "direct_fixes": x.1, "lost_boxes": x.3, "killing_page": kp, "killing_node": kn, "sets": ysha, "next": "SAT condition station"})).ok();
            }
            let why = if !lossy.is_empty() { format!("fix exists but loses boxes: {}", lossy.iter().map(|x| format!("{} (+{} −{})", move_key(&runs[x.0].m), x.1, x.3)).collect::<Vec<_>>().join(" · ")) }
                else if !conf.is_empty() { format!("confounded: fixes only by moving the parent ({}) — climb", conf.iter().map(|x| move_key(&runs[x.0].m)).collect::<Vec<_>>().join(", ")) }
                else if !partial.is_empty() { format!("partial: closes {:.0}% of the error ({}) — two-move search (DEBUG station)", partial[0].4 * 100.0, partial.iter().map(|x| move_key(&runs[x.0].m)).collect::<Vec<_>>().join(", ")) }
                else { "no knob moves these boxes".into() };
            step("ABSTAIN", why.clone(), serde_json::json!(null));
            let mut card = abstain_card(sig, &inst.iter().map(|x| (x.0.clone(), x.1, x.4)).collect::<Vec<_>>());
            card["screen"] = serde_json::json!(why);
            writeln!(open_f, "{}", card).ok();
            eprintln!("  open     {sig:40} ({} inst): {why}", inst.len());
        }
        writeln!(tr, "{}", serde_json::json!({"sig": sig, "program": prog_sha(&prog), "steps": t})).ok();
    }
    let (r1, h1) = (count(&base, &replay), count(&base, &holdout));
    eprintln!("core v2: {solved} solved of {} · {} interventions + {gates_run} re-gates · replay {r0} → {r1} · holdout {h0} → {h1} · {:.1}s", queue.len(), runs.len(), t0.elapsed().as_secs_f64());
}

// ── M2–M5: the firing core. One pool of workers; every task has a TYPE (station); in the same tick different types run
// on different threads for different problems (MIMD), and screens run lanes inside (M1). Replicas per type follow queue
// depth (PIPE-2 rule: pick the type with the deepest queue per active worker). The governor yields to other load (CPU
// training): a worker waits while the 1-minute load average is above the cap. Cycles repeat while a cycle still solves.
type Job = Box<dyn FnOnce(&Shared) + Send>;
type Inst = (PathBuf, u32, u32, usize, f32);

struct FireRun { m: Move, extra: Vec<Move>, ver: usize, pages: HashMap<PathBuf, (Vec<u32>, Vec<u32>, HashMap<u32, [f32; 4]>)> }

struct Shared {
    pages: Vec<PathBuf>, replay: Vec<PathBuf>, holdout: Vec<PathBuf>, stem: String, ysha: String, topk: usize, max_problems: usize, load_cap: f32,
    prog: std::sync::RwLock<(Prog, usize)>,
    base: std::sync::RwLock<HashMap<PathBuf, HashSet<u32>>>,
    bdiff: std::sync::Mutex<HashMap<PathBuf, HashMap<u32, [f32; 4]>>>,
    led: std::sync::Mutex<HashMap<PathBuf, HashMap<u32, HashMap<&'static str, i32>>>>,
    problems: std::sync::Mutex<HashMap<String, Vec<Inst>>>,
    watch_nodes: std::sync::RwLock<HashMap<PathBuf, HashSet<u32>>>,
    grand: std::sync::Mutex<HashMap<(PathBuf, u32), (u32, String)>>, // origin box → grandparent (climb)
    book: std::sync::RwLock<(Vec<u8>, Vec<Move>)>,           // sifted codebook: rows (n × hv::D phases) + the fix each row recalls
    phv: std::sync::Mutex<HashMap<String, Vec<u8>>>,          // problem → its hypervector
    fired: std::sync::Mutex<HashMap<(String, String), HashSet<PathBuf>>>, // (term key, owner display) → pages where it fired (demand index)
    incr: std::sync::Mutex<(usize, usize, usize)>,            // pages re-laid out, pages skipped, incremental≠full mismatches (OMR_INCR_CHECK)
    sieve: std::sync::Mutex<(HashSet<(String, String)>, usize)>, // (sig, move) suggested by recall; commits that came from recall
    runs: std::sync::Mutex<HashMap<String, std::sync::Arc<FireRun>>>,
    waiters: std::sync::Mutex<HashMap<String, Vec<String>>>,
    pending: std::sync::Mutex<HashMap<String, usize>>,
    baseline_left: std::sync::atomic::AtomicUsize,
    commit: std::sync::Mutex<(usize, usize, Vec<String>)>, // solved, re-gates, log lines
    watches: std::sync::Mutex<Vec<serde_json::Value>>,
    queues: std::sync::Mutex<(HashMap<&'static str, std::collections::VecDeque<Job>>, HashMap<&'static str, usize>)>,
    outstanding: std::sync::atomic::AtomicUsize,
    files: std::sync::Mutex<HashMap<&'static str, std::fs::File>>,
}

impl Shared {
    fn push(&self, kind: &'static str, j: Job) {
        self.outstanding.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.queues.lock().unwrap().0.entry(kind).or_default().push_back(j);
    }
    fn write(&self, f: &'static str, v: serde_json::Value) { use std::io::Write; if let Some(fh) = self.files.lock().unwrap().get_mut(f) { writeln!(fh, "{v}").ok(); } }
    fn use_prog(&self) -> (Prog, usize) { let g = self.prog.read().unwrap(); (g.0.clone(), g.1) }
}

fn load1() -> f32 { std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next()?.parse().ok()).unwrap_or(0.0) }

/// Watch tasks (M3): the doctor code that fires beside repair — layout time per page and the check rings (R0–R3).
fn watch_page(p: &Path, rings: bool) -> Option<serde_json::Value> {
    let t = std::time::Instant::now();
    let b = crate::page::Bundle::open(p)?;
    let l = crate::page::load(&b, (1280.0, 800.0));
    let fonts = crate::font::Fonts::new();
    for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
    let dref = &l.dom; let base = b.url.clone();
    let sizes = |n: u32| -> Option<(u32, u32)> {
        if dref.tag(n) == "svg" { return crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
        crate::img_bytes(dref, n, &base, &b).and_then(|bytes| crate::img::size(&bytes))
    };
    let t1 = std::time::Instant::now();
    let mut lay = crate::layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
    lay.run();
    let lay_ms = t1.elapsed().as_secs_f64() * 1e3;
    if !rings { return Some(serde_json::json!({"watch": "perf", "page": name(p), "load_ms": (t1 - t).as_secs_f64() * 1e3, "layout_ms": lay_ms, "boxes": lay.out.boxes.len(), "items": lay.out.items.len()})); }
    let f = crate::checks::run(&l.dom, &l.styled, &lay.out.boxes);
    let mut by: HashMap<&str, usize> = HashMap::new(); for x in &f { *by.entry(x.ring).or_default() += 1; }
    Some(serde_json::json!({"watch": "rings", "page": name(p), "findings": f.len(), "by_ring": by}))
}

fn t_baseline(sh: &Shared, p: PathBuf) {
    set_program(Program::from_map(sh.use_prog().0));
    crate::layout::ledger_start(); let f = score_full(&p); let lg = crate::layout::ledger_take();
    if let Some(c) = cached(&p) {
        let mut fi = sh.fired.lock().unwrap();
        for (n, k, _) in &lg { let d = crate::layout::node_feature(&c.l.dom, &c.l.styled, *n, "disp"); fi.entry((k.to_string(), d)).or_default().insert(p.clone()); fi.entry((k.to_string(), "*".into())).or_default().insert(p.clone()); }
    }
    { let mut led = sh.led.lock().unwrap(); let m = led.entry(p.clone()).or_default(); for (n, k, v) in lg { *m.entry(n).or_default().entry(k).or_insert(0) += v; } }
    if let Some(f) = f {
        sh.base.write().unwrap().insert(p.clone(), f.agree); sh.bdiff.lock().unwrap().insert(p.clone(), f.diff);
        if sh.replay.contains(&p) { let mut pr = sh.problems.lock().unwrap(); let mut gr = sh.grand.lock().unwrap(); for o in f.origins { gr.insert((p.clone(), o.n), (o.gp, o.gpd.clone())); pr.entry(o.sig).or_default().push((p.clone(), o.n, o.parent, o.axis, o.err)); } }
    }
    if sh.baseline_left.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 { sh.push("cluster", Box::new(t_cluster)); }
}

fn t_cluster(sh: &Shared) {
    let mut q: Vec<(String, usize)> = sh.problems.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.len())).collect();
    q.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))); q.truncate(sh.max_problems);
    { let pr = sh.problems.lock().unwrap(); let mut w = sh.watch_nodes.write().unwrap();
      for (s, _) in &q { for (p, n, par, _, _) in &pr[s] { let e = w.entry(p.clone()).or_default(); e.insert(*n); e.insert(*par); } } }
    for (s, _) in q { sh.push("predict", Box::new(move |sh: &Shared| t_predict(sh, s))); }
}

fn t_predict(sh: &Shared, sig: String) {
    let inst = sh.problems.lock().unwrap()[&sig].clone();
    let parts: Vec<&str> = sig.split('|').collect();
    let mut cand: HashMap<(String, String, i8), (usize, f32)> = HashMap::new();
    { let led = sh.led.lock().unwrap();
      for (p, n, par, _, e) in &inst { let Some(pm) = led.get(p) else { continue };
        for (node, ctx, own) in [(*n, parts[1], true), (*par, parts[2], false)] { let Some(terms) = pm.get(&node) else { continue };
          for (k, &v) in terms { if v == 0 { continue; } for w in [0i8, -1] {
            let after = e - (v as f32 / 64.0) * if k.starts_with("line.") { 1.0 } else { (1 - w) as f32 }; // max-plus: marginal
            let c = cand.entry((format!("k:{k}"), ctx.to_string(), w)).or_insert((0, 0.0));
            if own && after.abs() <= 2.0 { c.0 += 1; } c.1 += if own { (e.abs() - after.abs()).max(0.0) } else { 0.01 }; } } } } }
    let mut v: Vec<_> = cand.into_iter().collect();
    v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.partial_cmp(&a.1 .1).unwrap()).then(a.0.cmp(&b.0)));
    v.truncate(sh.topk);
    // SIEVE by resonance: the problem as a hypervector (signature, owner features, error) sifted against every past fix
    { let (p0, n0, _, _, _) = &inst[0]; let mut errs: Vec<f32> = inst.iter().map(|x| x.4).collect(); errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
      let feats: Vec<(String, String)> = page_feats(p0, &[*n0].into_iter().collect()).remove(n0).map(|m| { let mut v: Vec<(String, String)> = m.into_iter().map(|(k, x)| (k.to_string(), x)).collect(); v.sort(); v }).unwrap_or_default();
      let q = crate::hv::problem(&sig, &feats, errs[errs.len() / 2]);
      let bk = sh.book.read().unwrap();
      if !bk.1.is_empty() {
          let sc = crate::hv::sift(&bk.0, &q); let full = (crate::hv::D as i64 * 127 * 127) as f32;
          let mut idx: Vec<usize> = (0..sc.len()).collect(); idx.sort_by_key(|&i| std::cmp::Reverse(sc[i]));
          let mut sv = sh.sieve.lock().unwrap();
          for &i in idx.iter().take(5) { if (sc[i] as f32) < 0.30 * full { break; }
              let m = &bk.1[i]; let c = (m.point.clone(), m.ctx.clone(), m.w);
              if !v.iter().any(|x| x.0 == c) { v.insert(0, (c, (0, 0.0))); sv.0.insert((sig.clone(), move_key(m))); } } }
      sh.phv.lock().unwrap().insert(sig.clone(), q); }
    // rule switches carry no magnitude to predict from: every rule that fired on these boxes (or their parents) is tried off
    { let led = sh.led.lock().unwrap(); let mut extra: Vec<((String, String, i8), (usize, f32))> = Vec::new();
      for (p, n, par, _, _) in &inst { let Some(pm) = led.get(p) else { continue };
        for (node, ctx) in [(*n, parts[1]), (*par, parts[2])] { if let Some(t) = pm.get(&node) { for k in t.keys().filter(|k| k.starts_with("rule.")) {
            let c = (format!("k:{k}"), ctx.to_string(), 0i8); if !v.iter().any(|x| x.0 == c) && !extra.iter().any(|x| x.0 == c) { extra.push((c, (0, 0.0))); } } } } }
      // climb: the grandparent's terms and rules (a table cell's width is decided by its table, two levels up)
      let gr = sh.grand.lock().unwrap();
      for (p, n, _, _, _) in &inst { let Some((gp, gpd)) = gr.get(&(p.clone(), *n)) else { continue }; if gpd.is_empty() { continue; }
        let Some(pm) = led.get(p) else { continue }; let Some(t) = pm.get(gp) else { continue };
        for (k, &val) in t { if val == 0 { continue; } for w in [0i8, -1] { if k.starts_with("rule.") && w == -1 { continue; }
            let c = (format!("k:{k}"), gpd.clone(), w); if !v.iter().any(|x| x.0 == c) && !extra.iter().any(|x| x.0 == c) { extra.push((c, (0, 0.0))); } } } }
      extra.sort_by(|a, b| a.0.cmp(&b.0)); v.extend(extra.into_iter().take(10)); }
    let mut need = 0;
    for ((pt, ctx, w), _) in v {
        let m = Move { point: pt, ctx, term: 0, w }; let key = move_key(&m);
        if sh.runs.lock().unwrap().contains_key(&key) { continue; }
        let mut wt = sh.waiters.lock().unwrap();
        need += 1;
        if let Some(list) = wt.get_mut(&key) { list.push(sig.clone()); } else { wt.insert(key, vec![sig.clone()]); sh.push("screen", Box::new(move |sh: &Shared| t_screen(sh, m))); }
    }
    if need == 0 { sh.push("decide", Box::new(move |sh: &Shared| t_decide(sh, sig))); } else { sh.pending.lock().unwrap().insert(sig, need); }
}

fn t_screen(sh: &Shared, m: Move) {
    let r = screen_move(sh, &m);
    let key = move_key(&m);
    sh.runs.lock().unwrap().insert(key.clone(), std::sync::Arc::new(r));
    let ws = sh.waiters.lock().unwrap().remove(&key).unwrap_or_default();
    for s in ws { let mut pd = sh.pending.lock().unwrap(); let c = pd.get_mut(&s).unwrap(); *c -= 1; if *c == 0 { pd.remove(&s); drop(pd); sh.push("decide", Box::new(move |sh: &Shared| t_decide(sh, s))); } }
}

fn screen_move(sh: &Shared, m: &Move) -> FireRun {
    screen_moves(sh, m, &[])
}

/// One intervention that sets `m` and every move in `extra` together (a compound fix: the DEBUG station's two-edit case).
fn screen_moves(sh: &Shared, m: &Move, extra: &[Move]) -> FireRun {
    let m = m.clone();
    let (prog, ver) = sh.use_prog();
    let all = extra.iter().fold(apply(&prog, &m), |p, x| apply(&p, x));
    set_program(Program::from_map(all));
    let base = sh.base.read().unwrap().clone(); let watch = sh.watch_nodes.read().unwrap().clone();
    let mut r = FireRun { m: m.clone(), extra: extra.to_vec(), ver, pages: HashMap::new() };
    // demand (Adapton-style dirty marking, clean-room from the published idea): a knob can only change a page where its term
    // was evaluated on a box of the knob's context; every other page keeps its baseline result without being laid out
    let need: Option<HashSet<PathBuf>> = { let fi = sh.fired.lock().unwrap(); let mut acc = HashSet::new(); let mut all = false;
        for mv in std::iter::once(&m).chain(extra.iter()) {
            match mv.point.strip_prefix("k:") { Some(key) => { let base = mv.ctx.split('&').next().unwrap_or("*").trim_end_matches("@q");
                    if let Some(ps) = fi.get(&(key.to_string(), base.to_string())) { acc.extend(ps.iter().cloned()); } }
                None => all = true }
        }
        if all { None } else { Some(acc) } };
    let check = std::env::var("OMR_INCR_CHECK").is_ok();
    let bd = sh.bdiff.lock().unwrap().clone();
    for p in &sh.pages {
        if let Some(nd) = &need { if !nd.contains(p) {
            sh.incr.lock().unwrap().1 += 1;
            let w: HashMap<u32, [f32; 4]> = watch.get(p).map(|ws| ws.iter().filter_map(|n| bd.get(p).and_then(|b| b.get(n)).map(|d| (*n, *d))).collect()).unwrap_or_default();
            if check { if let Some(f) = score_full(p) { let b = &base[p]; if f.agree != *b { sh.incr.lock().unwrap().2 += 1; eprintln!("  INCR MISMATCH {} on {}", move_key(&m), name(p)); } } }
            r.pages.insert(p.clone(), (Vec::new(), Vec::new(), w)); continue;
        } }
        sh.incr.lock().unwrap().0 += 1;
        let wset = std::rc::Rc::new(watch.get(p).cloned().unwrap_or_default());
        let Some(f) = score_lean(p, wset) else { continue }; let b = &base[p];
        let w: HashMap<u32, [f32; 4]> = watch.get(p).map(|ws| ws.iter().filter_map(|n| f.diff.get(n).map(|d| (*n, *d))).collect()).unwrap_or_default();
        r.pages.insert(p.clone(), (f.agree.difference(b).copied().collect(), b.difference(&f.agree).copied().collect(), w)); }
    r
}

fn t_decide(sh: &Shared, sig: String) {
    let inst = sh.problems.lock().unwrap()[&sig].clone();
    let bdiff = sh.bdiff.lock().unwrap().clone();
    let runs: Vec<std::sync::Arc<FireRun>> = sh.runs.lock().unwrap().values().cloned().collect();
    // CAUSAL: direct fixes (parent unmoved), confounded (parent moved), lost boxes, share of error closed
    let mut rows: Vec<(std::sync::Arc<FireRun>, usize, usize, usize, f32)> = Vec::new();
    for r in &runs { let (mut d, mut c, mut red) = (0, 0, 0.0f32);
        for (p, n, par, ax, e) in &inst { let Some((g, _, w)) = r.pages.get(p) else { continue };
            let pm = match (w.get(par), bdiff.get(p).and_then(|b| b.get(par))) { (Some(x), Some(y)) => x.iter().zip(y).any(|(a, b)| (a - b).abs() > 0.5), _ => false };
            if g.contains(n) { if pm { c += 1 } else { d += 1 } }
            let nd = w.get(n).map(|x| x[*ax]).unwrap_or(*e); if e.abs() > 0.0 { red += ((e.abs() - nd.abs()) / e.abs()).clamp(-1.0, 1.0); } }
        let lost: usize = r.pages.values().map(|x| x.1.len()).sum();
        if d > 0 || c > 0 || red > 0.25 * inst.len() as f32 { rows.push((r.clone(), d, c, lost, red / inst.len() as f32)); } }
    rows.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then(a.3.cmp(&b.3)).then(b.4.partial_cmp(&a.4).unwrap()).then(move_key(&a.0.m).cmp(&move_key(&b.0.m))));
    let mut trace = vec![serde_json::json!({"op": "GOAL", "a": format!("{} origin boxes of {sig}", inst.len())})];
    // a fix through the parent (confounded = the cause sits in the parent's term, which is where the knob acts) commits too
    for x in rows.iter().filter(|x| x.1 + x.2 > 0 && x.3 == 0).take(3) {
        match try_commit(sh, &sig, inst.len(), &x.0, x.1 + x.2, &mut trace) { Some(true) => { sh.write("trace", serde_json::json!({"sig": sig, "steps": trace})); return; } _ => continue }
    }
    // SAT station (sat.dpll): the best fix that loses boxes gets a condition that separates its fixed boxes from its broken ones
    if let Some(x) = rows.iter().filter(|x| x.1 + x.2 > 0 && x.3 > 0 && x.0.m.point.starts_with("k:")).max_by_key(|x| (x.1 + x.2, std::cmp::Reverse(x.3))) {
        let (s2, m2, d) = (sig.clone(), x.0.m.clone(), x.1 + x.2);
        trace.push(serde_json::json!({"op": "STEP", "a": "hand the lossy fix to the SAT station", "r": format!("{} (+{d} −{})", move_key(&m2), x.3)}));
        sh.push("sat", Box::new(move |sh: &Shared| t_sat(sh, s2, m2)));
    }
    t_decide_rest(sh, sig, inst, rows, trace);
}

/// GATE + COMMIT under the commit lock. Some(true) = committed, Some(false) = gate refused, None = no gain.
fn try_commit(sh: &Shared, sig: &str, ninst: usize, run: &FireRun, direct: usize, trace: &mut Vec<serde_json::Value>) -> Option<bool> {
    let x = (run, direct);
    {
        let m = &x.0.m;
        // GATE + COMMIT under the commit lock (one program writer); re-gate when the program moved since the screen
        let mut cl = sh.commit.lock().unwrap();
        let (prog, ver) = sh.use_prog();
        let base = sh.base.read().unwrap().clone();
        let mut newb = base.clone(); let mut killer = None;
        let with_all = |p: &Prog| x.0.extra.iter().fold(apply(p, m), |q, e| apply(&q, e));
        if ver != x.0.ver { cl.1 += 1; set_program(Program::from_map(with_all(&prog)));
            for p in &sh.pages { if let Some(f) = score_full(p) { if let Some(&k) = base[p].difference(&f.agree).next() { killer = Some((name(p), k)); break; } newb.insert(p.clone(), f.agree); } } }
        else { for (p, (g, _, _)) in &x.0.pages { newb.get_mut(p).unwrap().extend(g.iter().copied()); } }
        let cnt = |b: &HashMap<PathBuf, HashSet<u32>>, ps: &[PathBuf]| -> usize { ps.iter().filter_map(|p| b.get(p)).map(|s| s.len()).sum() };
        let (rb, hb, r1, h1) = (cnt(&base, &sh.replay), cnt(&base, &sh.holdout), cnt(&newb, &sh.replay), cnt(&newb, &sh.holdout));
        if let Some((kp, kn)) = killer { sh.write("quar", serde_json::json!({"ts": now(), "sig": sig, "key": move_key(m), "program": prog_sha(&prog), "reason": "re-gate: an agreeing box stopped agreeing", "killing_page": kp, "killing_node": kn, "sets": sh.ysha})); trace.push(serde_json::json!({"op": "CHECK", "a": move_key(m), "r": "fail"})); return Some(false); }
        if r1 + h1 <= rb + hb { trace.push(serde_json::json!({"op": "CHECK", "a": move_key(m), "r": "no gain on the current program"})); return None; }
        let trial = with_all(&prog);
        sh.write("hist", serde_json::json!({"ts": now(), "sig": sig, "move": move_key(m), "from": prog_sha(&prog), "to": prog_sha(&trial), "sets": sh.ysha, "replay": [rb, r1], "holdout": [hb, h1], "status": "PROPOSED", "gate": "every agreeing box kept, replay + holdout", "by": "core fire"}));
        let hvx = sh.phv.lock().unwrap().get(sig).map(|v| crate::hv::to_hex(v)).unwrap_or_default();
        { let mut sv = sh.sieve.lock().unwrap(); if sv.0.contains(&(sig.to_string(), move_key(m))) { sv.1 += 1; } }
        sh.write("book", serde_json::json!({"sig": sig, "point": m.point, "ctx": m.ctx, "term": m.term, "w": m.w, "gain": (r1 + h1) - (rb + hb), "tries": 1, "term_name": term_name(m), "hv": hvx}));
        if let Some(h) = crate::hv::from_hex(&hvx) { let mut bk = sh.book.write().unwrap(); bk.0.extend(h); bk.1.push(m.clone()); }
        Program::from_map(trial.clone()).save(&format!("{}.json", sh.stem));
        *sh.prog.write().unwrap() = (trial, ver + 1); *sh.base.write().unwrap() = newb;
        let mk = std::iter::once(move_key(m)).chain(x.0.extra.iter().map(move_key)).collect::<Vec<_>>().join(" + ");
        cl.0 += 1; cl.2.push(format!("  solved   {sig:40} ({} inst) {mk} direct {}: replay +{} holdout +{}", ninst, x.1, r1 - rb, h1 - hb));
        trace.push(serde_json::json!({"op": "EMIT", "a": "commit (PROPOSED)", "r": move_key(m)}));
        Some(true)
    }
}

fn t_decide_rest(sh: &Shared, sig: String, inst: Vec<Inst>, rows: Vec<(std::sync::Arc<FireRun>, usize, usize, usize, f32)>, mut trace: Vec<serde_json::Value>) {
    let lossy: Vec<_> = rows.iter().filter(|x| x.1 > 0 && x.3 > 0).take(3).collect();
    let conf: Vec<_> = rows.iter().filter(|x| x.1 == 0 && x.2 > 0).take(3).collect();
    let part: Vec<_> = rows.iter().filter(|x| x.1 == 0 && x.2 == 0).take(3).collect();
    { let s3 = sig.clone(); sh.push("solve", Box::new(move |sh: &Shared| t_solve(sh, s3))); }
    if part.len() >= 2 && lossy.is_empty() {
        let ms: Vec<Move> = part.iter().map(|x| x.0.m.clone()).collect(); let s2 = sig.clone();
        sh.push("debug", Box::new(move |sh: &Shared| t_debug(sh, s2, ms)));
    }
    let why = if !lossy.is_empty() { format!("fix loses boxes: {}", lossy.iter().map(|x| format!("{} (+{} −{})", move_key(&x.0.m), x.1, x.3)).collect::<Vec<_>>().join(" · ")) }
        else if !conf.is_empty() { format!("confounded — climb: {}", conf.iter().map(|x| move_key(&x.0.m)).collect::<Vec<_>>().join(", ")) }
        else if !part.is_empty() { format!("partial {:.0}% — DEBUG pairs: {}", part[0].4 * 100.0, part.iter().map(|x| move_key(&x.0.m)).collect::<Vec<_>>().join(", ")) }
        else { "no knob moves these boxes".into() };
    trace.push(serde_json::json!({"op": "ABSTAIN", "a": why}));
    sh.write("trace", serde_json::json!({"sig": sig, "steps": trace}));
    sh.write("open", serde_json::json!({"sig": sig, "instances": inst.len(), "screen": why}));
    sh.commit.lock().unwrap().2.push(format!("  open     {sig:40} ({} inst): {why}", inst.len()));
}

/// `omni-render core corpus --fire [--program P] [--workers 3] [--topk 6] [--cycles 3] [--load-cap L]`
pub fn run_fire(args: &[String]) {
    let dir = args.first().cloned().unwrap_or_else(|| "corpus".into());
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let prog_path = arg("--program").unwrap_or_else(|| "omr.program.json".into());
    let workers: usize = arg("--workers").and_then(|v| v.parse().ok()).unwrap_or(3);
    let cycles: usize = arg("--cycles").and_then(|v| v.parse().ok()).unwrap_or(3);
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as f32;
    let load_cap: f32 = arg("--load-cap").and_then(|v| v.parse().ok()).unwrap_or(ncpu * 1.5);
    let stem = prog_path.trim_end_matches(".json").to_string();
    let (replay, holdout, ysha) = frozen_sets(&dir, &format!("{stem}.sets.json"));
    let pages: Vec<PathBuf> = replay.iter().chain(&holdout).cloned().collect();
    let app = |path: String| std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for cyc in 1..=cycles {
        let t0 = std::time::Instant::now();
        let mut files = HashMap::new();
        for (k, f) in [("book", "codebook"), ("quar", "quarantine"), ("hist", "history"), ("trace", "traces"), ("fire", "fire")] { files.insert(k, app(format!("{stem}.{f}.jsonl"))); }
        files.insert("open", std::fs::File::create(format!("{stem}.open.jsonl")).unwrap());
        let sh = Shared { pages: pages.clone(), replay: replay.clone(), holdout: holdout.clone(), stem: stem.clone(), ysha: ysha.clone(),
            topk: arg("--topk").and_then(|v| v.parse().ok()).unwrap_or(6), max_problems: arg("--max").and_then(|v| v.parse().ok()).unwrap_or(63), load_cap,
            prog: std::sync::RwLock::new((Program::load(&prog_path).map(), 0)), base: Default::default(), bdiff: Default::default(), led: Default::default(),
            problems: Default::default(), watch_nodes: Default::default(), grand: Default::default(), book: std::sync::RwLock::new(load_book(&stem)), phv: Default::default(), fired: Default::default(), incr: Default::default(), sieve: Default::default(), runs: Default::default(), waiters: Default::default(), pending: Default::default(),
            baseline_left: std::sync::atomic::AtomicUsize::new(pages.len()), commit: Default::default(), watches: Default::default(),
            queues: Default::default(), outstanding: std::sync::atomic::AtomicUsize::new(0), files: std::sync::Mutex::new(files) };
        let r0 = Program::load(&prog_path).map();
        for p in &pages { let q = p.clone(); sh.push("baseline", Box::new(move |sh: &Shared| t_baseline(sh, q)));
            let q = p.clone(); sh.push("watch.perf", Box::new(move |sh: &Shared| { set_program(Program::from_map(sh.use_prog().0)); if let Some(v) = watch_page(&q, false) { sh.watches.lock().unwrap().push(v); } }));
            let q = p.clone(); sh.push("watch.rings", Box::new(move |sh: &Shared| { set_program(Program::from_map(sh.use_prog().0)); if let Some(v) = watch_page(&q, true) { sh.watches.lock().unwrap().push(v); } })); }
        let timeline: std::sync::Mutex<Vec<Vec<(&'static str, usize)>>> = Default::default();
        let done = std::sync::atomic::AtomicBool::new(false);
        let yielded = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|sc| {
            sc.spawn(|| while !done.load(std::sync::atomic::Ordering::SeqCst) {
                let a: Vec<(&'static str, usize)> = sh.queues.lock().unwrap().1.iter().filter(|x| *x.1 > 0).map(|(k, v)| (*k, *v)).collect();
                if !a.is_empty() { timeline.lock().unwrap().push(a); }
                std::thread::sleep(std::time::Duration::from_millis(100));
            });
            for _ in 0..workers { sc.spawn(|| loop {
                if load1() > sh.load_cap { yielded.fetch_add(1, std::sync::atomic::Ordering::Relaxed); std::thread::sleep(std::time::Duration::from_millis(500)); continue; }
                let job = { let mut q = sh.queues.lock().unwrap(); let (qs, act) = &mut *q;
                    // pipeline rule: later stations drain first (a ready decide/SAT never waits behind screens), each type's share
                    // grows with its queue depth per active worker (PIPE-2 elastic replicas); watches fill idle slots
                    let stage = |k: &str| -> f32 { match k { "decide" | "sat" | "debug" | "solve" => 16.0, "cluster" => 32.0, "predict" => 8.0, "screen" => 1.0, "baseline" => 2.0, _ => 0.5 } };
                    let pick = qs.iter().filter(|(_, v)| !v.is_empty()).max_by(|a, b| { let fa = stage(a.0) * a.1.len() as f32 / (1 + act.get(a.0).copied().unwrap_or(0)) as f32; let fb = stage(b.0) * b.1.len() as f32 / (1 + act.get(b.0).copied().unwrap_or(0)) as f32; fa.partial_cmp(&fb).unwrap().then(b.0.cmp(a.0)) }).map(|(k, _)| *k);
                    pick.map(|k| { *act.entry(k).or_default() += 1; (k, qs.get_mut(k).unwrap().pop_front().unwrap()) }) };
                match job {
                    Some((k, j)) => { j(&sh); *sh.queues.lock().unwrap().1.get_mut(k).unwrap() -= 1; sh.outstanding.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); }
                    None => { if sh.outstanding.load(std::sync::atomic::Ordering::SeqCst) == 0 { break; } std::thread::sleep(std::time::Duration::from_millis(10)); }
                }
            }); }
            while sh.outstanding.load(std::sync::atomic::Ordering::SeqCst) > 0 { std::thread::sleep(std::time::Duration::from_millis(50)); }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let tl = timeline.into_inner().unwrap();
        let kinds_max = tl.iter().map(|a| a.len()).max().unwrap_or(0);
        let mixed = tl.iter().filter(|a| a.len() >= 2).count();
        let mut type_ticks: HashMap<&str, usize> = HashMap::new(); for a in &tl { for (k, _) in a { *type_ticks.entry(k).or_default() += 1; } }
        let (solved, regates, lines) = sh.commit.into_inner().unwrap();
        let mut lines = lines; lines.sort();
        for l in &lines { eprintln!("{l}"); }
        let w = sh.watches.into_inner().unwrap();
        let lay_ms: f64 = w.iter().filter_map(|v| v["layout_ms"].as_f64()).sum(); let rings: u64 = w.iter().filter_map(|v| v["findings"].as_u64()).sum();
        let hm = harmony(&pages, &sh.led.lock().unwrap(), &Program::load(&prog_path));
        eprintln!("  harmony: {hm}");
        let base = sh.base.into_inner().unwrap(); let cnt = |ps: &[PathBuf]| -> usize { ps.iter().filter_map(|p| base.get(p)).map(|s| s.len()).sum() };
        let incr_json = { let x = sh.incr.lock().unwrap(); serde_json::json!({"laid_out": x.0, "skipped": x.1, "mismatches": x.2}) };
        let summary = serde_json::json!({"cycle": cyc, "solved": solved, "regates": regates, "screens": sh.runs.into_inner().unwrap().len(), "replay": cnt(&replay), "holdout": cnt(&holdout),
            "secs": t0.elapsed().as_secs_f64(), "ticks": tl.len(), "max_types_same_tick": kinds_max, "ticks_with_2plus_types": mixed, "type_ticks": type_ticks,
            "watch": {"pages": w.len() / 2, "layout_ms_total": lay_ms, "ring_findings": rings}, "governor_yields": yielded.into_inner(), "incremental": incr_json, "harmony": {"clauses": hm["clauses"], "conflicts": hm["conflicts"], "dead": hm["dead"].as_array().map_or(0, |a| a.len())}, "sieve": {"suggested": sh.sieve.lock().unwrap().0.len(), "solved_from_recall": sh.sieve.lock().unwrap().1, "book_rows": sh.book.read().unwrap().1.len()}, "program_changed": prog_sha(&r0) != prog_sha(&Program::load(&prog_path).map())});
        eprintln!("core fire cycle {cyc}: {summary}");
        { let mut f = app(format!("{stem}.fire.jsonl")); use std::io::Write; writeln!(f, "{summary}").ok(); }
        if solved == 0 { break; }
    }
}

/// Features of the given nodes on one page (the same `node_feature` the engine evaluates conditions with).
fn page_feats(p: &Path, nodes: &HashSet<u32>) -> HashMap<u32, HashMap<&'static str, String>> {
    let Some(b) = crate::page::Bundle::open(p) else { return HashMap::new() };
    let l = crate::page::load(&b, (1280.0, 800.0));
    nodes.iter().map(|&n| (n, crate::layout::FEATURES.iter().map(|f| (*f, crate::layout::node_feature(&l.dom, &l.styled, n, f))).collect())).collect()
}

/// The smallest conjunction (≤ 3 literals) true on every positive and false on every negative — a set cover solved by DPLL
/// with most-constrained-first branching (ported from sat.dpll's search; the evolved MOMS-style order: try the literal
/// that kills the most negatives first). Literals: `f=v` shared by all positives, `f!=v` for values only negatives have.
fn sat_condition(pos: &[HashMap<&'static str, String>], neg: &[HashMap<&'static str, String>]) -> Option<Vec<String>> {
    if pos.is_empty() { return None; }
    let mut lits: Vec<(String, Vec<bool>)> = Vec::new(); // literal, kills[neg i]
    for f in crate::layout::FEATURES {
        let pv: HashSet<&String> = pos.iter().map(|x| &x[f]).collect();
        if pv.len() == 1 { let v = *pv.iter().next().unwrap(); let k: Vec<bool> = neg.iter().map(|x| &x[f] != v).collect(); if k.iter().any(|&b| b) { lits.push((format!("{f}={v}"), k)); } }
        let nv: HashSet<&String> = neg.iter().map(|x| &x[f]).filter(|v| !pv.contains(v)).collect();
        for v in nv { let k: Vec<bool> = neg.iter().map(|x| &x[f] == v).collect(); lits.push((format!("{f}!={v}"), k)); }
    }
    fn dpll(lits: &[(String, Vec<bool>)], alive: &[usize], depth: usize, chosen: &mut Vec<usize>) -> bool {
        if alive.is_empty() { return true; }
        if depth == 0 { return false; }
        // most constrained negative first: the one the fewest literals kill
        let pick = *alive.iter().min_by_key(|&&i| lits.iter().filter(|l| l.1[i]).count()).unwrap();
        let mut opts: Vec<usize> = (0..lits.len()).filter(|&j| lits[j].1[pick]).collect();
        opts.sort_by_key(|&j| std::cmp::Reverse(alive.iter().filter(|&&i| lits[j].1[i]).count()));
        for j in opts {
            chosen.push(j);
            let rest: Vec<usize> = alive.iter().copied().filter(|&i| !lits[j].1[i]).collect();
            if dpll(lits, &rest, depth - 1, chosen) { return true; }
            chosen.pop();
        }
        false
    }
    let alive: Vec<usize> = (0..neg.len()).collect();
    for depth in 1..=3 { let mut ch = Vec::new(); if dpll(&lits, &alive, depth, &mut ch) { let mut v: Vec<String> = ch.iter().map(|&j| lits[j].0.clone()).collect(); v.sort(); return Some(v); } }
    None
}

fn t_sat(sh: &Shared, sig: String, m: Move) {
    let key = m.point.trim_start_matches("k:").to_string();
    let base_ctx = m.ctx.split('&').next().unwrap_or("").to_string();
    let plain = base_ctx.trim_end_matches("@q").to_string();
    let parts: Vec<String> = sig.split('|').map(String::from).collect();
    if parts[1] != plain && parts[2] != plain { return; } // a run screened for another problem's context
    let Some(run) = sh.runs.lock().unwrap().get(&move_key(&m)).cloned() else { return };
    let inst = sh.problems.lock().unwrap()[&sig].clone();
    let led = sh.led.lock().unwrap().clone();
    let owns = |p: &PathBuf, n: u32| led.get(p).and_then(|pm| pm.get(&n)).and_then(|t| t.get(key.as_str())).map_or(false, |v| *v != 0); // a zero term is untouched by its knob
    // positives: the term's owner (the instance box or its parent, whichever has the knob's context) on repaired instances;
    // negatives: for every box the fix broke, the nearest ancestor-or-self that owns the term in the same context — the
    // owner whose change broke it (backdoor path closed: the cause, not every bystander)
    let mut pos = Vec::new(); let mut neg = Vec::new();
    let mut pages: Vec<&PathBuf> = run.pages.iter().filter(|(_, v)| !v.1.is_empty()).map(|(p, _)| p).collect();
    for (p, ..) in &inst { if !pages.contains(&p) { pages.push(p); } }
    for p in pages {
        let Some(b) = crate::page::Bundle::open(p) else { continue };
        let l = crate::page::load(&b, (1280.0, 800.0));
        let disp_ok = |n: u32| crate::layout::node_feature(&l.dom, &l.styled, n, "disp") == plain;
        let feats = |n: u32| -> HashMap<&'static str, String> { crate::layout::FEATURES.iter().map(|f| (*f, crate::layout::node_feature(&l.dom, &l.styled, n, f))).collect() };
        let mut pset = HashSet::new();
        let (g, lost, _) = &run.pages[p];
        for (ip, n, par, _, _) in &inst { if ip != p || !g.contains(n) { continue; }
            for o in [*n, *par] { if owns(p, o) && disp_ok(o) && pset.insert(o) { pos.push(feats(o)); break; } } }
        let mut nset = HashSet::new();
        for &x in lost { let mut a = x; for _ in 0..64 { if a == crate::dom::NONE { break; } if owns(p, a) && disp_ok(a) { if !pset.contains(&a) && nset.insert(a) { neg.push(feats(a)); } break; } a = l.dom.nodes[a as usize].parent; } }
    }
    let mut cond = if neg.is_empty() || pos.is_empty() { None } else { sat_condition(&pos, &neg) };
    let mut fca_note = String::new();
    if cond.is_none() && !pos.is_empty() && !neg.is_empty() {
        // FCA (Ganter & Wille, math index #31): each positive's own attributes generate a concept; the separating ≤ 3-literal
        // condition inside that intent that excludes every negative and covers the MOST positives is the closed concept to use.
        // If a positive's intent is identical to a negative's, no feature we have separates them — named as the missing attribute.
        let holds = |x: &HashMap<&'static str, String>, c: &[String]| c.iter().all(|l| if let Some((f, v)) = l.split_once("!=") { x.get(f).map_or(true, |y| y != v) } else if let Some((f, v)) = l.split_once('=') { x.get(f).map_or(false, |y| y == v) } else { false });
        let mut best: Option<(usize, Vec<String>)> = None; let mut twins = 0;
        for p1 in pos.iter().take(24) {
            if neg.iter().any(|n| n == p1) { twins += 1; continue; }
            if let Some(c) = sat_condition(std::slice::from_ref(p1), &neg) { let cov = pos.iter().filter(|x| holds(x, &c)).count(); if best.as_ref().map_or(true, |b| cov > b.0) { best = Some((cov, c)); } }
        }
        fca_note = format!("FCA: {} of {} positives covered by the best concept; {twins} positives indistinguishable from a negative on all {} features", best.as_ref().map_or(0, |b| b.0), pos.len(), crate::layout::FEATURES.len());
        cond = best.map(|b| b.1);
    }
    let mut trace = vec![serde_json::json!({"op": "GOAL", "a": format!("condition for {} separating {} fixed owners from {} broken-page owners", move_key(&m), pos.len(), neg.len())})];
    if !fca_note.is_empty() { trace.push(serde_json::json!({"op": "DERIVE", "a": fca_note.clone()})); }
    let Some(cond) = cond else {
        trace.push(serde_json::json!({"op": "ABSTAIN", "a": format!("no conjunction of ≤ 3 node features separates them. {fca_note}")}));
        sh.write("trace", serde_json::json!({"sig": sig, "station": "sat", "steps": trace}));
        sh.commit.lock().unwrap().2.push(format!("  sat-open {sig:40} {}: no condition ≤ 3 literals ({} pos, {} neg) {fca_note}", move_key(&m), pos.len(), neg.len()));
        return;
    };
    let m2 = Move { point: m.point.clone(), ctx: format!("{}&{}", base_ctx, cond.join("&")), term: 0, w: m.w };
    trace.push(serde_json::json!({"op": "DERIVE", "a": "smallest separating condition (DPLL set cover)", "r": move_key(&m2)}));
    let r2 = screen_move(sh, &m2);
    let lost: usize = r2.pages.values().map(|x| x.1.len()).sum();
    let fixed = inst.iter().filter(|(p, n, ..)| r2.pages.get(p).map_or(false, |x| x.0.contains(n))).count();
    trace.push(serde_json::json!({"op": "INTERVENE", "a": move_key(&m2), "r": format!("fixes {fixed}, loses {lost}")}));
    let res = if lost == 0 && fixed > 0 { try_commit(sh, &sig, inst.len(), &r2, fixed, &mut trace) } else { Some(false) };
    if res != Some(true) {
        sh.write("quar", serde_json::json!({"ts": now(), "sig": sig, "key": move_key(&m2), "reason": format!("conditioned fix: fixes {fixed}, loses {lost}"), "sets": sh.ysha}));
        sh.commit.lock().unwrap().2.push(format!("  sat-fail {sig:40} {} fixes {fixed} loses {lost}", move_key(&m2)));
    }
    sh.write("trace", serde_json::json!({"sig": sig, "station": "sat", "steps": trace}));
}

/// DEBUG station (code.debug: "two bugs, no single-edit fix"): pairs of partial moves screened together, smallest change first;
/// a pair that repairs instances with 0 boxes lost goes to the gate as one compound commit.
fn t_debug(sh: &Shared, sig: String, ms: Vec<Move>) {
    let inst = sh.problems.lock().unwrap()[&sig].clone();
    let mut trace = vec![serde_json::json!({"op": "GOAL", "a": format!("two-move search over {} partial moves for {sig}", ms.len())})];
    for i in 0..ms.len() { for j in i + 1..ms.len() {
        if ms[i].point == ms[j].point && ms[i].ctx == ms[j].ctx { continue; }
        let r = screen_moves(sh, &ms[i], &[ms[j].clone()]);
        let lost: usize = r.pages.values().map(|x| x.1.len()).sum();
        let fixed = inst.iter().filter(|(p, n, ..)| r.pages.get(p).map_or(false, |x| x.0.contains(n))).count();
        trace.push(serde_json::json!({"op": "INTERVENE", "a": format!("{} + {}", move_key(&ms[i]), move_key(&ms[j])), "r": format!("fixes {fixed}, loses {lost}")}));
        if fixed > 0 && lost == 0 && try_commit(sh, &sig, inst.len(), &r, fixed, &mut trace) == Some(true) {
            sh.write("trace", serde_json::json!({"sig": sig, "station": "debug", "steps": trace})); return;
        }
    } }
    trace.push(serde_json::json!({"op": "ABSTAIN", "a": "no pair repairs without loss"}));
    sh.write("trace", serde_json::json!({"sig": sig, "station": "debug", "steps": trace}));
}

/// The codebook as sift rows: entries with a stored hypervector use it; older entries (before hv) are re-encoded from their
/// signature alone (no features, error 0) — weaker recall, still in the same space.
fn load_book(stem: &str) -> (Vec<u8>, Vec<Move>) {
    let (mut rows, mut ms) = (Vec::new(), Vec::new());
    for l in std::fs::read_to_string(format!("{stem}.codebook.jsonl")).unwrap_or_default().lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let m = Move { point: j["point"].as_str().unwrap_or("").to_string(), ctx: j["ctx"].as_str().unwrap_or("*").to_string(), term: j["term"].as_u64().unwrap_or(0) as usize, w: j["w"].as_i64().unwrap_or(1) as i8 };
        if m.point.is_empty() || j["gain"].as_i64().unwrap_or(0) <= 0 { continue; }
        let h = j["hv"].as_str().and_then(crate::hv::from_hex).unwrap_or_else(|| crate::hv::problem(j["sig"].as_str().unwrap_or(""), &[], 0.0));
        rows.extend(h); ms.push(m);
    }
    (rows, ms)
}

/// HARMONY check of the program as a Horn rule base (logic.chain: forward chaining over node facts; the cemented law
/// horn.closure_order_independent holds only when no two clauses that fire on the same node conclude different weights at the
/// same specificity). For every node that owns a knob's term on the given pages: fire every clause whose base context and
/// literals hold; report conflicts (order-dependent conclusions), dead clauses (never fire) and firing counts.
fn harmony(pages: &[PathBuf], led: &HashMap<PathBuf, HashMap<u32, HashMap<&'static str, i32>>>, prog: &Program) -> serde_json::Value {
    let cl = prog.clauses();
    let keys: HashSet<String> = cl.iter().map(|c| c.0.clone()).collect();
    let mut fired: HashMap<(String, String), usize> = HashMap::new(); let mut conflicts = Vec::new(); let mut nodes = 0;
    for p in pages {
        let Some(pm) = led.get(p) else { continue };
        let owners: HashSet<u32> = pm.iter().filter(|(_, t)| t.keys().any(|k| keys.contains(*k))).map(|(n, _)| *n).collect();
        if owners.is_empty() { continue; }
        let Some(b) = crate::page::Bundle::open(p) else { continue };
        let l = crate::page::load(&b, (1280.0, 800.0));
        for &n in &owners {
            nodes += 1;
            let ctx = { let d = crate::layout::node_feature(&l.dom, &l.styled, n, "disp"); if l.dom.quirks { format!("{d}@q") } else { d } };
            for k in pm[&n].keys().filter(|k| keys.contains(**k)) {
                let mut hits: Vec<(usize, i8, &String)> = Vec::new();
                for (ck, c, w) in cl.iter().filter(|c| c.0 == *k) {
                    let mut it = c.split('&'); let base = it.next().unwrap_or("");
                    if !(base == ctx || ctx.strip_suffix("@q") == Some(base) || base == "*") { continue; }
                    let lits: Vec<&str> = it.collect();
                    if lits.iter().all(|x| if let Some((f, v)) = x.split_once("!=") { crate::layout::node_feature(&l.dom, &l.styled, n, f) != v } else if let Some((f, v)) = x.split_once('=') { crate::layout::node_feature(&l.dom, &l.styled, n, f) == v } else { false }) {
                        *fired.entry((ck.clone(), c.clone())).or_default() += 1; hits.push((lits.len(), *w, c));
                    }
                }
                if let Some(top) = hits.iter().map(|h| h.0).max() {
                    let ws: HashSet<i8> = hits.iter().filter(|h| h.0 == top).map(|h| h.1).collect();
                    if ws.len() > 1 && conflicts.len() < 20 { conflicts.push(format!("{}#{n} {k}: {:?}", name(p), hits.iter().filter(|h| h.0 == top).map(|h| format!("{}={}", h.2, h.1)).collect::<Vec<_>>())); }
                }
            }
        }
    }
    let dead: Vec<String> = cl.iter().filter(|c| !fired.contains_key(&(c.0.clone(), c.1.clone()))).map(|c| format!("{}[{}]={}", c.0, c.1, c.2)).collect();
    serde_json::json!({"clauses": cl.len(), "owner_nodes": nodes, "conflicts": conflicts.len(), "conflict_examples": conflicts, "dead": dead,
        "firing": cl.iter().map(|c| format!("{}[{}]={} fires on {}", c.0, c.1, c.2, fired.get(&(c.0.clone(), c.1.clone())).copied().unwrap_or(0))).collect::<Vec<_>>()})
}

/// Exact integer solve (math index #14, target value search over many knobs at once): the box's error on its axis is a sum of
/// its recorded terms, e_i' = e_i − Σ_k v_ik·(1 − w_k)/64. Enumerate w ∈ {−1, 0, 1}^K over the K ≤ 6 largest terms the
/// instances share (≤ 729 vectors, no relayout), keep vectors that close the most instances to ≤ 2 px with ≥ 2 knobs changed
/// (single knobs were already screened), then screen the best two as compound fixes and gate them.
fn t_solve(sh: &Shared, sig: String) {
    let inst = sh.problems.lock().unwrap()[&sig].clone();
    let parts: Vec<String> = sig.split('|').map(String::from).collect();
    let led = sh.led.lock().unwrap().clone();
    let mut mag: HashMap<&'static str, f32> = HashMap::new();
    for (p, n, ..) in &inst { if let Some(t) = led.get(p).and_then(|pm| pm.get(n)) { for (k, v) in t { if !k.starts_with("rule.") && *v != 0 { *mag.entry(k).or_default() += (*v as f32).abs(); } } } }
    let mut keys: Vec<&'static str> = mag.keys().copied().collect(); keys.sort_by(|a, b| mag[b].partial_cmp(&mag[a]).unwrap().then(a.cmp(b))); keys.truncate(6);
    if keys.len() < 2 { return; }
    let rows: Vec<(f32, Vec<f32>)> = inst.iter().map(|(p, n, _, _, e)| (*e, keys.iter().map(|k| led.get(p).and_then(|pm| pm.get(n)).and_then(|t| t.get(k)).map_or(0.0, |v| *v as f32 / 64.0)).collect())).collect();
    let kk = keys.len(); let total = 3usize.pow(kk as u32);
    let mut best: Vec<(usize, usize, Vec<i8>)> = Vec::new(); // closed, changed, weights
    for code in 0..total {
        let mut c = code; let w: Vec<i8> = (0..kk).map(|_| { let d = (c % 3) as i8 - 1; c /= 3; d }).collect();
        let changed = w.iter().filter(|&&x| x != 1).count(); if changed < 2 { continue; }
        let closed = rows.iter().filter(|(e, v)| (e - v.iter().zip(&w).zip(&keys).map(|((vi, wi), k)| vi * if k.starts_with("line.") { (*wi != 1) as i32 as f32 } else { (1 - wi) as f32 }).sum::<f32>()).abs() <= 2.0).count();
        if closed > 0 { best.push((closed, changed, w)); }
    }
    best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut trace = vec![serde_json::json!({"op": "GOAL", "a": format!("exact ternary solve over {} terms of {sig}", kk)}),
        serde_json::json!({"op": "DERIVE", "a": "vectors closing instances (predicted)", "r": best.iter().take(3).map(|b| format!("closes {} with {:?}", b.0, b.2)).collect::<Vec<_>>()})];
    for (closed, _, w) in best.iter().take(2) {
        let ms: Vec<Move> = keys.iter().zip(w.iter()).filter(|(_, x)| **x != 1).map(|(k, x)| Move { point: format!("k:{k}"), ctx: parts[1].clone(), term: 0, w: *x }).collect();
        let r = screen_moves(sh, &ms[0], &ms[1..]);
        let lost: usize = r.pages.values().map(|x| x.1.len()).sum();
        let fixed = inst.iter().filter(|(p, n, ..)| r.pages.get(p).map_or(false, |x| x.0.contains(n))).count();
        trace.push(serde_json::json!({"op": "INTERVENE", "a": ms.iter().map(move_key).collect::<Vec<_>>().join(" + "), "r": format!("predicted {closed}, fixes {fixed}, loses {lost}")}));
        if fixed > 0 && lost == 0 && try_commit(sh, &sig, inst.len(), &r, fixed, &mut trace) == Some(true) { break; }
    }
    sh.write("trace", serde_json::json!({"sig": sig, "station": "solve", "steps": trace}));
}

/// AGM challenge + revision (mind.revise; omniglyph challenge outcomes). `omni-render core <new-evidence-dir> --challenge
/// --program P`: every clause of the program is tested on pages it was NOT learned from by removing it alone:
/// SUSTAINED = the page agrees on more boxes without the clause (a counterexample), UPHELD = it agrees on fewer (support),
/// INVALID = the clause never fires there. Revision never edits: the least-entrenched sustained clause (entrenchment = boxes it
/// gained when committed, from history) is dropped first, the challenge repeats, and the result is written as a PROPOSED
/// program version `<stem>.revised.json` beside the original; every outcome goes to `<stem>.challenges.jsonl`.
pub fn run_challenge(args: &[String]) {
    let dir = args.first().cloned().unwrap_or_else(|| "results/ir-bundles".into());
    let prog_path = args.iter().position(|a| a == "--program").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "omr.program.json".into());
    let stem = prog_path.trim_end_matches(".json").to_string();
    let mut pages: Vec<PathBuf> = std::fs::read_dir(&dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists() && p.join("chrome-snap.json").exists()).collect()).unwrap_or_default();
    pages.sort();
    let mut entrench: HashMap<String, i64> = HashMap::new();
    for l in std::fs::read_to_string(format!("{stem}.history.jsonl")).unwrap_or_default().lines() { if let Ok(j) = serde_json::from_str::<serde_json::Value>(l) {
        let g = j["replay"][1].as_i64().unwrap_or(0) - j["replay"][0].as_i64().unwrap_or(0) + j["holdout"][1].as_i64().unwrap_or(0) - j["holdout"][0].as_i64().unwrap_or(0);
        if let Some(m) = j["move"].as_str() { *entrench.entry(m.split(" + ").next().unwrap_or(m).to_string()).or_default() += g; } } }
    let mut prog = Program::load(&prog_path).map();
    let score_all = |pm: &Prog| -> (usize, HashMap<PathBuf, usize>) { set_program(Program::from_map(pm.clone())); let mut t = 0; let mut per = HashMap::new(); for p in &pages { if let Some(f) = score_full(p) { t += f.agree.len(); per.insert(p.clone(), f.agree.len()); } } (t, per) };
    use std::io::Write;
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(format!("{stem}.challenges.jsonl")).unwrap();
    let mut dropped = Vec::new();
    loop {
        let (base, _) = score_all(&prog);
        let mut sustained: Vec<(String, String, i64, i64)> = Vec::new(); // key, ctx, Δ without, entrenchment
        let clauses: Vec<(String, String)> = prog.iter().filter(|(k, _)| k.starts_with("k:")).flat_map(|(k, m)| m.keys().map(move |c| (k.clone(), c.clone()))).collect();
        for (k, c) in &clauses {
            let mut without = prog.clone(); without.get_mut(k).unwrap().remove(c); if without[k].is_empty() { without.remove(k); }
            let (t, _) = score_all(&without);
            let d = t as i64 - base as i64;
            let w = prog[k][c][0];
            let mk = format!("{k}[{c}].0={w}");
            let outcome = if d > 0 { "SUSTAINED" } else if d < 0 { "UPHELD" } else { "INVALID" };
            writeln!(out, "{}", serde_json::json!({"ts": now(), "clause": mk, "evidence": dir, "pages": pages.iter().map(|p| name(p)).collect::<Vec<_>>(), "agree_with": base, "agree_without": t, "outcome": outcome, "entrenchment": entrench.get(&mk).copied().unwrap_or(0)})).ok();
            eprintln!("  {outcome:9} {mk:60} with {base} without {t} (entrenchment {})", entrench.get(&mk).copied().unwrap_or(0));
            if d > 0 { sustained.push((k.clone(), c.clone(), d, entrench.get(&mk).copied().unwrap_or(0))); }
        }
        // AGM: give up the least entrenched sustained clause first, then re-challenge the rest
        let Some(x) = sustained.iter().min_by_key(|x| (x.3, std::cmp::Reverse(x.2))).cloned() else { break };
        prog.get_mut(&x.0).unwrap().remove(&x.1); if prog[&x.0].is_empty() { prog.remove(&x.0); }
        dropped.push(format!("{}[{}] (entrenchment {}, +{} boxes on new evidence without it)", x.0, x.1, x.3, x.2));
    }
    if !dropped.is_empty() { Program::from_map(prog).save(&format!("{stem}.revised.json")); }
    eprintln!("challenge on {} new pages: revision drops {} clause(s): {:?}{}", pages.len(), dropped.len(), dropped, if dropped.is_empty() { "" } else { " → PROPOSED version written to <program>.revised.json (original untouched)" });
}
