// SPDX-License-Identifier: LicenseRef-PolyForm-Small-Business-1.0.0 OR LicenseRef-PolyForm-Noncommercial-1.0.0
//! FIX — the doctor repairs a page's geometry ON ITS OWN (doctor-core PROCEDURE §3–§7; author 10-07: "continue on to having the doctor
//! fix the errors" · "you are designing the system and procedures it will use … for any new website").
//! Loop, per site: MAP (T25 core vs the pristine plate, `jit::t25::fire_pristine`) → for the first unsolved defect box (tree order:
//! frames before children): CLASSIFY (FORMAT text-driven · ROUNDING sub-pixel share · STRUCTURE) → CANDIDATES (page-code rewrites:
//! exact declarations computed from the map's error, ordered by class and by what the knowledge base learnt worked for this family
//! key) → JUDGE in repair mode (re-load + re-map with the candidate in the site's fix sheet: the box's twist must reach 0, NO box
//! that was pristine may lose it, total defects must drop) → keep the first that passes → re-map → next box.
//! The fix sheet is CSS: one rule per repaired box, addressed by its structural path (`:root > :nth-child(i) > …`), loaded last in
//! the cascade (`page::build_sheet`) — the published page is never edited; `OMR_FIXES=<dir>` makes the browser load `<slug>.css`.
//! Knowledge base `results/doctor-kb.jsonl`: (family key, template) → outcome; recalled first next time (IFS role = reuse).
use crate::jit::t25 as T;
use std::collections::{HashMap, HashSet};
use std::path::Path;

thread_local! { static SHEET: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }
pub fn set_sheet(s: Option<String>) { SHEET.with(|x| *x.borrow_mut() = s); }
pub fn slug(url: &str) -> String {
    let s: String = url.trim_start_matches("https://").trim_start_matches("http://").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.trim_matches('-').chars().take(80).collect()
}
/// The fix sheet for a page: the doctor's trial sheet on this thread, else `OMR_FIXES/<slug(url)>.css`, else none.
pub fn sheet_for(url: &str) -> Option<String> {
    if let Some(s) = SHEET.with(|x| x.borrow().clone()) { return Some(s); }
    let d = std::env::var("OMR_FIXES").ok()?;
    std::fs::read_to_string(Path::new(&d).join(format!("{}.css", slug(url)))).ok()
}

/// Structural selector of an element (element children only, as CSS `:nth-child` counts); None inside shadow / pseudo trees.
pub fn selector(dom: &crate::dom::Dom, n: u32) -> Option<String> {
    use crate::dom::{Kind, NONE};
    let el = |c: u32| dom.nodes[c as usize].kind == Kind::Element;
    let mut parts = Vec::new(); let mut m = n;
    loop {
        if dom.tag(m).starts_with("::") { return None; }
        let p = dom.nodes[m as usize].parent;
        if p == NONE || !el(p) { parts.push(":root".to_string()); break; }
        let k = dom.children(p).filter(|&c| el(c) && !dom.tag(c).starts_with("::")).position(|c| c == m)? + 1;
        parts.push(format!(":nth-child({k})"));
        m = p;
    }
    parts.reverse();
    Some(parts.join(" > "))
}

#[derive(Clone)]
pub struct Info { pub sel: String, pub kind: usize, pub d: i64, pub err: [i64; 5], pub key: String, pub rect: [i64; 4], pub frect: [i64; 4],
                  pub ml: i64, pub mt: i64, pub fcw: i64, pub fcl: i64, pub fsel: Option<String>, pub pos: u8, pub text_chars: usize, pub frame_disp: String }
pub struct Run { pub boxes: usize, pub defects: Vec<Info>, pub clean: HashSet<String>, pub unreachable: usize, pub overlaps: usize }

fn px(u: i64) -> String { format!("{}px", u as f64 / 64.0) }

/// Load + lay out the page with `css` as its fix sheet, the doctor awake (repair mode); the map with every box addressed by selector.
pub fn run(p: &Path, css: &str) -> Option<Run> { run_geo(p, css, &HashMap::new()) }

/// Every element's structural selector → node (to resolve a stored route-2 table on a freshly loaded document).
pub fn sel_index(dom: &crate::dom::Dom) -> HashMap<String, u32> {
    (0..dom.nodes.len() as u32).filter(|&n| dom.nodes[n as usize].kind == crate::dom::Kind::Element).filter_map(|n| selector(dom, n).map(|s| (s, n))).collect()
}
/// Browser load: the site's stored route-2 table (`OMR_FIXES/<slug>.geo.json`) armed for this document (not during doctor trials).
pub fn arm_geo_for(url: &str, dom: &crate::dom::Dom) {
    if SHEET.with(|x| x.borrow().is_some()) { return; }
    let Ok(d) = std::env::var("OMR_FIXES") else { return };
    let Ok(t) = std::fs::read_to_string(Path::new(&d).join(format!("{}.geo.json", slug(url)))) else { return };
    let Ok(g) = serde_json::from_str::<HashMap<String, (i32, i32, i32)>>(&t) else { return };
    let ix = sel_index(dom);
    crate::layout::geo_set(dom, Some(g.into_iter().filter_map(|(s, v)| ix.get(&s).map(|&n| (n, v))).collect()));
}

/// `run` with a route-2 table (selector → (dx, dy, width or −1)) laid into the engine.
pub fn run_geo(p: &Path, css: &str, geo: &HashMap<String, (i32, i32, i32)>) -> Option<Run> {
    use crate::dom::{Kind, NONE};
    set_sheet(Some(css.to_string()));
    let b = crate::page::Bundle::open(p)?;
    let l = crate::page::load(&b, (1280.0, 800.0));
    set_sheet(None);
    if !geo.is_empty() { let ix = sel_index(&l.dom); crate::layout::geo_set(&l.dom, Some(geo.iter().filter_map(|(s, v)| ix.get(s).map(|&n| (n, *v))).collect())); }
    let fonts = crate::font::Fonts::new();
    for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
    let base = b.url.clone(); let dref = &l.dom;
    let sizes = |n: u32| -> Option<(u32, u32)> {
        if dref.tag(n) == "svg" { crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)) }
        else { crate::img_bytes(dref, n, &base, &b).and_then(|bytes| crate::img::size(&bytes)) }
    };
    T::arm_pristine(Some(T::PristinePlate { page: p.file_name()?.to_string_lossy().to_string(), only: None, wake: "repair" }));
    { let mut lay = crate::layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800); lay.run(); }
    T::arm_pristine(None);
    crate::layout::geo_set(&l.dom, None);
    let m = T::take_map()?;
    let dom = &l.dom;
    let st = |n: u32| &l.styled.table[l.styled.ids[n as usize] as usize];
    let mut unreachable = 0;
    let clean: HashSet<String> = m.clean.iter().filter_map(|&n| selector(dom, n)).collect();
    let mut defects = Vec::new();
    for d in &m.defects {
        let Some(sel) = selector(dom, d.node) else { unreachable += 1; continue };
        let fs = st(d.frame);
        let pl = crate::layout::res0(fs.padding[3], d.frect[2] as i32) as i64 + fs.border_w[3] as i64;
        let pr = crate::layout::res0(fs.padding[1], d.frect[2] as i32) as i64 + fs.border_w[1] as i64;
        let fcw = d.frect[2] - pl - pr;
        let s = st(d.node);
        let text_chars: usize = dom.children(d.node).filter(|&c| dom.nodes[c as usize].kind == Kind::Text).map(|c| dom.text(c).trim().chars().count()).sum();
        let _ = NONE;
        defects.push(Info { sel, kind: d.hits[0].0, d: d.hits[0].1, err: d.err, rect: d.rect, frect: d.frect,
            key: format!("{}>{}|{}|{:?}", d.key.0, d.key.1, d.key.2, d.key.3),
            ml: crate::layout::res0(s.margin[3], fcw as i32) as i64, mt: crate::layout::res0(s.margin[0], fcw as i32) as i64, fcw, fcl: d.frect[0] + pl, fsel: selector(dom, d.frame), pos: s.position, text_chars,
            frame_disp: d.key.0.clone() });
    }
    Some(Run { boxes: m.boxes, defects, clean, unreachable, overlaps: m.overlaps })
}

/// CLASSIFY (PROCEDURE §3): FORMAT = the box's size comes from its own text and the error is on a size axis; ROUNDING = a
/// sub-pixel miss inside a share-distributing frame (flex / grid / table); STRUCTURE = everything else.
pub fn classify(d: &Info) -> &'static str {
    let axis = (0..4).find(|&k| d.err[k] != 0).unwrap_or(0);
    if d.text_chars > 0 && axis >= 2 { return "FORMAT"; }
    if (0..4).all(|k| d.err[k].abs() < 64) && ["Flex", "InlineFlex", "Grid", "InlineGrid", "Table", "TableRow"].iter().any(|x| d.frame_disp == *x) { return "ROUNDING"; }
    "STRUCTURE"
}

/// CANDIDATES: page-code rewrites with exact values from the map (err = ours − pristine, 1/64 px). (template name, declarations).
pub fn candidates(d: &Info, class: &str) -> Vec<(&'static str, String)> {
    // a declaration prefixed `@frame ` is written on the box's FRAME (its selector), not the box
    let axis = (0..4).find(|&k| d.err[k] != 0).unwrap_or(0);
    let e = d.err[axis];
    let imp = |s: String| s.split(';').filter(|x| !x.trim().is_empty()).map(|x| format!("{} !important", x.trim())).collect::<Vec<_>>().join("; ");
    let mut c: Vec<(&'static str, String)> = Vec::new();
    match axis {
        0 => {
            let right_flush = d.kind == 1 && e == -d.d;
            let overflow_right = d.kind == 0 && d.rect[0] + d.rect[2] > d.frect[0] + d.frect[2];
            if right_flush { c.push(("w_exact", imp(format!("width: {}; box-sizing: border-box", px(d.rect[2] - e))))); }
            if overflow_right { c.push(("maxw_frame", imp("max-width: 100%; box-sizing: border-box".into()))); c.push(("minw0", imp("min-width: 0".into())));
                                c.push(("w_frame", imp(format!("width: {}; box-sizing: border-box", px(d.fcw))))); }
            c.push(("ml_shift", imp(format!("margin-left: {}", px(d.ml - e)))));
            // a box wholly beyond its frame's right edge = an item of a horizontal row: the frame is a scroller (page structure)
            let beyond = d.kind == 0 && d.rect[0] >= d.frect[0] + d.frect[2];
            if d.kind == 0 && d.fsel.is_some() { let f = ("frame_scroll", format!("@frame {}", imp("overflow-x: auto".into()))); if beyond { c.insert(0, f) } else { c.push(f) } }
            if d.pos == 0 { c.push(("pos_shift", imp(format!("position: relative; left: {}", px(-e))))); }
        }
        1 => {
            if d.kind == 0 && d.rect[1] < d.frect[1] { c.push(("va_top", imp("vertical-align: top".into()))); }
            c.push(("mt_shift", imp(format!("margin-top: {}", px(d.mt - e)))));
            if d.pos == 0 { c.push(("pos_shift", imp(format!("position: relative; top: {}", px(-e))))); }
        }
        2 => {
            if class == "FORMAT" && d.text_chars > 0 { c.push(("ls_fit", imp(format!("letter-spacing: {}px", -(e as f64 / 64.0) / d.text_chars as f64)))); }
            c.push(("w_exact", imp(format!("width: {}; box-sizing: border-box", px(d.rect[2] - e)))));
        }
        _ => c.push(("h_exact", imp(format!("height: {}; box-sizing: border-box", px(d.rect[3] - e))))),
    }
    c
}

/// The knowledge base: (family key, template) → (passed, failed). Templates that passed for this key are tried first; a
/// template that failed 3+ times for a key and never passed goes last (negative knowledge).
pub struct Kb { pub stats: HashMap<(String, String), (u32, u32)>, path: String }
impl Kb {
    pub fn load(path: &str) -> Kb {
        let mut stats = HashMap::new();
        for l in std::fs::read_to_string(path).unwrap_or_default().lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
            let k = (v["key"].as_str().unwrap_or("").to_string(), v["template"].as_str().unwrap_or("").to_string());
            let e: &mut (u32, u32) = stats.entry(k).or_default();
            if v["pass"].as_bool() == Some(true) { e.0 += 1 } else { e.1 += 1 }
        }
        Kb { stats, path: path.into() }
    }
    pub fn order(&self, key: &str, c: &mut [(&'static str, String)]) {
        let score = |t: &str| { let (p, f) = self.stats.get(&(key.to_string(), t.to_string())).copied().unwrap_or((0, 0)); if p > 0 { -(p as i64) } else if f >= 3 { 100 } else { 0 } };
        c.sort_by_key(|x| score(x.0));
    }
    pub fn record(&mut self, row: serde_json::Value) {
        let k = (row["key"].as_str().unwrap_or("").to_string(), row["template"].as_str().unwrap_or("").to_string());
        let e = self.stats.entry(k).or_default();
        if row["pass"].as_bool() == Some(true) { e.0 += 1 } else { e.1 += 1 }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) { let _ = writeln!(f, "{row}"); }
    }
}

/// ROUTE 2 — RESTRUCTURE (author 10-07: "when we're at 97 %+ geometric fit we begin to restructure to fit our needs"): the core's exact
/// targets written back into layout. Each round every remaining defect box gets its delta from the map (position by −err; a box
/// wider than its frame's content box gets that width at the content edge; a width miss gets the exact width); the page is
/// re-laid with the table and re-mapped; the round is kept only if NO pristine box is lost and defects drop. A round that fails
/// falls back to one box at a time. Rounds repeat because siblings are not carried by each other (a moved sibling moves its
/// neighbour's target).
pub fn restructure(p: &Path, css: &str, mut cur: Run, rounds: usize) -> (HashMap<String, (i32, i32, i32)>, Run, usize, usize) {
    let mut geo: HashMap<String, (i32, i32, i32)> = HashMap::new();
    let (mut used, mut trials) = (0usize, 0usize);
    let delta = |g: &mut HashMap<String, (i32, i32, i32)>, d: &Info| {
        let e = g.entry(d.sel.clone()).or_insert((0, 0, -1));
        if d.kind == 0 && d.rect[2] > d.fcw { e.2 = d.fcw as i32; e.0 += (d.fcl - d.rect[0]) as i32; return; }
        if d.err[2] != 0 { e.2 = (d.rect[2] - d.err[2]) as i32; }
        e.0 -= d.err[0] as i32; e.1 -= d.err[1] as i32;
    };
    for _ in 0..rounds {
        if cur.defects.is_empty() { break; }
        used += 1;
        let mut g2 = geo.clone();
        for d in &cur.defects { delta(&mut g2, d); }
        trials += 1;
        if let Some(r2) = run_geo(p, css, &g2) {
            if cur.clean.difference(&r2.clean).count() == 0 && r2.defects.len() < cur.defects.len() && r2.overlaps <= cur.overlaps { geo = g2; cur = r2; continue; }
        }
        let mut progressed = false;
        for d in cur.defects.clone() {
            let mut g3 = geo.clone(); delta(&mut g3, &d); trials += 1;
            let Some(r3) = run_geo(p, css, &g3) else { continue };
            if cur.clean.difference(&r3.clean).count() == 0 && r3.defects.len() < cur.defects.len() && r3.overlaps <= cur.overlaps { geo = g3; cur = r3; progressed = true; }
        }
        if !progressed { break; }
    }
    (geo, cur, used, trials)
}

/// Every page bundle in a directory (a second corpus such as corpus-top100: not part of the frozen yardstick).
pub fn bundles(dir: &str) -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect()).unwrap_or_default();
    v.sort(); v
}

/// `omni-render core corpus --abc t25solve (--page DIR | --dir CORPUS | [--all]) [--max-tries N] [--fixes DIR] [--kb F]`
pub fn solve(a: &[String]) {
    let arg = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let prog = arg("--program").unwrap_or_else(|| "results/omr.program.json".into());
    crate::layout::set_program(crate::layout::Program::load(&prog));
    let pages: Vec<std::path::PathBuf> = if let Some(d) = arg("--page") { vec![d.into()] } else if let Some(d) = arg("--dir") { bundles(&d) } else {
        let (replay, hold, _) = super::super::frozen_sets("corpus", &format!("{}.sets.json", prog.trim_end_matches(".json")));
        if a.iter().any(|x| x == "--all") { replay.iter().chain(hold.iter()).cloned().collect() } else { replay }
    };
    let max_tries: usize = arg("--max-tries").and_then(|v| v.parse().ok()).unwrap_or(400);
    let fixes = arg("--fixes").unwrap_or_else(|| "results/doctor-fixes".into());
    std::fs::create_dir_all(&fixes).ok();
    let geo_at: f64 = arg("--geo-at").and_then(|v| v.parse().ok()).unwrap_or(0.97);
    let mut geo_boxes = 0usize;
        let mut kb = Kb::load(&arg("--kb").unwrap_or_else(|| "results/doctor-kb.jsonl".into()));
    let (mut tot_b0, mut tot_b1, mut tot_d0, mut tot_d1, mut tot_fix) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut by_class: HashMap<&'static str, (usize, usize)> = HashMap::new();
    let mut by_tmpl: HashMap<&'static str, usize> = HashMap::new();
    for p in &pages {
        let t0 = std::time::Instant::now();
        let Some(bundle) = crate::page::Bundle::open(p) else { continue };
        let site = slug(&bundle.url);
        let mut css = String::new();
        let Some(mut cur) = run(p, &css) else { eprintln!("  t25solve: {}: cannot lay out", p.display()); continue };
        let (b0, d0) = (cur.boxes, cur.defects.len());
        let mut tried: HashSet<String> = HashSet::new();
        let (mut tries, mut fixed) = (0usize, 0usize);
        'boxes: while let Some(d) = cur.defects.iter().find(|d| !tried.contains(&d.sel)).cloned() {
            tried.insert(d.sel.clone());
            let class = classify(&d);
            let mut cands = candidates(&d, class);
            kb.order(&d.key, &mut cands);
            let e_cls = by_class.entry(class).or_default(); e_cls.0 += 1;
            for (tmpl, decl) in cands {
                if tries >= max_tries { break 'boxes; }
                tries += 1;
                let (tsel, decl) = match decl.strip_prefix("@frame ") { Some(x) => (d.fsel.clone().unwrap_or_default(), x.to_string()), None => (d.sel.clone(), decl.clone()) };
                if tsel.is_empty() { continue; }
                let rule = format!("/* doctor: {} {} err {:?} class {} key {} */\n{} {{ {} }}\n", T::KINDS[d.kind], tmpl, &d.err[..4], class, d.key, tsel, decl);
                let css2 = format!("{css}{rule}");
                let Some(r2) = run(p, &css2) else { continue };
                let gone = !r2.defects.iter().any(|x| x.sel == d.sel);
                let lost = cur.clean.difference(&r2.clean).count();
                let pass = gone && lost == 0 && r2.defects.len() < cur.defects.len() && r2.overlaps <= cur.overlaps;
                kb.record(serde_json::json!({"key": d.key, "template": tmpl, "class": class, "kind": T::KINDS[d.kind], "pass": pass, "site": site,
                    "err": &d.err[..4], "gone": gone, "lost": lost, "defects": [cur.defects.len(), r2.defects.len()]}));
                if pass { css = css2; cur = r2; fixed += 1; by_class.entry(class).or_default().1 += 1; *by_tmpl.entry(tmpl).or_default() += 1; break; }
            }
        }
        if !css.is_empty() { std::fs::write(Path::new(&fixes).join(format!("{site}.css")), format!("/* doctor-core fix sheet for {} — generated, verified on the pristine map (0 lost) */\n{css}", bundle.url)).ok(); }
        let d1 = cur.defects.len();
        let fit = (cur.boxes - d1 - cur.unreachable) as f64 / cur.boxes.max(1) as f64;
        let mut r2note = String::new();
        if d1 > 0 && fit >= geo_at {
            let (geo, after, rounds, trials) = restructure(p, &css, cur, 6);
            r2note = format!(" · ROUTE 2 at fit {:.1} %: {d1} → {} ({} boxes restructured, {rounds} rounds, {trials} trials)", fit * 100.0, after.defects.len(), geo.len());
            if !geo.is_empty() { std::fs::write(Path::new(&fixes).join(format!("{site}.geo.json")), serde_json::to_string(&geo).unwrap()).ok(); geo_boxes += geo.len(); }
            cur = after;
        }
        eprintln!("  {site:.48}: boxes {b0} · defects {d0} → {d1} (route 1: fixed {fixed}, tries {tries}){r2note} · unreachable {} · {:.1} s", cur.unreachable, t0.elapsed().as_secs_f64());
        tot_b0 += b0; tot_b1 += cur.boxes; tot_d0 += d0; tot_d1 += cur.defects.len(); tot_fix += fixed;
    }
    eprintln!("t25solve · {} pages · defects {tot_d0} → {tot_d1} · route-1 fixed {tot_fix} · route-2 boxes {geo_boxes} · boxes {tot_b0} → {tot_b1} · pristine {:.2} % → {:.2} %",
        pages.len(), 100.0 * (tot_b0 - tot_d0) as f64 / tot_b0.max(1) as f64, 100.0 * (tot_b1 - tot_d1) as f64 / tot_b1.max(1) as f64);
    let mut cv: Vec<_> = by_class.into_iter().collect(); cv.sort();
    for (c, (n, f)) in cv { eprintln!("  class {c:9}: {n} boxes tried · {f} fixed"); }
    let mut tv: Vec<_> = by_tmpl.into_iter().collect(); tv.sort_by(|x, y| y.1.cmp(&x.1));
    for (t, n) in tv { eprintln!("  template {t:10}: {n}"); }
}
