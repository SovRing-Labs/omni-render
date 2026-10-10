// SPDX-License-Identifier: MIT OR Apache-2.0
//! `omni-render explain <page> <selector> [--json] [--all]` — why a box has its size and position.
//! For each element matching the selector: our box (and Chrome's, for frozen corpus pages), the winning declaration per
//! property with the rule and stylesheet that set it, and the layout derivation recorded by `why` (intrinsic sizes,
//! flow margins, flex resolution, containing blocks, every later move).
use crate::{css, dom, font, layout, page, style, svg, verify, why};
use std::path::Path;

/// A page given as a corpus bundle dir, a substring of one under corpus/, or a local .html file.
fn open(target: &str) -> Option<(Box<dyn page::Source>, Option<std::path::PathBuf>)> {
    let p = Path::new(target);
    if p.join("manifest.json").exists() { return page::Bundle::open(p).map(|b| (Box::new(b) as Box<dyn page::Source>, Some(p.to_path_buf()))); }
    if p.is_file() { return page::Local::open(p).map(|l| (Box::new(l) as Box<dyn page::Source>, None)); }
    let mut hits: Vec<_> = std::fs::read_dir("corpus").ok()?.filter_map(|e| e.ok()).map(|e| e.path())
        .filter(|d| d.file_name().map_or(false, |n| n.to_string_lossy().contains(target)) && d.join("manifest.json").exists()).collect();
    hits.sort();
    let d = hits.into_iter().next()?;
    page::Bundle::open(&d).map(|b| (Box::new(b) as Box<dyn page::Source>, Some(d)))
}

fn label(d: &dom::Dom, n: u32) -> String {
    let mut s = format!("<{}", d.tag(n));
    if let Some(id) = d.attr(n, "id") { s += &format!("#{id}"); }
    if let Some(c) = d.attr(n, "class") { for c in c.split_whitespace().take(4) { s += &format!(".{c}"); } }
    s + ">"
}

/// Winning declaration per property name (as written: shorthands and longhands both listed), cascade order:
/// matched rules sorted (origin, specificity, order), then the style attribute, then !important in reverse origin.
pub(crate) fn provenance(d: &dom::Dom, sheet: &css::Sheet, e: u32) -> Vec<(String, String, String)> {
    let f = css::ElemFacts::new(d);
    let mut matched = Vec::new();
    sheet.matching(d, &f, e, &mut matched);
    matched.retain(|&(_, _, ri)| sheet.rules[ri as usize].sel.pseudo == 0);
    matched.sort_unstable_by_key(|&(sp, o, _)| (o >= sheet.ua_rules, sp, o));
    let from = |o: u32| -> String {
        if o < sheet.ua_rules { return "UA sheet".into(); }
        match sheet.trace.as_ref().and_then(|t| t.get(&o)) {
            Some((src, sel)) => {
                let file = src.split(['?', '#']).next().unwrap_or(src);
                let file = file.trim_end_matches('/').rsplit('/').next().unwrap_or(file);
                format!("{} — {}", sel.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(90).collect::<String>(), file.chars().take(40).collect::<String>())
            }
            None => format!("rule #{o}"),
        }
    };
    let mut win: Vec<(String, String, String)> = Vec::new();
    let mut set = |name: &str, val: String, who: String, win: &mut Vec<(String, String, String)>| {
        // a shorthand overrides every longhand set before it; the winner sits where it won in cascade order
        const SHORT: [&str; 12] = ["margin", "padding", "border", "inset", "flex", "grid", "font", "gap", "overflow", "place-items", "place-content", "place-self"];
        if SHORT.contains(&name) { let pre = format!("{name}-"); win.retain(|w| !w.0.starts_with(&pre)); }
        win.retain(|w| w.0 != name);
        win.push((name.to_string(), val, who));
    };
    let inline: Vec<css::Decl> = d.attr(e, "style").map(|s| css::parse_decls(&css::tokenize(s))).unwrap_or_default();
    for imp in [false, true] {
        let order: Vec<(u32, u32, u32)> = if imp { matched.iter().rev().copied().collect::<Vec<_>>().into_iter().rev().collect() } else { matched.clone() };
        for &(_, o, ri) in &order {
            for dd in sheet.rules[ri as usize].decls.iter() { if dd.important == imp && !dd.name.starts_with("--") { set(&dd.name, style::tok_str(&dd.value), from(o) + if imp { " !important" } else { "" }, &mut win); } }
        }
        for dd in &inline { if dd.important == imp && !dd.name.starts_with("--") { set(&dd.name, style::tok_str(&dd.value), "style attribute".into(), &mut win); } }
    }
    win
}

/// Properties that can change a box's size or position (the rest — colour, animation, cursor… — are hidden by default).
pub(crate) fn layout_prop(p: &str) -> bool {
    const P: [&str; 30] = ["display", "position", "float", "clear", "width", "height", "min-", "max-", "margin", "padding", "border", "box-sizing",
        "top", "right", "bottom", "left", "inset", "flex", "order", "align", "justify", "place", "grid", "gap", "row-gap", "column-gap",
        "font", "line-height", "white-space", "overflow"];
    const Q: [&str; 9] = ["letter-spacing", "vertical-align", "text-align", "transform", "translate", "table-layout", "content", "aspect-ratio", "visibility"];
    P.iter().any(|x| p.starts_with(x)) || Q.contains(&p)
}

pub fn run(target: &str, selector: &str, json: bool, all: bool, off_only: bool) {
    css::TRACE.store(true, std::sync::atomic::Ordering::Relaxed);
    let (src, dir) = match open(target) { Some(x) => x, None => { eprintln!("explain: no page at {target} (bundle dir, corpus substring or .html file)"); std::process::exit(2); } };
    let l = page::load(src.as_ref(), (1280.0, 800.0));
    let sel = match css::parse_complex(&css::tokenize(selector)) { Some(s) => s, None => { eprintln!("explain: cannot parse selector {selector}"); std::process::exit(2); } };
    let f = css::ElemFacts::new(&l.dom);
    let targets: Vec<u32> = (0..l.dom.nodes.len() as u32)
        .filter(|&i| l.dom.nodes[i as usize].kind == dom::Kind::Element && css::match_parts(&l.dom, &f, i, &sel.parts)).collect();
    if targets.is_empty() { eprintln!("explain: no element matches {selector}"); std::process::exit(1); }
    let chrome = dir.as_ref().and_then(|d| verify::chrome_elements(&d.join("chrome-snap.json")));
    let align = chrome.as_ref().map(|ch| verify::align(&l.dom, ch)).unwrap_or_default();
    let base = src.url();
    let fonts = font::Fonts::new();
    for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
    let dom_ref = &l.dom;
    let srcr = src.as_ref();
    let sizes = |n: u32| -> Option<(u32, u32)> {
        if dom_ref.tag(n) == "svg" { return svg::intrinsic(dom_ref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
        crate::img_bytes(dom_ref, n, &base, srcr).and_then(|bytes| crate::img::size(&bytes))
    };
    let mut out_json = Vec::new();
    let mut shown = 0usize;
    for &e in targets.iter() {
        if shown > 0 && !all { eprintln!("explain: more matches; --all to show them"); break; }
        // one layout run per watched element: the recorder follows exactly one node
        why::watch(e);
        let mut lay = layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
        lay.run();
        let steps = why::take();
        why::watch(u32::MAX);
        let ours = lay.out.boxes.iter().find(|b| b.0 == e).map(|b| b.1);
        let ch = align.get(&e).and_then(|&j| chrome.as_ref().map(|c| &c[j]));
        if off_only {
            let agree = match (ours, ch.and_then(|c| c.bounds)) {
                (Some(b), Some(cb)) => { let o = [b[0] as f32 / 64.0, b[1] as f32 / 64.0, b[2] as f32 / 64.0, b[3] as f32 / 64.0]; (0..4).all(|k| (o[k] - cb[k]).abs() <= 2.0f32.max(cb[k].abs() * 0.05)) }
                _ => true,
            };
            if agree { continue; }
        }
        shown += 1;
        let prov = provenance(&l.dom, &l.sheet, e);
        let st = &l.styled.table[l.styled.ids[e as usize] as usize];
        if json {
            out_json.push(serde_json::json!({
                "node": e, "element": label(&l.dom, e),
                "ours": ours.map(|b| [b[0] as f64 / 64.0, b[1] as f64 / 64.0, b[2] as f64 / 64.0, b[3] as f64 / 64.0]),
                "chrome": ch.and_then(|c| c.bounds),
                "computed": { "display": format!("{:?}", st.display), "position": st.position, "width": format!("{:?}", st.width), "height": format!("{:?}", st.height) },
                "declarations": prov.iter().map(|(p, v, w)| serde_json::json!({ "property": p, "value": v, "from": w })).collect::<Vec<_>>(),
                "derivation": steps.iter().map(|(s, m)| serde_json::json!({ "stage": s, "step": m })).collect::<Vec<_>>(),
            }));
            continue;
        }
        println!("{}  (node {e})", label(&l.dom, e));
        let fmt = |b: [i32; 4]| format!("[{} {} {} {}]", why::px(b[0]), why::px(b[1]), why::px(b[2]), why::px(b[3]));
        match ours { Some(b) => println!("  ours    {}", fmt(b)), None => println!("  ours    (no box: display {:?})", st.display) }
        if let Some(c) = ch {
            if let Some(cb) = c.bounds {
                let d = ours.map(|b| { let o = [b[0] as f32 / 64.0, b[1] as f32 / 64.0, b[2] as f32 / 64.0, b[3] as f32 / 64.0];
                    ["x", "y", "w", "h"].iter().enumerate().filter(|(i, _)| (o[*i] - cb[*i]).abs() > 2.0f32.max(cb[*i].abs() * 0.05)).map(|(i, n)| format!("{n} {:+.1}", o[i] - cb[i])).collect::<Vec<_>>().join(" ") }).unwrap_or_default();
                println!("  chrome  [{:.1} {:.1} {:.1} {:.1}]  {}", cb[0], cb[1], cb[2], cb[3], if d.is_empty() { "match".to_string() } else { format!("off: {d}") });
            }
        }
        // ancestor chain: the outermost ancestor that disagrees with Chrome is usually the real cause
        if chrome.is_some() {
            let agree = |n: u32| -> Option<bool> {
                let b = lay.out.boxes.iter().find(|b| b.0 == n).map(|b| b.1)?;
                let cb = align.get(&n).and_then(|&j| chrome.as_ref().and_then(|c| c[j].bounds))?;
                let o = [b[0] as f32 / 64.0, b[1] as f32 / 64.0, b[2] as f32 / 64.0, b[3] as f32 / 64.0];
                Some((0..4).all(|k| (o[k] - cb[k]).abs() <= 2.0f32.max(cb[k].abs() * 0.05)))
            };
            let mut chain = Vec::new();
            let mut p = l.dom.nodes[e as usize].parent;
            while p != dom::NONE && l.dom.nodes[p as usize].kind == dom::Kind::Element && chain.len() < 12 { chain.push(p); p = l.dom.nodes[p as usize].parent; }
            let outer_off = chain.iter().rev().find(|&&a| agree(a) == Some(false)).copied();
            if let Some(a) = outer_off {
                let b = lay.out.boxes.iter().find(|b| b.0 == a).map(|b| b.1).unwrap();
                let cb = align.get(&a).and_then(|&j| chrome.as_ref().and_then(|c| c[j].bounds)).unwrap();
                println!("  outermost wrong ancestor: {} (node {a}) ours {} chrome [{:.1} {:.1} {:.1} {:.1}] — explain that first", label(&l.dom, a), fmt(b), cb[0], cb[1], cb[2], cb[3]);
            } else if ours.is_some() && ch.is_some() {
                let unk = chain.iter().filter(|&&a| agree(a).is_none()).count();
                println!("  ancestors: none disagree with Chrome ({} of {} not comparable: no box or no Chrome twin) — the cause is in this box or an uncomparable ancestor", unk, chain.len());
            }
            if std::env::var("OMR_CHAIN").is_ok() {
                for &a in &chain {
                    let b = lay.out.boxes.iter().find(|b| b.0 == a).map(|b| fmt(b.1)).unwrap_or("-".into());
                    let c = align.get(&a).and_then(|&j| chrome.as_ref().and_then(|cc| cc[j].bounds)).map(|cb| format!("[{:.1} {:.1} {:.1} {:.1}]", cb[0], cb[1], cb[2], cb[3])).unwrap_or("?".into());
                    println!("    ↑ {} (node {a}) ours {b} chrome {c} {:?}", label(&l.dom, a), l.styled.table[l.styled.ids[a as usize] as usize].display);
                }
            }
        }
        println!("  declarations (winning, cascade order):");
        for (p, v, w) in prov.iter().filter(|x| layout_prop(&x.0)) { println!("    {p}: {v}   ← {w}"); }
        println!("  derivation:");
        for (s, m) in &steps { println!("    [{s}] {m}"); }
        if steps.is_empty() { println!("    (no layout steps recorded — element produced no box)"); }
    }
    if json { println!("{}", serde_json::to_string_pretty(&out_json).unwrap()); }
}

/// `omni-render check [corpus-dir] [page-substring]` — run the check rings over every page; per page and per ring:
/// findings, and how many of them sit on boxes Chrome agrees with (= the invariant is too strict there).
pub fn check(dir: &str, only: Option<&str>) {
    let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect();
    ents.sort();
    let mut tot: std::collections::BTreeMap<&'static str, (usize, usize)> = Default::default();
    for p in ents {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(o) = only { if !name.contains(o) { continue; } }
        let b = match page::Bundle::open(&p) { Some(b) => b, None => continue };
        let l = page::load(&b, (1280.0, 800.0));
        let fonts = font::Fonts::new();
        for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
        let dom_ref = &l.dom;
        let base = b.url.clone();
        let sizes = |n: u32| -> Option<(u32, u32)> {
            if dom_ref.tag(n) == "svg" { return svg::intrinsic(dom_ref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
            crate::img_bytes(dom_ref, n, &base, &b).and_then(|bytes| crate::img::size(&bytes))
        };
        let mut lay = layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
        lay.run();
        let found = crate::checks::run(&l.dom, &l.styled, &lay.out.boxes);
        let chrome = verify::chrome_elements(&p.join("chrome-snap.json"));
        let align = chrome.as_ref().map(|ch| verify::align(&l.dom, ch)).unwrap_or_default();
        let ours: std::collections::HashMap<u32, [i32; 4]> = lay.out.boxes.iter().map(|b| (b.0, b.1)).collect();
        let agrees = |n: u32| -> bool {
            match (ours.get(&n), align.get(&n).and_then(|&j| chrome.as_ref().and_then(|c| c[j].bounds))) {
                (Some(o), Some(cb)) => { let o = [o[0] as f32 / 64.0, o[1] as f32 / 64.0, o[2] as f32 / 64.0, o[3] as f32 / 64.0]; (0..4).all(|k| (o[k] - cb[k]).abs() <= 2.0f32.max(cb[k].abs() * 0.05)) }
                _ => false,
            }
        };
        // RA alignment ring: our element sequence vs Chrome's — a mismatch means a parser/DOM divergence and every box
        // comparison after it is unreliable
        if let Some(ch) = &chrome {
            let ours_el: Vec<u32> = (0..l.dom.nodes.len() as u32).filter(|&i| l.dom.nodes[i as usize].kind == dom::Kind::Element && !l.dom.tag(i).starts_with("::")).collect();
            if let Some(k) = (0..ours_el.len().min(ch.len())).find(|&k| l.dom.tag(ours_el[k]) != ch[k].tag) {
                println!("  RA alignment: ours {} elements, Chrome {}; first tag mismatch at #{k}: ours <{}> after {} vs Chrome <{}>", ours_el.len(), ch.len(), l.dom.tag(ours_el[k]), label(&l.dom, l.dom.nodes[ours_el[k] as usize].parent), ch[k].tag);
                *tot.entry("RA alignment").or_default() = { let e = tot.get("RA alignment").copied().unwrap_or((0, 0)); (e.0 + 1, e.1) };
            } else if ours_el.len() != ch.len() {
                println!("  RA alignment: sequences agree over {} but lengths differ (ours {}, Chrome {})", ours_el.len().min(ch.len()), ours_el.len(), ch.len());
            }
        }
        let mut per: std::collections::BTreeMap<&'static str, (usize, usize)> = Default::default();
        for f in &found { let e = per.entry(f.ring).or_default(); e.0 += 1; if agrees(f.node) { e.1 += 1; } }
        println!("{name}: {} findings {}", found.len(), per.iter().map(|(r, (n, a))| format!("· {r} {n} ({a} Chrome-agreed)")).collect::<Vec<_>>().join(" "));
        for f in found.iter().filter(|f| !agrees(f.node)).take(std::env::var("OMR_CHECK_N").ok().and_then(|v| v.parse().ok()).unwrap_or(3)) {
            println!("    {} {} — {}", f.ring, label(&l.dom, f.node), f.msg);
        }
        for (r, (n, a)) in per { let e = tot.entry(r).or_default(); e.0 += n; e.1 += a; }
    }
    println!("TOTAL {}", tot.iter().map(|(r, (n, a))| format!("{r}: {n} findings, {a} on Chrome-agreed boxes ({:.0}% false alarm)", if *n > 0 { *a as f64 * 100.0 / *n as f64 } else { 0.0 })).collect::<Vec<_>>().join(" · "));
}
