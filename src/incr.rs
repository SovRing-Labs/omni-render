// SPDX-License-Identifier: MIT OR Apache-2.0
//! INCR-R3 (TERNARY-STACK-0 R3 + style trit): page changes done only where a change trit is non-zero, proven against the
//! full pipeline. `omni-render incr corpus [K]`: per page, a chain of K synthetic edits (text change, class append, class
//! swap) on the frozen DOM; each new document is restyled FULL (reference, seeded with the session's tables) and
//! INCREMENTAL (style memo: key hit = trit 0 reuse, miss = trit + recompute); every element's computed style and every
//! generated box must be identical (Style: Eq). Plan: an unpublished plan
use crate::dom::{Dom, Kind, Span};
use crate::page;
use crate::style;
use crate::{font, layout, raster};
use std::time::Instant;

fn lcg(s: &mut u64) -> u64 { *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); *s >> 33 }

fn clone_dom(d: &Dom) -> Dom {
    Dom { nodes: d.nodes.clone(), names: d.names.clone(), texts: d.texts.clone(), attrs: d.attrs.clone(), bytes: d.bytes.clone(),
          quirks: d.quirks, sheets: d.sheets.clone(), scripts: d.scripts.clone() }
}
fn push_bytes(d: &mut Dom, s: &str) -> Span { let off = d.bytes.len() as u32; d.bytes.extend_from_slice(s.as_bytes()); Span { off, len: s.len() as u32 } }

/// One structure-preserving edit (node indices stay valid): 0 = change a text node, 1 = append a class, 2 = swap an
/// element's class for another element's class value. Returns (new dom, edited node, kind name) or None if no candidate.
fn edit(d: &Dom, kind: u8, rng: &mut u64) -> Option<(Dom, u32, &'static str)> {
    let n = d.nodes.len() as u32;
    let class_of = |i: u32| d.attr_list(i).iter().position(|a| &*d.names[a.name as usize].local == "class");
    let in_body = |mut i: u32| { while i != crate::dom::NONE { if d.tag(i) == "body" { return true; } i = d.nodes[i as usize].parent; } false };
    let cands: Vec<u32> = (0..n).filter(|&i| {
        let nd = d.nodes[i as usize];
        match kind {
            0 => nd.kind == Kind::Text && d.text(i).trim().len() > 3 && in_body(i),
            _ => nd.kind == Kind::Element && class_of(i).is_some() && in_body(i),
        }
    }).collect();
    if cands.is_empty() { return None; }
    let i = cands[(lcg(rng) as usize) % cands.len()];
    let mut nd = clone_dom(d);
    match kind {
        0 => { let t = format!("{} edited", d.text(i)); let sp = push_bytes(&mut nd, &t); nd.texts[d.nodes[i as usize].data as usize] = sp; Some((nd, i, "text")) }
        1 => {
            let k = d.nodes[i as usize].data as usize + class_of(i).unwrap();
            let v = format!("{} incr-x", d.span_str(d.attrs[k].val)); let sp = push_bytes(&mut nd, &v); nd.attrs[k].val = sp; Some((nd, i, "class+"))
        }
        _ => {
            let j = cands[(lcg(rng) as usize) % cands.len()];
            let (ki, kj) = (d.nodes[i as usize].data as usize + class_of(i).unwrap(), d.nodes[j as usize].data as usize + class_of(j).unwrap());
            nd.attrs[ki].val = d.attrs[kj].val; Some((nd, i, "class-swap"))
        }
    }
}

/// Per element: computed style + generated boxes identical (reference vs incremental).
fn style_mismatches(d: &Dom, a: &style::Styled, b: &style::Styled) -> usize {
    let mut bad = 0;
    for i in 0..d.nodes.len() {
        if d.nodes[i].kind != Kind::Element { continue; }
        if a.table[a.ids[i] as usize] != b.table[b.ids[i] as usize] { bad += 1; }
    }
    let pa: Vec<_> = a.pseudo.iter().map(|(e, w, id, t)| (*e, *w, a.table[*id as usize].clone(), t.clone())).collect();
    let pb: Vec<_> = b.pseudo.iter().map(|(e, w, id, t)| (*e, *w, b.table[*id as usize].clone(), t.clone())).collect();
    if pa != pb { bad += 1; }
    bad
}

pub fn run(dir: &str, k: usize) {
    let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect();
    ents.sort();
    let only = std::env::var("INCR_ONLY").ok();
    let vp = (1280.0f32, 800.0f32);
    let (mut tot_bad, mut tot_full, mut tot_incr, mut edits) = (0usize, 0f64, 0f64, 0usize);
    let mut rows = Vec::new();
    for p in ents {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(o) = &only { if !name.contains(o.as_str()) { continue; } }
        let b = match page::Bundle::open(&p) { Some(b) => b, None => continue };
        // web fonts as page::load resolves them (load runs a plain compute: before the memo session starts)
        let fonts = font::Fonts::new();
        { let l0 = page::load(&b, vp); for (fam, lo, hi, it, bytes) in &l0.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); } }
        // gate harness: images without intrinsic size / pixels (identical for the full and the incremental path)
        let sizes = |_n: u32| -> Option<(u32, u32)> { None };
        let imgf = |_n: u32, _m: u8, _w: i32, _h: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> { None };
        // lay out one document; with Some(memo) the layout change trit is on (memo moved in and handed back)
        let lay_items = |dd: &Dom, st: &style::Styled, memo: Option<&mut layout::LayoutMemo>| -> (Vec<layout::Item>, Vec<(u32, [i32; 4])>, i32, f64) {
            let mut st2 = style::Styled { ids: st.ids.clone(), table: st.table.clone(), families: st.families.clone(), pseudo: st.pseudo.clone(), ..Default::default() };
            let ld = page::apply_pseudo(clone_dom(dd), &mut st2);
            let t = Instant::now();
            let mut lay = layout::Layout::new(&ld, &st2, &fonts, &sizes, 1280, 800);
            let has = memo.is_some();
            if let Some(m) = &memo { let _ = m; }
            let mref = memo;
            if let Some(m) = mref { lay.memo = Some(std::mem::take(m)); let ph = lay.run(); let ms = t.elapsed().as_secs_f64() * 1e3; *m = lay.memo.take().unwrap();
                return (std::mem::take(&mut lay.out.items), std::mem::take(&mut lay.out.boxes), ph, ms); }
            let _ = has;
            let ph = lay.run();
            let ms = t.elapsed().as_secs_f64() * 1e3;
            (std::mem::take(&mut lay.out.items), std::mem::take(&mut lay.out.boxes), ph, ms)
        };
        let page_h = |ph: i32| ((ph / style::FX) as usize).clamp(800, 8000);
        let mut lmemo = layout::LayoutMemo::default();
        let (mut tstore, mut tmemo) = (raster::Store::default(), raster::TileMemo::default());
        let d0 = Dom::parse(&b.html());
        let (sheet, _, _) = page::build_sheet(&d0, &b, vp, false);
        let mut memo = style::StyleMemo::new();
        let t = Instant::now(); let s0 = style::compute_incr(&d0, &sheet, vp, &mut memo); let warm = t.elapsed().as_secs_f64() * 1e3;
        { let (items, _, ph, _) = lay_items(&d0, &s0, Some(&mut lmemo)); raster::render_incr(&items, &fonts, &imgf, 1280, 0, page_h(ph), &mut tstore, &mut tmemo); }
        let mut rng = 0x1DC0FFEEu64 ^ name.len() as u64;
        let mut cur = d0;
        let mut prow = Vec::new();
        for e in 0..k {
            let Some((nd, node, kind)) = edit(&cur, (e % 3) as u8, &mut rng) else { continue };
            // reference: plain full compute (timing) and the seeded full compute (bit-exact reference)
            let t = Instant::now(); let _ = style::compute(&nd, &sheet, vp); let full_ms = t.elapsed().as_secs_f64() * 1e3;
            let reference = style::compute_seeded(&nd, &sheet, vp, &memo);
            let (h0, m0) = (memo.hits, memo.misses + memo.shared);
            let t = Instant::now(); let inc = style::compute_incr(&nd, &sheet, vp, &mut memo); let incr_ms = t.elapsed().as_secs_f64() * 1e3;
            let (hits, miss) = (memo.hits - h0, memo.misses + memo.shared - m0);
            let bad = style_mismatches(&nd, &reference, &inc);
            // layout gate: plain full layout vs layout change trit — items, boxes, page height identical
            let (fitems, fboxes, fph, lay_ms) = lay_items(&nd, &inc, None);
            let (lh0, lm0, lr0) = (lmemo.hits, lmemo.misses, lmemo.reused_nodes);
            let (items, iboxes, iph, lay_incr_ms) = lay_items(&nd, &inc, Some(&mut lmemo));
            let lbad = (fitems != items) as usize + (fboxes != iboxes) as usize + (fph != iph) as usize;
            if std::env::var("INCR_DEBUG").is_ok() { eprintln!("  lmemo calls {} rej(nonbfc-float / float-intrude / size / forced) {:?} unstored {} hits {} misses {}", lmemo.calls, lmemo.rej, lmemo.unstored, lmemo.hits, lmemo.misses); }
            let h = page_h(iph);
            let mut fstore = raster::Store::default();
            let pf = raster::render(&items, &fonts, &imgf, 1280, 0, h, &mut fstore);
            let (th0, tm0) = (tmemo.hits, tmemo.misses);
            let pi = raster::render_incr(&items, &fonts, &imgf, 1280, 0, h, &mut tstore, &mut tmemo);
            let tbad = if pf.grid.len() != pi.grid.len() { pf.grid.len().max(1) } else {
                pf.grid.iter().zip(&pi.grid).filter(|(a, b)| fstore.tiles[**a as usize] != tstore.tiles[**b as usize]).count() };
            let bad = bad + tbad + lbad;
            tot_bad += bad; tot_full += full_ms; tot_incr += incr_ms; edits += 1;
            prow.push(serde_json::json!({"edit": kind, "node": node, "full_ms": full_ms, "incr_ms": incr_ms, "hits": hits, "recomputed": miss, "mismatches": bad,
                "tile_mismatches": tbad, "layout_mismatches": lbad, "layout_ms": lay_ms, "layout_incr_ms": lay_incr_ms,
                "bfc_hits": lmemo.hits - lh0, "bfc_misses": lmemo.misses - lm0, "nodes_reused": lmemo.reused_nodes - lr0, "raster_full_ms": pf.ms, "raster_incr_ms": pi.ms, "tiles": pi.grid.len(), "tiles_redrawn": tmemo.misses - tm0, "tiles_reused": tmemo.hits - th0}));
            cur = nd;
        }
        let pf: f64 = prow.iter().map(|r| r["full_ms"].as_f64().unwrap()).sum::<f64>() / prow.len().max(1) as f64;
        let pi: f64 = prow.iter().map(|r| r["incr_ms"].as_f64().unwrap()).sum::<f64>() / prow.len().max(1) as f64;
        let pbad: usize = prow.iter().map(|r| r["mismatches"].as_u64().unwrap() as usize).sum();
        let prec: f64 = prow.iter().map(|r| r["recomputed"].as_f64().unwrap()).sum::<f64>() / prow.len().max(1) as f64;
        let avg = |k: &str| prow.iter().map(|r| r[k].as_f64().unwrap()).sum::<f64>() / prow.len().max(1) as f64;
        println!("{:<40} nodes {:>6} | style {:>7.2} → {:>5.2} ms ({:>4.1}x) recomp {:>6.1} | layout {:>6.1} → {:>5.1} ms | raster {:>6.1} → {:>5.1} ms, tiles redrawn {:>5.1}/{:>4.0} | bad {}",
            &name[..name.len().min(40)], cur.nodes.len(), pf, pi, pf / pi.max(1e-6), prec, avg("layout_ms"), avg("layout_incr_ms"), avg("raster_full_ms"), avg("raster_incr_ms"), avg("tiles_redrawn"), avg("tiles"), pbad);
        rows.push(serde_json::json!({"page": name, "nodes": cur.nodes.len(), "warm_ms": warm, "edits": prow}));
    }
    let sum = |k: &str| rows.iter().flat_map(|r| r["edits"].as_array().unwrap().iter().map(|e| e[k].as_f64().unwrap()).collect::<Vec<_>>()).sum::<f64>();
    println!("TOTAL {} edits · restyle {:.1} → {:.1} ms ({:.1}x) · layout {:.1} → {:.1} ms ({:.1}x) · raster {:.1} → {:.1} ms ({:.1}x), tiles redrawn {:.0} of {:.0} · mismatches (style + layout + tile) {}",
        edits, tot_full, tot_incr, tot_full / tot_incr.max(1e-6), sum("layout_ms"), sum("layout_incr_ms"), sum("layout_ms") / sum("layout_incr_ms").max(1e-6), sum("raster_full_ms"), sum("raster_incr_ms"),
        sum("raster_full_ms") / sum("raster_incr_ms").max(1e-6), sum("tiles_redrawn"), sum("tiles"), tot_bad);
    std::fs::create_dir_all("results").ok();
    std::fs::write("results/incr-s1.json", serde_json::to_string_pretty(&serde_json::json!({"edits": edits, "mismatches": tot_bad,
        "full_ms": tot_full, "incr_ms": tot_incr, "pages": rows})).unwrap()).unwrap();
}

// ---------------------------------------------------------------- S4: the app frame loop on change trits
/// An in-memory document (the app runtimes produce whole HTML strings; nothing is fetched).
struct MemSrc { html: Vec<u8> }
impl page::Source for MemSrc {
    fn url(&self) -> String { "file:///app/".into() }
    fn html(&self) -> Vec<u8> { self.html.clone() }
    fn get(&self, _url: &str) -> Option<Vec<u8>> { None }
}

/// INCR-R3 S4 — one app session: every event's new HTML goes through parse → style trit → layout trit → tile trit, with the
/// sheet, fonts and the three memos kept across frames (rebuilt when the document's CSS text changes). `OMR_INCR_CHECK=1`
/// also runs the full pipeline per frame (page::load + plain layout + full raster) and compares style, layout and tiles.
pub struct Session {
    sheet: Option<&'static crate::css::Sheet>, // leaked per CSS version: memos borrow it for the session
    css_key: u64,
    smemo: style::StyleMemo<'static>,
    lmemo: layout::LayoutMemo,
    tmemo: raster::TileMemo,
    store: raster::Store,
    fonts: font::Fonts,
    check: bool,
    pub frames: Vec<serde_json::Value>,
    pub mismatches: usize,
}
impl Session {
    pub fn new() -> Session {
        Session { sheet: None, css_key: 0, smemo: style::StyleMemo::new(), lmemo: Default::default(), tmemo: Default::default(), store: Default::default(),
                  fonts: font::Fonts::new(), check: std::env::var("OMR_INCR_CHECK").is_ok(), frames: Vec::new(), mismatches: 0 }
    }
    pub fn frame(&mut self, html: &str, label: &str) {
        let vp = (1280.0f32, 800.0f32);
        let sizes = |_n: u32| -> Option<(u32, u32)> { None };
        let imgf = |_n: u32, _m: u8, _w: i32, _h: i32| -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> { None };
        let t0 = Instant::now();
        let (dom, fast) = crate::fastparse::parse(html.as_bytes()); // A: direct parser for runtime HTML, html5ever fallback
        let parse_ms = t0.elapsed().as_secs_f64() * 1e3;
        let mut parse_bad = 0;
        if self.check && fast { let r = Dom::parse(html.as_bytes()); parse_bad = (r.dump() != dom.dump() || r.sheets != dom.sheets || r.quirks != dom.quirks) as usize; }
        let key = { let mut h = 0xcbf29ce484222325u64; for (_, s) in &dom.sheets { for b in s.bytes() { h = (h ^ b as u64).wrapping_mul(0x100000001b3); } } h };
        let mut reset = false;
        if self.sheet.is_none() || key != self.css_key {
            let src = MemSrc { html: html.as_bytes().to_vec() };
            let (sh, _, _) = page::build_sheet(&dom, &src, vp, false);
            self.sheet = Some(Box::leak(Box::new(sh)));
            self.css_key = key;
            self.smemo = style::StyleMemo::new(); self.lmemo = Default::default(); self.tmemo = Default::default(); self.store = Default::default();
            self.fonts = font::Fonts::new();
            let l = page::load(&src, vp); // web fonts as load resolves them
            for (fam, lo, hi, it, bytes) in &l.fonts { self.fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
            reset = true;
        }
        let sheet = self.sheet.unwrap();
        let reference = if self.check { Some(style::compute_seeded(&dom, sheet, vp, &self.smemo)) } else { None };
        let (h0, m0) = (self.smemo.hits, self.smemo.misses + self.smemo.shared);
        let t = Instant::now();
        let st = style::compute_incr(&dom, sheet, vp, &mut self.smemo);
        let style_ms = t.elapsed().as_secs_f64() * 1e3;
        let (s_hit, s_miss) = (self.smemo.hits - h0, self.smemo.misses + self.smemo.shared - m0);
        let mut bad = reference.as_ref().map_or(0, |r| style_mismatches(&dom, r, &st)) + parse_bad;
        let mut st2 = style::Styled { ids: st.ids.clone(), table: st.table.clone(), families: st.families.clone(), pseudo: st.pseudo.clone(), ..Default::default() };
        let ld = page::apply_pseudo(dom, &mut st2);
        let (lh0, lr0) = (self.lmemo.hits, self.lmemo.reused_nodes);
        let t = Instant::now();
        let mut lay = layout::Layout::new(&ld, &st2, &self.fonts, &sizes, 1280, 800);
        lay.memo = Some(std::mem::take(&mut self.lmemo));
        let ph = lay.run();
        self.lmemo = lay.memo.take().unwrap();
        let layout_ms = t.elapsed().as_secs_f64() * 1e3;
        let (items, boxes) = (std::mem::take(&mut lay.out.items), std::mem::take(&mut lay.out.boxes));
        drop(lay);
        let tm0 = self.tmemo.misses;
        let pg = raster::render_incr(&items, &self.fonts, &imgf, 1280, 0, 800, &mut self.store, &mut self.tmemo);
        let raster_ms = pg.ms;
        let incr_ms = parse_ms + style_ms + layout_ms + raster_ms;
        let mut row = serde_json::json!({"frame": label, "parse_ms": parse_ms, "fast_parse": fast, "style_ms": style_ms, "layout_ms": layout_ms, "raster_ms": raster_ms,
            "incr_ms": incr_ms, "styles_reused": s_hit, "styles_recomputed": s_miss, "layout_hits": self.lmemo.hits - lh0,
            "layout_nodes_reused": self.lmemo.reused_nodes - lr0, "tiles_redrawn": self.tmemo.misses - tm0, "tiles": pg.grid.len(), "sheet_reset": reset});
        if self.check {
            // full-pipeline twin on the same document: plain layout + full raster must match; then what the app pays today
            let mut plain = layout::Layout::new(&ld, &st2, &self.fonts, &sizes, 1280, 800);
            let pph = plain.run();
            bad += (plain.out.items != items) as usize + (plain.out.boxes != boxes) as usize + (pph != ph) as usize;
            let mut fstore = raster::Store::default();
            let fp = raster::render(&plain.out.items, &self.fonts, &imgf, 1280, 0, 800, &mut fstore);
            bad += fp.grid.iter().zip(&pg.grid).filter(|(a, b)| fstore.tiles[**a as usize] != self.store.tiles[**b as usize]).count();
            drop(plain);
            let t = Instant::now();
            let src = MemSrc { html: html.as_bytes().to_vec() };
            let l = page::load(&src, vp); // today's path: parse + filtered sheet + full cascade + generated boxes
            let mut fl = layout::Layout::new(&l.dom, &l.styled, &self.fonts, &sizes, 1280, 800);
            fl.run();
            let mut s2 = raster::Store::default();
            raster::render(&fl.out.items, &self.fonts, &imgf, 1280, 0, 800, &mut s2);
            row["full_ms"] = serde_json::json!(t.elapsed().as_secs_f64() * 1e3);
            row["mismatches"] = serde_json::json!(bad);
            self.mismatches += bad;
        }
        eprintln!("  frame {label}: parse {parse_ms:.1}{} + style {style_ms:.2} ({s_hit} reused / {s_miss} new) + layout {layout_ms:.1} + raster {raster_ms:.1} ({} tiles redrawn) = {incr_ms:.1} ms{}{}", if fast { " (fast)" } else { " (html5ever)" },
            row["tiles_redrawn"], row.get("full_ms").map(|f| format!(" · full pipeline {:.1} ms", f.as_f64().unwrap())).unwrap_or_default(),
            row.get("mismatches").map(|m| format!(" · mismatches {m}")).unwrap_or_default());
        self.frames.push(row);
    }
    pub fn finish(&self, path: &str) {
        std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({"frames": self.frames, "mismatches": self.mismatches})).unwrap()).ok();
        eprintln!("incr session: {} frames · mismatches {} · -> {path}", self.frames.len(), self.mismatches);
    }
}
