// SPDX-License-Identifier: MIT OR Apache-2.0
//! omni-render — CLI: `stats <corpus>` / `bench <corpus>` / `dump <page.html>`
mod jsi_date;
mod agent;
mod incr;
mod fastparse;
mod corert0;
mod css;
mod net;
mod cookie;
mod dom;
mod font;
mod hit;
mod woff;
mod fuzz;
mod img;
mod jit;
mod layout;
mod raster;
mod paint;
mod sandbox;
mod pageproc;
mod jsparse;
mod jsdom;
mod scroll;
mod snapshot;
mod runtime;
mod page;
mod style;
mod svg;
mod verify;
mod why;
mod explain;
mod checks;
mod selt;
mod uiir;
mod jsi;
mod jsi_ast;
mod jsc;
mod jscomp;
mod jsi_lib;
mod jsi_re;
mod svrt;
#[cfg(feature = "t25")] mod repair;
mod hv;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

/// Counting allocator: live heap bytes, for honest bytes/node numbers.
struct Count;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() { let n = LIVE.fetch_add(l.size(), Relaxed) + l.size(); PEAK.fetch_max(n, Relaxed); }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) }; LIVE.fetch_sub(l.size(), Relaxed); }
}
#[global_allocator]
static A: Count = Count;

fn pages(dir: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok())
        .filter(|e| e.path().join("page.html").exists())
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path().join("page.html")).unwrap()))
        .collect();
    v.sort();
    v
}

fn rcdom_count(h: &markup5ever_rcdom::Handle) -> usize {
    1 + h.children.borrow().iter().map(rcdom_count).sum::<usize>()
}

fn bench(dir: &str) {
    use html5ever::tendril::TendrilSink;
    let mut rows = Vec::new();
    let (mut tstruct, mut tcontent, mut tsheets) = (0usize, 0usize, 0usize);
    let (mut tn, mut tours, mut trc, mut tms_o, mut tms_r) = (0usize, 0usize, 0usize, 0f64, 0f64);
    for (name, html) in pages(dir) {
        // ours
        let base = LIVE.load(Relaxed);
        PEAK.store(base, Relaxed);
        let t = Instant::now();
        let d = dom::Dom::parse(&html);
        let ms_o = t.elapsed().as_secs_f64() * 1e3;
        let ours = LIVE.load(Relaxed) - base;
        let peak_o = PEAK.load(Relaxed) - base;
        let n = d.nodes.len();
        let structure = d.nodes.len() * 16 + d.attrs.len() * 12 + d.texts.len() * 8;
        let content = d.bytes.len();
        let sheets: usize = d.sheets.iter().map(|x| x.1.len()).sum();
        drop(d);
        // html5ever RcDom (the reference tree builder's own DOM)
        let base = LIVE.load(Relaxed);
        PEAK.store(base, Relaxed);
        let t = Instant::now();
        let rc = html5ever::parse_document(markup5ever_rcdom::RcDom::default(), Default::default()).from_utf8().one(&html[..]);
        let ms_r = t.elapsed().as_secs_f64() * 1e3;
        let rcb = LIVE.load(Relaxed) - base;
        let rn = rcdom_count(&rc.document);
        drop(rc);
        tn += n; tours += ours; tstruct += structure; tcontent += content; tsheets += sheets; trc += rcb; tms_o += ms_o; tms_r += ms_r;
        rows.push(serde_json::json!({"page": name, "html_kb": html.len()/1024, "nodes": n, "rc_nodes": rn,
            "ours_bytes": ours, "ours_peak": peak_o, "rc_bytes": rcb, "ours_b_per_node": ours as f64 / n as f64,
            "rc_b_per_node": rcb as f64 / rn as f64, "ours_ms": ms_o, "rc_ms": ms_r}));
        println!("{:<44} nodes {:>6}  ours {:>6.1} B/node {:>6.2} ms | rcdom {:>6.1} B/node {:>6.2} ms",
            &name[..name.len().min(44)], n, ours as f64 / n as f64, ms_o, rcb as f64 / rn as f64, ms_r);
    }
    println!("TOTAL nodes {tn}  ours {:.1} B/node {:.0} ms | rcdom {:.1} B/node {:.0} ms | {:.1}x less RAM",
        tours as f64 / tn as f64, tms_o, trc as f64 / tn as f64, tms_r, trc as f64 / tours as f64);
    println!("  breakdown/node: structure {:.1} B (16 B node + attrs + text spans) | content {:.1} B (deduped text+attr values) | css text {:.1} B | allocator/other {:.1} B",
        tstruct as f64 / tn as f64, tcontent as f64 / tn as f64, tsheets as f64 / tn as f64,
        (tours - tstruct - tcontent - tsheets) as f64 / tn as f64);
    std::fs::create_dir_all("results").ok();
    std::fs::write("results/render0a-dom.json", serde_json::to_string_pretty(&serde_json::json!({
        "pages": rows, "total_nodes": tn, "ours_b_per_node": tours as f64 / tn as f64,
        "rc_b_per_node": trc as f64 / tn as f64, "ours_ms": tms_o, "rc_ms": tms_r})).unwrap()).unwrap();
}

fn style_cmd(dir: &str, only: Option<&str>) {
    let mut sc = verify::Score::default();
    let mut rows = Vec::new();
    let (mut tn, mut tstyles, mut tms) = (0usize, 0usize, 0f64);
    let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect();
    ents.sort();
    let mut all_styles: std::collections::HashSet<style::Style> = std::collections::HashSet::new();
    let mut tshared = 0usize;
    let mut tsplit = (0f64, 0f64, 0f64, 0f64);
    let mut tsetup = 0f64;
    let mut tvg = (0f64, 0f64);
    for p in ents {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(o) = only { if !name.contains(o) { continue; } }
        let b = match page::Bundle::open(&p) { Some(b) => b, None => continue };
        let base = LIVE.load(Relaxed);
        let l = page::load(&b, (1280.0, 800.0));
        let mem = LIVE.load(Relaxed) - base;
        let els = l.dom.nodes.iter().filter(|n| n.kind == dom::Kind::Element).count();
        let mut s1 = verify::Score::default();
        if let Some(ch) = verify::chrome_elements(&p.join("chrome-snap.json")) { verify::score(&l.dom, &l.styled.ids, &l.styled.table, &ch, &mut s1); }
        let (h, t) = s1.hit.values().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        for (k, v) in &s1.hit { let e = sc.hit.entry(k).or_insert((0, 0)); e.0 += v.0; e.1 += v.1; }
        for (k, v) in &s1.misses { *sc.misses.entry(k.clone()).or_insert(0) += v; }
        sc.aligned += s1.aligned;
        for st in &l.styled.table { all_styles.insert(st.clone()); }
        tn += els; tstyles += l.styled.table.len(); tms += l.styled.stats.ms; tshared += l.styled.stats.shared;
        tsplit.0 += l.styled.stats.match_ms; tsplit.1 += l.styled.stats.fp_ms; tsplit.2 += l.styled.stats.apply_ms; tsplit.3 += l.styled.stats.intern_ms; tsetup += l.styled.stats.setup_ms; tvg.0 += l.styled.stats.vars_ms; tvg.1 += l.styled.stats.gather_ms;
        println!("{:<44} els {:>6} rules {:>6} styles {:>5} ({:>4.1}%) css {:>5}KB miss {} | parse {:>5.1} css {:>5.1} cascade {:>6.1} ms | mem {:>6}KB | agree {:>5.1}% of {}",
            &name[..name.len().min(44)], els, l.sheet.rules.len(), l.styled.table.len(), 100.0 * l.styled.table.len() as f64 / els.max(1) as f64,
            l.css_bytes / 1024, l.missing_css, l.parse_ms, l.css_ms, l.styled.stats.ms, mem / 1024, 100.0 * h as f64 / t.max(1) as f64, t);
        rows.push(serde_json::json!({"page": name, "elements": els, "styles": l.styled.table.len(), "rules": l.sheet.rules.len(),
            "cascade_ms": l.styled.stats.ms, "agree": h as f64 / t.max(1) as f64, "compared": t, "mem_bytes": mem}));
    }
    let (h, t) = sc.hit.values().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    println!("cascade split: matching {:.0} ms · sibling fingerprints {:.0} ms · apply {:.0} ms · intern {:.0} ms · setup (facts, tables) {:.0} ms · vars {:.0} ms · gather+inline+sort {:.0} ms", tsplit.0, tsplit.1, tsplit.2, tsplit.3, tsetup, tvg.0, tvg.1);
    println!("style sharing: {} of {} elements ({:.1}%) reused a sibling's style", tshared, tn, 100.0 * tshared as f64 / tn.max(1) as f64);
    println!("TOTAL elements {tn} | distinct styles/page {:.1}% of elements | union across pages {} (u16 codeword covers all: {}) | cascade {:.0} ms | Chrome agreement {:.1}% ({h}/{t})",
        100.0 * tstyles as f64 / tn as f64, all_styles.len(), all_styles.len() < 65536, tms, 100.0 * h as f64 / t.max(1) as f64);
    let mut per: Vec<_> = sc.hit.iter().collect();
    per.sort_by(|a, b| (a.1.0 as f64 / a.1.1 as f64).partial_cmp(&(b.1.0 as f64 / b.1.1 as f64)).unwrap());
    for (k, (a, b)) in &per { println!("  {:<22} {:>5.1}%  ({a}/{b})", k, 100.0 * *a as f64 / *b as f64); }
    let mut miss: Vec<_> = sc.misses.into_iter().collect();
    miss.sort_by(|a, b| b.1.cmp(&a.1));
    println!("top mismatches:");
    for (k, v) in miss.iter().take(25) { println!("  {v:>5}  {k}"); }
    std::fs::create_dir_all("results").ok();
    std::fs::write("results/render0a-style.json", serde_json::to_string_pretty(&serde_json::json!({"pages": rows,
        "agree": h as f64 / t.max(1) as f64, "per_prop": sc.hit.iter().map(|(k, v)| (k.to_string(), v.0 as f64 / v.1 as f64)).collect::<std::collections::HashMap<_,_>>(),
        "union_styles": all_styles.len(), "elements": tn})).unwrap()).unwrap();
}

use page::{img_bytes, img_srcs};

/// Grayscale SSIM over 8×8 windows (Wang et al. 2004 constants), on the overlapping region.
fn ssim(a: &[u32], b: &[u8], w: usize, h: usize) -> f64 {
    let g = |c: u32| ((c >> 24) as f64 * 0.299 + ((c >> 16) & 255) as f64 * 0.587 + ((c >> 8) & 255) as f64 * 0.114);
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (mut tot, mut n) = (0f64, 0usize);
    for by in (0..h.saturating_sub(7)).step_by(8) { for bx in (0..w.saturating_sub(7)).step_by(8) {
        let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for y in by..by + 8 { for x in bx..bx + 8 {
            let va = g(a[y * w + x]);
            let i = (y * w + x) * 3;
            let vb = b[i] as f64 * 0.299 + b[i + 1] as f64 * 0.587 + b[i + 2] as f64 * 0.114;
            sa += va; sb += vb; saa += va * va; sbb += vb * vb; sab += va * vb;
        } }
        let (ma, mb) = (sa / 64.0, sb / 64.0);
        let (va, vb, cov) = (saa / 64.0 - ma * ma, sbb / 64.0 - mb * mb, sab / 64.0 - ma * mb);
        tot += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
        n += 1;
    } }
    if n == 0 { 0.0 } else { tot / n as f64 }
}

fn read_png_rgb(p: &std::path::Path) -> Option<(usize, usize, Vec<u8>)> {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(p).ok()?));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let ch = match info.color_type { png::ColorType::Rgba => 4, png::ColorType::Rgb => 3, _ => return None };
    let mut rgb = Vec::with_capacity(info.width as usize * info.height as usize * 3);
    for px in buf[..info.width as usize * info.height as usize * ch].chunks(ch) { rgb.extend_from_slice(&px[..3]); }
    Some((info.width as usize, info.height as usize, rgb))
}

fn write_png(p: &std::path::Path, w: usize, h: usize, img: &[u32]) {
    let f = std::fs::File::create(p).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    let mut wr = e.write_header().unwrap();
    let mut data = Vec::with_capacity(w * h * 3);
    for &c in &img[..w * h] { data.extend_from_slice(&[(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8]); }
    wr.write_image_data(&data).unwrap();
}

fn render_cmd(dir: &str, only: Option<&str>) {
    let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect();
    ents.sort();
    std::fs::create_dir_all("results/render").ok();
    let mut shared = raster::Store::default(); // one tile store across ALL pages: cross-tab dedupe
    let mut glyph_total = 0usize;
    let mut miss_kind: std::collections::HashMap<&'static str, usize> = Default::default();
    let mut width_kind: std::collections::HashMap<String, usize> = Default::default();
    let mut rows = Vec::new();
    let (mut s_vp, mut s_full, mut n_ok) = (0f64, 0f64, 0usize);
    let (mut lay_hit, mut lay_tot) = (0usize, 0usize);
    for p in ents {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(o) = only { if !name.contains(o) { continue; } }
        let b = match page::Bundle::open(&p) { Some(b) => b, None => continue };
        let base_mem = LIVE.load(Relaxed);
        PEAK.store(base_mem, Relaxed);
        let t0 = Instant::now();
        let mut l = page::load(&b, (1280.0, 800.0));
        let t_style = t0.elapsed().as_secs_f64() * 1e3;
        let mut base = b.url.clone();
        for i in 0..l.dom.nodes.len() as u32 { if l.dom.tag(i) == "base" { if let Some(h) = l.dom.attr(i, "href") { base = page::resolve(&b.url, h); break; } } }
        let dom_ref = &l.dom;
        let sizes = |n: u32| -> Option<(u32, u32)> {
            if dom_ref.tag(n) == "svg" { return svg::intrinsic(dom_ref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
            img_bytes(dom_ref, n, &base, &b).and_then(|bytes| img::size(&bytes))
        };
        let fonts = font::Fonts::new(); // per page: web families are page-scoped
        for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
        let t1 = Instant::now();
        let mut lay = layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
        // DOCTOR first load (doctor-core PROCEDURE §1, author 10-07): dormant unless OMR_DOCTOR=1 — then the T25 doctor core wakes for
        // this first layout, maps the page against its own pristine geometry and goes dormant again (map → results/doctor-map/)
        let doctor = std::env::var("OMR_DOCTOR").is_ok_and(|v| v == "1");
        #[cfg(feature = "t25")] if doctor { jit::t25::arm_pristine(Some(jit::t25::PristinePlate { page: name.clone(), only: None, wake: "first_load" })); }
        let page_h = lay.run();
        #[cfg(feature = "t25")]
        if doctor {
            jit::t25::arm_pristine(None);
            if let Some(m) = jit::t25::take_map() {
                std::fs::create_dir_all("results/doctor-map").ok();
                let rows: Vec<serde_json::Value> = m.defects.iter().map(|d| serde_json::json!({"node": d.node, "frame": d.frame,
                    "hits": d.hits.iter().map(|h| (jit::t25::KINDS[h.0], h.1)).collect::<Vec<_>>(), "err": d.err, "key": [&d.key.0, &d.key.1, d.key.2, d.key.3]})).collect();
                std::fs::write(format!("results/doctor-map/{name}.json"), serde_json::json!({"page": name, "wake": m.wake, "boxes": m.boxes, "pristine": m.pristine,
                    "by_kind": m.by_kind, "kernel": format!("{:016x}", m.kernel), "fire_us": m.fire_us, "layout_us": m.layout_us, "defects": rows}).to_string()).ok();
                eprintln!("doctor[first_load] {name}: {} boxes · pristine {} · defects {} · fire {:.0} µs", m.boxes, m.pristine, m.defects.len(), m.fire_us);
            }
        }
        let t_layout = t1.elapsed().as_secs_f64() * 1e3;
        let out = std::mem::take(&mut lay.out);
        drop(lay);
        if std::env::var("OMR_ITEMS").is_ok() {
            eprintln!("items {} boxes {}", out.items.len(), out.boxes.len());
            for it in out.items.iter().take(12) { let (x, y, w, h) = it.bounds(); eprintln!("  {:?} at {} {} {} {}", std::mem::discriminant(it), x / 64, y / 64, w / 64, h / 64); }
        }
        // decoded images cached at display size
        let cache: std::cell::RefCell<std::collections::HashMap<u32, Option<std::rc::Rc<(u32, u32, Vec<u32>)>>>> = Default::default();
        let disp: std::collections::HashMap<u32, (i32, i32)> = out.items.iter().filter_map(|i| if let layout::Item::Image { node, w, h, mode: 0, .. } = i { Some((*node, (*w, *h))) } else { None }).collect();
        let styled_ref = &l.styled;
        let bg_cache: std::cell::RefCell<std::collections::HashMap<u32, Option<std::rc::Rc<(u32, u32, Vec<u32>)>>>> = Default::default();
        let imgf = |n: u32, mode: u8, bw: i32, bh: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> {
            if mode == 0 {
                return cache.borrow_mut().entry(n).or_insert_with(|| {
                    let (w, h) = *disp.get(&n)?;
                    if dom_ref.tag(n) == "svg" {
                        let st = &styled_ref.table[styled_ref.ids[n as usize] as usize];
                        let css = svg::Css { fill: st.svg_fill, stroke: st.svg_stroke, sw: st.svg_sw.map(|x| x as f32 / style::FX as f32), color: st.color };
                        return svg::render_css(dom_ref.outer_xml(n).as_bytes(), (w / style::FX).max(1) as u32, (h / style::FX).max(1) as u32, css).map(std::rc::Rc::new);
                    }
                    let bytes = img_bytes(dom_ref, n, &base, &b)?;
                    img::decode(&bytes, (w / style::FX).max(1) as u32, (h / style::FX).max(1) as u32).map(std::rc::Rc::new)
                }).clone();
            }
            let st = &styled_ref.table[styled_ref.ids[n as usize] as usize];
            let sid = st.bg_img;
            // tile size: explicit background-size, else natural (raster ≤ the box for vectors: never decode larger than shown)
            let dim = |l: style::Len, base: i32| -> Option<i32> { match l { style::Len::Px(v) => Some((v / style::FX).max(1)), style::Len::Pct(p) => Some((base as i64 * p as i64 / 10000).max(1) as i32), _ => None } };
            let (dw, dh) = (dim(st.bg_dim[0], bw), dim(st.bg_dim[1], bh));
            let key = (sid as u32) << 8 ^ (dw.unwrap_or(0) as u32) << 20 ^ dh.unwrap_or(0) as u32 ^ (bw.min(4095) as u32) << 4;
            bg_cache.borrow_mut().entry(key).or_insert_with(|| {
                let raw = &styled_ref.families[sid as usize];
                let u = raw.strip_prefix("url(")?.trim_end_matches(')').trim().trim_matches('"').trim_matches('\'').to_string();
                if u.starts_with("data:") { return None; }
                // url() is relative to its stylesheet; the interned value lost that base, so resolve against the
                // document first, then fall back to the corpus manifest entry with the same path tail
                let bytes = b.get(&page::resolve(&base, &u)).or_else(|| {
                    let tail = u.trim_start_matches("../").trim_start_matches("./");
                    b.res.keys().find(|k| k.split('?').next().unwrap_or("").ends_with(tail)).and_then(|k| b.get(k))
                })?;
                if let Some((iw, ih, _)) = svg::intrinsic(&bytes) {
                    // vector background: render at the tile size it is shown at
                    let (tw, th) = match (dw, dh) {
                        (Some(w), Some(h)) => (w, h),
                        (Some(w), None) => (w, (w as f32 * ih / iw.max(1e-3)) as i32),
                        (None, Some(h)) => ((h as f32 * iw / ih.max(1e-3)) as i32, h),
                        _ => ((iw as i32).min(bw.max(1)), (ih as i32).min(bh.max(1))),
                    };
                    return svg::render_at(&bytes, tw.clamp(1, 2048) as u32, th.clamp(1, 2048) as u32).map(std::rc::Rc::new);
                }
                let full = img::decode(&bytes, 2048, 2048)?;
                match (dw, dh) {
                    (Some(w), Some(h)) => img::decode(&bytes, w as u32, h as u32).map(std::rc::Rc::new),
                    _ => Some(std::rc::Rc::new(full)),
                }
            }).clone()
        };
        let full_h = ((page_h / style::FX) as usize).clamp(800, 8000);
        let tiles_before = shared.tiles.len();
        let bytes_before = shared.bytes();
        let mut own = raster::Store::default();
        let vp = raster::render(&out.items, &fonts, &imgf, 1280, 0, 800, &mut own);
        let vp_bytes = own.bytes();
        // resident RAM of a visible tab = live heap now: DOM + rules + styles + display list + first-screen tiles
        // + glyph cache + web fonts + images decoded at display size (minus the transient bytes of the HTML input)
        let resident_all = LIVE.load(Relaxed).saturating_sub(base_mem);
        // the parsed rule set is only needed to restyle; a static document can drop it after the cascade
        let before_sheet = LIVE.load(Relaxed);
        let sheet_rules = l.sheet.rules.len();
        drop(std::mem::replace(&mut l.sheet, css::Sheet::new()));
        let sheet_b = before_sheet.saturating_sub(LIVE.load(Relaxed));
        let resident = resident_all - sheet_b;
        let dom_b = l.dom.heap_bytes();
        let items_b = out.items.capacity() * std::mem::size_of::<layout::Item>() + out.items.iter().map(|i| if let layout::Item::Text { glyphs, .. } = i { glyphs.capacity() * 4 } else { 0 }).sum::<usize>();
        let style_b = l.styled.table.len() * std::mem::size_of::<style::Style>() + l.styled.ids.len() * 2;
        let full = raster::render(&out.items, &fonts, &imgf, 1280, 0, full_h, &mut own);
        let _ = raster::render(&out.items, &fonts, &imgf, 1280, 0, full_h, &mut shared);
        let t_total = t0.elapsed().as_secs_f64() * 1e3;
        // RENDER-0c: scripted scroll — 40 px wheel steps to the bottom, then 400 px page-ups back to the top
        let mut sstore = raster::Store::default();
        let mut sc = scroll::Scroller::new(&out.items, &fonts, &imgf, 1280, 800, full_h, &mut sstore);
        let mut frames: Vec<f64> = Vec::new();
        // thin client: per step, the server sends only tiles the client has never seen (content-addressed) + one
        // 4-byte id per newly exposed tile slot
        let (mut sent_tiles, mut sent_bytes, mut sent_ids) = (sstore.tiles.len(), sstore.bytes(), 0usize);
        let initial_bytes = sent_bytes;
        loop {
            let t = Instant::now(); let before_rows = sc.rasterized_rows;
            let n = sc.scroll(40, &mut sstore); if n == 0 { break; }
            frames.push(t.elapsed().as_secs_f64() * 1e3);
            sent_ids += (sc.rasterized_rows - before_rows) * 1280usize.div_ceil(raster::T) * 4;
        }
        let stream_bytes = sstore.bytes() - sent_bytes + sent_ids;
        sent_tiles = sstore.tiles.len() - sent_tiles; sent_bytes = stream_bytes;
        let steps = frames.len().max(1);
        let first_pass = frames.len();
        loop { let t = Instant::now(); let n = sc.scroll(-400, &mut sstore); if n == 0 { break; } frames.push(t.elapsed().as_secs_f64() * 1e3); }
        let mut sorted = frames.clone(); sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (f_med, f_p95, f_max) = if sorted.is_empty() { (0.0, 0.0, 0.0) } else { (sorted[sorted.len() / 2], sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)], *sorted.last().unwrap()) };
        let back_max = frames[first_pass..].iter().cloned().fold(0.0, f64::max);
        let mem_peak = PEAK.load(Relaxed) - base_mem;
        let img = raster::compose(&full, &own, 1280);
        write_png(std::path::Path::new(&format!("results/render/{name}.png")), 1280, full_h.min(full.th * raster::T), &img);
        let (mut sv, mut sf) = (0.0, 0.0);
        if let Some((cw, ch, rgb)) = read_png_rgb(&p.join("chrome.png")) {
            if cw == 1280 {
                sv = ssim(&img, &rgb, 1280, 800.min(ch));
                sf = ssim(&img, &rgb, 1280, ch.min(full_h));
            }
        }
        // layout vs Chrome bounds (aligned elements, box within 2 px or 5%)
        let (mut lh, mut lt) = (0, 0);
        let mut first_div: Option<String> = None;
        let mut divs_shown = 0usize;
        let mut width_ok: std::collections::HashSet<u32> = Default::default();
        width_ok.insert(0);
        if let Some(ch) = verify::chrome_elements(&p.join("chrome-snap.json")) {
            let mut mine: std::collections::HashMap<u32, [i32; 4]> = Default::default();
            for (n, bx) in &out.boxes { mine.entry(*n).or_insert(*bx); }
            let ours_el: Vec<u32> = (0..l.dom.nodes.len() as u32).filter(|&i| l.dom.nodes[i as usize].kind == dom::Kind::Element && !l.dom.tag(i).starts_with("::")).collect();
            let (mut i, mut j) = (0, 0);
            while i < ours_el.len() && j < ch.len() {
                if l.dom.tag(ours_el[i]) != ch[j].tag {
                    let mut f = false;
                    for k in 1..8 { if j + k < ch.len() && l.dom.tag(ours_el[i]) == ch[j + k].tag { j += k; f = true; break; }
                                    if i + k < ours_el.len() && l.dom.tag(ours_el[i + k]) == ch[j].tag { i += k; f = true; break; } }
                    if !f { i += 1; j += 1; continue; }
                }
                if let (Some(cb), Some(ob)) = (ch[j].bounds, mine.get(&ours_el[i])) {
                    if cb[2] > 0.0 && cb[3] > 0.0 && l.styled.table[l.styled.ids[ours_el[i] as usize] as usize].display != style::Display::Inline {
                        lt += 1;
                        let o = [ob[0] as f32 / 64.0, ob[1] as f32 / 64.0, ob[2] as f32 / 64.0, ob[3] as f32 / 64.0];
                        let ok = (0..4).all(|k| (o[k] - cb[k]).abs() <= 2.0f32.max(cb[k].abs() * 0.05));
                        let ndiv: usize = std::env::var("OMR_DIV").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
                        if !ok && divs_shown < ndiv && !matches!(l.dom.tag(ours_el[i]), "html" | "body") {
                            divs_shown += 1;
                            let cls = l.dom.attr(ours_el[i], "class").unwrap_or("").chars().take(40).collect::<String>();
                            println!("    div#{divs_shown} <{}.{}> ours [{:.0} {:.0} {:.0} {:.0}] chrome [{:.0} {:.0} {:.0} {:.0}]", l.dom.tag(ours_el[i]), cls, o[0], o[1], o[2], o[3], cb[0], cb[1], cb[2], cb[3]);
                        }
                        if !ok {
                            let bad: Vec<bool> = (0..4).map(|k| (o[k] - cb[k]).abs() > 2.0f32.max(cb[k].abs() * 0.05)).collect();
                            let key = match (bad[0], bad[1], bad[2], bad[3]) { (false, true, false, false) => "y only", (_, _, true, _) => "width", (_, _, false, true) => "height (width ok)", (true, _, false, false) => "x (size ok)", _ => "other" };
                            *miss_kind.entry(key).or_insert(0usize) += 1;
                            let pd0 = l.dom.nodes[ours_el[i] as usize].parent;
                            if key == "width" && width_ok.contains(&pd0) {
                                let st = &l.styled.table[l.styled.ids[ours_el[i] as usize] as usize];
                                let pd = pd0;
                                let pdisp = if pd != dom::NONE && l.dom.nodes[pd as usize].kind == dom::Kind::Element { format!("{:?}", l.styled.table[l.styled.ids[pd as usize] as usize].display) } else { "-".into() };
                                let k = format!("{:?} in {} {}", st.display, pdisp, if o[2] > cb[2] { "too wide" } else { "too narrow" });
                                *width_kind.entry(k.clone()).or_insert(0usize) += 1;
                                if std::env::var("OMR_SAMPLE").map_or(false, |q| k == q) {
                                    let cls = l.dom.attr(ours_el[i], "class").unwrap_or("").chars().take(30).collect::<String>();
                                    let pcls = l.dom.attr(pd, "class").unwrap_or("").chars().take(30).collect::<String>();
                                    let ps = &l.styled.table[l.styled.ids[pd as usize] as usize];
                                    println!("    SAMPLE {} <{}.{}> w ours {:.0} chrome {:.0} | grow {} shrink {} basis {:?} width {:?} | parent .{} dir {} wrap {} align {}", name, l.dom.tag(ours_el[i]), cls, o[2], cb[2], st.grow, st.shrink, st.basis, st.width, pcls, ps.flex_dir, ps.flex_wrap, ps.align_items);
                                }
                            }
                        }
                        if (o[2] - cb[2]).abs() <= 2.0f32.max(cb[2].abs() * 0.05) { width_ok.insert(ours_el[i]); }
                        if ok { lh += 1; } else if first_div.is_none() && !matches!(l.dom.tag(ours_el[i]), "html" | "body") {
                            let cls = l.dom.attr(ours_el[i], "class").unwrap_or("").split_whitespace().next().unwrap_or("").chars().take(30).collect::<String>();
                            first_div = Some(format!("<{}.{}> ours [{:.0} {:.0} {:.0} {:.0}] chrome [{:.0} {:.0} {:.0} {:.0}]", l.dom.tag(ours_el[i]), cls, o[0], o[1], o[2], o[3], cb[0], cb[1], cb[2], cb[3]));
                        }
                    }
                }
                i += 1; j += 1;
            }
        }
        lay_hit += lh; lay_tot += lt;
        if let Some(fd) = &first_div { println!("  first divergence: {fd}"); }
        if let Ok(q) = std::env::var("OMR_BOXES") {
            // debug: our boxes vs Chrome's for elements whose tag or class contains q
            if let Some(ch) = verify::chrome_elements(&p.join("chrome-snap.json")) {
                let ours_el: Vec<u32> = (0..l.dom.nodes.len() as u32).filter(|&i| l.dom.nodes[i as usize].kind == dom::Kind::Element && !l.dom.tag(i).starts_with("::")).collect();
                let mut mine: std::collections::HashMap<u32, [i32; 4]> = Default::default();
                for (nn, bx) in &out.boxes { mine.entry(*nn).or_insert(*bx); }
                let mut shown = 0;
                for (k, &e) in ours_el.iter().enumerate() {
                    let cls = l.dom.attr(e, "class").unwrap_or("");
                    if !(l.dom.tag(e) == q.as_str() || cls.split_whitespace().any(|c| c == q.as_str())) || shown > 25 { continue; }
                    shown += 1;
                    let o = mine.get(&e).map(|b| format!("[{} {} {} {}]", b[0] / 64, b[1] / 64, b[2] / 64, b[3] / 64)).unwrap_or("-".into());
                    let c = ch.get(k).and_then(|c| if c.tag == l.dom.tag(e) { c.bounds } else { None }).map(|b| format!("[{:.0} {:.0} {:.0} {:.0}]", b[0], b[1], b[2], b[3])).unwrap_or("?".into());
                    let st = &l.styled.table[l.styled.ids[e as usize] as usize];
                    println!("    <{} .{}> ours {} chrome~ {} disp {:?} float {}", l.dom.tag(e), &cls[..cls.len().min(24)], o, c, st.display, st.float);
                }
            }
        }
        glyph_total += fonts.cache_bytes();
        let new_shared = shared.tiles.len() - tiles_before;
        let full_tiles = full.tw * full.th;
        let img_b: usize = cache.borrow().values().flatten().map(|i| i.2.len() * 4).sum::<usize>() + bg_cache.borrow().values().flatten().map(|i| i.2.len() * 4).sum::<usize>();
        println!("  decoded images {} KB ({} replaced, {} backgrounds)", img_b / 1024, cache.borrow().values().flatten().count(), bg_cache.borrow().values().flatten().count());
        // hit testing: grid vs linear over 100k pseudo-random points, results must agree
        let hg = hit::HitGrid::build(&out.boxes, 1280 * style::FX, page_h);
        let mut rng = 0x2545F4914F6CDD1Du64;
        let pts: Vec<(i32, i32)> = (0..100_000).map(|_| { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (((rng >> 8) % (1280 * 64)) as i32, ((rng >> 32) % (page_h.max(64) as u64)) as i32) }).collect();
        let th = Instant::now(); let mut acc = 0u64; for &(x, y) in &pts { acc += std::hint::black_box(hg.hit(std::hint::black_box(x), y)).unwrap_or(0) as u64; } let hit_ns = th.elapsed().as_secs_f64() * 1e9 / pts.len() as f64;
        let tl = Instant::now(); let mut acc2 = 0u64; for &(x, y) in pts.iter().take(5000) { acc2 += std::hint::black_box(hg.hit_linear(std::hint::black_box(x), y)).unwrap_or(0) as u64; } let lin_ns = tl.elapsed().as_secs_f64() * 1e9 / 5000.0;
        let agree = pts.iter().take(5000).all(|&(x, y)| hg.hit(x, y) == hg.hit_linear(x, y));
        let _ = (acc, acc2);
        println!("  hit test: grid {:.0} ns/query vs linear {:.0} ns ({} boxes) · agree {} · grid {} KB", hit_ns, lin_ns, out.boxes.len(), agree, hg.bytes() / 1024);
        // background tab: signed packed snapshot (tiles dropped) — size, pack/restore time, tamper check
        let key = *b"omni-render-key!";
        let tp = Instant::now(); let blob = snapshot::pack(&l.dom, &out.items, &key); let pack_ms = tp.elapsed().as_secs_f64() * 1e3;
        let tr = Instant::now(); let back = snapshot::unpack(&blob, &key); let restore_ms = tr.elapsed().as_secs_f64() * 1e3;
        let ok = back.as_ref().map_or(false, |(d2, it2)| d2.nodes.len() == l.dom.nodes.len() && it2.len() == out.items.len());
        let mut bad = blob.clone(); let mid = bad.len() / 2; bad[mid] ^= 1;
        let tamper_refused = snapshot::unpack(&bad, &key).is_none();
        drop(back);
        println!("  background tab: snapshot {} KB (pack {:.1} ms, restore {:.1} ms, roundtrip {}, tamper refused {})", blob.len() / 1024, pack_ms, restore_ms, ok, tamper_refused);
        println!("  rule set {} KB ({} rules, dropped after cascade)", sheet_b / 1024, sheet_rules);
        println!("  resident {} KB (DOM {} KB, styles {} KB, display list {} KB, first-screen tiles {} KB, web fonts {} KB) | peak {} KB",
            resident / 1024, dom_b / 1024, style_b / 1024, items_b / 1024, vp_bytes / 1024, fonts.web_bytes.get() / 1024, mem_peak / 1024);
        println!("  thin client: first screen {} KB, then {:.1} KB per 40 px step ({} new tiles over {} steps) vs {:.0} KB per step as raw RGBA",
            initial_bytes / 1024, sent_bytes as f64 / 1024.0 / steps as f64, sent_tiles, steps, 1280.0 * 40.0 * 4.0 / 1024.0);
        println!("  scroll: {} frames, median {:.2} ms p95 {:.2} max {:.2} | page-up (cached rows) max {:.2} ms | rows rasterized {}", frames.len(), f_med, f_p95, f_max, back_max, sc.rasterized_rows);
        println!("{:<40} layout {:>6.1}ms style+parse {:>6.1}ms raster {:>6.1}ms | vp {:>4}KB ({:.0}x<RGBA) | full {}x{} tiles {} uniq {} ({:.0}%) {}KB | +shared {} | peak {:>6}KB | SSIM vp {:.3} full {:.3} | layout {:>4.1}% of {}",
            &name[..name.len().min(40)], t_layout, t_style, vp.ms, vp_bytes / 1024, (1280.0 * 800.0 * 4.0) / vp_bytes.max(1) as f64,
            full.tw, full.th, full_tiles, own.tiles.len(), 100.0 * own.tiles.len() as f64 / full_tiles.max(1) as f64, own.bytes() / 1024,
            new_shared, mem_peak / 1024, sv, sf, 100.0 * lh as f64 / lt.max(1) as f64, lt);
        let _ = bytes_before;
        if sv > 0.0 { s_vp += sv; s_full += sf; n_ok += 1; }
        rows.push(serde_json::json!({"page": name, "layout_ms": t_layout, "style_ms": t_style, "raster_vp_ms": vp.ms, "total_ms": t_total,
            "vp_bytes": vp_bytes, "full_tiles": full_tiles, "unique_tiles": own.tiles.len(), "full_bytes": own.bytes(),
            "ssim_vp": sv, "ssim_full": sf, "layout_agree": lh as f64 / lt.max(1) as f64, "layout_n": lt, "peak_bytes": mem_peak,
            "glyph_cache_bytes": fonts.cache_bytes(), "web_font_bytes": fonts.web_bytes.get(), "web_fonts": l.fonts.len(),
            "snapshot_bytes": blob.len(), "snapshot_restore_ms": restore_ms, "snapshot_ok": ok, "tamper_refused": tamper_refused, "hit_ns": hit_ns, "hit_linear_ns": lin_ns, "hit_agree": agree, "resident_bytes": resident, "dom_bytes": dom_b, "style_bytes": style_b, "items_bytes": items_b, "stream_first_kb": initial_bytes / 1024, "stream_kb_per_step": sent_bytes as f64 / 1024.0 / steps as f64, "scroll_frames": frames.len(), "scroll_med_ms": f_med, "scroll_p95_ms": f_p95, "scroll_max_ms": f_max, "pageup_max_ms": back_max}));
    }
    let mut mk: Vec<_> = miss_kind.iter().collect(); mk.sort_by(|a, b| b.1.cmp(a.1));
    let mut wk: Vec<_> = width_kind.iter().collect(); wk.sort_by(|a, b| b.1.cmp(a.1));
    println!("ORIGIN width mismatches (parent width right) by (display in parent display): {}", wk.iter().take(14).map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(" · "));
    println!("mismatch kinds: {}", mk.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(" · "));
    println!("MEAN SSIM viewport {:.3} full {:.3} over {} pages | layout box agreement {:.1}% ({}/{}) | shared store {} tiles {} KB across all pages | glyph cache {} KB",
        s_vp / n_ok.max(1) as f64, s_full / n_ok.max(1) as f64, n_ok, 100.0 * lay_hit as f64 / lay_tot.max(1) as f64, lay_hit, lay_tot,
        shared.tiles.len(), shared.bytes() / 1024, glyph_total / 1024);
    std::fs::write("results/render0b.json", serde_json::to_string_pretty(&serde_json::json!({"pages": rows,
        "ssim_vp": s_vp / n_ok.max(1) as f64, "ssim_full": s_full / n_ok.max(1) as f64, "layout_agree": lay_hit as f64 / lay_tot.max(1) as f64,
        "shared_tiles": shared.tiles.len(), "shared_bytes": shared.bytes()})).unwrap()).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("bench") => bench(a.get(2).map(|s| s.as_str()).unwrap_or("corpus")),
        Some("fastparse") => fastparse::cli(&a[2..]),
        Some("incr") => incr::run(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).and_then(|s| s.parse().ok()).unwrap_or(6)),
        Some("style") => style_cmd(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).map(|s| s.as_str())),
        Some("render") => render_cmd(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).map(|s| s.as_str())),
        Some("agent") => {
            // agent <url|@listfile> [--out dir]: fetch over HTTP, style, emit visible-content markdown; reports pages/s
            let client = net::Client::new();
            let mut urls: Vec<String> = Vec::new();
            for x in &a[2..] { if x == "--out" { break; } if let Some(f) = x.strip_prefix('@') { urls.extend(std::fs::read_to_string(f).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())); } else { urls.push(x.clone()); } }
            let out_dir = a.iter().position(|x| x == "--out").and_then(|i| a.get(i + 1)).cloned();
            let t = Instant::now();
            let (mut ok, mut chars) = (0, 0);
            let (mut t_parse, mut t_css, mut t_casc, mut t_md, mut t_net, mut t_match) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for u in &urls {
                let tn = Instant::now();
                let live = match page::Live::fetch(&client, u) { Some(l) => l, None => { eprintln!("fetch failed {u}"); continue } };
                t_net += tn.elapsed().as_secs_f64();
                let l = page::load(&live, (1280.0, 800.0));
                t_parse += l.parse_ms / 1e3; t_css += l.css_ms / 1e3; t_casc += l.styled.stats.ms / 1e3; t_match += l.styled.stats.match_ms / 1e3;
                let tm = Instant::now();
                let md = agent::markdown(&l.dom, &l.styled, &live.url);
                t_md += tm.elapsed().as_secs_f64();
                chars += md.len();
                ok += 1;
                match &out_dir {
                    Some(d) => { std::fs::create_dir_all(d).ok(); let name: String = u.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect(); std::fs::write(format!("{d}/{}.md", &name[name.len().saturating_sub(60)..]), &md).ok(); }
                    None => if urls.len() == 1 { print!("{md}"); },
                }
            }
            let secs = t.elapsed().as_secs_f64();
            eprintln!("network wait total {:.2}s (server side, not engine CPU)", client.wait_s.get());
            eprintln!("phases: doc fetch {:.2}s | html parse {:.2}s | css fetch+parse {:.2}s | cascade {:.2}s (matching {:.2}s) | markdown {:.2}s", t_net, t_parse, t_css, t_casc, t_match, t_md);
            eprintln!("agent: {ok}/{} pages in {:.2}s = {:.1} pages/s | {} requests {} KB | {} chars out | live heap peak {} KB",
                urls.len(), secs, ok as f64 / secs, client.requests.get(), client.bytes.get() / 1024, chars, PEAK.load(Relaxed) / 1024);
        }
        Some("cssbench") => {
            // cssbench <page dir>: cost split of the page's stylesheets — tokenize vs rule parse (with the DOM key filter)
            let b = page::Bundle::open(std::path::Path::new(&a[2])).unwrap();
            let d = dom::Dom::parse(&b.html());
            let m = css::Media { width: 1280.0, height: 800.0 };
            let css_all: Vec<String> = b.res.iter().filter(|(_, v)| v.1 == "stylesheet").filter_map(|(u, _)| b.get(u)).map(|x| String::from_utf8_lossy(&x).into_owned()).collect();
            let total: usize = css_all.iter().map(|x| x.len()).sum();
            let t = Instant::now(); let mut nt = 0; for c in &css_all { nt += css::tokenize(c).len(); } let tk = t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now(); let mut sh = css::Sheet::new(); let mut o = 0; for c in &css_all { sh.add(c, &m, &mut o); } let full = t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now(); let mut sh2 = css::Sheet::new(); sh2.filter = Some(css::KeyFilter::from_dom(&d)); let mut o = 0; for c in &css_all { sh2.add(c, &m, &mut o); } let filt = t.elapsed().as_secs_f64() * 1e3;
            let big: Vec<(usize, String)> = { let mut v: Vec<(usize, String)> = sh2.by_tag.iter().map(|(k, v)| (v.len(), format!("tag {k}"))).chain(sh2.by_class.iter().map(|(k, v)| (v.len(), format!(".{k}")))).collect(); v.sort(); v.reverse(); v.truncate(6); v };
            println!("universal bucket {} rules · biggest buckets {:?}", sh2.universal.len(), big);
            println!("css {} KB, {} tokens | tokenize-all {:.1} ms | parse unfiltered {:.1} ms ({} rules) | parse filtered {:.1} ms ({} rules kept, {} skipped)",
                total / 1024, nt, tk, full, sh.rules.len(), filt, sh2.rules.len(), sh2.skipped);
        }
        Some("renderer") => {
            // sandboxed child: lock down first, then read the document frame from the broker
            let mode = sandbox::lockdown();
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let (url, doc) = {
                let (_, u) = sandbox::read_frame(&mut stdin.lock()).expect("url frame");
                let (_, d) = sandbox::read_frame(&mut stdin.lock()).expect("doc frame");
                (String::from_utf8_lossy(&u).into_owned(), d)
            };
            let probes = sandbox::probes();
            let src = sandbox::PipeSource { url: url.clone(), doc, io: std::cell::RefCell::new((stdin, stdout)) };
            let l = page::load(&src, (1280.0, 800.0));
            let md = agent::markdown(&l.dom, &l.styled, &url);
            let report = format!("{}\n{}", match &mode { Ok(m) => m.clone(), Err(e) => format!("LOCKDOWN FAILED: {e}") },
                probes.iter().map(|(k, ok)| format!("{}: {}", k, if *ok { "PASS" } else { "FAIL" })).collect::<Vec<_>>().join("\n"));
            let (_, o) = &mut *src.io.borrow_mut();
            sandbox::write_frame(&mut o.lock(), b'S', report.as_bytes()).ok();
            sandbox::write_frame(&mut o.lock(), b'R', md.as_bytes()).ok();
        }
        Some("page-renderer") => { pageproc::serve(); jscomp::forget_store(); }
        Some("js-parse") => jsparse::cli(&a[2..]),
        Some("roundtrip") => {
            // live tree → HTML → parse must give the same document (JS-0 re-render path): first differing dump line
            let d = dom::Dom::parse(&std::fs::read(&a[2]).unwrap());
            let html = jsdom::Live::from_dom(&d, "https://x/").serialize(d.quirks);
            let d2 = dom::Dom::parse(html.as_bytes());
            let strip = |s: String| s.lines().map(|l| { let mut o = l.to_string(); while let Some(p) = o.find(" data-omr=\"") { let e = o[p + 11..].find('"').map(|q| p + 12 + q).unwrap_or(o.len()); o.replace_range(p..e, ""); } o }).collect::<Vec<_>>();
            let (x, y) = (strip(d.dump()), strip(d2.dump()));
            println!("nodes {} vs {} · sheets {} vs {} · quirks {} vs {}", d.nodes.len(), d2.nodes.len(), d.sheets.len(), d2.sheets.len(), d.quirks, d2.quirks);
            if let Some(i) = (0..x.len().min(y.len())).find(|&i| x[i] != y[i]) { for k in i.saturating_sub(3)..(i + 4).min(x.len()) { println!("  a {}", x[k].chars().take(140).collect::<String>()); } for k in i.saturating_sub(3)..(i + 4).min(y.len()) { println!("  b {}", y[k].chars().take(140).collect::<String>()); } } else { println!("same dump ({} lines)", x.len()); }
            if a.len() > 3 { std::fs::write(&a[3], html).unwrap(); }
        }
        Some("js-src") => {
            // JS source → jsparse (the run-time front-end) → the same interpreter `js` runs: the in-sandbox path
            let src = std::fs::read_to_string(&a[2]).expect("js file");
            let t0 = Instant::now();
            let prog = match jsparse::parse(&src, !a.iter().any(|x| x == "--script")) { Ok(p) => p, Err(e) => { eprintln!("{e}"); std::process::exit(2); } };
            if std::env::var("OMR_TIME").is_ok() { eprintln!("parse {:.0} ms", t0.elapsed().as_secs_f64() * 1e3); let t1 = Instant::now(); let n = jsi_ast::program(&prog["body"]); eprintln!("lower {:.0} ms ({} stmts)", t1.elapsed().as_secs_f64() * 1e3, n.len()); fn walk(v: &serde_json::Value, seen: &mut std::collections::HashMap<u64, u32>, tot: &mut usize) -> u64 { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); match v { serde_json::Value::Object(o) => { *tot += 1; 1u8.hash(&mut h); for (k, x) in o { k.hash(&mut h); walk(x, seen, tot).hash(&mut h); } let d = h.finish(); *seen.entry(d).or_insert(0) += 1; return d; } serde_json::Value::Array(a) => { 2u8.hash(&mut h); for x in a { walk(x, seen, tot).hash(&mut h); } } o => { 3u8.hash(&mut h); o.to_string().hash(&mut h); } } h.finish() } let mut seen = std::collections::HashMap::new(); let mut tot = 0usize; walk(&prog, &mut seen, &mut tot); let uniq = seen.len(); let dup_nodes: usize = seen.values().map(|&c| (c as usize).saturating_sub(1)).sum(); eprintln!("nodes {tot} distinct subtrees {uniq} ({:.1} % of nodes are repeats of an earlier identical subtree)", 100.0 * dup_nodes as f64 / tot as f64); }
            let mut it = jsi::Interp::new();
            let r = it.run_program(&prog);
            print!("{}", it.out);
            if let Some(e) = r { eprintln!("Uncaught {e}"); std::process::exit(1); }
        }
        Some("page") => {
            // BROWSER-BODY-0 step 2 check: one page through the broker + the long-lived locked-down page process.
            // `page <url> [--w W] [--h H] [--png out.png] [--y Y] [--find TEXT] [--hit X,Y]`
            let flag = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
            let (w, h) = (flag("--w").and_then(|v| v.parse().ok()).unwrap_or(1280usize), flag("--h").and_then(|v| v.parse().ok()).unwrap_or(800usize));
            let b = pageproc::Broker::new();
            let t = Instant::now();
            let Some((u, doc)) = pageproc::fetch_doc(&b, &a[2]) else { eprintln!("fetch failed: {}", b.client.last_error.borrow()); std::process::exit(2) };
            eprintln!("http {}", b.status.get());
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.arg("page-renderer");
            let mut p = pageproc::PageProc::open(cmd, &b, &u, doc, w, h).expect("page process");
            // the page's scripts run on the first tick (the window does that asynchronously; here we wait for it)
            if p.meta["scripts_pending"] == true { let r = p.tick(&b, 0); if r.get("relayout").is_none() { p.meta["js"] = r["js"].clone(); } }
            // `--ticks N [--tick-ms M]`: N more clock ticks (timers, fetch callbacks, app boot) as the window would give a live page
            if let Some(n) = flag("--ticks").and_then(|v| v.parse::<usize>().ok()) {
                let ms = flag("--tick-ms").and_then(|v| v.parse::<i64>().ok()).unwrap_or(100);
                for _ in 0..n { std::thread::sleep(std::time::Duration::from_millis(ms.clamp(0, 5000) as u64)); let r = p.tick(&b, ms); if std::env::var_os("OMR_TICK_TRACE").is_some() { eprintln!("tick {}", r.to_string().chars().take(400).collect::<String>()); } if r.get("relayout").is_none() { if let Some(j) = r.get("js") { p.meta["js"] = j.clone(); } } }
            }
            eprintln!("{}", p.report);
            println!("meta {}", p.meta);
            let y = flag("--y").and_then(|v| v.parse().ok()).unwrap_or(0);
            let s = p.frame(&b, y).expect("frame");
            println!("frame y {} of {} · {}x{} · {:.1} ms total · renderer rss {} kB · {} fetches served, {} refused",
                     s.y, s.page_h, s.w, s.h, t.elapsed().as_secs_f64() * 1e3, p.rss_kb(), b.served.get(), b.refused.get());
            if let Some(q) = flag("--find") { println!("find {}", p.find(&b, &q)); }
            if a.iter().any(|x| x == "--a11y") {
                // the accessibility tree a screen reader gets: role counts, then the first rows
                let nodes = p.a11y(&b);
                let mut by: std::collections::BTreeMap<String, usize> = Default::default();
                for n in &nodes { *by.entry(n[0].as_str().unwrap_or("?").to_string()).or_default() += 1; }
                println!("a11y {} nodes {:?}", nodes.len(), by);
                let lim = flag("--a11y-rows").and_then(|v| v.parse().ok()).unwrap_or(25usize);
                for (k, n) in nodes.iter().take(lim).enumerate() { println!("  {k:>4} {:<13} p={:<4} {:?} {}", n[0].as_str().unwrap_or(""), n[2], n[1].as_str().unwrap_or("").chars().take(70).collect::<String>(), n[7]); }
            }
            if let Some(xy) = flag("--hit") { let (x, yy) = xy.split_once(',').unwrap(); println!("hit {}", p.hit(&b, x.parse().unwrap(), yy.parse().unwrap())); }
            let mut s = s;
            for (k, v) in a.iter().enumerate() { if v == "--click" { let (x, yy) = a[k + 1].split_once(',').unwrap(); println!("click {} → {}", a[k + 1], p.click(&b, x.parse().unwrap(), yy.parse().unwrap())); s = p.frame(&b, y).expect("frame"); } }
            if let Some(path) = flag("--png") {
                let f = std::fs::File::create(&path).unwrap();
                let mut e = png::Encoder::new(std::io::BufWriter::new(f), s.w as u32, s.h as u32);
                e.set_color(png::ColorType::Rgb);
                e.set_depth(png::BitDepth::Eight);
                let mut wr = e.write_header().unwrap();
                let data: Vec<u8> = s.px.iter().flat_map(|c| [(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8]).collect();
                wr.write_image_data(&data).unwrap();
                println!("png {path}");
            }
        }
        Some("safe") => {
            // broker: owns the network; spawns the locked-down renderer per page and serves its fetches
            let client = net::Client::new();
            let url = a[2].clone();
            let t = Instant::now();
            let (final_url, doc) = client.get(&url).expect("fetch");
            let mut child = std::process::Command::new(std::env::current_exe().unwrap()).arg("renderer")
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("spawn renderer");
            let mut cin = child.stdin.take().unwrap();
            let mut cout = child.stdout.take().unwrap();
            sandbox::write_frame(&mut cin, b'U', final_url.as_bytes()).unwrap();
            sandbox::write_frame(&mut cin, b'D', &doc).unwrap();
            let (mut served, mut refused) = (0, 0);
            while let Some((tag, data)) = sandbox::read_frame(&mut cout) {
                match tag {
                    b'G' => {
                        let u = String::from_utf8_lossy(&data).into_owned();
                        // fetch policy: http(s) only, no local files, size cap enforced by net/frame
                        let ok = u.starts_with("http://") || u.starts_with("https://");
                        match if ok { client.get(&u) } else { None } {
                            Some((_, b)) => { served += 1; sandbox::write_frame(&mut cin, b'B', &b).unwrap(); }
                            None => { refused += 1; sandbox::write_frame(&mut cin, b'N', &[]).unwrap(); }
                        }
                    }
                    b'S' => eprintln!("{}", String::from_utf8_lossy(&data)),
                    b'R' => { print!("{}", String::from_utf8_lossy(&data)); break; }
                    _ => break,
                }
            }
            let st = child.wait().unwrap();
            eprintln!("safe: renderer exit {:?}, {} fetches served, {} refused, {:.1} ms", st.code(), served, refused, t.elapsed().as_secs_f64() * 1e3);
        }
        Some("fuzz") => fuzz::run(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).and_then(|s| s.parse().ok()).unwrap_or(60)),
        Some("fuzz-worker") => fuzz::worker(),
        Some("fuzz-repro") => fuzz::repro(&a[2]),
        Some("paint") => {
            // cold start: process entry → first screen of tiles, from a local bundle. `--eink` writes the
            // first screen as 2-bit grayscale (4 levels) — the native output of the 2-bit raster on e-ink panels.
            let t0 = Instant::now();
            let dir = std::path::Path::new(&a[2]);
            let b = page::Bundle::open(dir).expect("bundle");
            let l = page::load(&b, (1280.0, 800.0));
            let fonts = font::Fonts::new();
            for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
            let base = b.url.clone();
            let dom_ref = &l.dom;
            let sizes = |n: u32| -> Option<(u32, u32)> {
                if dom_ref.tag(n) == "svg" { return svg::intrinsic(dom_ref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
                img_bytes(dom_ref, n, &base, &b).and_then(|bytes| img::size(&bytes))
            };
            let mut lay = layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
            lay.run();
            let out = std::mem::take(&mut lay.out);
            drop(lay);
            let pcache: std::cell::RefCell<std::collections::HashMap<u32, Option<std::rc::Rc<(u32, u32, Vec<u32>)>>>> = Default::default();
            let imgf = |n: u32, mode: u8, w: i32, h: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> {
                if mode != 0 { return None; }
                pcache.borrow_mut().entry(n).or_insert_with(|| img_bytes(dom_ref, n, &base, &b).and_then(|bytes| img::decode(&bytes, w.max(1) as u32, h.max(1) as u32)).map(std::rc::Rc::new)).clone()
            };
            let mut store = raster::Store::default();
            let vp = raster::render(&out.items, &fonts, &imgf, 1280, 0, 800, &mut store);
            let first_paint = t0.elapsed().as_secs_f64() * 1e3;
            println!("first paint {:.1} ms (parse+style {:.1} ms, raster {:.1} ms) · tiles {} unique of {} · {} KB", first_paint, l.parse_ms + l.css_ms + l.styled.stats.ms, vp.ms, store.tiles.len(), vp.tw * vp.th, store.bytes() / 1024);
            if a.iter().any(|x| x == "--eink") {
                let img = raster::compose(&vp, &store, 1280);
                let f = std::fs::File::create("results/eink.png").unwrap();
                let mut e = png::Encoder::new(std::io::BufWriter::new(f), 1280, 800);
                e.set_color(png::ColorType::Grayscale);
                e.set_depth(png::BitDepth::Two);
                let mut wr = e.write_header().unwrap();
                let mut data = vec![0u8; 1280 * 800 / 4];
                for (i, &c) in img[..1280 * 800].iter().enumerate() {
                    let g = ((c >> 24) * 299 + ((c >> 16) & 255) * 587 + ((c >> 8) & 255) * 114) / 1000;
                    let lv = ((g + 42) / 85).min(3) as u8; // 4 gray levels
                    data[i / 4] |= lv << (6 - 2 * (i % 4));
                }
                wr.write_image_data(&data).unwrap();
                println!("e-ink frame: results/eink.png (1280×800, 2 bit/px = {} KB raw)", data.len() / 1024);
            }
        }
        Some("measure") => {
            let f = font::Fonts::new();
            let px: f32 = a[3].parse().unwrap();
            let (b, i) = (a.iter().any(|x| x == "bold"), a.iter().any(|x| x == "italic"));
            let id = f.resolve(&a[4], if b { 700 } else { 400 }, i);
            let w: i32 = a[2].chars().map(|ch| f.advance(id, ch, (px * 64.0) as i32).2).sum();
            let (asc, desc) = f.metrics(id, (px * 64.0) as i32);
            println!("width {:.2}px asc {:.2} desc {:.2}", w as f32 / 64.0, asc as f32 / 64.0, desc as f32 / 64.0);
        }
        Some("js") => jsi::cli(&a[2]),
        Some("sv-run") => svrt::cli(&a[2..]),
        Some("js-mod") => jsi::mod_cli(&a[2..]),
        Some("ir-run") => uiir::run_cli(&a[2..]),
        Some("ir-html") => { let extra: Vec<&str> = a.iter().skip(4).map(|s| s.as_str()).collect(); uiir::cli(&a[2], &a[3], &extra); }
        #[cfg(feature = "t25")] Some("core") => repair::run(&a[2..]),
        Some("mismatches") => checks::mismatches(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).map(|s| s.as_str()).unwrap_or("results/mismatches.jsonl")),
        Some("selt") => selt::bench(&a[2]),
        Some("fetch") => {
            // one GET through the broker's client (http/https): final URL + bytes, or refused (bad cert, no route…)
            let c = net::Client::new();
            let t = Instant::now();
            match c.get(&a[2]) {
                Some((u, b)) => println!("ok {} bytes from {u} in {:.0} ms", b.len(), t.elapsed().as_secs_f64() * 1e3),
                None => { println!("refused {} ({:.0} ms)", a[2], t.elapsed().as_secs_f64() * 1e3); std::process::exit(1); }
            }
        }
        Some("check") => explain::check(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).map(|s| s.as_str())),
        Some("explain") => {
            let rest: Vec<&str> = a.iter().skip(2).map(|s| s.as_str()).collect();
            let flags: Vec<&str> = rest.iter().copied().filter(|x| x.starts_with("--")).collect();
            let pos: Vec<&str> = rest.iter().copied().filter(|x| !x.starts_with("--")).collect();
            if pos.len() < 2 { eprintln!("usage: omni-render explain <page-dir|corpus-substring|file.html> <selector> [--json] [--all] [--off]"); std::process::exit(2); }
            explain::run(pos[0], pos[1], flags.contains(&"--json"), flags.contains(&"--all"), flags.contains(&"--off"));
        }
        Some("jitw0") => jit::jitw0::main(a.get(2).map(|s| s.as_str()).unwrap_or("results/rg1_trits.bin")),
        Some("jit2") => jit::sift::main(),
        Some("corert0") => corert0::main(a.get(2).map(|s| s.as_str()).unwrap_or("corpus"), a.get(3).and_then(|s| s.parse().ok()).unwrap_or(60)),
        Some("grep") => jsi_re::grep_main(&a[2..]),
        Some("re-dfa-check") => jsi_re::dfa_check(a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20000)),
        Some("ir-check") => jit::ir::main(),
        #[cfg(feature = "t25")] Some("t25-bus-bench") => jit::t25::bus_bench(a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1_000_000)),
        Some("find-index") => runtime::find_index(a.get(2).map(|s| s.as_str()).unwrap_or("../../docs"), a.get(3).map(|s| s.as_str()).unwrap_or("results/find.ofx")),
        Some("runtime0") => runtime::main(a.get(2).map(|s| s.as_str()).unwrap_or("../../docs"), a.get(3).map(|s| s.as_str()).unwrap_or("results/find.ofx")),
        Some("dump") => print!("{}", dom::Dom::parse(&std::fs::read(&a[2]).unwrap()).dump()),
        _ => eprintln!("usage: omni-render bench <corpus> | dump <page.html>"),
    }
}
