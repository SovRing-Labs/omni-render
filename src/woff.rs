// SPDX-License-Identifier: MIT OR Apache-2.0
//! woff — web font containers to plain sfnt (TrueType/OpenType) bytes, written from the W3C specs:
//! WOFF 1.0 (per-table zlib) and WOFF 2.0 (one Brotli stream + transformed glyf/loca and hmtx, §5).
//! Every read is bounds-checked; malformed input returns None (fuzzed with the rest of the engine).

struct R<'a> { b: &'a [u8], i: usize }
impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self { R { b, i: 0 } }
    fn u8(&mut self) -> Option<u8> { let v = *self.b.get(self.i)?; self.i += 1; Some(v) }
    fn u16(&mut self) -> Option<u16> { Some((self.u8()? as u16) << 8 | self.u8()? as u16) }
    fn i16(&mut self) -> Option<i16> { self.u16().map(|v| v as i16) }
    fn u32(&mut self) -> Option<u32> { Some((self.u16()? as u32) << 16 | self.u16()? as u32) }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> { let e = self.i.checked_add(n)?; let s = self.b.get(self.i..e)?; self.i = e; Some(s) }
    /// UIntBase128 (WOFF2 §4.1)
    fn base128(&mut self) -> Option<u32> {
        let mut acc: u32 = 0;
        for k in 0..5 {
            let b = self.u8()?;
            if k == 0 && b == 0x80 { return None; }
            if acc & 0xFE00_0000 != 0 { return None; }
            acc = (acc << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 { return Some(acc); }
        }
        None
    }
    /// 255UInt16 (WOFF2 §6.1.1)
    fn u255(&mut self) -> Option<u16> {
        match self.u8()? {
            253 => self.u16(),
            255 => Some(self.u8()? as u16 + 253),
            254 => Some(self.u8()? as u16 + 506),
            c => Some(c as u16),
        }
    }
}

fn be16(v: &mut Vec<u8>, x: u16) { v.extend_from_slice(&x.to_be_bytes()); }
fn be32(v: &mut Vec<u8>, x: u32) { v.extend_from_slice(&x.to_be_bytes()); }

/// Assemble an sfnt from (tag, data) tables.
fn sfnt(flavor: u32, mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    let n = tables.len() as u16;
    let mut es = 0u16; while (1u32 << (es + 1)) <= n as u32 { es += 1; }
    let sr = 16u16 << es;
    let mut out = Vec::new();
    be32(&mut out, flavor); be16(&mut out, n); be16(&mut out, sr); be16(&mut out, es); be16(&mut out, n * 16 - sr);
    let mut off = 12 + 16 * n as u32;
    for (tag, d) in &tables {
        out.extend_from_slice(tag);
        be32(&mut out, 0); be32(&mut out, off); be32(&mut out, d.len() as u32);
        off += (d.len() as u32 + 3) & !3;
    }
    for (_, d) in &tables { out.extend_from_slice(d); while out.len() % 4 != 0 { out.push(0); } }
    out
}

pub fn to_sfnt(b: &[u8]) -> Option<Vec<u8>> {
    match b.get(0..4)? {
        b"wOFF" => woff1(b),
        b"wOF2" => woff2(b),
        [0, 1, 0, 0] | b"OTTO" | b"true" => Some(b.to_vec()),
        _ => None,
    }
}

fn woff1(b: &[u8]) -> Option<Vec<u8>> {
    let mut r = R::new(b);
    r.take(4)?;
    let flavor = r.u32()?;
    r.u32()?;
    let n = r.u16()?;
    r.take(30)?; // reserved .. privLength
    let mut tables = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let tag: [u8; 4] = r.take(4)?.try_into().ok()?;
        let (off, clen, olen) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
        r.u32()?;
        if olen > 64 << 20 { return None; }
        let src = b.get(off..off.checked_add(clen)?)?;
        let data = if clen < olen { miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(src, olen).ok()? } else { src.to_vec() };
        tables.push((tag, data));
    }
    Some(sfnt(flavor, tables))
}

const KNOWN: [&[u8; 4]; 63] = [b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm", b"glyf", b"loca", b"prep",
    b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB",
    b"EBSC", b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar", b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc",
    b"feat", b"fmtx", b"fvar", b"gvar", b"hsty", b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc",
    b"Feat", b"Sill"];

fn woff2(b: &[u8]) -> Option<Vec<u8>> {
    let mut r = R::new(b);
    r.take(4)?;
    let flavor = r.u32()?;
    r.u32()?;
    let n = r.u16()?;
    r.u16()?;
    r.u32()?;
    let comp_len = r.u32()? as usize;
    r.take(24)?; // version, meta, priv
    struct Ent { tag: [u8; 4], olen: u32, tlen: u32, xf: u8 }
    let mut ents = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let fl = r.u8()?;
        let tag: [u8; 4] = if fl & 63 == 63 { r.take(4)?.try_into().ok()? } else { *KNOWN[(fl & 63) as usize] };
        let xf = fl >> 6;
        let olen = r.base128()?;
        let transformed = if &tag == b"glyf" || &tag == b"loca" { xf == 0 } else { xf != 0 };
        let tlen = if transformed { r.base128()? } else { olen };
        ents.push(Ent { tag, olen, tlen, xf: if transformed { 1 } else { 0 } });
    }
    if flavor == 0x74746366 { return None; } // collections: not needed for web fonts
    let comp = b.get(r.i..r.i.checked_add(comp_len)?)?;
    let total: usize = ents.iter().map(|e| e.tlen as usize).sum();
    if total > 64 << 20 { return None; }
    let mut data = Vec::with_capacity(total);
    {
        let mut dec = brotli_decompressor::Decompressor::new(comp, 4096);
        use std::io::Read;
        dec.by_ref().take(total as u64 + 1).read_to_end(&mut data).ok()?;
    }
    if data.len() < total { return None; }
    let mut off = 0usize;
    let mut raw: Vec<([u8; 4], &[u8], u8, u32)> = Vec::new();
    for e in &ents { raw.push((e.tag, &data[off..off + e.tlen as usize], e.xf, e.olen)); off += e.tlen as usize; }
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut glyf_out: Option<(Vec<u8>, Vec<u8>, Vec<i16>)> = None; // glyf, loca, xMins
    let head_index_format = raw.iter().find(|t| &t.0 == b"head").and_then(|t| t.1.get(50..52)).map(|x| i16::from_be_bytes([x[0], x[1]])).unwrap_or(1);
    for (tag, d, xf, _) in &raw {
        if tag == b"glyf" && *xf == 1 { glyf_out = Some(reconstruct_glyf(d)?); }
    }
    let num_hmetrics = raw.iter().find(|t| &t.0 == b"hhea").and_then(|t| t.1.get(34..36)).map(|x| u16::from_be_bytes([x[0], x[1]])).unwrap_or(0);
    let num_glyphs = raw.iter().find(|t| &t.0 == b"maxp").and_then(|t| t.1.get(4..6)).map(|x| u16::from_be_bytes([x[0], x[1]])).unwrap_or(0);
    for (tag, d, xf, _) in raw {
        match &tag {
            b"glyf" if xf == 1 => tables.push((tag, glyf_out.as_ref()?.0.clone())),
            b"loca" if xf == 1 => {
                let (_, loca, _) = glyf_out.as_ref()?;
                tables.push((tag, loca.clone()));
            }
            b"hmtx" if xf == 1 => tables.push((tag, reconstruct_hmtx(d, num_hmetrics, num_glyphs, glyf_out.as_ref().map(|g| &g.2[..]).unwrap_or(&[]))?)),
            b"head" => {
                // loca written in long format → indexToLocFormat = 1
                let mut h = d.to_vec();
                if h.len() >= 52 { h[50] = 0; h[51] = 1; }
                let _ = head_index_format;
                tables.push((tag, h));
            }
            _ => tables.push((tag, d.to_vec())),
        }
    }
    Some(sfnt(flavor, tables))
}

/// WOFF2 §5.1–5.3: rebuild glyf + loca (long) from the transformed streams. Returns (glyf, loca, per-glyph xMin).
fn reconstruct_glyf(d: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Vec<i16>)> {
    let mut h = R::new(d);
    h.u16()?;
    let opt = h.u16()?;
    let ng = h.u16()? as usize;
    h.u16()?;
    let sizes: Vec<usize> = (0..7).map(|_| h.u32().map(|x| x as usize)).collect::<Option<_>>()?;
    let mut off = h.i;
    let mut take = |n: usize| -> Option<&[u8]> { let s = d.get(off..off.checked_add(n)?)?; off += n; Some(s) };
    let (nc, np, fl, gl, co, bb, ins) = (take(sizes[0])?, take(sizes[1])?, take(sizes[2])?, take(sizes[3])?, take(sizes[4])?, take(sizes[5])?, take(sizes[6])?);
    let overlap = if opt & 1 != 0 { Some(take((ng + 7) / 8)?) } else { None };
    let _ = overlap;
    let (mut nc, mut np, mut fl, mut gl, mut co, mut ins) = (R::new(nc), R::new(np), R::new(fl), R::new(gl), R::new(co), R::new(ins));
    let bitmap_len = ((ng + 31) / 32) * 4;
    let bmap = bb.get(..bitmap_len)?;
    let mut bbr = R::new(&bb[bitmap_len..]);
    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity((ng + 1) * 4);
    let mut xmins = vec![0i16; ng];
    for g in 0..ng {
        be32(&mut loca, glyf.len() as u32);
        let n_contours = nc.i16()?;
        let has_bbox = bmap[g >> 3] & (0x80 >> (g & 7)) != 0;
        if n_contours == 0 { continue; }
        let start = glyf.len();
        if n_contours < 0 {
            // composite: copy component records; bbox is mandatory
            let bbox = bbr.take(8)?;
            be16(&mut glyf, 0xFFFF);
            glyf.extend_from_slice(bbox);
            xmins[g] = i16::from_be_bytes([bbox[0], bbox[1]]);
            let mut have_instr = false;
            loop {
                let flags = co.u16()?;
                be16(&mut glyf, flags);
                let mut n = 2; // glyph index
                n += if flags & 1 != 0 { 4 } else { 2 };
                if flags & 8 != 0 { n += 2 } else if flags & 0x40 != 0 { n += 4 } else if flags & 0x80 != 0 { n += 8 }
                glyf.extend_from_slice(co.take(n)?);
                if flags & 0x100 != 0 { have_instr = true; }
                if flags & 0x20 == 0 { break; }
            }
            if have_instr {
                let il = gl.u255()? as usize;
                be16(&mut glyf, il as u16);
                glyf.extend_from_slice(ins.take(il)?);
            }
        } else {
            let nct = n_contours as usize;
            let mut ends = Vec::with_capacity(nct);
            let mut total = 0usize;
            for _ in 0..nct { total += np.u255()? as usize; ends.push(total - 1); if total > 65535 { return None; } }
            let mut xs = Vec::with_capacity(total);
            let mut ys = Vec::with_capacity(total);
            let mut on = Vec::with_capacity(total);
            let (mut x, mut y) = (0i32, 0i32);
            for _ in 0..total {
                let f = fl.u8()?;
                on.push(f & 0x80 == 0);
                let f = f & 0x7f;
                let sign = |flag: u8, v: i32| if flag & 1 != 0 { v } else { -v };
                let (dx, dy) = if f < 10 { let b0 = gl.u8()? as i32; (0, sign(f, (((f & 14) as i32) << 7) + b0)) }
                    else if f < 20 { let b0 = gl.u8()? as i32; (sign(f, ((((f - 10) & 14) as i32) << 7) + b0), 0) }
                    else if f < 84 { let b0 = (f - 20) as i32; let b1 = gl.u8()? as i32; (sign(f, 1 + (b0 & 0x30) + (b1 >> 4)), sign(f >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f))) }
                    else if f < 120 { let b0 = (f - 84) as i32; let (b1, b2) = (gl.u8()? as i32, gl.u8()? as i32); (sign(f, 1 + ((b0 / 12) << 8) + b1), sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + b2)) }
                    else if f < 124 { let (b1, b2, b3) = (gl.u8()? as i32, gl.u8()? as i32, gl.u8()? as i32); (sign(f, (b1 << 4) + (b2 >> 4)), sign(f >> 1, ((b2 & 0x0f) << 8) + b3)) }
                    else { let (b1, b2, b3, b4) = (gl.u8()? as i32, gl.u8()? as i32, gl.u8()? as i32, gl.u8()? as i32); (sign(f, (b1 << 8) + b2), sign(f >> 1, (b3 << 8) + b4)) };
                x += dx; y += dy;
                xs.push(x); ys.push(y);
            }
            let il = gl.u255()? as usize;
            let bbox = if has_bbox { let b8 = bbr.take(8)?; [i16::from_be_bytes([b8[0], b8[1]]), i16::from_be_bytes([b8[2], b8[3]]), i16::from_be_bytes([b8[4], b8[5]]), i16::from_be_bytes([b8[6], b8[7]])] }
                else { [*xs.iter().min()? as i16, *ys.iter().min()? as i16, *xs.iter().max()? as i16, *ys.iter().max()? as i16] };
            xmins[g] = bbox[0];
            be16(&mut glyf, nct as u16);
            for v in bbox { be16(&mut glyf, v as u16); }
            for e in &ends { be16(&mut glyf, *e as u16); }
            be16(&mut glyf, il as u16);
            glyf.extend_from_slice(ins.take(il)?);
            // flags: on-curve bit only → both coordinates as signed 16-bit deltas (simple and exact)
            for &o in &on { glyf.push(if o { 1 } else { 0 }); }
            let (mut px, mut py) = (0i32, 0i32);
            for &v in &xs { be16(&mut glyf, (v - px) as i16 as u16); px = v; }
            for &v in &ys { be16(&mut glyf, (v - py) as i16 as u16); py = v; }
        }
        while (glyf.len() - start) % 4 != 0 { glyf.push(0); }
    }
    be32(&mut loca, glyf.len() as u32);
    Some((glyf, loca, xmins))
}

/// WOFF2 §5.4: hmtx with elided lsb arrays → lsb = glyph xMin.
fn reconstruct_hmtx(d: &[u8], nh: u16, ng: u16, xmins: &[i16]) -> Option<Vec<u8>> {
    let mut r = R::new(d);
    let fl = r.u8()?;
    let adv: Vec<u16> = (0..nh).map(|_| r.u16()).collect::<Option<_>>()?;
    let lsb: Vec<i16> = if fl & 1 == 0 { (0..nh).map(|_| r.i16()).collect::<Option<_>>()? } else { (0..nh as usize).map(|g| *xmins.get(g).unwrap_or(&0)).collect() };
    let mono: Vec<i16> = if fl & 2 == 0 { (nh..ng).map(|_| r.i16()).collect::<Option<_>>()? } else { (nh as usize..ng as usize).map(|g| *xmins.get(g).unwrap_or(&0)).collect() };
    let mut out = Vec::new();
    for k in 0..nh as usize { be16(&mut out, adv[k]); be16(&mut out, lsb[k] as u16); }
    for v in mono { be16(&mut out, v as u16); }
    Some(out)
}
