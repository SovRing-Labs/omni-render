// SPDX-License-Identifier: MIT OR Apache-2.0
//! Scroll — the screen is one framebuffer; tile rows are rasterized once, on first exposure, into the
//! content-addressed store and kept as tile ids. A scroll step = one memmove of the framebuffer + decoding
//! only the newly exposed strip from tiles. Steps inside already-rasterized rows touch no layout and no glyphs.
use crate::font::Fonts;
use crate::layout::Item;
use crate::raster::{bin_rows, ImgFn, Store, Tile, T};

pub struct Scroller<'a> {
    items: &'a [Item],
    fonts: &'a Fonts,
    img: &'a ImgFn<'a>,
    pub w: usize,
    pub vh: usize,
    pub page_h: usize,
    rows: Vec<Vec<u32>>,          // binned items per tile row
    tiles: Vec<Option<Vec<u32>>>, // per tile row: tile ids (None = not rasterized yet)
    pub fb: Vec<u32>,             // the screen
    pub y: usize,                 // scroll offset (px)
    pub rasterized_rows: usize,
}

impl<'a> Scroller<'a> {
    pub fn new(items: &'a [Item], fonts: &'a Fonts, img: &'a ImgFn<'a>, w: usize, vh: usize, page_h: usize, store: &mut Store) -> Self {
        let th = page_h.div_ceil(T).max(1);
        let mut s = Scroller { items, fonts, img, w, vh, page_h: page_h.max(vh), rows: bin_rows(items, 0, th), tiles: vec![None; th],
                               fb: vec![0xffffffff; w * vh], y: 0, rasterized_rows: 0 };
        s.paint_strip(0, vh, store);
        s
    }

    fn ensure_row(&mut self, r: usize, store: &mut Store) {
        if r >= self.tiles.len() || self.tiles[r].is_some() { return; }
        let tw = self.w.div_ceil(T);
        let mut buf = vec![0u32; T * T];
        let mut ids = Vec::with_capacity(tw);
        for c in 0..tw {
            let smooth = crate::raster::draw_tile(&self.items, &self.rows[r], self.fonts, self.img, (c * T) as i32, (r * T) as i32, &mut buf);
            ids.push(store.put(Tile::encode_smooth(&buf, smooth)));
        }
        self.tiles[r] = Some(ids);
        self.rasterized_rows += 1;
    }

    /// Fill screen rows [a, b) (screen coordinates) from tiles at the current offset.
    fn paint_strip(&mut self, a: usize, b: usize, store: &mut Store) {
        let mut sy = a;
        while sy < b {
            let py = self.y + sy;
            let r = py / T;
            let in_row = py % T;
            let n = (T - in_row).min(b - sy);
            if r >= self.tiles.len() { for yy in sy..sy + n { self.fb[yy * self.w..(yy + 1) * self.w].fill(0xffffffff); } sy += n; continue; }
            self.ensure_row(r, store);
            let ids = self.tiles[r].clone().unwrap();
            for (c, &id) in ids.iter().enumerate() {
                let t = &store.tiles[id as usize];
                let x0 = c * T;
                let wcols = T.min(self.w - x0);
                match t {
                    Tile::Solid(col) => for yy in 0..n { self.fb[(sy + yy) * self.w + x0..(sy + yy) * self.w + x0 + wcols].fill(*col); },
                    _ => {
                        let px = t.decode();
                        for yy in 0..n { let src = &px[(in_row + yy) * T..(in_row + yy) * T + wcols]; self.fb[(sy + yy) * self.w + x0..(sy + yy) * self.w + x0 + wcols].copy_from_slice(src); }
                    }
                }
            }
            sy += n;
        }
    }

    /// Scroll by dy px (positive = down). Returns the number of screen rows that had to be repainted.
    pub fn scroll(&mut self, dy: i64, store: &mut Store) -> usize {
        let max_y = self.page_h.saturating_sub(self.vh) as i64;
        let ny = (self.y as i64 + dy).clamp(0, max_y) as usize;
        let d = ny as i64 - self.y as i64;
        if d == 0 { return 0; }
        let ad = d.unsigned_abs() as usize;
        self.y = ny;
        if ad >= self.vh { self.paint_strip(0, self.vh, store); return self.vh; }
        if d > 0 {
            self.fb.copy_within(ad * self.w.., 0);
            self.paint_strip(self.vh - ad, self.vh, store);
        } else {
            self.fb.copy_within(..(self.vh - ad) * self.w, ad * self.w);
            self.paint_strip(0, ad, store);
        }
        ad
    }
}
