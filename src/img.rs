// SPDX-License-Identifier: MIT OR Apache-2.0
//! Images — layout needs only intrinsic sizes, read from headers (no decode): PNG, GIF, JPEG, WebP, SVG.
//! Pixel decode is lazy and only for images on screen; PNG via the `png` crate (safe Rust).
//! Decoded pixels are kept at DISPLAY size, never at source size (a 4000 px photo shown at 300 px costs 300 px).

pub fn size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() >= 24 && &b[0..8] == b"\x89PNG\r\n\x1a\n" {
        return Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)));
    }
    if b.len() >= 10 && (&b[0..6] == b"GIF87a" || &b[0..6] == b"GIF89a") {
        return Some((u16::from_le_bytes([b[6], b[7]]) as u32, u16::from_le_bytes([b[8], b[9]]) as u32));
    }
    if b.len() >= 4 && b[0] == 0xFF && b[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF { i += 1; continue; }
            let m = b[i + 1];
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) { i += 2; continue; }
            let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                return Some((u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32, u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32));
            }
            i += 2 + len;
        }
        return None;
    }
    if b.len() >= 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        match &b[12..16] {
            b"VP8 " => return Some(((u16::from_le_bytes([b[26], b[27]]) & 0x3fff) as u32, (u16::from_le_bytes([b[28], b[29]]) & 0x3fff) as u32)),
            b"VP8L" => { let v = u32::from_le_bytes(b[21..25].try_into().ok()?); return Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1)); }
            b"VP8X" => {
                let w = 1 + (b[24] as u32 | (b[25] as u32) << 8 | (b[26] as u32) << 16);
                let h = 1 + (b[27] as u32 | (b[28] as u32) << 8 | (b[29] as u32) << 16);
                return Some((w, h));
            }
            _ => return None,
        }
    }
    if let Some((w, h, _)) = crate::svg::intrinsic(b) { return Some((w.round() as u32, h.round() as u32)); }
    let head = std::str::from_utf8(&b[..b.len().min(2048)]).ok()?;
    if let Some(i) = head.find("<svg") {
        let tag = &head[i..head[i..].find('>').map(|j| i + j).unwrap_or(head.len())];
        let attr = |n: &str| -> Option<f32> {
            let k = tag.find(&format!(" {n}=\""))? + n.len() + 3;
            tag[k..].split('"').next()?.trim_end_matches("px").parse().ok()
        };
        if let (Some(w), Some(h)) = (attr("width"), attr("height")) { return Some((w as u32, h as u32)); }
        if let Some(k) = tag.find("viewBox=\"") {
            let v: Vec<f32> = tag[k + 9..].split('"').next()?.split([' ', ',']).filter_map(|x| x.parse().ok()).collect();
            if v.len() == 4 { return Some((v[2] as u32, v[3] as u32)); }
        }
        return Some((300, 150));
    }
    None
}

/// Decode any supported format to RGBA, then box-filter down to at most max_w × max_h (display size).
pub fn decode(b: &[u8], max_w: u32, max_h: u32) -> Option<(u32, u32, Vec<u32>)> {
    if crate::svg::intrinsic(b).is_some() && !b.starts_with(b"\x89PNG") { return crate::svg::render_at(b, max_w.clamp(1, 4096), max_h.clamp(1, 4096)); }
    let (w, h, px) = decode_full(b)?;
    if w == 0 || h == 0 { return None; }
    let (ow, oh) = (w.min(max_w.max(1)), h.min(max_h.max(1)));
    if ow == w && oh == h { return Some((w, h, px)); }
    Some((ow, oh, downscale(&px, w, h, ow, oh)))
}

/// Area-average downscale (each output pixel = mean of the source box it covers); alpha-weighted colour.
fn downscale(px: &[u32], w: u32, h: u32, ow: u32, oh: u32) -> Vec<u32> {
    let mut out = vec![0u32; (ow * oh) as usize];
    for y in 0..oh {
        let (y0, y1) = ((y as u64 * h as u64 / oh as u64) as u32, (((y + 1) as u64 * h as u64 + oh as u64 - 1) / oh as u64) as u32);
        for x in 0..ow {
            let (x0, x1) = ((x as u64 * w as u64 / ow as u64) as u32, (((x + 1) as u64 * w as u64 + ow as u64 - 1) / ow as u64) as u32);
            let (mut r, mut g, mut bb, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1.min(h) { for sx in x0..x1.min(w) {
                let c = px[(sy * w + sx) as usize];
                let al = (c & 255) as u64;
                r += ((c >> 24) as u64) * al; g += (((c >> 16) & 255) as u64) * al; bb += (((c >> 8) & 255) as u64) * al; a += al; n += 1;
            } }
            out[(y * ow + x) as usize] = if a == 0 { 0 } else {
                ((r / a) as u32) << 24 | ((g / a) as u32) << 16 | ((bb / a) as u32) << 8 | (a / n.max(1)) as u32
            };
        }
    }
    out
}

fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 { (r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | a as u32 }

/// Decoded-size ceiling: a header may claim any size; we refuse to allocate beyond this (fuzz finding, 3 GB alloc).
const MAX_PIXELS: u64 = 40_000_000;

fn decode_full(b: &[u8]) -> Option<(u32, u32, Vec<u32>)> {
    if let Some((w, h)) = size(b) { if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS || w > 30000 || h > 30000 { return None; } }
    if b.len() >= 8 && &b[0..8] == b"\x89PNG\r\n\x1a\n" {
        let mut dec = png::Decoder::new(std::io::Cursor::new(b));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut r = dec.read_info().ok()?;
        let mut buf = vec![0u8; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let ch = match info.color_type { png::ColorType::Rgba => 4, png::ColorType::Rgb => 3, png::ColorType::GrayscaleAlpha => 2, png::ColorType::Grayscale => 1, _ => return None };
        let px = buf[..(info.width * info.height) as usize * ch].chunks(ch).map(|p| match ch {
            4 => pack(p[0], p[1], p[2], p[3]), 3 => pack(p[0], p[1], p[2], 255), 2 => pack(p[0], p[0], p[0], p[1]), _ => pack(p[0], p[0], p[0], 255) }).collect();
        return Some((info.width, info.height, px));
    }
    if b.len() >= 4 && b[0] == 0xFF && b[1] == 0xD8 {
        let mut d = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(b));
        let pix = d.decode().ok()?;
        let info = d.info()?;
        let (w, h) = (info.width as u32, info.height as u32);
        let ch = pix.len() / (w as usize * h as usize).max(1);
        let px = pix.chunks(ch.max(1)).take((w * h) as usize).map(|p| match ch { 1 => pack(p[0], p[0], p[0], 255), 4 => pack(p[0], p[1], p[2], p[3]), _ => pack(p[0], p[1], p[2], 255) }).collect();
        return Some((w, h, px));
    }
    if b.len() >= 6 && &b[0..3] == b"GIF" {
        let mut o = gif::DecodeOptions::new();
        o.set_color_output(gif::ColorOutput::RGBA);
        let mut d = o.read_info(std::io::Cursor::new(b)).ok()?;
        let (w, h) = (d.width() as u32, d.height() as u32);
        let mut canvas = vec![0u32; (w * h) as usize];
        if let Ok(Some(f)) = d.read_next_frame() { // first frame only: animations stay still (calm, cheap)
            let (fl, ft, fw, fh) = (f.left as u32, f.top as u32, f.width as u32, f.height as u32);
            for y in 0..fh { for x in 0..fw {
                let i = ((y * fw + x) * 4) as usize;
                if i + 3 >= f.buffer.len() || x + fl >= w || y + ft >= h { continue; }
                let p = &f.buffer[i..i + 4];
                canvas[((y + ft) * w + x + fl) as usize] = pack(p[0], p[1], p[2], p[3]);
            } }
        }
        return Some((w, h, canvas));
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(b)).ok()?;
        let (w, h) = d.dimensions();
        let alpha = d.has_alpha();
        let mut buf = vec![0u8; d.output_buffer_size()?];
        d.read_image(&mut buf).ok()?;
        let ch = if alpha { 4 } else { 3 };
        let px = buf.chunks(ch).map(|p| if alpha { pack(p[0], p[1], p[2], p[3]) } else { pack(p[0], p[1], p[2], 255) }).collect();
        return Some((w, h, px));
    }
    crate::svg::render(b)
}
