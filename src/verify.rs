// SPDX-License-Identifier: MIT OR Apache-2.0
//! Ground truth harness: align our elements with Chrome's DOMSnapshot (same HTML, JS off) and score
//! computed-style agreement per property (and, later, layout boxes against Chrome's bounds).
use crate::dom::{Dom, Kind};
use crate::style::{Display, Len, Style, FX};
use std::collections::HashMap;

pub struct ChromeEl { pub tag: String, pub styles: HashMap<String, String>, pub bounds: Option<[f32; 4]> }

pub fn chrome_elements(snap_path: &std::path::Path) -> Option<Vec<ChromeEl>> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(snap_path).ok()?).ok()?;
    let names: Vec<String> = v["styles"].as_array()?.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect();
    let snap = &v["snap"];
    let strings: Vec<&str> = snap["strings"].as_array()?.iter().map(|x| x.as_str().unwrap_or("")).collect();
    let doc = &snap["documents"][0];
    let nodes = &doc["nodes"];
    let parent: Vec<i64> = nodes["parentIndex"].as_array()?.iter().map(|x| x.as_i64().unwrap_or(-1)).collect();
    let ntype: Vec<i64> = nodes["nodeType"].as_array()?.iter().map(|x| x.as_i64().unwrap_or(0)).collect();
    let nname: Vec<usize> = nodes["nodeName"].as_array()?.iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect();
    let pseudo: std::collections::HashSet<usize> = nodes["pseudoType"]["index"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect()).unwrap_or_default();
    let n = parent.len();
    // skip shadow roots' subtrees and pseudo elements
    let mut skip = vec![false; n];
    for i in 0..n {
        let p = parent[i];
        skip[i] = ntype[i] == 11 || pseudo.contains(&i) || (p >= 0 && skip[p as usize]);
    }
    let lay = &doc["layout"];
    let mut sty: HashMap<usize, (Vec<usize>, [f32; 4])> = HashMap::new();
    if let (Some(ix), Some(st), Some(bd)) = (lay["nodeIndex"].as_array(), lay["styles"].as_array(), lay["bounds"].as_array()) {
        for k in 0..ix.len() {
            let ni = ix[k].as_u64().unwrap_or(0) as usize;
            let s: Vec<usize> = st[k].as_array().map(|a| a.iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect()).unwrap_or_default();
            let b: Vec<f32> = bd[k].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default();
            if b.len() == 4 { sty.entry(ni).or_insert((s, [b[0], b[1], b[2], b[3]])); }
        }
    }
    let mut out = Vec::new();
    for i in 0..n {
        if ntype[i] != 1 || skip[i] { continue; }
        let tag = strings.get(nname[i]).unwrap_or(&"").to_ascii_lowercase();
        let (styles, bounds) = match sty.get(&i) {
            Some((s, b)) if !s.is_empty() => (names.iter().cloned().zip(s.iter().map(|&k| strings.get(k).unwrap_or(&"").to_string())).collect(), Some(*b)),
            _ => (HashMap::new(), None),
        };
        out.push(ChromeEl { tag, styles, bounds });
    }
    Some(out)
}

fn rgb(c: u32) -> String {
    let (r, g, b, a) = (c >> 24, (c >> 16) & 255, (c >> 8) & 255, c & 255);
    if a == 255 { format!("rgb({r}, {g}, {b})") } else { format!("rgba({r}, {g}, {b}, {})", trim(a as f32 / 255.0)) }
}
fn trim(x: f32) -> String { let s = format!("{:.2}", x); s.trim_end_matches('0').trim_end_matches('.').to_string() }
fn px(v: i32) -> String { let f = v as f32 / FX as f32; let s = format!("{:.3}", f); format!("{}px", s.trim_end_matches('0').trim_end_matches('.')) }

fn disp(d: Display) -> &'static str {
    match d {
        Display::None => "none", Display::Block => "block", Display::Inline => "inline", Display::InlineBlock => "inline-block",
        Display::ListItem => "list-item", Display::Table => "table", Display::InlineTable => "inline-table", Display::TableRow => "table-row",
        Display::TableCell => "table-cell", Display::TableRowGroup => "table-row-group", Display::TableHeaderGroup => "table-header-group",
        Display::TableFooterGroup => "table-footer-group", Display::TableCaption => "table-caption", Display::TableColumn => "table-column",
        Display::TableColumnGroup => "table-column-group", Display::Flex => "flex", Display::InlineFlex => "inline-flex",
        Display::Grid => "grid", Display::InlineGrid => "inline-grid", Display::Contents => "contents",
    }
}

/// Our value in Chrome's serialization, or None when not comparable (e.g. a used value we resolve in layout).
fn ours(s: &Style, prop: &str) -> Option<String> {
    let l = |x: Len| match x { Len::Px(v) => Some(px(v)), _ => None };
    Some(match prop {
        "display" => disp(s.display).to_string(),
        "position" => ["static", "relative", "absolute", "fixed", "sticky"][s.position as usize].to_string(),
        "float" => ["none", "left", "right"][s.float as usize].to_string(),
        "color" => rgb(s.color),
        "background-color" => rgb(s.bg),
        "font-size" => px(s.font_size),
        "font-weight" => s.font_weight.to_string(),
        "font-style" => if s.italic { "italic".into() } else { "normal".into() },
        "margin-top" => l(s.margin[0])?, "margin-right" => l(s.margin[1])?, "margin-bottom" => l(s.margin[2])?, "margin-left" => l(s.margin[3])?,
        "padding-top" => l(s.padding[0])?, "padding-right" => l(s.padding[1])?, "padding-bottom" => l(s.padding[2])?, "padding-left" => l(s.padding[3])?,
        "border-top-width" => px(s.border_w[0]),
        "text-align" => ["start", "left", "right", "center", "justify", "-webkit-center"][s.text_align as usize].to_string(),
        "white-space" => ["normal", "pre", "nowrap", "pre-wrap", "pre-line"][s.white_space as usize].to_string(),
        "line-height" => match s.line_height { Len::Auto => "normal".into(), Len::Px(v) => px(v), Len::Pct(p) => px((s.font_size as i64 * p as i64 / 10000) as i32), _ => return None },
        "visibility" => if s.visibility == 1 { "hidden".into() } else { "visible".into() },
        "box-sizing" => if s.border_box { "border-box".into() } else { "content-box".into() },
        "text-decoration-line" => { let mut v = Vec::new(); if s.decoration & 1 != 0 { v.push("underline") } if s.decoration & 4 != 0 { v.push("overline") } if s.decoration & 2 != 0 { v.push("line-through") } if v.is_empty() { "none".into() } else { v.join(" ") } }
        "list-style-type" => ["none", "disc", "circle", "square", "decimal", "?"][s.list_style as usize].to_string(),
        _ => return None,
    })
}

fn same(prop: &str, a: &str, b: &str) -> bool {
    if a == b { return true; }
    if prop == "text-align" && ((a == "start" && b == "left") || (a == "left" && b == "start") || b == "-webkit-center" && a == "center") { return true; }
    if prop == "white-space" && b == "normal" && a == "normal" { return true; }
    if a.ends_with("px") && b.ends_with("px") {
        let (x, y) = (a.trim_end_matches("px").parse::<f32>().unwrap_or(-1.0), b.trim_end_matches("px").parse::<f32>().unwrap_or(-2.0));
        return (x - y).abs() <= 0.5 + 0.01 * y.abs();
    }
    false
}

pub const PROPS: &[&str] = &["display", "position", "float", "color", "background-color", "font-size", "font-weight", "font-style",
    "margin-top", "margin-right", "margin-bottom", "margin-left", "padding-top", "padding-right", "padding-bottom", "padding-left",
    "border-top-width", "text-align", "white-space", "line-height", "visibility", "box-sizing", "text-decoration-line", "list-style-type"];

#[derive(Default, Clone)]
pub struct Score { pub hit: HashMap<&'static str, (usize, usize)>, pub aligned: usize, pub ours_el: usize, pub chrome_el: usize, pub misses: HashMap<String, usize> }

/// Align element sequences (preorder) by tag with a greedy resync, then compare rendered elements.
pub fn score(dom: &Dom, ids: &[u16], table: &[Style], ch: &[ChromeEl], sc: &mut Score) {
    let ours_el: Vec<u32> = (0..dom.nodes.len() as u32).filter(|&i| dom.nodes[i as usize].kind == Kind::Element && !dom.tag(i).starts_with("::")).collect();
    sc.ours_el += ours_el.len();
    sc.chrome_el += ch.len();
    let (mut i, mut j) = (0, 0);
    while i < ours_el.len() && j < ch.len() {
        let e = ours_el[i];
        if dom.tag(e) != ch[j].tag {
            // resync: look ahead a little on either side
            let mut found = false;
            for k in 1..8 {
                if j + k < ch.len() && dom.tag(e) == ch[j + k].tag { j += k; found = true; break; }
                if i + k < ours_el.len() && dom.tag(ours_el[i + k]) == ch[j].tag { i += k; found = true; break; }
            }
            if !found { i += 1; j += 1; continue; }
        }
        let e = ours_el[i];
        let c = &ch[j];
        if !c.styles.is_empty() {
            sc.aligned += 1;
            let s = &table[ids[e as usize] as usize];
            for &p in PROPS {
                if let (Some(a), Some(b)) = (ours(s, p), c.styles.get(p)) {
                    let ent = sc.hit.entry(p).or_insert((0, 0));
                    ent.1 += 1;
                    if same(p, &a, b) { ent.0 += 1; } else { *sc.misses.entry(format!("{p}: ours {a} | chrome {b} <{}>", c.tag)).or_insert(0) += 1; }
                }
            }
        }
        i += 1;
        j += 1;
    }
}

/// Pair our elements with Chrome's (same HTML, JS off): document order, re-synced on tag mismatches within 8 steps.
pub fn align(dom: &Dom, ch: &[ChromeEl]) -> HashMap<u32, usize> {
    let ours: Vec<u32> = (0..dom.nodes.len() as u32).filter(|&i| dom.nodes[i as usize].kind == Kind::Element && !dom.tag(i).starts_with("::")).collect();
    let mut m = HashMap::new();
    let (mut i, mut j) = (0, 0);
    while i < ours.len() && j < ch.len() {
        if dom.tag(ours[i]) != ch[j].tag {
            let mut f = false;
            for k in 1..8 {
                if j + k < ch.len() && dom.tag(ours[i]) == ch[j + k].tag { j += k; f = true; break; }
                if i + k < ours.len() && dom.tag(ours[i + k]) == ch[j].tag { i += k; f = true; break; }
            }
            if !f { i += 1; j += 1; continue; }
        }
        m.insert(ours[i], j);
        i += 1; j += 1;
    }
    m
}
