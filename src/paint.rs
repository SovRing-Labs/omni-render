// SPDX-License-Identifier: MIT OR Apache-2.0
//! Paint — rounded boxes, box shadows, gradients and backdrop blur (BROWSER-PRODUCT-0 P2 A1–A4, author 2026-10-07: the "Orbit"
//! design leads, the renderer learns what it calls for). Clean-room from the specs:
//!   CSS Backgrounds 3 §5 (border-radius; §5.5 overlapping radii scale down by one factor) · §7 (box-shadow; σ = blur / 2;
//!   outer shadows only outside the border box, inset only inside the padding box) · CSS Images 3/4 (linear / radial / conic
//!   gradients, stop fix-up, premultiplied interpolation) · Filter Effects 2 (backdrop-filter: blur = Gaussian σ, saturate).
//! Geometry is exact: a rounded box IS its signed-distance function d(p); a pixel's coverage = clamp(0.5 − d, 0, 1) at its centre
//! (analytic anti-aliasing, no supersampling). A Gaussian-blurred shape ≈ ½·erfc(d / (σ√2)) over the same distance — exact for a
//! straight edge, close at corners. Only boxes that ask for any of this become `Item::Box`; every other box stays an `Item::Rect`.
use crate::css::{tokenize, Tok};
use crate::style::{parse_color, FX};

#[derive(Clone, Debug, PartialEq)]
pub struct Shadow { pub dx: f32, pub dy: f32, pub blur: f32, pub spread: f32, pub color: u32, pub inset: bool }

/// A gradient resolved against its box (px, relative to the box's top-left). Stops: (offset 0..1 along the gradient, RGBA).
#[derive(Clone, Debug, PartialEq)]
pub enum Grad {
    Linear { dir: (f32, f32), len: f32, stops: Vec<(f32, u32)> },
    Radial { cx: f32, cy: f32, rx: f32, ry: f32, stops: Vec<(f32, u32)> },
    Conic { cx: f32, cy: f32, from: f32, stops: Vec<(f32, u32)> },
}

/// One decorated box: everything a CSS box paints under its content, in paint order.
#[derive(Clone, Debug, PartialEq)]
pub struct Deco {
    pub x: i32, pub y: i32, pub w: i32, pub h: i32, // border box, FX units
    pub r: [f32; 4],          // corner radii px: top-left, top-right, bottom-right, bottom-left (already §5.5-scaled)
    pub fill: u32,            // background-color RGBA (0 = none)
    pub grad: Option<Grad>,
    pub bw: [f32; 4],         // border widths px: top right bottom left
    pub bc: [u32; 4],         // border colours
    pub shadows: Vec<Shadow>, // CSS order: first = top-most
    pub blur: f32,            // backdrop-filter blur σ px (0 = none)
    pub sat: f32,             // backdrop-filter saturate (1 = none)
    pub alpha: u8,            // group opacity multiplier
    pub clip: [i32; 4],       // overflow clip x0 y0 x1 y1 (FX); i32::MIN / MAX = none
}

impl Deco {
    pub fn has_backdrop(&self) -> bool { self.blur > 0.0 || (self.sat - 1.0).abs() > 1e-3 }
    pub fn smooth(&self) -> bool { self.grad.is_some() || !self.shadows.is_empty() || self.has_backdrop() }
    /// Paint extent (FX): the border box grown by the outer shadows' reach (offset + spread + 3σ).
    pub fn paint_bounds(&self) -> (i32, i32, i32, i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (self.x, self.y, self.x + self.w, self.y + self.h);
        for s in self.shadows.iter().filter(|s| !s.inset) {
            let reach = ((s.spread.max(0.0) + 1.5 * s.blur) * FX as f32).ceil() as i32 + FX;
            let (dx, dy) = ((s.dx * FX as f32) as i32, (s.dy * FX as f32) as i32);
            x0 = x0.min(self.x + dx - reach); y0 = y0.min(self.y + dy - reach);
            x1 = x1.max(self.x + self.w + dx + reach); y1 = y1.max(self.y + self.h + dy + reach);
        }
        (x0.max(self.clip[0]), y0.max(self.clip[1]), (x1.min(self.clip[2]) - x0.max(self.clip[0])).max(0), (y1.min(self.clip[3]) - y0.max(self.clip[1])).max(0))
    }
}

pub const NO_CLIP: [i32; 4] = [i32::MIN, i32::MIN, i32::MAX, i32::MAX];

// ───────────────────────────── wire form (snapshot / page process) ─────────────────────────────

impl Deco {
    pub fn write(&self, o: &mut Vec<u8>) {
        let i = |o: &mut Vec<u8>, v: i32| o.extend_from_slice(&v.to_le_bytes());
        let u = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
        let f = |o: &mut Vec<u8>, v: f32| o.extend_from_slice(&v.to_le_bytes());
        for v in [self.x, self.y, self.w, self.h] { i(o, v); }
        for v in self.r { f(o, v); }
        u(o, self.fill);
        for v in self.bw { f(o, v); }
        for v in self.bc { u(o, v); }
        f(o, self.blur); f(o, self.sat); o.push(self.alpha);
        for v in self.clip { i(o, v); }
        u(o, self.shadows.len() as u32);
        for s in &self.shadows { f(o, s.dx); f(o, s.dy); f(o, s.blur); f(o, s.spread); u(o, s.color); o.push(s.inset as u8); }
        let st = |o: &mut Vec<u8>, s: &Vec<(f32, u32)>| { u(o, s.len() as u32); for (p, c) in s { f(o, *p); u(o, *c); } };
        match &self.grad {
            None => o.push(0),
            Some(Grad::Linear { dir, len, stops }) => { o.push(1); f(o, dir.0); f(o, dir.1); f(o, *len); st(o, stops); }
            Some(Grad::Radial { cx, cy, rx, ry, stops }) => { o.push(2); f(o, *cx); f(o, *cy); f(o, *rx); f(o, *ry); st(o, stops); }
            Some(Grad::Conic { cx, cy, from, stops }) => { o.push(3); f(o, *cx); f(o, *cy); f(o, *from); st(o, stops); }
        }
    }
    pub fn read(b: &[u8]) -> Option<Deco> {
        let mut k = 0usize;
        let mut take = |n: usize| -> Option<&[u8]> { let s = b.get(k..k + n)?; k += n; Some(s) };
        macro_rules! i { () => { i32::from_le_bytes(take(4)?.try_into().ok()?) } }
        macro_rules! u { () => { u32::from_le_bytes(take(4)?.try_into().ok()?) } }
        macro_rules! f { () => { f32::from_le_bytes(take(4)?.try_into().ok()?) } }
        macro_rules! b8 { () => { take(1)?[0] } }
        let (x, y, w, h) = (i!(), i!(), i!(), i!());
        let r = [f!(), f!(), f!(), f!()];
        let fill = u!();
        let bw = [f!(), f!(), f!(), f!()];
        let bc = [u!(), u!(), u!(), u!()];
        let (blur, sat, alpha) = (f!(), f!(), b8!());
        let clip = [i!(), i!(), i!(), i!()];
        let ns = (u!() as usize).min(64);
        let mut shadows = Vec::with_capacity(ns);
        for _ in 0..ns { shadows.push(Shadow { dx: f!(), dy: f!(), blur: f!(), spread: f!(), color: u!(), inset: b8!() != 0 }); }
        macro_rules! stops { () => {{ let n = (u!() as usize).min(256); let mut v = Vec::with_capacity(n); for _ in 0..n { v.push((f!(), u!())); } v }} }
        let grad = match b8!() {
            1 => Some(Grad::Linear { dir: (f!(), f!()), len: f!(), stops: stops!() }),
            2 => Some(Grad::Radial { cx: f!(), cy: f!(), rx: f!(), ry: f!(), stops: stops!() }),
            3 => Some(Grad::Conic { cx: f!(), cy: f!(), from: f!(), stops: stops!() }),
            _ => None,
        };
        Some(Deco { x, y, w, h, r, fill, grad, bw, bc, shadows, blur, sat, alpha, clip })
    }
}

// ───────────────────────────── geometry ─────────────────────────────

/// Signed distance (px) from point (px, py) to a rounded rectangle [x0, x0+w] × [y0, y0+h] with corner radii r (tl tr br bl).
#[inline]
pub fn rrect_sd(px: f32, py: f32, x0: f32, y0: f32, w: f32, h: f32, r: &[f32; 4]) -> f32 {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let (qx, qy) = (px - (x0 + hw), py - (y0 + hh));
    let rr = if qx < 0.0 { if qy < 0.0 { r[0] } else { r[3] } } else if qy < 0.0 { r[1] } else { r[2] };
    let rr = rr.min(hw).min(hh).max(0.0);
    let (ax, ay) = (qx.abs() - (hw - rr), qy.abs() - (hh - rr));
    let (mx, my) = (ax.max(0.0), ay.max(0.0));
    (mx * mx + my * my).sqrt() + ax.max(ay).min(0.0) - rr
}

#[inline] fn cov(d: f32) -> f32 { (0.5 - d).clamp(0.0, 1.0) }

/// erfc via Abramowitz & Stegun 7.1.26 (|error| < 1.5e-7).
#[inline]
fn erfc(x: f32) -> f32 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * z);
    let y = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429)))) * (-z * z).exp();
    if x >= 0.0 { y } else { 2.0 - y }
}

/// Coverage of a shape blurred by a Gaussian of std-dev σ, from the shape's signed distance d.
#[inline] fn soft(d: f32, sigma: f32) -> f32 { if sigma < 0.25 { cov(d) } else { 0.5 * erfc(d / (sigma * std::f32::consts::SQRT_2)) } }

/// CSS Backgrounds 3 §5.5: if adjacent radii overflow a side, all radii scale by the smallest side factor.
pub fn fit_radii(mut r: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    let f = [w / (r[0] + r[1]), h / (r[1] + r[2]), w / (r[2] + r[3]), h / (r[3] + r[0])].into_iter().filter(|v| v.is_finite()).fold(1.0f32, f32::min);
    if f < 1.0 { for v in &mut r { *v *= f; } }
    r
}

// ───────────────────────────── colour ─────────────────────────────

#[inline] fn ch(c: u32, sh: u32) -> f32 { ((c >> sh) & 255) as f32 }
/// Premultiplied RGBA (0..255 floats).
#[inline] fn premul(c: u32) -> [f32; 4] { let a = ch(c, 0) / 255.0; [ch(c, 24) * a, ch(c, 16) * a, ch(c, 8) * a, ch(c, 0)] }
#[inline] fn unpremul(p: [f32; 4]) -> u32 {
    if p[3] <= 0.0 { return 0; }
    let a = p[3] / 255.0;
    let q = |v: f32| (v / a).round().clamp(0.0, 255.0) as u32;
    q(p[0]) << 24 | q(p[1]) << 16 | q(p[2]) << 8 | (p[3].round().clamp(0.0, 255.0) as u32)
}
/// src-over of colour `c` scaled by coverage k (0..1) onto opaque dst.
#[inline]
fn over(dst: u32, c: u32, k: f32) -> u32 {
    let a = (((c & 255) as f32 * k) as u32).min(255) * 257 >> 8; // 0..=255 → 0..=256 scale
    if a == 0 { return dst; }
    let na = 256 - a;
    let m = |sh: u32| ((((c >> sh) & 255) * a + ((dst >> sh) & 255) * na) >> 8) << sh;
    m(24) | m(16) | m(8) | 0xff
}

fn stop_color(stops: &[(f32, u32)], t: f32) -> u32 {
    if stops.is_empty() { return 0; }
    if t <= stops[0].0 { return stops[0].1; }
    for w in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (w[0], w[1]);
        if t <= t1 {
            if t1 - t0 <= 1e-6 { return c1; }
            let f = (t - t0) / (t1 - t0);
            let (a, b) = (premul(c0), premul(c1));
            return unpremul([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f]);
        }
    }
    stops[stops.len() - 1].1
}

impl Grad {
    /// Colour at (lx, ly) px relative to the box origin; (w, h) = box size px.
    pub fn at(&self, lx: f32, ly: f32, w: f32, h: f32) -> u32 {
        match self {
            Grad::Linear { dir, len, stops } => {
                let t = ((lx - w * 0.5) * dir.0 + (ly - h * 0.5) * dir.1) / len.max(1e-3) + 0.5;
                stop_color(stops, t)
            }
            Grad::Radial { cx, cy, rx, ry, stops } => {
                let (dx, dy) = ((lx - cx) / rx.max(1e-3), (ly - cy) / ry.max(1e-3));
                stop_color(stops, (dx * dx + dy * dy).sqrt())
            }
            Grad::Conic { cx, cy, from, stops } => {
                let a = (lx - cx).atan2(-(ly - cy)).to_degrees(); // 0 = up, clockwise (y grows down)
                stop_color(stops, ((a - from).rem_euclid(360.0)) / 360.0)
            }
        }
    }
}

// ───────────────────────────── parsing ─────────────────────────────

/// Split tokens at top-level commas (function arguments stay whole).
fn split_commas(t: &[Tok]) -> Vec<&[Tok]> {
    let (mut out, mut depth, mut s) = (Vec::new(), 0i32, 0usize);
    for (i, k) in t.iter().enumerate() {
        match k { Tok::Func(_) | Tok::LParen => depth += 1, Tok::RParen => depth -= 1, Tok::Comma if depth == 0 => { out.push(&t[s..i]); s = i + 1; } _ => {} }
    }
    out.push(&t[s..]);
    out
}

/// Top-level space-separated components (a function call with its arguments = one component).
fn parts(t: &[Tok]) -> Vec<&[Tok]> {
    let (mut out, mut depth, mut s) = (Vec::new(), 0i32, None::<usize>);
    for (i, k) in t.iter().enumerate() {
        match k {
            Tok::Ws if depth == 0 => { if let Some(a) = s.take() { out.push(&t[a..i]); } }
            _ => {
                if s.is_none() { s = Some(i); }
                match k { Tok::Func(_) | Tok::LParen => depth += 1, Tok::RParen => depth -= 1, _ => {} }
            }
        }
    }
    if let Some(a) = s { out.push(&t[a..]); }
    out
}

/// A length in px (px, em, rem, unitless 0); `basis` resolves % (None = % not allowed).
fn px_len(t: &[Tok], fs: f32, basis: Option<f32>) -> Option<f32> {
    match t.first()? {
        Tok::Num(n) if *n == 0.0 => Some(0.0),
        Tok::Dim(n, u) => match u.to_ascii_lowercase().as_str() { "px" => Some(*n), "em" => Some(n * fs), "rem" => Some(n * 16.0), "vw" | "vh" => None, _ => None },
        Tok::Pct(p) => basis.map(|b| p / 100.0 * b),
        _ => None,
    }
}
fn angle_deg(t: &[Tok]) -> Option<f32> {
    match t.first()? {
        Tok::Num(n) if *n == 0.0 => Some(0.0),
        Tok::Dim(n, u) => match u.to_ascii_lowercase().as_str() { "deg" => Some(*n), "turn" => Some(n * 360.0), "rad" => Some(n.to_degrees()), "grad" => Some(n * 0.9), _ => None },
        _ => None,
    }
}

/// Stops: `color [p1] [p2]`, positions as % / length (along `line` px) / angle (conic). Missing positions are fixed up per
/// CSS Images 3 §3.5.3: first 0, last 1, monotonic, runs of unpositioned stops spread evenly.
fn stops(args: &[&[Tok]], cc: u32, line: f32, conic: bool, fs: f32) -> Vec<(f32, u32)> {
    let mut raw: Vec<(Option<f32>, u32)> = Vec::new();
    for a in args {
        let ps = parts(a);
        let Some(ci) = ps.iter().position(|p| parse_color(p, cc).is_some()) else { continue };
        let c = parse_color(ps[ci], cc).unwrap();
        let pos: Vec<f32> = ps.iter().enumerate().filter(|(i, _)| *i != ci).filter_map(|(_, p)| {
            if conic { if let Some(Tok::Pct(v)) = p.first() { return Some(v / 100.0); } angle_deg(p).map(|d| d / 360.0) }
            else { px_len(p, fs, Some(line)).map(|v| v / line.max(1e-3)) }
        }).collect();
        if pos.is_empty() { raw.push((None, c)); } else { for v in pos.iter().take(2) { raw.push((Some(*v), c)); } }
    }
    if raw.is_empty() { return Vec::new(); }
    if raw[0].0.is_none() { raw[0].0 = Some(0.0); }
    let n = raw.len();
    if raw[n - 1].0.is_none() { raw[n - 1].0 = Some(1.0); }
    let mut hi = f32::MIN;
    for r in raw.iter_mut() { if let Some(v) = r.0 { hi = hi.max(v); r.0 = Some(hi); } }
    let mut i = 0;
    while i < n {
        if raw[i].0.is_none() {
            let a = i - 1; let mut b = i; while raw[b].0.is_none() { b += 1; }
            let (va, vb) = (raw[a].0.unwrap(), raw[b].0.unwrap());
            for k in i..b { raw[k].0 = Some(va + (vb - va) * (k - a) as f32 / (b - a) as f32); }
            i = b;
        } else { i += 1; }
    }
    raw.into_iter().map(|(p, c)| (p.unwrap(), c)).collect()
}

/// `background-image` value → a gradient resolved for a w × h px box (first gradient layer only; url() layers are images).
pub fn parse_gradient(src: &str, w: f32, h: f32, cc: u32, fs: f32) -> Option<Grad> {
    let toks = tokenize(src);
    let i = toks.iter().position(|t| matches!(t, Tok::Func(f) if f.ends_with("gradient")))?;
    let Tok::Func(name) = &toks[i] else { return None };
    let name = name.to_ascii_lowercase();
    // the function's argument tokens up to its matching ')'
    let (mut depth, mut end) = (1, toks.len());
    for (k, t) in toks.iter().enumerate().skip(i + 1) {
        match t { Tok::Func(_) | Tok::LParen => depth += 1, Tok::RParen => { depth -= 1; if depth == 0 { end = k; break; } } _ => {} }
    }
    let inner = &toks[i + 1..end];
    let args: Vec<&[Tok]> = split_commas(inner).into_iter().map(|a| { let k = a.iter().take_while(|t| **t == Tok::Ws).count(); &a[k..] }).collect();
    let first = parts(args.first()?);
    let is_stop = |a: &[Tok]| parts(a).iter().any(|p| parse_color(p, cc).is_some());
    let head = if is_stop(args[0]) { None } else { Some(first) };
    let body = if head.is_some() { &args[1..] } else { &args[..] };
    let pos_at = |ps: &[&[Tok]]| -> (f32, f32) {
        let mut c = (w * 0.5, h * 0.5);
        if let Some(k) = ps.iter().position(|p| matches!(p.first(), Some(Tok::Ident(x)) if x == "at")) {
            let rest = &ps[k + 1..];
            let kw = |p: &[Tok]| if let Some(Tok::Ident(x)) = p.first() { Some(x.to_ascii_lowercase()) } else { None };
            let mut xs: Vec<f32> = Vec::new(); let mut ys: Vec<f32> = Vec::new();
            for (j, p) in rest.iter().enumerate() {
                match kw(p).as_deref() {
                    Some("left") => xs.push(0.0), Some("right") => xs.push(w), Some("top") => ys.push(0.0), Some("bottom") => ys.push(h),
                    Some("center") => {}
                    _ => if let Some(v) = px_len(p, fs, Some(if j == 0 { w } else { h })) { if j == 0 { xs.push(v) } else { ys.push(v) } },
                }
            }
            if let Some(x) = xs.first() { c.0 = *x; }
            if let Some(y) = ys.first() { c.1 = *y; }
        }
        c
    };
    match name.trim_start_matches("-webkit-").trim_start_matches("repeating-") {
        "linear-gradient" => {
            let mut a = 180.0f32;
            if let Some(hd) = &head {
                if let Some(d) = hd.first().and_then(|p| angle_deg(p)) { a = d; }
                else if matches!(hd.first().and_then(|p| p.first()), Some(Tok::Ident(t)) if t == "to") {
                    let (mut sx, mut sy) = (0.0f32, 0.0f32);
                    for p in &hd[1..] { if let Some(Tok::Ident(k)) = p.first() { match k.as_str() { "left" => sx = -1.0, "right" => sx = 1.0, "top" => sy = -1.0, "bottom" => sy = 1.0, _ => {} } } }
                    // a corner: the gradient line is perpendicular to the diagonal through the other two corners
                    a = if sx != 0.0 && sy != 0.0 { (sx * h).atan2(-(sy * w)).to_degrees() } else { (sx).atan2(-sy).to_degrees() };
                }
            }
            let r = a.to_radians();
            let dir = (r.sin(), -r.cos());
            let len = (w * r.sin()).abs() + (h * r.cos()).abs();
            Some(Grad::Linear { dir, len, stops: stops(body, cc, len, false, fs) })
        }
        "radial-gradient" => {
            let hd = head.unwrap_or_default();
            let c = pos_at(&hd);
            let before_at: Vec<&[Tok]> = hd.iter().take_while(|p| !matches!(p.first(), Some(Tok::Ident(x)) if x == "at")).copied().collect();
            let circle = before_at.iter().any(|p| matches!(p.first(), Some(Tok::Ident(x)) if x == "circle"));
            let lens: Vec<f32> = before_at.iter().enumerate().filter_map(|(j, p)| px_len(p, fs, Some(if j == 0 { w } else { h }))).collect();
            let ext = before_at.iter().find_map(|p| if let Some(Tok::Ident(x)) = p.first() { if x.contains("side") || x.contains("corner") { Some(x.clone()) } else { None } } else { None }).unwrap_or_else(|| "farthest-corner".into());
            let (sx_c, sx_f) = (c.0.min(w - c.0).abs(), c.0.max(w - c.0).abs());
            let (sy_c, sy_f) = (c.1.min(h - c.1).abs(), c.1.max(h - c.1).abs());
            let (rx, ry) = if lens.len() >= 2 { (lens[0], lens[1]) } else if lens.len() == 1 { (lens[0], lens[0]) } else {
                match (circle, ext.as_str()) {
                    (true, "closest-side") => { let r = sx_c.min(sy_c); (r, r) }
                    (true, "farthest-side") => { let r = sx_f.max(sy_f); (r, r) }
                    (true, "closest-corner") => { let r = (sx_c * sx_c + sy_c * sy_c).sqrt(); (r, r) }
                    (true, _) => { let r = (sx_f * sx_f + sy_f * sy_f).sqrt(); (r, r) }
                    (false, "closest-side") => (sx_c, sy_c),
                    (false, "farthest-side") => (sx_f, sy_f),
                    (false, "closest-corner") => (sx_c * std::f32::consts::SQRT_2, sy_c * std::f32::consts::SQRT_2),
                    (false, _) => (sx_f * std::f32::consts::SQRT_2, sy_f * std::f32::consts::SQRT_2),
                }
            };
            Some(Grad::Radial { cx: c.0, cy: c.1, rx, ry, stops: stops(body, cc, rx.max(1e-3), false, fs) })
        }
        "conic-gradient" => {
            let hd = head.unwrap_or_default();
            let mut from = 0.0;
            if let Some(k) = hd.iter().position(|p| matches!(p.first(), Some(Tok::Ident(x)) if x == "from")) { from = hd.get(k + 1).and_then(|p| angle_deg(p)).unwrap_or(0.0); }
            let c = pos_at(&hd);
            Some(Grad::Conic { cx: c.0, cy: c.1, from, stops: stops(body, cc, 1.0, true, fs) })
        }
        _ => None,
    }
}

/// `box-shadow` value → shadows (CSS order). Lengths: offset-x offset-y [blur [spread]]; colour defaults to currentColor.
pub fn parse_shadows(src: &str, cc: u32, fs: f32) -> Vec<Shadow> {
    let toks = tokenize(src);
    let mut out = Vec::new();
    for layer in split_commas(&toks) {
        let mut s = Shadow { dx: 0.0, dy: 0.0, blur: 0.0, spread: 0.0, color: cc, inset: false };
        let mut lens = Vec::new();
        for p in parts(layer) {
            if matches!(p.first(), Some(Tok::Ident(x)) if x.eq_ignore_ascii_case("inset")) { s.inset = true; }
            else if let Some(v) = px_len(p, fs, None) { lens.push(v); }
            else if let Some(c) = parse_color(p, cc) { s.color = c; }
        }
        if lens.len() < 2 { continue; }
        s.dx = lens[0]; s.dy = lens[1];
        s.blur = lens.get(2).copied().unwrap_or(0.0).max(0.0);
        s.spread = lens.get(3).copied().unwrap_or(0.0);
        if s.color & 0xff != 0 { out.push(s); }
    }
    out
}

/// `backdrop-filter` value → (blur σ px, saturate factor). Other filter functions are ignored for now.
pub fn parse_backdrop(src: &str, fs: f32) -> (f32, f32) {
    let toks = tokenize(src);
    let (mut blur, mut sat) = (0.0f32, 1.0f32);
    let mut i = 0;
    while i < toks.len() {
        if let Tok::Func(f) = &toks[i] {
            let arg = toks[i + 1..].iter().find(|t| !matches!(t, Tok::Ws));
            match (f.to_ascii_lowercase().as_str(), arg) {
                ("blur", Some(t)) => blur = px_len(std::slice::from_ref(t), fs, None).unwrap_or(0.0),
                ("saturate", Some(Tok::Pct(p))) => sat = p / 100.0,
                ("saturate", Some(Tok::Num(n))) => sat = *n,
                _ => {}
            }
        }
        i += 1;
    }
    (blur, sat)
}

// ───────────────────────────── raster ─────────────────────────────

/// Draw one decorated box into a T×T tile at (tx, ty) px. `T` = tile edge.
pub fn draw(d: &Deco, tx: i32, ty: i32, t: usize, buf: &mut [u32]) {
    let f = FX as f32;
    let (bx, by, bw, bh) = (d.x as f32 / f, d.y as f32 / f, d.w as f32 / f, d.h as f32 / f);
    let (px0, py0, pw, ph) = d.paint_bounds();
    let (x0, y0) = ((px0 / FX - tx).max(0), (py0 / FX - ty).max(0));
    let (x1, y1) = (((px0 + pw) / FX + 1 - tx).min(t as i32), ((py0 + ph) / FX + 1 - ty).min(t as i32));
    if x0 >= x1 || y0 >= y1 { return; }
    let k = d.alpha as f32 / 255.0;
    // padding box (inside the borders), radii shrunk by the border widths (§5.3)
    let (ix, iy, iw, ih) = (bx + d.bw[3], by + d.bw[0], (bw - d.bw[1] - d.bw[3]).max(0.0), (bh - d.bw[0] - d.bw[2]).max(0.0));
    let ir = [(d.r[0] - d.bw[3].max(d.bw[0])).max(0.0), (d.r[1] - d.bw[1].max(d.bw[0])).max(0.0), (d.r[2] - d.bw[1].max(d.bw[2])).max(0.0), (d.r[3] - d.bw[3].max(d.bw[2])).max(0.0)];
    let has_border = d.bw.iter().zip(d.bc.iter()).any(|(w, c)| *w > 0.0 && c & 0xff != 0);
    let masks = shadow_masks(d);
    // how deep inside the border box a pixel must be for borders and inset shadows to be out of reach
    let deep = 1.0 + d.bw.iter().cloned().fold(0.0f32, f32::max)
        + d.shadows.iter().filter(|s| s.inset).map(|s| s.dx.abs().max(s.dy.abs()) + s.spread.max(0.0) + 1.5 * s.blur + 1.0).fold(0.0f32, f32::max)
        + d.r.iter().cloned().fold(0.0f32, f32::max) * 0.0;
    let one_bc = d.bc.iter().all(|c| *c == d.bc[0]);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let (gx, gy) = ((tx + xx) as f32 + 0.5, (ty + yy) as f32 + 0.5);
            if ((gx * f) as i32) < d.clip[0] || ((gy * f) as i32) < d.clip[1] || ((gx * f) as i32) >= d.clip[2] || ((gy * f) as i32) >= d.clip[3] { continue; }
            let p = &mut buf[yy as usize * t + xx as usize];
            let db = rrect_sd(gx, gy, bx, by, bw, bh, &d.r);
            if db < -deep {
                // deep inside: no edge, border or inset shadow reaches here — fill + gradient only
                if d.fill & 0xff != 0 { *p = over(*p, d.fill, k); }
                if let Some(g) = &d.grad { let c = g.at(gx - bx, gy - by, bw, bh); if c & 0xff != 0 { *p = over(*p, c, k); } }
                continue;
            }
            let cb = cov(db);
            // 1. outer shadows (last listed paints first), only outside the border box — from the cached masks
            if let Some(m) = &masks {
                if cb < 1.0 {
                    let (mx, my) = ((tx + xx) - m.0, (ty + yy) - m.1);
                    if mx >= 0 && my >= 0 && (mx as usize) < m.2 && (my as usize) < m.3 {
                        for (col, mask) in m.4.iter() { let a = mask[my as usize * m.2 + mx as usize]; if a > 0 { *p = over(*p, *col, a as f32 / 255.0 * k); } }
                    }
                }
            }
            if cb <= 0.0 { continue; }
            // 2. background colour + gradient (background-clip: border-box)
            if d.fill & 0xff != 0 { *p = over(*p, d.fill, cb * k); }
            if let Some(g) = &d.grad { let c = g.at(gx - bx, gy - by, bw, bh); if c & 0xff != 0 { *p = over(*p, c, cb * k); } }
            // 3. inset shadows: inside the padding box, outside the offset + shrunk "hole"
            let ci = if has_border || d.shadows.iter().any(|s| s.inset) { cov(rrect_sd(gx, gy, ix, iy, iw, ih, &ir)) } else { cb };
            for s in d.shadows.iter().rev().filter(|s| s.inset) {
                let hr = ir.map(|r| if r > 0.0 { (r - s.spread).max(0.0) } else { 0.0 });
                let dh = rrect_sd(gx - s.dx, gy - s.dy, ix + s.spread, iy + s.spread, (iw - 2.0 * s.spread).max(0.0), (ih - 2.0 * s.spread).max(0.0), &hr);
                let a = (1.0 - soft(dh, s.blur * 0.5)) * ci;
                if a > 0.002 { *p = over(*p, s.color, a * k); }
            }
            // 4. borders: border box minus padding box; a side's colour where that side is nearest (by distance / width)
            if has_border {
                let a = (cb - ci).max(0.0);
                if a > 0.0 {
                    let c = if one_bc { d.bc[0] } else {
                        let (lx, ly) = (gx - bx, gy - by);
                        let cand = [(ly / d.bw[0].max(1e-3), 0), ((bw - lx) / d.bw[1].max(1e-3), 1), ((bh - ly) / d.bw[2].max(1e-3), 2), (lx / d.bw[3].max(1e-3), 3)];
                        let s = cand.iter().filter(|(_, i)| d.bw[*i] > 0.0).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).map(|(_, i)| *i).unwrap_or(0);
                        d.bc[s]
                    };
                    *p = over(*p, c, a * k);
                }
            }
        }
    }
}

/// The outer shadows of a box as coverage masks over its paint extent (x0, y0, w, h px; one (colour, mask) per shadow, paint
/// order) — they depend on the box's geometry only, so they are computed once and reused across frames (scrolling, clicks).
type Masks = std::rc::Rc<(i32, i32, usize, usize, Vec<(u32, Vec<u8>)>)>;
fn shadow_masks(d: &Deco) -> Option<Masks> {
    use std::hash::{Hash, Hasher};
    thread_local! { static CACHE: std::cell::RefCell<Vec<(u64, Masks)>> = const { std::cell::RefCell::new(Vec::new()) }; }
    if !d.shadows.iter().any(|s| !s.inset) { return None; }
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (d.x, d.y, d.w, d.h).hash(&mut hs);
    for v in d.r { v.to_bits().hash(&mut hs); }
    for s in &d.shadows { (s.dx.to_bits(), s.dy.to_bits(), s.blur.to_bits(), s.spread.to_bits(), s.color, s.inset).hash(&mut hs); }
    let key = hs.finish();
    if let Some(m) = CACHE.with(|c| c.borrow().iter().find(|e| e.0 == key).map(|e| e.1.clone())) { return Some(m); }
    let f = FX as f32;
    let (bx, by, bw, bh) = (d.x as f32 / f, d.y as f32 / f, d.w as f32 / f, d.h as f32 / f);
    let (px0, py0, pw, ph) = { let b = Deco { clip: NO_CLIP, ..d.clone() }; b.paint_bounds() };
    let (x0, y0, w, h) = (px0 / FX, py0 / FX, (pw / FX + 2).max(1) as usize, (ph / FX + 2).max(1) as usize);
    let mut out = Vec::new();
    for s in d.shadows.iter().rev().filter(|s| !s.inset) {
        let sr = d.r.map(|r| if r > 0.0 { (r + s.spread).max(0.0) } else { 0.0 });
        let mut m = vec![0u8; w * h];
        for yy in 0..h { for xx in 0..w {
            let (gx, gy) = ((x0 + xx as i32) as f32 + 0.5, (y0 + yy as i32) as f32 + 0.5);
            let cb = cov(rrect_sd(gx, gy, bx, by, bw, bh, &d.r));
            if cb >= 1.0 { continue; }
            let ds = rrect_sd(gx - s.dx, gy - s.dy, bx - s.spread, by - s.spread, bw + 2.0 * s.spread, bh + 2.0 * s.spread, &sr);
            if ds > 1.5 * s.blur + 1.0 { continue; }
            m[yy * w + xx] = (soft(ds, s.blur * 0.5) * (1.0 - cb) * 255.0).round().min(255.0) as u8;
        } }
        out.push((s.color, m));
    }
    let m: Masks = std::rc::Rc::new((x0, y0, w, h, out));
    CACHE.with(|c| { let mut c = c.borrow_mut(); c.push((key, m.clone())); if c.len() > 32 { c.remove(0); } });
    Some(m)
}

/// Backdrop filter: blur + saturate what is already drawn under the box, inside its rounded border box. `under(x, y, w, h)`
/// returns the page as drawn so far (items below this one) for a px region — the caller re-rasterises an apron around the tile
/// so the blur sees across tile edges.
/// Composite an already blurred backdrop `img` (origin (ix, iy) px, `iw` wide) into one tile, inside the box (overlay path).
pub fn backdrop_from(d: &Deco, tx: i32, ty: i32, t: usize, buf: &mut [u32], img: &[u32], ix: i32, iy: i32, iw: usize, scale: usize) {
    let f = FX as f32;
    let (bx, by, bw, bh) = (d.x as f32 / f, d.y as f32 / f, d.w as f32 / f, d.h as f32 / f);
    if bx >= (tx + t as i32) as f32 || by >= (ty + t as i32) as f32 || bx + bw <= tx as f32 || by + bh <= ty as f32 { return; }
    let ih = img.len() / iw.max(1);
    let k = d.alpha as f32 / 255.0;
    for yy in 0..t {
        for xx in 0..t {
            let (gx, gy) = ((tx + xx as i32) as f32 + 0.5, (ty + yy as i32) as f32 + 0.5);
            let (gxf, gyf) = ((gx * f) as i32, (gy * f) as i32);
            if gxf < d.clip[0] || gyf < d.clip[1] || gxf >= d.clip[2] || gyf >= d.clip[3] { continue; }
            let c = cov(rrect_sd(gx, gy, bx, by, bw, bh, &d.r)) * k;
            if c <= 0.0 { continue; }
            let (sx, sy) = ((tx + xx as i32 - ix) / scale as i32, (ty + yy as i32 - iy) / scale as i32);
            if sx < 0 || sy < 0 || sx as usize >= iw || sy as usize >= ih { continue; }
            let s = img[sy as usize * iw + sx as usize] | 0xff; // saturate is baked into the cached image
            let p = &mut buf[yy * t + xx];
            *p = over(*p, s, c);
        }
    }
}

/// The blur for one glass box over the region it reads (box + apron of 3σ, ≤ 96 px): returns (origin x, origin y, width, pixels).
/// Cached across frames by a hash of the pixels it blurs — a chrome click over an unchanged page blurs nothing.
/// Returns (origin x, origin y, stored width, scale, pixels): the stored image is `scale`× smaller than the region.
/// `full_w` (overlay): the region spans the whole window width, so a glass box sliding sideways (Orbit's fold) reuses one blur.
pub fn blurred_region(d: &Deco, under: &dyn Fn(i32, i32, usize, usize) -> Vec<u32>, full_w: Option<usize>) -> (i32, i32, usize, usize, std::rc::Rc<Vec<u32>>) {
    use std::hash::{Hash, Hasher};
    thread_local! { static CACHE: std::cell::RefCell<Vec<(u64, usize, usize, std::rc::Rc<Vec<u32>>)>> = const { std::cell::RefCell::new(Vec::new()) }; }
    let a = ((3.0 * d.blur).ceil() as i32).clamp(0, 96);
    let (x0, y0) = (d.x / FX - a, d.y / FX - a);
    let (w, h) = ((d.w / FX + 2 * a + 2).max(1) as usize, (d.h / FX + 2 * a + 2).max(1) as usize);
    let (x0, w) = match full_w { Some(fw) => (0, fw), None => (x0, w) };
    let t0 = std::time::Instant::now();
    let src = under(x0, y0, w, h);
    let t_under = t0.elapsed().as_secs_f64() * 1e3;
    let tr = std::env::var_os("OMR_PAINT_TRACE").is_some();
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (x0, y0, w, h, d.blur.to_bits()).hash(&mut hs); src.hash(&mut hs);
    let key = hs.finish();
    if tr { eprintln!("  blur {w}x{h}: under {t_under:.1} ms, hash {:.1} ms", t0.elapsed().as_secs_f64() * 1e3 - t_under); }
    if let Some((sw, sc, hit)) = CACHE.with(|c| c.borrow().iter().find(|e| e.0 == key).map(|e| (e.1, e.2, e.3.clone()))) { return (x0, y0, sw, sc, hit); }
    let t1 = std::time::Instant::now();
    let (img, scale) = if a > 0 && d.blur >= 6.0 {
        // large σ: blur at reduced resolution (k×k mean; k = 2, or 4 from σ 16), σ/k — kept small and sampled nearest (the
        // image is smooth: at σ 22 px a 4× grid is far below what the eye resolves through glass)
        let k = if d.blur >= 16.0 { 4usize } else { 2 };
        let (hw, hh) = (w.div_ceil(k), h.div_ceil(k));
        let mut half = vec![0u32; hw * hh];
        for y in 0..hh { for x in 0..hw {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for yy in y * k..((y + 1) * k).min(h) { for xx in x * k..((x + 1) * k).min(w) { let p = src[yy * w + xx]; r += p >> 24; g += (p >> 16) & 255; b += (p >> 8) & 255; n += 1; } }
            half[y * hw + x] = (r / n) << 24 | (g / n) << 16 | (b / n) << 8 | 0xff;
        } }
        gauss3(&mut half, hw, hh, d.blur / k as f32);
        (half, k)
    } else { let mut img = src; if a > 0 { gauss3(&mut img, w, h, d.blur); } (img, 1usize) };
    let img = if (d.sat - 1.0).abs() > 1e-3 { img.into_iter().map(|p| saturate(p, d.sat)).collect() } else { img };
    if tr { eprintln!("  blur {w}x{h}: blur {:.1} ms", t1.elapsed().as_secs_f64() * 1e3); }
    let img = std::rc::Rc::new(img);
    let sw = w.div_ceil(scale);
    CACHE.with(|c| { let mut c = c.borrow_mut(); c.push((key, sw, scale, img.clone())); if c.len() > 8 { c.remove(0); } });
    (x0, y0, sw, scale, img)
}

pub fn backdrop(d: &Deco, tx: i32, ty: i32, t: usize, buf: &mut [u32], under: &dyn Fn(i32, i32, usize, usize) -> Vec<u32>) {
    let f = FX as f32;
    let (bx, by, bw, bh) = (d.x as f32 / f, d.y as f32 / f, d.w as f32 / f, d.h as f32 / f);
    // does the box touch this tile at all?
    if bx >= (tx + t as i32) as f32 || by >= (ty + t as i32) as f32 || bx + bw <= tx as f32 || by + bh <= ty as f32 { return; }
    let a = ((3.0 * d.blur).ceil() as i32).clamp(0, 96);
    let side = t + 2 * a as usize;
    let mut img = if a > 0 { under(tx - a, ty - a, side, side) } else { buf.to_vec() };
    if a > 0 { gauss3(&mut img, side, side, d.blur); }
    let k = d.alpha as f32 / 255.0;
    for yy in 0..t {
        for xx in 0..t {
            let (gx, gy) = ((tx + xx as i32) as f32 + 0.5, (ty + yy as i32) as f32 + 0.5);
            let gxf = (gx * f) as i32; let gyf = (gy * f) as i32;
            if gxf < d.clip[0] || gyf < d.clip[1] || gxf >= d.clip[2] || gyf >= d.clip[3] { continue; }
            let c = cov(rrect_sd(gx, gy, bx, by, bw, bh, &d.r)) * k;
            if c <= 0.0 { continue; }
            let mut s = img[(yy + a as usize) * side + xx + a as usize] | 0xff;
            if (d.sat - 1.0).abs() > 1e-3 { s = saturate(s, d.sat); }
            let p = &mut buf[yy * t + xx];
            *p = over(*p, s, c);
        }
    }
}

/// Saturate (Filter Effects 1 §12.2: the feColorMatrix saturate matrix).
fn saturate(c: u32, s: f32) -> u32 {
    let (r, g, b) = (ch(c, 24), ch(c, 16), ch(c, 8));
    let m = |a: f32, bb: f32, cc: f32| (a * r + bb * g + cc * b).round().clamp(0.0, 255.0) as u32;
    let rr = m(0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s);
    let gg = m(0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s);
    let bb = m(0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s);
    rr << 24 | gg << 16 | bb << 8 | 0xff
}

/// Gaussian blur ≈ three box blurs per axis (box width from σ: w = √(12σ²/3 + 1)), integer running sums — O(1) per pixel per pass.
pub(crate) fn gauss3(img: &mut [u32], w: usize, h: usize, sigma: f32) {
    let bw = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt().round() as usize).max(1);
    let r = bw / 2;
    if r == 0 || w == 0 || h == 0 { return; }
    let mut ch: [Vec<u32>; 3] = [img.iter().map(|p| p >> 24).collect(), img.iter().map(|p| (p >> 16) & 255).collect(), img.iter().map(|p| (p >> 8) & 255).collect()];
    let mut tmp = vec![0u32; w.max(h)];
    for c in ch.iter_mut() {
        for _ in 0..3 {
            for y in 0..h { box1(&mut c[y * w..(y + 1) * w], 1, w, r, &mut tmp); }
            for x in 0..w { box1(&mut c[x..], w, h, r, &mut tmp); }
        }
    }
    for (i, p) in img.iter_mut().enumerate() { *p = ch[0][i] << 24 | ch[1][i] << 16 | ch[2][i] << 8 | 0xff; }
}

/// One box-blur pass of radius r over n samples at `stride` (edges clamp), integer sums.
fn box1(v: &mut [u32], stride: usize, n: usize, r: usize, tmp: &mut [u32]) {
    let d = (2 * r + 1) as u32;
    let first = v[0]; let last = v[(n - 1) * stride];
    let at = |v: &[u32], i: isize| -> u32 { if i < 0 { first } else if i as usize >= n { last } else { v[i as usize * stride] } };
    let mut acc: u32 = (-(r as isize)..=r as isize).map(|i| at(v, i)).sum();
    let half = d / 2;
    for i in 0..n {
        tmp[i] = (acc + half) / d;
        acc = acc + at(v, (i + r + 1) as isize) - at(v, i as isize - r as isize);
    }
    for i in 0..n { v[i * stride] = tmp[i]; }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rrect_distance() {
        let r = [10.0; 4];
        assert!((rrect_sd(50.0, 50.0, 0.0, 0.0, 100.0, 100.0, &r) + 50.0).abs() < 1e-4); // centre: 50 inside
        assert!(rrect_sd(0.5, 0.5, 0.0, 0.0, 100.0, 100.0, &r) > 0.0); // a rounded corner pixel is outside
        assert!((rrect_sd(50.0, -5.0, 0.0, 0.0, 100.0, 100.0, &r) - 5.0).abs() < 1e-4); // 5 above the top edge
    }
    #[test]
    fn wire_roundtrip() {
        let d = Deco { x: 64, y: 128, w: 6400, h: 3200, r: [8.0, 8.0, 22.0, 0.0], fill: 0x161c2c94,
            grad: parse_gradient("conic-gradient(from -60deg, #00ff00 0 112deg, transparent 0 120deg)", 26.0, 26.0, 0, 16.0),
            bw: [1.0; 4], bc: [0xffffff24; 4], shadows: parse_shadows("0 18px 50px -18px rgba(0,0,0,.6)", 0, 16.0),
            blur: 22.0, sat: 1.6, alpha: 200, clip: NO_CLIP };
        let mut o = Vec::new(); d.write(&mut o);
        assert_eq!(Deco::read(&o), Some(d));
    }
    #[test]
    fn radii_fit() { assert_eq!(fit_radii([999.0; 4], 100.0, 40.0), [20.0; 4]); }
    #[test]
    fn gradients_parse() {
        let g = parse_gradient("linear-gradient(90deg,#ff0000,#0000ff)", 100.0, 10.0, 0, 16.0).unwrap();
        assert_eq!(g.at(0.0, 5.0, 100.0, 10.0), 0xff0000ff); // the left edge is the first stop
        assert_eq!(g.at(100.0, 5.0, 100.0, 10.0), 0x0000ffff);
        let c = parse_gradient("conic-gradient(from -60deg, #00ff00 0 112deg, transparent 0 120deg, #ffff00 0 232deg)", 26.0, 26.0, 0, 16.0).unwrap();
        assert_eq!(c.at(13.0, 1.0, 26.0, 26.0), 0x00ff00ff); // straight up = 60° after `from` → first arc
        let r = parse_gradient("radial-gradient(40% 35% at 18% 12%, rgba(124,199,255,.16), transparent 70%)", 1000.0, 800.0, 0, 16.0).unwrap();
        if let Grad::Radial { cx, cy, rx, ry, .. } = r { assert_eq!((cx, cy, rx, ry), (180.0, 96.0, 400.0, 280.0)); } else { panic!() }
    }
    #[test]
    fn shadows_parse() {
        let s = parse_shadows("inset 0 1px 0 rgba(255,255,255,.1), 0 18px 50px -18px rgba(0,0,0,.6)", 0, 16.0);
        assert_eq!(s.len(), 2);
        assert!(s[0].inset && s[0].dy == 1.0);
        assert!(!s[1].inset && s[1].blur == 50.0 && s[1].spread == -18.0);
        assert_eq!(parse_backdrop("blur(22px) saturate(160%)", 16.0), (22.0, 1.6));
    }
}
