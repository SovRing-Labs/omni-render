// SPDX-License-Identifier: MIT OR Apache-2.0
//! SVG — own minimal renderer for icons/logos (SVG 1.1 static subset): path (all commands incl. arcs), rect, circle,
//! ellipse, polygon, polyline, line; <g> with transform (translate/scale/matrix/rotate) and inherited fill/stroke;
//! fill / stroke / opacity from attributes or style=""; gradients fall back to their first stop colour.
//! Rasterized with the same signed-area coverage accumulator as glyphs. No scripts, no external refs (privacy + safety).
use crate::font::raster_coverage;
use std::collections::HashMap;

type M = [f32; 6]; // affine a b c d e f
const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
fn mul(m: &M, n: &M) -> M { [m[0] * n[0] + m[2] * n[1], m[1] * n[0] + m[3] * n[1], m[0] * n[2] + m[2] * n[3], m[1] * n[2] + m[3] * n[3], m[0] * n[4] + m[2] * n[5] + m[4], m[1] * n[4] + m[3] * n[5] + m[5]] }
fn ap(m: &M, x: f32, y: f32) -> (f32, f32) { (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]) }

struct Tag { name: String, attrs: HashMap<String, String>, close: bool, selfclose: bool }

/// Tiny XML tag scanner (no DTDs, no entities beyond the basics — none are needed for drawing).
fn tags(src: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src.get(i..).and_then(|r| r.find('<')) {
        i += k + 1;
        if src[i..].starts_with("!--") { i = src[i..].find("-->").map(|j| i + j + 3).unwrap_or(src.len()); continue; }
        if src[i..].starts_with('!') || src[i..].starts_with('?') { i = src[i..].find('>').map(|j| i + j + 1).unwrap_or(src.len()); continue; }
        let close = b.get(i) == Some(&b'/');
        if close { i += 1; }
        let st = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' { i += 1; }
        let name = src[st..i].to_ascii_lowercase();
        let name = name.rsplit(':').next().unwrap_or("").to_string();
        let mut attrs = HashMap::new();
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
            if i >= b.len() || b[i] == b'>' || b[i] == b'/' { break; }
            let ks = i;
            while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() && b[i] != b'>' { i += 1; }
            let key = src[ks..i].to_ascii_lowercase();
            while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
            if b.get(i) == Some(&b'=') {
                i += 1;
                while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
                if i >= b.len() { break; }
                let q = b[i];
                if q == b'"' || q == b'\'' {
                    i += 1;
                    let vs = i;
                    while i < b.len() && b[i] != q { i += 1; }
                    attrs.insert(key, src[vs..i.min(b.len())].to_string());
                    i = (i + 1).min(b.len());
                } else {
                    let vs = i;
                    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' { i += 1; }
                    attrs.insert(key, src[vs..i].to_string());
                }
            } else { attrs.insert(key, String::new()); }
        }
        let selfclose = b.get(i) == Some(&b'/');
        i = i.min(src.len());
        i = src[i..].find('>').map(|j| i + j + 1).unwrap_or(src.len());
        out.push(Tag { name, attrs, close, selfclose });
    }
    out
}

fn nums(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
            let st = i;
            i += 1;
            let mut dot = c == b'.';
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() { i += 1; }
                else if d == b'.' && !dot { dot = true; i += 1; }
                else if (d == b'e' || d == b'E') && i + 1 < b.len() { i += 1; if b[i] == b'-' || b[i] == b'+' { i += 1; } }
                else { break; }
            }
            if let Ok(v) = s[st..i].parse() { out.push(v); }
        } else { i += 1; }
    }
    out
}

fn color(v: &str, cur: u32) -> Option<u32> {
    let v = v.trim();
    if v == "none" || v == "transparent" { return Some(0); }
    if v == "currentColor" { return Some(cur); }
    if v.starts_with("url(") { return None; } // resolved by caller via gradient stops
    let toks = crate::css::tokenize(v);
    crate::style::parse_color(&toks, cur)
}

#[derive(Clone)]
struct St { fill: u32, stroke: u32, sw: f32, op: f32, m: M, fill_url: Option<String>, cur: u32 }

/// CSS applied to an inline <svg> element (SVG 2 §13 presentation: fill / stroke / stroke-width inherit into the content;
/// `currentColor` = the element's CSS color). None = the SVG's own default.
#[derive(Clone, Copy, Default)]
pub struct Css { pub fill: Option<u32>, pub stroke: Option<u32>, pub sw: Option<f32>, pub color: u32 }

fn style_of(t: &Tag, p: &St, grads: &HashMap<String, u32>) -> St {
    let mut s = p.clone();
    let mut props: Vec<(String, String)> = t.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Some(st) = t.attrs.get("style") {
        for d in st.split(';') { if let Some((k, v)) = d.split_once(':') { props.push((k.trim().to_ascii_lowercase(), v.trim().to_string())); } }
    }
    for (k, v) in props {
        match k.as_str() {
            "fill" => { if let Some(id) = v.strip_prefix("url(#").and_then(|x| x.split(')').next()) { s.fill = *grads.get(id).unwrap_or(&s.fill); s.fill_url = Some(id.into()); } else if let Some(c) = color(&v, p.cur) { s.fill = c; } }
            "stroke" => { if let Some(id) = v.strip_prefix("url(#").and_then(|x| x.split(')').next()) { s.stroke = *grads.get(id).unwrap_or(&0); } else if let Some(c) = color(&v, p.cur) { s.stroke = c; } }
            "stroke-width" => { if let Some(n) = nums(&v).first() { s.sw = *n; } }
            "opacity" | "fill-opacity" => { if let Some(n) = nums(&v).first() { s.op *= n.clamp(0.0, 1.0); } }
            "transform" => s.m = mul(&p.m, &transform(&v)),
            _ => {}
        }
    }
    s
}

fn transform(v: &str) -> M {
    let mut m = ID;
    for part in v.split(')') {
        let (name, args) = match part.split_once('(') { Some(x) => x, None => continue };
        let a = nums(args);
        let t = match name.trim().trim_start_matches(',').trim() {
            "translate" => [1.0, 0.0, 0.0, 1.0, *a.first().unwrap_or(&0.0), *a.get(1).unwrap_or(&0.0)],
            "scale" => { let sx = *a.first().unwrap_or(&1.0); [sx, 0.0, 0.0, *a.get(1).unwrap_or(&sx), 0.0, 0.0] }
            "matrix" if a.len() == 6 => [a[0], a[1], a[2], a[3], a[4], a[5]],
            "rotate" => {
                let r = a.first().unwrap_or(&0.0).to_radians();
                let (c, s) = (r.cos(), r.sin());
                let rot = [c, s, -s, c, 0.0, 0.0];
                if a.len() == 3 { mul(&mul(&[1.0, 0.0, 0.0, 1.0, a[1], a[2]], &rot), &[1.0, 0.0, 0.0, 1.0, -a[1], -a[2]]) } else { rot }
            }
            _ => ID,
        };
        m = mul(&m, &t);
    }
    m
}

/// Flatten path data into polylines (subpaths) in user space.
fn path(d: &str) -> Vec<Vec<(f32, f32)>> {
    let mut subs: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur = (0.0f32, 0.0f32);
    let mut start = cur;
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut cmd = b'M';
    let b = d.as_bytes();
    let mut i = 0;
    let mut args: Vec<f32> = Vec::new();
    let flush_cmd = |cmd: u8, a: &[f32], cur: &mut (f32, f32), start: &mut (f32, f32), subs: &mut Vec<Vec<(f32, f32)>>, last_ctrl: &mut Option<(f32, f32)>| {
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let n = match up { b'M' | b'L' | b'T' => 2, b'H' | b'V' => 1, b'C' => 6, b'S' | b'Q' => 4, b'A' => 7, _ => 0 };
        if up == b'Z' {
            if let Some(s) = subs.last_mut() { s.push(*start); }
            *cur = *start; *last_ctrl = None;
            return;
        }
        if n == 0 { return; }
        let mut k = 0;
        let mut first = true;
        while k + n <= a.len() {
            let p = &a[k..k + n];
            let off = |x: f32, y: f32, c: (f32, f32)| if rel { (c.0 + x, c.1 + y) } else { (x, y) };
            match up {
                b'M' => {
                    let q = off(p[0], p[1], *cur);
                    if first { subs.push(vec![q]); *start = q; } else if let Some(s) = subs.last_mut() { s.push(q); }
                    *cur = q; *last_ctrl = None;
                }
                b'L' => { let q = off(p[0], p[1], *cur); push(subs, *cur, q); *cur = q; *last_ctrl = None; }
                b'H' => { let q = (if rel { cur.0 + p[0] } else { p[0] }, cur.1); push(subs, *cur, q); *cur = q; *last_ctrl = None; }
                b'V' => { let q = (cur.0, if rel { cur.1 + p[0] } else { p[0] }); push(subs, *cur, q); *cur = q; *last_ctrl = None; }
                b'C' | b'S' => {
                    let (c1, c2, e) = if up == b'C' { (off(p[0], p[1], *cur), off(p[2], p[3], *cur), off(p[4], p[5], *cur)) } else {
                        let c1 = last_ctrl.map(|c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1)).unwrap_or(*cur);
                        (c1, off(p[0], p[1], *cur), off(p[2], p[3], *cur)) };
                    let a0 = *cur;
                    for s in 1..=12 { let t = s as f32 / 12.0; let u = 1.0 - t;
                        let q = (u * u * u * a0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * e.0, u * u * u * a0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * e.1);
                        push(subs, *cur, q); *cur = q; }
                    *last_ctrl = Some(c2); *cur = e;
                }
                b'Q' | b'T' => {
                    let (c, e) = if up == b'Q' { (off(p[0], p[1], *cur), off(p[2], p[3], *cur)) } else {
                        (last_ctrl.map(|c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1)).unwrap_or(*cur), off(p[0], p[1], *cur)) };
                    let a0 = *cur;
                    for s in 1..=8 { let t = s as f32 / 8.0; let u = 1.0 - t;
                        let q = (u * u * a0.0 + 2.0 * u * t * c.0 + t * t * e.0, u * u * a0.1 + 2.0 * u * t * c.1 + t * t * e.1);
                        push(subs, *cur, q); *cur = q; }
                    *last_ctrl = Some(c); *cur = e;
                }
                b'A' => {
                    let e = off(p[5], p[6], *cur);
                    for q in arc(*cur, p[0], p[1], p[2], p[3] != 0.0, p[4] != 0.0, e) { push(subs, *cur, q); *cur = q; }
                    *cur = e; *last_ctrl = None;
                }
                _ => {}
            }
            k += n;
            first = false;
        }
    };
    while i <= b.len() {
        let c = if i < b.len() { b[i] } else { b'Z' + 0 };
        if i == b.len() || (c.is_ascii_alphabetic() && c != b'e' && c != b'E') {
            flush_cmd(cmd, &args, &mut cur, &mut start, &mut subs, &mut last_ctrl);
            args.clear();
            if i == b.len() { break; }
            cmd = c;
            if c == b'Z' || c == b'z' { flush_cmd(c, &[], &mut cur, &mut start, &mut subs, &mut last_ctrl); cmd = b'L'; if c == b'z' { cmd = b'l'; } }
            i += 1;
            continue;
        }
        let st = i;
        while i < b.len() && !(b[i].is_ascii_alphabetic() && b[i] != b'e' && b[i] != b'E') { i += 1; }
        args.extend(nums(&d[st..i]));
    }
    subs
}

fn push(subs: &mut Vec<Vec<(f32, f32)>>, from: (f32, f32), q: (f32, f32)) {
    if subs.is_empty() { subs.push(vec![from]); }
    subs.last_mut().unwrap().push(q);
}

/// Elliptical arc (SVG implementation notes F.6.5 endpoint → centre parameterisation), flattened.
fn arc(p0: (f32, f32), rx: f32, ry: f32, phi: f32, large: bool, sweep: bool, p1: (f32, f32)) -> Vec<(f32, f32)> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 { return vec![p1]; }
    let phi = phi.to_radians();
    let (c, s) = (phi.cos(), phi.sin());
    let (dx, dy) = ((p0.0 - p1.0) / 2.0, (p0.1 - p1.1) / 2.0);
    let (x1, y1) = (c * dx + s * dy, -s * dx + c * dy);
    let l = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if l > 1.0 { rx *= l.sqrt(); ry *= l.sqrt(); }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    if large == sweep { k = -k; }
    let (cx1, cy1) = (k * rx * y1 / ry, -k * ry * x1 / rx);
    let (cx, cy) = (c * cx1 - s * cy1 + (p0.0 + p1.0) / 2.0, s * cx1 + c * cy1 + (p0.1 + p1.1) / 2.0);
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| { let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy); a };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 { dt -= std::f32::consts::TAU; } else if sweep && dt < 0.0 { dt += std::f32::consts::TAU; }
    let n = ((dt.abs() / 0.3).ceil() as usize).clamp(2, 64);
    (1..=n).map(|i| { let t = t1 + dt * i as f32 / n as f32; (c * rx * t.cos() - s * ry * t.sin() + cx, s * rx * t.cos() + c * ry * t.sin() + cy) }).collect()
}

fn shape(t: &Tag) -> Vec<Vec<(f32, f32)>> {
    let f = |k: &str| t.attrs.get(k).and_then(|v| nums(v).first().copied()).unwrap_or(0.0);
    match t.name.as_str() {
        "path" => t.attrs.get("d").map(|d| path(d)).unwrap_or_default(),
        "rect" => { let (x, y, w, h) = (f("x"), f("y"), f("width"), f("height")); vec![vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]] }
        "circle" | "ellipse" => {
            let (cx, cy) = (f("cx"), f("cy"));
            let (rx, ry) = if t.name == "circle" { (f("r"), f("r")) } else { (f("rx"), f("ry")) };
            vec![(0..=48).map(|i| { let a = i as f32 / 48.0 * std::f32::consts::TAU; (cx + rx * a.cos(), cy + ry * a.sin()) }).collect()]
        }
        "polygon" | "polyline" => {
            let v = t.attrs.get("points").map(|p| nums(p)).unwrap_or_default();
            let mut pts: Vec<(f32, f32)> = v.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect();
            if t.name == "polygon" { if let Some(&p) = pts.first() { pts.push(p); } }
            vec![pts]
        }
        "line" => vec![vec![(f("x1"), f("y1")), (f("x2"), f("y2"))]],
        _ => Vec::new(),
    }
}

/// Stroke a polyline as quads (butt caps, no joins — good enough for icon strokes).
fn stroke_polys(sub: &[(f32, f32)], w: f32) -> Vec<Vec<(f32, f32)>> {
    let h = w / 2.0;
    sub.windows(2).filter_map(|s| {
        let (a, b) = (s[0], s[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = (dx * dx + dy * dy).sqrt();
        if l < 1e-6 { return None; }
        let (nx, ny) = (-dy / l * h, dx / l * h);
        Some(vec![(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny), (a.0 + nx, a.1 + ny)])
    }).collect()
}

/// SVG/CSS absolute length → px (96 dpi); % and unknown units → None.
fn svg_len(v: &str) -> Option<f32> {
    let v = v.trim();
    if v.ends_with('%') { return None; }
    let n = nums(v).first().copied()?;
    let unit: String = v.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E').trim().to_ascii_lowercase();
    Some(n * match unit.as_str() { "" | "px" => 1.0, "pt" => 4.0 / 3.0, "pc" => 16.0, "mm" => 96.0 / 25.4, "cm" => 96.0 / 2.54, "in" => 96.0, "em" => 16.0, "ex" => 8.0, _ => return None })
}

pub fn intrinsic(b: &[u8]) -> Option<(f32, f32, [f32; 4])> {
    let src = std::str::from_utf8(b).ok()?;
    let ts = tags(src);
    let t = ts.iter().find(|t| t.name == "svg" && !t.close)?;
    let vb: Vec<f32> = t.attrs.get("viewbox").map(|v| nums(v)).unwrap_or_default();
    let w = t.attrs.get("width").and_then(|v| svg_len(v));
    let h = t.attrs.get("height").and_then(|v| svg_len(v));
    let vb = if vb.len() == 4 { [vb[0], vb[1], vb[2], vb[3]] } else { [0.0, 0.0, w.unwrap_or(300.0), h.unwrap_or(150.0)] };
    let (w, h) = match (w, h) { (Some(w), Some(h)) => (w, h), (Some(w), None) => (w, w * vb[3] / vb[2].max(1e-3)), (None, Some(h)) => (h * vb[2] / vb[3].max(1e-3), h), _ => (vb[2], vb[3]) };
    Some((w, h, vb))
}

/// Render at intrinsic size.
pub fn render(b: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    let (w, h, _) = intrinsic(b)?;
    render_at(b, w.round().clamp(1.0, 4096.0) as u32, h.round().clamp(1.0, 4096.0) as u32)
}

/// Render at exactly `ow`×`oh` px (vector: crisp at display size, never larger than shown).
pub fn render_at(b: &[u8], ow: u32, oh: u32) -> Option<(u32, u32, Vec<u32>)> { render_css(b, ow, oh, Css { color: 0x000000ff, ..Css::default() }) }

/// Render an inline <svg> with the element's CSS (fill / stroke / stroke-width / color) as its root style.
pub fn render_css(b: &[u8], ow: u32, oh: u32, css: Css) -> Option<(u32, u32, Vec<u32>)> {
    let src = std::str::from_utf8(b).ok()?;
    let (_, _, vb) = intrinsic(b)?;
    let ts = tags(src);
    // gradients → first stop colour
    let mut grads: HashMap<String, u32> = HashMap::new();
    let mut cur_grad: Option<String> = None;
    for t in &ts {
        if (t.name == "lineargradient" || t.name == "radialgradient") && !t.close { cur_grad = t.attrs.get("id").cloned(); }
        if t.name == "stop" {
            if let Some(g) = &cur_grad {
                let mut c = t.attrs.get("stop-color").cloned();
                if let Some(st) = t.attrs.get("style") { for d in st.split(';') { if let Some((k, v)) = d.split_once(':') { if k.trim() == "stop-color" { c = Some(v.trim().into()); } } } }
                if let Some(c) = c.and_then(|c| color(&c, 0x000000ff)) { grads.entry(g.clone()).or_insert(c); }
            }
        }
    }
    let (sx, sy) = (ow as f32 / vb[2].max(1e-3), oh as f32 / vb[3].max(1e-3));
    let s = sx.min(sy);
    let (tx, ty) = ((ow as f32 - vb[2] * s) / 2.0 - vb[0] * s, (oh as f32 - vb[3] * s) / 2.0 - vb[1] * s);
    let root = St { fill: css.fill.unwrap_or(0x000000ff), stroke: css.stroke.unwrap_or(0), sw: css.sw.unwrap_or(1.0), op: 1.0, m: [s, 0.0, 0.0, s, tx, ty], fill_url: None, cur: css.color };
    let mut img = vec![0u32; (ow * oh) as usize];
    let mut stack: Vec<St> = vec![root];
    let mut skip = 0usize; // inside <defs>, <clipPath>, <mask>, <symbol>, <style>, <title>
    for t in &ts {
        let hidden = matches!(t.name.as_str(), "defs" | "clippath" | "mask" | "symbol" | "style" | "title" | "desc" | "lineargradient" | "radialgradient" | "pattern" | "filter" | "metadata");
        if t.close {
            if hidden { skip = skip.saturating_sub(1); }
            else if (t.name == "g" || t.name == "svg" || t.name == "a") && stack.len() > 1 { stack.pop(); }
            continue;
        }
        if hidden { if !t.selfclose { skip += 1; } continue; }
        if skip > 0 { continue; }
        let st = style_of(t, stack.last().unwrap(), &grads);
        if t.name == "g" || t.name == "svg" || t.name == "a" { if !t.selfclose { stack.push(st); } continue; }
        let subs = shape(t);
        if subs.is_empty() { continue; }
        let tr = |subs: &[Vec<(f32, f32)>]| -> Vec<(f32, f32, f32, f32)> {
            let mut segs = Vec::new();
            for sp in subs { for w in sp.windows(2) { let (a, b) = (ap(&st.m, w[0].0, w[0].1), ap(&st.m, w[1].0, w[1].1)); segs.push((a.0, a.1, b.0, b.1)); }
                // implicit close for filling
                if let (Some(f), Some(l)) = (sp.first(), sp.last()) { if f != l { let (a, b) = (ap(&st.m, l.0, l.1), ap(&st.m, f.0, f.1)); segs.push((a.0, a.1, b.0, b.1)); } } }
            segs
        };
        if st.fill & 255 != 0 && t.name != "line" && t.name != "polyline" || (t.name == "polyline" && st.fill & 255 != 0 && st.stroke == 0) {
            paint(&mut img, ow, oh, &tr(&subs), st.fill, st.op);
        }
        if st.stroke & 255 != 0 && st.sw > 0.0 {
            let polys: Vec<Vec<(f32, f32)>> = subs.iter().flat_map(|sp| stroke_polys(sp, st.sw)).collect();
            for p in polys { paint(&mut img, ow, oh, &tr(&[p]), st.stroke, st.op); }
        }
    }
    Some((ow, oh, img))
}

fn paint(img: &mut [u32], w: u32, h: u32, segs: &[(f32, f32, f32, f32)], color: u32, op: f32) {
    if segs.is_empty() { return; }
    let cov = raster_coverage(segs, 0.0, 0.0, w as usize, h as usize);
    for (i, &a) in cov.iter().enumerate() {
        let a = (a.min(1.0) * op * (color & 255) as f32) as u32;
        if a == 0 { continue; }
        let d = img[i];
        let da = d & 255;
        // source-over in straight alpha
        let oa = a + da * (255 - a) / 255;
        if oa == 0 { continue; }
        let ch = |sh: u32| (((color >> sh) & 255) * a + ((d >> sh) & 255) * da * (255 - a) / 255) / oa;
        img[i] = ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | oa;
    }
}
