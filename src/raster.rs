// SPDX-License-Identifier: MIT OR Apache-2.0
//! Raster — the display list drawn into 64×64 tiles; each tile is stored in the smallest exact form:
//! 1 colour → 4 bytes; ≤4 colours → 2 bits/px + palette; ≤16 → 4 bits/px + palette; else RGB565.
//! Tiles are content-addressed (SipHash of the encoded bytes) so identical tiles — page background,
//! repeated rules, the same glyph run — exist once per page and across tabs.
use crate::font::Fonts;
use crate::layout::Item;
use crate::style::FX;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

pub const T: usize = 64;

#[derive(Clone, Hash, PartialEq, Eq)]
/// `Rgb888`: tiles holding smooth paint (gradients, shadows, glass) — RGB565 would band them (P2 Orbit).
pub enum Tile { Solid(u32), P2 { pal: Vec<u32>, px: Vec<u8> }, P4 { pal: Vec<u32>, px: Vec<u8> }, Rgb565(Vec<u16>), Rgb888(Vec<u8>) }

impl Tile {
    pub fn bytes(&self) -> usize {
        match self { Tile::Solid(_) => 4, Tile::P2 { pal, px } | Tile::P4 { pal, px } => pal.len() * 4 + px.len(), Tile::Rgb565(v) => v.len() * 2, Tile::Rgb888(v) => v.len() }
    }
    /// Smooth tiles keep 8 bits per channel when they hold more than 16 colours.
    pub fn encode_smooth(rgba: &[u32], smooth: bool) -> Tile {
        if smooth && { let mut pal: Vec<u32> = Vec::with_capacity(17); for &c in rgba { if !pal.contains(&c) { pal.push(c); if pal.len() > 16 { break; } } } pal.len() > 16 } {
            return Tile::Rgb888(rgba.iter().flat_map(|&c| [(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8]).collect());
        }
        Tile::encode(rgba)
    }
    pub fn encode(rgba: &[u32]) -> Tile {
        let mut pal: Vec<u32> = Vec::with_capacity(16);
        for &c in rgba {
            if !pal.contains(&c) { pal.push(c); if pal.len() > 16 { break; } }
        }
        match pal.len() {
            1 => Tile::Solid(pal[0]),
            2..=4 => {
                let mut px = vec![0u8; T * T / 4];
                for (i, c) in rgba.iter().enumerate() { let k = pal.iter().position(|p| p == c).unwrap() as u8; px[i >> 2] |= k << ((i & 3) * 2); }
                Tile::P2 { pal, px }
            }
            5..=16 => {
                let mut px = vec![0u8; T * T / 2];
                for (i, c) in rgba.iter().enumerate() { let k = pal.iter().position(|p| p == c).unwrap() as u8; px[i >> 1] |= k << ((i & 1) * 4); }
                Tile::P4 { pal, px }
            }
            _ => Tile::Rgb565(rgba.iter().map(|&c| (((c >> 27) & 31) << 11 | ((c >> 18) & 63) << 5 | ((c >> 11) & 31)) as u16).collect()),
        }
    }
    pub fn decode(&self) -> Vec<u32> {
        match self {
            Tile::Solid(c) => vec![*c; T * T],
            Tile::P2 { pal, px } => (0..T * T).map(|i| pal[((px[i >> 2] >> ((i & 3) * 2)) & 3) as usize]).collect(),
            Tile::P4 { pal, px } => (0..T * T).map(|i| pal[((px[i >> 1] >> ((i & 1) * 4)) & 15) as usize]).collect(),
            Tile::Rgb565(v) => v.iter().map(|&p| { let p = p as u32; let (r, g, b) = ((p >> 11) & 31, (p >> 5) & 63, p & 31);
                ((r << 3 | r >> 2) << 24) | ((g << 2 | g >> 4) << 16) | ((b << 3 | b >> 2) << 8) | 0xff }).collect(),
            Tile::Rgb888(v) => v.chunks_exact(3).map(|c| (c[0] as u32) << 24 | (c[1] as u32) << 16 | (c[2] as u32) << 8 | 0xff).collect(),
        }
    }
}

/// Content-addressed tile store, shareable across pages/tabs.
#[derive(Default)]
pub struct Store { pub tiles: Vec<Tile>, ix: HashMap<u64, u32>, pub refs: usize }

impl Store {
    pub fn put(&mut self, t: Tile) -> u32 {
        self.refs += 1;
        let mut h = std::collections::hash_map::DefaultHasher::new(); // SipHash-1-3
        t.hash(&mut h);
        let k = h.finish();
        if let Some(&i) = self.ix.get(&k) { if self.tiles[i as usize] == t { return i; } }
        self.tiles.push(t);
        let i = (self.tiles.len() - 1) as u32;
        self.ix.insert(k, i);
        i
    }
    pub fn bytes(&self) -> usize { self.tiles.iter().map(|t| t.bytes()).sum::<usize>() + self.ix.len() * 16 }
}

/// A rendered page: grid of tile ids (row-major), width/height in tiles.
pub struct Page { pub tw: usize, pub th: usize, pub grid: Vec<u32>, pub ms: f64 }

/// (node, mode, box w px, box h px) → RGBA pixels. For backgrounds the returned size IS the tile size.
pub type ImgFn<'a> = dyn Fn(u32, u8, i32, i32) -> Option<std::rc::Rc<(u32, u32, Vec<u32>)>> + 'a;

fn blend(dst: u32, src: u32, a: u32) -> u32 {
    // a in 0..=255; src alpha folded in
    let a = a * (src & 255) / 255;
    if a == 0 { return dst; }
    if a >= 255 { return src | 0xff; }
    let ch = |s: u32, d: u32, sh: u32| ((((s >> sh) & 255) * a + ((d >> sh) & 255) * (255 - a)) / 255) << sh;
    ch(src, dst, 24) | ch(src, dst, 16) | ch(src, dst, 8) | 0xff
}

/// Rasterize rows [y0, y1) px of the display list into the store. Items are binned per tile row first.
/// Bin display items by tile row (each item listed in every row it may touch).
pub fn bin_rows(items: &[Item], y0: usize, th: usize) -> Vec<Vec<u32>> {
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); th];
    for (k, it) in items.iter().enumerate() {
        let (_, iy, _, ih) = it.paint_bounds();
        let a = ((iy / FX) as i64 - y0 as i64 - 4).max(0) as usize / T;
        let b = (((iy + ih) / FX) as i64 - y0 as i64 + 4).max(-1);
        if b < 0 { continue; }
        let b = (b as usize / T).min(th.saturating_sub(1));
        for r in a..=b { if r < th { rows[r].push(k as u32); } }
    }
    rows
}

/// Pixels already on screen under a display list (RGBA rows, page coordinates from row `y0`): an overlay — Omni Web's chrome
/// over the page (BROWSER-PRODUCT-0 C1) — draws on them instead of white, and its glass blurs them.
pub struct Base<'a> { pub px: &'a [u32], pub w: usize, pub y0: i32 }
impl Base<'_> {
    fn fill(&self, tx: i32, ty: i32, side: usize, buf: &mut [u32]) {
        let rows = self.px.len() / self.w.max(1);
        for yy in 0..side { for xx in 0..side {
            let (x, y) = ((tx + xx as i32).clamp(0, self.w as i32 - 1) as usize, (ty + yy as i32 - self.y0).clamp(0, rows as i32 - 1) as usize);
            buf[yy * side + xx] = self.px[y * self.w + x];
        } }
    }
}

pub fn render(items: &[Item], fonts: &Fonts, img: &ImgFn, width_px: usize, y0: usize, y1: usize, store: &mut Store) -> Page {
    render_on(items, fonts, img, width_px, y0, y1, store, None)
}

pub fn render_on(items: &[Item], fonts: &Fonts, img: &ImgFn, width_px: usize, y0: usize, y1: usize, store: &mut Store, base: Option<&Base>) -> Page {
    let t0 = std::time::Instant::now();
    let tw = width_px.div_ceil(T);
    let th = (y1 - y0).div_ceil(T);
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); th];
    for (k, it) in items.iter().enumerate() {
        let (_, iy, _, ih) = it.paint_bounds();
        let a = ((iy / FX) as i64 - y0 as i64 - 4).max(0) as usize / T;
        let b = (((iy + ih) / FX) as i64 - y0 as i64 + 4).max(-1);
        if b < 0 { continue; }
        let b = (b as usize / T).min(th.saturating_sub(1));
        for r in a..=b { if r < th { rows[r].push(k as u32); } }
    }
    let mut grid = Vec::with_capacity(tw * th);
    let mut buf = vec![0u32; T * T];
    for (r, row_items) in rows.iter().enumerate() {
        for c in 0..tw {
            let (tx, ty) = ((c * T) as i32, (y0 + r * T) as i32);
            let smooth = draw_tile_on(items, row_items, fonts, img, tx, ty, &mut buf, base);
            grid.push(store.put(Tile::encode_smooth(&buf, smooth || base.is_some())));
        }
    }
    Page { tw, th, grid, ms: t0.elapsed().as_secs_f64() * 1e3 }
}

/// INCR-R3 S3 — tile repaint trit. A tile is a pure function of its origin and the ordered display items that touch it
/// (fonts and images fixed for the page session). Key hit = trit 0: the stored tile id is reused (no draw, no encode, no
/// store hash); miss = trit +: drawn and stored. The memo keeps only the current frame's keys.
/// Gotcha: Image items key on the node id — valid while a node's image does not change within the session.
#[derive(Default)]
pub struct TileMemo { map: HashMap<u128, u32>, pub hits: usize, pub misses: usize }
#[inline] fn mix64(mut z: u64) -> u64 { z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); z ^ (z >> 31) }
struct H2(u64, u64);
impl H2 { fn new() -> H2 { H2(0x243f6a8885a308d3, 0x13198a2e03707344) }
    #[inline] fn put(&mut self, p: u64) { self.0 = mix64(self.0 ^ p.wrapping_mul(0x9e3779b97f4a7c15)); self.1 = mix64(self.1.rotate_left(17) ^ p.wrapping_add(0xa4093822299f31d0)); }
    fn key(&self) -> u128 { ((self.0 as u128) << 64) | self.1 as u128 } }
fn put_item(h: &mut H2, it: &Item) {
    let c = |v: i32| v as u32 as u64;
    match it {
        Item::Rect { x, y, w, h: hh, color } => { h.put(1); h.put(c(*x) << 32 | c(*y)); h.put(c(*w) << 32 | c(*hh)); h.put(*color as u64); }
        Item::Image { x, y, w, h: hh, node, mode } => { h.put(2); h.put(c(*x) << 32 | c(*y)); h.put(c(*w) << 32 | c(*hh)); h.put((*node as u64) << 8 | *mode as u64); }
        Item::Box(d) => { h.put(4); h.put(c(d.x) << 32 | c(d.y)); h.put(c(d.w) << 32 | c(d.h));
            let mut s = std::collections::hash_map::DefaultHasher::new(); format!("{d:?}").hash(&mut s); h.put(s.finish()); }
        Item::Text { x, y, px, color, face, glyphs, deco, w } => {
            h.put(3); h.put(c(*x) << 32 | c(*y)); h.put(c(*px) << 32 | c(*w)); h.put((*color as u64) << 32 | (*face as u64) << 8 | *deco as u64);
            h.put(glyphs.len() as u64); for g in glyphs { h.put(*g as u64); }
        }
    }
}
/// `render` with the tile repaint trit (same tiles, same store semantics).
pub fn render_incr(items: &[Item], fonts: &Fonts, img: &ImgFn, width_px: usize, y0: usize, y1: usize, store: &mut Store, memo: &mut TileMemo) -> Page {
    let t0 = std::time::Instant::now();
    let tw = width_px.div_ceil(T);
    let th = (y1 - y0).div_ceil(T);
    let rows = bin_rows(items, y0, th);
    let mut grid = Vec::with_capacity(tw * th);
    let mut buf = vec![0u32; T * T];
    let mut new_map: HashMap<u128, u32> = HashMap::with_capacity(memo.map.len());
    for (r, row_items) in rows.iter().enumerate() {
        for cc in 0..tw {
            let (tx, ty) = ((cc * T) as i32, (y0 + r * T) as i32);
            let mut h = H2::new(); h.put((tx as u32 as u64) << 32 | ty as u32 as u64);
            let mut glass = false;
            for &k in row_items { let it = &items[k as usize]; if touches(it, tx, ty) { put_item(&mut h, it); glass |= matches!(it, Item::Box(d) if d.has_backdrop()); } }
            // a glass tile also depends on what lies under its blur apron (beyond the tile): key every item near it
            if glass { for it in items { let (x, y, w, hh) = it.paint_bounds(); let (a, m) = (96 * FX, T as i32 * FX);
                if x < tx * FX + m + a && x + w > tx * FX - a && y < ty * FX + m + a && y + hh > ty * FX - a { put_item(&mut h, it); } } }
            let key = h.key();
            let id = match memo.map.get(&key).copied().or_else(|| new_map.get(&key).copied()) {
                Some(id) => { memo.hits += 1; id }
                None => {
                    memo.misses += 1;
                    let smooth = draw_tile(items, row_items, fonts, img, tx, ty, &mut buf);
                    store.put(Tile::encode_smooth(&buf, smooth))
                }
            };
            new_map.insert(key, id);
            grid.push(id);
        }
    }
    memo.map = new_map;
    Page { tw, th, grid, ms: t0.elapsed().as_secs_f64() * 1e3 }
}

/// Cheap cull: does the item's box (plus a glyph-overhang margin) touch the tile at (tx, ty)?
#[inline]
pub(crate) fn touches(it: &Item, tx: i32, ty: i32) -> bool {
    let (x, y, w, h) = it.paint_bounds();
    let m = if let Item::Text { px, .. } = it { px / FX } else { 1 };
    let (x0, x1, y0, y1) = (x / FX - m, (x + w) / FX + m + 1, y / FX - m, (y + h) / FX + m + 1);
    x1 >= tx && x0 < tx + T as i32 && y1 >= ty && y0 < ty + T as i32
}

/// Draw the items touching one tile (white first). A backdrop-filter box re-rasterises an apron of what lies under it so the
/// blur reads across tile edges. Returns whether the tile holds smooth paint (stored lossless).
pub(crate) fn draw_tile(items: &[Item], row_items: &[u32], fonts: &Fonts, img: &ImgFn, tx: i32, ty: i32, buf: &mut [u32]) -> bool {
    draw_tile_on(items, row_items, fonts, img, tx, ty, buf, None)
}

pub(crate) fn draw_tile_on(items: &[Item], row_items: &[u32], fonts: &Fonts, img: &ImgFn, tx: i32, ty: i32, buf: &mut [u32], base: Option<&Base>) -> bool {
    match base { Some(b) => b.fill(tx, ty, T, buf), None => buf.fill(0xffffffff) }
    let mut smooth = false;
    for &k in row_items {
        let it = &items[k as usize];
        if !touches(it, tx, ty) { continue; }
        if let Item::Box(d) = it {
            smooth |= d.smooth();
            if d.has_backdrop() {
                let under = |ax: i32, ay: i32, aw: usize, ah: usize| -> Vec<u32> {
                    let mut out = vec![0xffffffffu32; aw * ah];
                    let mut sub = vec![0u32; T * T];
                    let mut sy = 0; while sy < ah { let mut sx = 0; while sx < aw {
                        let (ox, oy) = (ax + sx as i32, ay + sy as i32);
                        match base { Some(b) => b.fill(ox, oy, T, &mut sub), None => sub.fill(0xffffffff) }
                        for j in 0..k as usize { let u = &items[j]; if touches(u, ox, oy) { draw(u, fonts, img, ox, oy, &mut sub); } }
                        for yy in 0..T.min(ah - sy) { for xx in 0..T.min(aw - sx) { out[(sy + yy) * aw + sx + xx] = sub[yy * T + xx]; } }
                        sx += T; } sy += T; }
                    out
                };
                crate::paint::backdrop(d, tx, ty, T, buf, &under);
            }
        }
        draw(it, fonts, img, tx, ty, buf);
    }
    smooth
}

pub(crate) fn draw(it: &Item, fonts: &Fonts, img: &ImgFn, tx: i32, ty: i32, buf: &mut [u32]) {
    match it {
        Item::Box(d) => crate::paint::draw(d, tx, ty, T, buf),
        Item::Rect { x, y, w, h, color } => {
            let (x0, y0) = ((*x + FX / 2).div_euclid(FX) - tx, (*y + FX / 2).div_euclid(FX) - ty);
            let (x1, y1) = ((*x + *w + FX / 2).div_euclid(FX) - tx, (*y + *h + FX / 2).div_euclid(FX) - ty);
            let (x1, y1) = (x1.max(x0 + if *w > 0 { 1 } else { 0 }), y1.max(y0 + if *h > 0 { 1 } else { 0 }));
            for yy in y0.max(0)..y1.min(T as i32) { for xx in x0.max(0)..x1.min(T as i32) {
                let p = &mut buf[yy as usize * T + xx as usize]; *p = blend(*p, *color, 255); } }
        }
        Item::Image { x, y, w, h, node, mode } => {
            let (x0, y0, ww, hh) = (x / FX - tx, y / FX - ty, (w / FX).max(1), (h / FX).max(1));
            if x0 >= T as i32 || y0 >= T as i32 || x0 + ww <= 0 || y0 + hh <= 0 { return; }
            let im = img(*node, *mode, ww, hh);
            if *mode == 0 {
                match im {
                    Some(im) => {
                        let (iw, ih, ref px) = *im;
                        for yy in y0.max(0)..(y0 + hh).min(T as i32) { for xx in x0.max(0)..(x0 + ww).min(T as i32) {
                            let sx = ((xx - x0) as i64 * iw as i64 / ww as i64) as usize;
                            let sy = ((yy - y0) as i64 * ih as i64 / hh as i64) as usize;
                            let c = px[sy.min(ih as usize - 1) * iw as usize + sx.min(iw as usize - 1)];
                            let p = &mut buf[yy as usize * T + xx as usize]; *p = blend(*p, c, 255);
                        } }
                    }
                    None => {
                        for yy in y0.max(0)..(y0 + hh).min(T as i32) { for xx in x0.max(0)..(x0 + ww).min(T as i32) {
                            let p = &mut buf[yy as usize * T + xx as usize]; *p = blend(*p, 0xeeeeeeff, 255); } }
                    }
                }
                return;
            }
            // background image: natural size (or cover/contain), tiled per repeat mode, clipped to the box
            let im = match im { Some(i) => i, None => return };
            let (iw, ih, ref px) = *im;
            if iw == 0 || ih == 0 { return; }
            let m = *mode;
            let rep = (m - 1) & 7;
            let (tw, th) = if m & 8 != 0 { let s = (ww as f32 / iw as f32).max(hh as f32 / ih as f32); ((iw as f32 * s) as i32, (ih as f32 * s) as i32) }
                else if m & 16 != 0 { let s = (ww as f32 / iw as f32).min(hh as f32 / ih as f32); ((iw as f32 * s) as i32, (ih as f32 * s) as i32) }
                else { (iw as i32, ih as i32) };
            let (tw, th) = (tw.max(1), th.max(1));
            let (ox, oy) = if m & 32 != 0 { ((ww - tw) / 2, (hh - th) / 2) } else { (0, 0) };
            for yy in y0.max(0)..(y0 + hh).min(T as i32) {
                let ly = yy - y0 - oy;
                let ty_ = if rep == 0 || rep == 3 { ly.rem_euclid(th) } else if ly < 0 || ly >= th { continue } else { ly };
                for xx in x0.max(0)..(x0 + ww).min(T as i32) {
                    let lx = xx - x0 - ox;
                    let tx_ = if rep == 0 || rep == 2 { lx.rem_euclid(tw) } else if lx < 0 || lx >= tw { continue } else { lx };
                    let sx = (tx_ as i64 * iw as i64 / tw as i64) as usize;
                    let sy = (ty_ as i64 * ih as i64 / th as i64) as usize;
                    let c = px[sy.min(ih as usize - 1) * iw as usize + sx.min(iw as usize - 1)];
                    let p = &mut buf[yy as usize * T + xx as usize]; *p = blend(*p, c, 255);
                }
            }
        }
        Item::Text { x, y, px, color, glyphs, face, .. } => {
            let (bx, by) = (x / FX - tx, (y + FX / 2) / FX - ty);
            let fid = *face;
            for &pg in glyphs {
                let (g, off) = ((pg >> 16) as u16, ((pg & 0xffff) as i32) * 16);
                let gx = bx + (off + (x % FX) + FX / 2) / FX;
                if gx > T as i32 + 2 || gx + (px / FX) * 2 < 0 { continue; }
                let m = match fonts.glyph(fid, g, *px) { Some(m) => m, None => continue };
                let (ox, oy) = (gx + m.left as i32, by + m.top as i32);
                if ox >= T as i32 || oy >= T as i32 || ox + m.w as i32 <= 0 || oy + m.h as i32 <= 0 { continue; }
                for my in 0..m.h as i32 {
                    let yy = oy + my;
                    if yy < 0 || yy >= T as i32 { continue; }
                    for mx in 0..m.w as i32 {
                        let xx = ox + mx;
                        if xx < 0 || xx >= T as i32 { continue; }
                        let lv = m.at(mx as usize, my as usize);
                        if lv == 0 { continue; }
                        let p = &mut buf[yy as usize * T + xx as usize];
                        *p = blend(*p, *color, lv as u32);
                    }
                }
            }
        }
    }
}

/// Does the item's paint extent touch the square region at (x, y) px of edge `side`?
fn touches_rect(it: &Item, x: i32, y: i32, side: i32) -> bool {
    let (ix, iy, iw, ih) = it.paint_bounds();
    let (x0, y0, x1, y1) = (ix / FX - 2, iy / FX - 2, (ix + iw) / FX + 2, (iy + ih) / FX + 2);
    x1 >= x && x0 < x + side && y1 >= y && y0 < y + side
}

/// Overlay paint (BROWSER-PRODUCT-0 fix 1, author 10-07 "quite laggy"): draw a display list over `base` (RGBA, w × h) into `out`,
/// touching only the tiles items cover — no tile store, no encode. Each glass box blurs its backdrop once per frame (not per
/// tile), from `base` plus the items below it, and that blur is reused across frames while the pixels under it are unchanged.
pub fn paint_over(items: &[Item], fonts: &Fonts, img: &ImgFn, w: usize, h: usize, base: &[u32], out: &mut [u32]) {
    let (tw, th) = (w.div_ceil(T), h.div_ceil(T));
    let rows = bin_rows(items, 0, th);
    let b = Base { px: base, w, y0: 0 };
    let mut blurs: HashMap<usize, (i32, i32, usize, usize, std::rc::Rc<Vec<u32>>)> = HashMap::new();
    let mut buf = vec![0u32; T * T];
    let trace = std::env::var_os("OMR_PAINT_TRACE").is_some();
    // tile memo across frames: a tile whose touching items and underlying pixels (its apron too, under glass) are unchanged is
    // copied from the last frame — opening a panel paints the panel, not the bar again
    thread_local! { static MEMO: std::cell::RefCell<HashMap<u64, Vec<u32>>> = std::cell::RefCell::new(HashMap::new()); }
    let mut new_memo: HashMap<u64, Vec<u32>> = HashMap::new();
    // one fast hash per 64 px tile of the base (multiply-xor over u64 pairs), combined over a glass tile's apron below
    let bh: Vec<u64> = (0..th * tw).map(|i| {
        let (r, c) = (i / tw, i % tw);
        let mut x: u64 = 0x9e3779b97f4a7c15 ^ i as u64;
        for y in r * T..((r + 1) * T).min(h) { let row = &base[y * w + c * T..y * w + ((c + 1) * T).min(w)];
            for p in row.chunks(2) { let v = p[0] as u64 | (*p.get(1).unwrap_or(&0) as u64) << 32; x = (x ^ v).wrapping_mul(0x100000001b3).rotate_left(29); } }
        x
    }).collect();
    let (mut hits, mut misses) = (0usize, 0usize);
    let (mut t_blur, mut t_box, mut t_other, mut ntiles, mut t_comp) = (0f64, 0f64, 0f64, 0usize, 0f64);
    for (r, row_items) in rows.iter().enumerate() {
        for c in 0..tw {
            let (tx, ty) = ((c * T) as i32, (r * T) as i32);
            let touching: Vec<u32> = row_items.iter().copied().filter(|&k| touches(&items[k as usize], tx, ty)).collect();
            if !touching.is_empty() { ntiles += 1; }
            if touching.is_empty() { continue; }
            let key = {
                use std::hash::Hasher;
                let mut hh = std::collections::hash_map::DefaultHasher::new();
                hh.write_i32(tx); hh.write_i32(ty); hh.write_usize(w); hh.write_usize(h);
                let mut h2 = H2::new();
                for &k in &touching { put_item(&mut h2, &items[k as usize]); }
                hh.write_u128(h2.key());
                let glass = touching.iter().any(|&k| matches!(&items[k as usize], Item::Box(d) if d.has_backdrop()));
                let a = if glass { 96 } else { 0 };
                let (r0, r1) = (((ty - a).max(0) as usize) / T, (((ty + T as i32 + a - 1).max(0) as usize) / T).min(th - 1));
                let (c0, c1) = (((tx - a).max(0) as usize) / T, (((tx + T as i32 + a - 1).max(0) as usize) / T).min(tw - 1));
                for rr in r0..=r1 { for cc in c0..=c1 { hh.write_u64(bh[rr * tw + cc]); } }
                if glass { let mut h3 = H2::new(); for (j, u) in items.iter().enumerate() { if !touching.contains(&(j as u32)) && touches_rect(u, tx - a, ty - a, T as i32 + 2 * a) { put_item(&mut h3, u); } } hh.write_u128(h3.key()); }
                hh.finish()
            };
            if let Some(px) = MEMO.with(|m| m.borrow().get(&key).cloned()) {
                hits += 1;
                for yy in 0..T { let y = ty as usize + yy; if y >= h { break; }
                    for xx in 0..T { let x = tx as usize + xx; if x >= w { break; } out[y * w + x] = px[yy * T + xx]; } }
                new_memo.insert(key, px);
                continue;
            }
            misses += 1;
            b.fill(tx, ty, T, &mut buf);
            for &k in &touching {
                let it = &items[k as usize];
                let t0 = std::time::Instant::now();
                if let Item::Box(d) = it {
                    if d.has_backdrop() {
                        let e = blurs.entry(k as usize).or_insert_with(|| crate::paint::blurred_region(d, &|ax: i32, ay: i32, aw: usize, ah: usize| {
                            let mut reg = vec![0u32; aw * ah];
                            let mut sub = vec![0u32; T * T];
                            let mut sy = 0; while sy < ah { let mut sx = 0; while sx < aw {
                                let (ox, oy) = (ax + sx as i32, ay + sy as i32);
                                b.fill(ox, oy, T, &mut sub);
                                for j in 0..k as usize { let u = &items[j]; if touches(u, ox, oy) { draw(u, fonts, img, ox, oy, &mut sub); } }
                                for yy in 0..T.min(ah - sy) { for xx in 0..T.min(aw - sx) { reg[(sy + yy) * aw + sx + xx] = sub[yy * T + xx]; } }
                                sx += T; } sy += T; }
                            reg
                        }, ((d.w / FX) as usize * 3 > w).then_some(w)));
                        let tb = std::time::Instant::now();
                        crate::paint::backdrop_from(d, tx, ty, T, &mut buf, &e.4, e.0, e.1, e.2, e.3);
                        if trace { t_comp += tb.elapsed().as_secs_f64(); }
                    }
                }
                let t1 = std::time::Instant::now();
                draw(it, fonts, img, tx, ty, &mut buf);
                if trace { t_blur += (t1 - t0).as_secs_f64(); if matches!(it, Item::Box(_)) { t_box += t1.elapsed().as_secs_f64(); } else { t_other += t1.elapsed().as_secs_f64(); } }
            }
            for yy in 0..T { let y = ty as usize + yy; if y >= h { break; }
                for xx in 0..T { let x = tx as usize + xx; if x >= w { break; } out[y * w + x] = buf[yy * T + xx]; } }
            new_memo.insert(key, buf.clone());
        }
    }
    MEMO.with(|m| *m.borrow_mut() = new_memo);
    if trace { eprintln!("paint_over: composite {:.1} ms (in backdrop)", t_comp * 1e3); }
    if trace { eprintln!("paint_over: memo {hits} hit / {misses} drawn · {ntiles} tiles · backdrop {:.1} ms · boxes {:.1} ms · text/rect/img {:.1} ms · {} items", t_blur * 1e3, t_box * 1e3, t_other * 1e3, items.len()); }
}

/// Compose the page back into RGBA rows (for PNG output / SSIM against Chrome).
pub fn compose(p: &Page, store: &Store, width_px: usize) -> Vec<u32> {
    let h = p.th * T;
    let mut img = vec![0u32; width_px * h];
    for r in 0..p.th { for c in 0..p.tw {
        let t = store.tiles[p.grid[r * p.tw + c] as usize].decode();
        for y in 0..T { for x in 0..T {
            let (gx, gy) = (c * T + x, r * T + y);
            if gx < width_px { img[gy * width_px + gx] = t[y * T + x]; }
        } }
    } }
    img
}
