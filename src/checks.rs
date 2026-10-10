// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check rings for layout (GEOMETRIC-COMPUTING-LAWS L6: checks DETECT, they never change output).
//! Invariants that CSS guarantees, run over a finished layout; a violation names the box and the rule it breaks.
//! Each finding is paired with Chrome's verdict on the same box (corpus pages): a violation on a box Chrome agrees
//! with means the invariant is too strict, not that layout is wrong — that pairing keeps the rings honest.
use crate::dom::{Dom, Kind, NONE};
use crate::layout::{res, res0};
use crate::style::{Display, Len, Styled, FX};
use std::collections::HashMap;

pub struct Finding { pub ring: &'static str, pub node: u32, pub msg: String }

const REPLACED: [&str; 9] = ["img", "svg", "video", "canvas", "iframe", "embed", "object", "input", "picture"];

fn el(d: &Dom, n: u32) -> bool { d.nodes[n as usize].kind == Kind::Element && !d.tag(n).starts_with("::") }

pub fn run(d: &Dom, st: &Styled, boxes: &[(u32, [i32; 4])]) -> Vec<Finding> {
    let s = |n: u32| &st.table[st.ids[n as usize] as usize];
    let mut bx: HashMap<u32, [i32; 4]> = HashMap::new();
    let mut count: HashMap<u32, u32> = HashMap::new();
    for (n, b) in boxes { bx.entry(*n).or_insert(*b); *count.entry(*n).or_default() += 1; }
    let px = |v: i32| crate::why::px(v);
    let mut out = Vec::new();
    for n in 0..d.nodes.len() as u32 {
        if !el(d, n) { continue; }
        let sn = s(n);
        let b = match bx.get(&n) { Some(b) => *b, None => continue };
        // R0 one box per block-level element (a second box = a stale layout pass left behind)
        if matches!(sn.display, Display::Block | Display::Flex | Display::Grid | Display::Table | Display::ListItem) && count[&n] > 1 {
            out.push(Finding { ring: "R0 one-box", node: n, msg: format!("{} boxes for one block-level element", count[&n]) });
        }
        if b[2] < 0 || b[3] < 0 { out.push(Finding { ring: "R0 size", node: n, msg: format!("negative size {}×{}", px(b[2]), px(b[3])) }); }
        let p = d.nodes[n as usize].parent;
        if p == NONE || !el(d, p) { continue; }
        let sp = s(p);
        let pb = match bx.get(&p) { Some(b) => *b, None => continue };
        let in_flow = sn.float == 0 && !matches!(sn.position, 2 | 3) && sn.display != Display::None;
        let pcw = pb[2] - res0(sp.padding[1], pb[2]) - res0(sp.padding[3], pb[2]) - sp.border_w[1] - sp.border_w[3];
        // R1 block flow width (CSS 2.2 §10.3.3): an auto-width in-flow block in a block container fills it — margin box = content width
        if in_flow && matches!(sp.display, Display::Block | Display::ListItem) && matches!(sn.display, Display::Block | Display::ListItem)
            && !REPLACED.contains(&d.tag(n)) && matches!(sn.width, Len::Auto) && matches!(sn.min_w, Len::Auto) && matches!(sn.max_w, Len::None) && sn.justify_self == 0
            && sp.text_align != 5 && !matches!(sn.margin[1], Len::Auto) && !matches!(sn.margin[3], Len::Auto)
            && res(sn.margin[1], pcw).is_some() && res(sn.margin[3], pcw).is_some() {
            let mbox = b[2] + res0(sn.margin[1], pcw) + res0(sn.margin[3], pcw);
            if (mbox - pcw).abs() > FX {
                out.push(Finding { ring: "R1 block-fill", node: n, msg: format!("margin box {} ≠ parent content width {} (auto width must fill, §10.3.3)", px(mbox), px(pcw)) });
            }
        }
        // R2 auto-height container contains its in-flow children's border boxes (no overflow clip involved)
        if in_flow && matches!(sp.height, Len::Auto) && matches!(sp.max_h, Len::None) && sp.overflow == 0
            && matches!(sp.display, Display::Block | Display::ListItem | Display::Flex | Display::Grid) && b[3] > 0 {
            // a negative bottom margin legally pulls the parent's content edge above the child's border box
            let child_bottom = b[1] + b[3] + res0(sn.margin[2], pb[2]).min(0);
            let parent_bottom = pb[1] + pb[3] - res0(sp.padding[2], pb[2]) - sp.border_w[2];
            if child_bottom > parent_bottom + FX && sn.position != 1 {
                out.push(Finding { ring: "R2 contain", node: n, msg: format!("bottom {} below auto-height parent's content bottom {} by {}", px(child_bottom), px(parent_bottom), px(child_bottom - parent_bottom)) });
            }
        }
    }
    // R3 single-line row flex with a growing item fills its line: Σ margin boxes + gaps = content width (Flexbox §9.7)
    for n in 0..d.nodes.len() as u32 {
        if !el(d, n) { continue; }
        let sn = s(n);
        if !matches!(sn.display, Display::Flex | Display::InlineFlex) || sn.flex_dir >= 2 || sn.flex_wrap { continue; }
        let b = match bx.get(&n) { Some(b) => *b, None => continue };
        let cw = b[2] - res0(sn.padding[1], b[2]) - res0(sn.padding[3], b[2]) - sn.border_w[1] - sn.border_w[3];
        let items: Vec<u32> = d.children(n).filter(|&c| el(d, c) && s(c).display != Display::None && !matches!(s(c).position, 2 | 3)).collect();
        if items.is_empty() || !items.iter().all(|c| bx.contains_key(c)) || d.children(n).any(|c| d.nodes[c as usize].kind == Kind::Text && !d.text(c).trim().is_empty()) { continue; }
        if !items.iter().any(|&c| s(c).grow > 0) || items.iter().any(|&c| !matches!(s(c).max_w, Len::None)) { continue; }
        if items.iter().any(|&c| matches!(s(c).margin[1], Len::Auto) || matches!(s(c).margin[3], Len::Auto)) { continue; }
        let gap = match sn.gap[1] { Len::Px(g) => g, _ => 0 };
        let sum: i32 = items.iter().map(|&c| bx[&c][2] + res0(s(c).margin[1], cw) + res0(s(c).margin[3], cw)).sum::<i32>() + gap * (items.len() as i32 - 1);
        if sum < cw - FX {
            out.push(Finding { ring: "R3 flex-fill", node: n, msg: format!("growing line leaves {} free (Σ items+gaps {} < content {})", px(cw - sum), px(sum), px(cw)) });
        }
    }
    out
}

/// `omni-render mismatches [corpus] [out.jsonl]`: every box that disagrees with Chrome, with the features a repair
/// search clusters on. `origin` = its parent agrees with Chrome (the error starts here, not inherited).
pub fn mismatches(dir: &str, out_path: &str) {
    use std::io::Write;
    let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").exists()).collect();
    ents.sort();
    let mut out = std::fs::File::create(out_path).unwrap();
    let (mut total, mut bad, mut origins) = (0usize, 0usize, 0usize);
    for p in ents {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if let Ok(only) = std::env::var("OMR_ONLY") { if !name.contains(&only) { continue; } }
        let b = match crate::page::Bundle::open(&p) { Some(b) => b, None => continue };
        let ch = match crate::verify::chrome_elements(&p.join("chrome-snap.json")) { Some(c) => c, None => continue };
        let l = crate::page::load(&b, (1280.0, 800.0));
        let fonts = crate::font::Fonts::new();
        for (fam, lo, hi, it, bytes) in &l.fonts { fonts.add_web(fam, *lo, *hi, *it, bytes.clone()); }
        let dref = &l.dom; let base = b.url.clone();
        let sizes = |n: u32| -> Option<(u32, u32)> {
            if dref.tag(n) == "svg" { return crate::svg::intrinsic(dref.outer_xml(n).as_bytes()).map(|(w, h, _)| (w.round() as u32, h.round() as u32)); }
            crate::img_bytes(dref, n, &base, &b).and_then(|bytes| crate::img::size(&bytes))
        };
        let mut lay = crate::layout::Layout::new(&l.dom, &l.styled, &fonts, &sizes, 1280, 800);
        lay.run();
        let ours: HashMap<u32, [i32; 4]> = lay.out.boxes.iter().map(|b| (b.0, b.1)).collect();
        let align = crate::verify::align(&l.dom, &ch);
        let st = |n: u32| &l.styled.table[l.styled.ids[n as usize] as usize];
        let agree = |n: u32| -> Option<bool> {
            let o = ours.get(&n)?; let cb = align.get(&n).and_then(|&j| ch[j].bounds)?;
            if cb[2] <= 0.0 || cb[3] <= 0.0 { return None; }
            let o = [o[0] as f32 / 64.0, o[1] as f32 / 64.0, o[2] as f32 / 64.0, o[3] as f32 / 64.0];
            Some((0..4).all(|k| (o[k] - cb[k]).abs() <= 2.0f32.max(cb[k].abs() * 0.05)))
        };
        for (&n, o) in &ours {
            if l.dom.nodes[n as usize].kind != Kind::Element || l.dom.tag(n).starts_with("::") { continue; }
            let Some(ok) = agree(n) else { continue };
            if st(n).display == Display::Inline { continue; }
            total += 1;
            if ok { continue; }
            bad += 1;
            let cb = align.get(&n).and_then(|&j| ch[j].bounds).unwrap();
            let p = l.dom.nodes[n as usize].parent;
            let origin = p != NONE && el(&l.dom, p) && agree(p) == Some(true);
            if origin { origins += 1; }
            let s = st(n);
            let ps = if p != NONE && el(&l.dom, p) { format!("{:?}", st(p).display) } else { "-".into() };
            let o = [o[0] as f32 / 64.0, o[1] as f32 / 64.0, o[2] as f32 / 64.0, o[3] as f32 / 64.0];
            let kind = if (o[2] - cb[2]).abs() > 2.0f32.max(cb[2] * 0.05) { "w" } else if (o[3] - cb[3]).abs() > 2.0f32.max(cb[3] * 0.05) { "h" } else if (o[0] - cb[0]).abs() > 2.0 { "x" } else { "y" };
            let has_text = l.dom.children(n).any(|c| l.dom.nodes[c as usize].kind == Kind::Text && !l.dom.text(c).trim().is_empty());
            fn subtree_text(d: &crate::dom::Dom, n: u32, out: &mut String) { for c in d.children(n) { if d.nodes[c as usize].kind == Kind::Text { out.push_str(d.text(c)); } else if d.nodes[c as usize].kind == Kind::Element { subtree_text(d, c, out); } } }
            let mut txt = String::new(); subtree_text(&l.dom, n, &mut txt);
            let txt: String = txt.split_whitespace().collect::<Vec<_>>().join(" ");
            // the text's own style: the deepest element holding the text decides the font
            let mut fe = n; loop { let k: Vec<u32> = l.dom.children(fe).filter(|&c| el(&l.dom, c)).collect(); if k.len() == 1 && !l.dom.children(fe).any(|c| l.dom.nodes[c as usize].kind == Kind::Text && !l.dom.text(c).trim().is_empty()) { fe = k[0]; } else { break; } }
            let fs = st(fe);
            let row = serde_json::json!({ "page": name, "node": n, "tag": l.dom.tag(n), "class": l.dom.attr(n, "class").unwrap_or("").split_whitespace().next().unwrap_or(""),
                "display": format!("{:?}", s.display), "parent_display": ps, "origin": origin, "kind": kind,
                "d": [o[0] - cb[0], o[1] - cb[1], o[2] - cb[2], o[3] - cb[3]], "ours": o, "chrome": cb,
                "position": s.position, "float": s.float, "overflow": s.overflow, "white_space": s.white_space, "text": has_text,
                "width": format!("{:?}", s.width).split('(').next().unwrap_or("").to_string(), "height": format!("{:?}", s.height).split('(').next().unwrap_or("").to_string(),
                "txt": txt.chars().take(120).collect::<String>(), "font": l.styled.families.get(fs.font_family as usize).cloned().unwrap_or_default(),
                "px": fs.font_size as f64 / 64.0, "ls": fs.letter_spacing as f64 / 64.0, "weight": fs.font_weight, "transform": fs.text_transform,
                "replaced": matches!(l.dom.tag(n), "img" | "svg" | "video" | "iframe" | "canvas" | "input" | "button" | "select" | "textarea") });
            writeln!(out, "{row}").ok();
        }
        eprintln!("{name}: cumulative {bad}/{total} mismatched, {origins} origins");
    }
    eprintln!("mismatches: {bad} of {total} comparable boxes; {origins} are origins (parent agrees) → {out_path}");
}
