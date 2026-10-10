// SPDX-License-Identifier: MIT OR Apache-2.0
//! snapshot — a background tab as one signed, packed blob: the packed DOM (nodes, names, text/attr arena) and the
//! display list (rects, glyph runs, images), zlib-compressed and authenticated with keyed SipHash-2-4 (own code,
//! from Aumasson & Bernstein 2012). Tiles are dropped; restoring = verify + inflate + re-raster on demand.
use crate::dom::{Attr, Dom, Kind, Node, Span};
use crate::layout::Item;

fn sip24(key: &[u8; 16], m: &[u8]) -> u64 {
    let k0 = u64::from_le_bytes(key[0..8].try_into().unwrap());
    let k1 = u64::from_le_bytes(key[8..16].try_into().unwrap());
    let (mut v0, mut v1, mut v2, mut v3) = (k0 ^ 0x736f6d6570736575, k1 ^ 0x646f72616e646f6d, k0 ^ 0x6c7967656e657261, k1 ^ 0x7465646279746573);
    macro_rules! round { () => {
        v0 = v0.wrapping_add(v1); v1 = v1.rotate_left(13); v1 ^= v0; v0 = v0.rotate_left(32);
        v2 = v2.wrapping_add(v3); v3 = v3.rotate_left(16); v3 ^= v2;
        v0 = v0.wrapping_add(v3); v3 = v3.rotate_left(21); v3 ^= v0;
        v2 = v2.wrapping_add(v1); v1 = v1.rotate_left(17); v1 ^= v2; v2 = v2.rotate_left(32);
    } }
    let n = m.len();
    let mut chunks = m.chunks_exact(8);
    for c in &mut chunks { let mi = u64::from_le_bytes(c.try_into().unwrap()); v3 ^= mi; round!(); round!(); v0 ^= mi; }
    let mut last = [0u8; 8];
    let rem = chunks.remainder();
    last[..rem.len()].copy_from_slice(rem);
    let b = u64::from_le_bytes(last) | ((n as u64 & 0xff) << 56);
    v3 ^= b; round!(); round!(); v0 ^= b;
    v2 ^= 0xff;
    round!(); round!(); round!(); round!();
    v0 ^ v1 ^ v2 ^ v3
}

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) { self.0.push(v) }
    fn u16(&mut self, v: u16) { self.0.extend_from_slice(&v.to_le_bytes()) }
    fn u32(&mut self, v: u32) { self.0.extend_from_slice(&v.to_le_bytes()) }
    fn i32(&mut self, v: i32) { self.0.extend_from_slice(&v.to_le_bytes()) }
    fn bytes(&mut self, b: &[u8]) { self.u32(b.len() as u32); self.0.extend_from_slice(b) }
}
struct R<'a> { b: &'a [u8], i: usize }
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> { let s = self.b.get(self.i..self.i.checked_add(n)?)?; self.i += n; Some(s) }
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    fn u16(&mut self) -> Option<u16> { Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?)) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn i32(&mut self) -> Option<i32> { Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn bytes(&mut self) -> Option<&'a [u8]> { let n = self.u32()? as usize; self.take(n) }
}

pub fn pack(dom: &Dom, items: &[Item], key: &[u8; 16]) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(dom.nodes.len() * 16 + dom.bytes.len() + items.len() * 24));
    w.0.extend_from_slice(b"OMR1");
    w.u32(dom.nodes.len() as u32);
    for n in &dom.nodes { w.u32(n.parent); w.u32(n.end); w.u32(n.data); w.u16(n.name); w.u8(n.kind as u8); w.u8(n.nattr); }
    w.u32(dom.names.len() as u32);
    for q in &dom.names { w.bytes(q.local.as_bytes()); }
    w.u32(dom.texts.len() as u32);
    for t in &dom.texts { w.u32(t.off); w.u32(t.len); }
    w.u32(dom.attrs.len() as u32);
    for a in &dom.attrs { w.u16(a.name); w.u32(a.val.off); w.u32(a.val.len); }
    w.bytes(&dom.bytes);
    w.u32(items.len() as u32);
    for it in items {
        match it {
            Item::Rect { x, y, w: ww, h, color } => { w.u8(0); w.i32(*x); w.i32(*y); w.i32(*ww); w.i32(*h); w.u32(*color); }
            Item::Text { x, y, px, color, face, glyphs, deco, w: ww } => {
                w.u8(1); w.i32(*x); w.i32(*y); w.i32(*px); w.u32(*color); w.u16(*face); w.u8(*deco); w.i32(*ww);
                w.u32(glyphs.len() as u32); for g in glyphs { w.u32(*g); }
            }
            Item::Image { x, y, w: ww, h, node, mode } => { w.u8(2); w.i32(*x); w.i32(*y); w.i32(*ww); w.i32(*h); w.u32(*node); w.u8(*mode); }
            Item::Box(d) => { w.u8(3); let mut b = Vec::new(); d.write(&mut b); w.bytes(&b); }
        }
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&w.0, 2);
    let mut out = Vec::with_capacity(z.len() + 12);
    out.extend_from_slice(&sip24(key, &z).to_le_bytes());
    out.extend_from_slice(&(w.0.len() as u32).to_le_bytes());
    out.extend_from_slice(&z);
    out
}

/// Verify the MAC, inflate, rebuild. None if tampered or malformed.
pub fn unpack(blob: &[u8], key: &[u8; 16]) -> Option<(Dom, Vec<Item>)> {
    let mac = u64::from_le_bytes(blob.get(0..8)?.try_into().ok()?);
    let raw_len = u32::from_le_bytes(blob.get(8..12)?.try_into().ok()?) as usize;
    let z = blob.get(12..)?;
    if sip24(key, z) != mac { return None; }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(z, raw_len.min(256 << 20)).ok()?;
    let mut r = R { b: &raw, i: 0 };
    if r.take(4)? != b"OMR1" { return None; }
    let nn = r.u32()? as usize;
    let mut nodes = Vec::with_capacity(nn);
    for _ in 0..nn {
        let (parent, end, data, name) = (r.u32()?, r.u32()?, r.u32()?, r.u16()?);
        let kind = match r.u8()? { 0 => Kind::Document, 1 => Kind::Element, 2 => Kind::Text, 3 => Kind::Comment, _ => Kind::Doctype };
        nodes.push(Node { parent, end, data, name, kind, nattr: r.u8()? });
    }
    let nm = r.u32()? as usize;
    let mut names = Vec::with_capacity(nm);
    for _ in 0..nm { let s = std::str::from_utf8(r.bytes()?).ok()?; names.push(html5ever::QualName::new(None, html5ever::ns!(html), html5ever::LocalName::from(s))); }
    let nt = r.u32()? as usize;
    let mut texts = Vec::with_capacity(nt);
    for _ in 0..nt { texts.push(Span { off: r.u32()?, len: r.u32()? }); }
    let na = r.u32()? as usize;
    let mut attrs = Vec::with_capacity(na);
    for _ in 0..na { attrs.push(Attr { name: r.u16()?, val: Span { off: r.u32()?, len: r.u32()? } }); }
    let bytes = r.bytes()?.to_vec();
    let ni = r.u32()? as usize;
    let mut items = Vec::with_capacity(ni);
    for _ in 0..ni {
        items.push(match r.u8()? {
            0 => Item::Rect { x: r.i32()?, y: r.i32()?, w: r.i32()?, h: r.i32()?, color: r.u32()? },
            1 => {
                let (x, y, px, color, face, deco, w) = (r.i32()?, r.i32()?, r.i32()?, r.u32()?, r.u16()?, r.u8()?, r.i32()?);
                let n = r.u32()? as usize;
                let mut g = Vec::with_capacity(n.min(1 << 24));
                for _ in 0..n { g.push(r.u32()?); }
                Item::Text { x, y, px, color, face, glyphs: g, deco, w }
            }
            3 => Item::Box(Box::new(crate::paint::Deco::read(r.bytes()?)?)),
            _ => Item::Image { x: r.i32()?, y: r.i32()?, w: r.i32()?, h: r.i32()?, node: r.u32()?, mode: r.u8()? },
        });
    }
    Some((Dom { nodes, names, texts, attrs, bytes, quirks: false, sheets: Vec::new(), scripts: Vec::new() }, items))
}
