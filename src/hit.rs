// SPDX-License-Identifier: MIT OR Apache-2.0
//! hit — what is under the pointer: a uniform grid over the page (64 px cells, the tile size) holding the boxes
//! that touch each cell. A query is one cell lookup + a scan of that cell's short list; the deepest box wins
//! (highest DOM preorder index = latest in document order = painted on top among in-flow boxes).
/// CSR layout: `start[cell]..start[cell+1]` indexes `ent` (box indices) — two flat arrays, no per-cell allocation.
pub struct HitGrid { cols: usize, rows: usize, start: Vec<u32>, ent: Vec<u32>, boxes: Vec<(u32, [i32; 4])> }

const CELL: i32 = 64 * 64; // 64 px in fixed point

impl HitGrid {
    pub fn build(boxes: &[(u32, [i32; 4])], width: i32, height: i32) -> HitGrid {
        let cols = ((width + CELL - 1) / CELL).max(1) as usize;
        let rows = ((height + CELL - 1) / CELL).max(1) as usize;
        let span = |b: &[i32; 4]| -> Option<(usize, usize, usize, usize)> {
            if b[2] <= 0 || b[3] <= 0 { return None; }
            let (c0, c1) = ((b[0].max(0) / CELL) as usize, (((b[0] + b[2]).max(0) / CELL) as usize).min(cols - 1));
            let (r0, r1) = ((b[1].max(0) / CELL) as usize, (((b[1] + b[3]).max(0) / CELL) as usize).min(rows - 1));
            if c0 >= cols || r0 >= rows { None } else { Some((c0, c1, r0, r1)) }
        };
        let mut count = vec![0u32; cols * rows + 1];
        for (_, b) in boxes { if let Some((c0, c1, r0, r1)) = span(b) { for r in r0..=r1 { for c in c0..=c1 { count[r * cols + c + 1] += 1; } } } }
        for i in 1..count.len() { count[i] += count[i - 1]; }
        let start = count.clone();
        let mut fill = count;
        let mut ent = vec![0u32; *start.last().unwrap() as usize];
        for (k, (_, b)) in boxes.iter().enumerate() {
            if let Some((c0, c1, r0, r1)) = span(b) { for r in r0..=r1 { for c in c0..=c1 { let cell = r * cols + c; ent[fill[cell] as usize] = k as u32; fill[cell] += 1; } } }
        }
        HitGrid { cols, rows, start, ent, boxes: boxes.to_vec() }
    }

    /// Deepest node whose border box contains (x, y) in fixed-point page coordinates.
    #[inline]
    pub fn hit(&self, x: i32, y: i32) -> Option<u32> {
        if x < 0 || y < 0 { return None; }
        let (c, r) = ((x / CELL) as usize, (y / CELL) as usize);
        if c >= self.cols || r >= self.rows { return None; }
        let mut best: Option<u32> = None;
        let cell = r * self.cols + c;
        for &k in &self.ent[self.start[cell] as usize..self.start[cell + 1] as usize] {
            let (n, b) = self.boxes[k as usize];
            if x >= b[0] && x < b[0] + b[2] && y >= b[1] && y < b[1] + b[3] && best.map_or(true, |m| n > m) { best = Some(n); }
        }
        best
    }

    /// Reference: scan every box (to verify the grid and to time against).
    pub fn hit_linear(&self, x: i32, y: i32) -> Option<u32> {
        self.boxes.iter().filter(|(_, b)| x >= b[0] && x < b[0] + b[2] && y >= b[1] && y < b[1] + b[3]).map(|(n, _)| *n).max()
    }

    pub fn bytes(&self) -> usize { self.start.capacity() * 4 + self.ent.capacity() * 4 + self.boxes.capacity() * 20 }
}
