// SPDX-License-Identifier: Apache-2.0
//! OpenType shaping subset: horizontal pair kerning and standard ligatures.
//!
//! Spec: Microsoft OpenType specification — "OpenType Font File" (chapter 2: table directory),
//! GPOS lookup type 2 "Pair Adjustment Positioning" (formats 1 and 2), GSUB lookup type 4 "Ligature Substitution",
//! extension lookups (types 9 / 7), Common Table Formats (Coverage, ClassDef, FeatureList, LookupList), and the
//! legacy `kern` table (format 0). Written from the specification text only.
//!
//! Scope (see docs/otshape.md for known gaps): features are selected for the `latn` script (else `DFLT`) default
//! language system; lookupFlag skipping and GDEF classes are not applied. Pair lookups are
//! consulted by (left, right) glyph id, so no sequence scanning is needed for kerning.

/// Parse and lookup errors. All reads are bounds-checked; malformed data never panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    BadFormat(&'static str),
    Unsupported(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;

// ---- big-endian readers --------------------------------------------------------------------------------

fn u16_at(d: &[u8], off: usize) -> Result<u16> {
    let b = d.get(off..off + 2).ok_or(Error::Truncated)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn i16_at(d: &[u8], off: usize) -> Result<i16> {
    Ok(u16_at(d, off)? as i16)
}

fn u32_at(d: &[u8], off: usize) -> Result<u32> {
    let b = d.get(off..off + 4).ok_or(Error::Truncated)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn slice(d: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    d.get(off..off.checked_add(len).ok_or(Error::Truncated)?).ok_or(Error::Truncated)
}

// ---- font directory ------------------------------------------------------------------------------------

/// A parsed sfnt (non-collection) font with the tables this module reads.
#[derive(Debug, Clone, Copy)]
pub struct Font<'a> {
    gpos: Option<&'a [u8]>,
    gsub: Option<&'a [u8]>,
    kern: Option<&'a [u8]>,
}

/// Parse the table directory (OpenType "Table Directory", chapter 2). TrueType and CFF flavours are accepted;
/// TrueType collections are rejected.
pub fn parse(data: &[u8]) -> Result<Font<'_>> {
    let version = u32_at(data, 0)?;
    if version == 0x7474_6366 {
        return Err(Error::Unsupported("ttcf collections"));
    }
    if !matches!(version, 0x0001_0000 | 0x4F54_544F | 0x7472_7565) {
        return Err(Error::BadFormat("sfnt version"));
    }
    let num = u16_at(data, 4)? as usize;
    let mut font = Font { gpos: None, gsub: None, kern: None };
    for i in 0..num {
        let rec = 12 + 16 * i;
        let tag = slice(data, rec, 4)?;
        let off = u32_at(data, rec + 8)? as usize;
        let len = u32_at(data, rec + 12)? as usize;
        let table = slice(data, off, len)?;
        match tag {
            b"GPOS" => font.gpos = Some(table),
            b"GSUB" => font.gsub = Some(table),
            b"kern" => font.kern = Some(table),
            _ => {}
        }
    }
    Ok(font)
}

// ---- Common Table Formats ------------------------------------------------------------------------------

/// Coverage table (OpenType "Coverage Table"): index of `gid` within the covered glyphs, or None.
fn coverage_index(d: &[u8], off: usize, gid: u16) -> Result<Option<u16>> {
    let format = u16_at(d, off)?;
    let count = u16_at(d, off + 2)? as usize;
    match format {
        1 => {
            // glyphArray is sorted by glyph id: binary search.
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let g = u16_at(d, off + 4 + 2 * mid)?;
                if g == gid {
                    return Ok(Some(mid as u16));
                } else if g < gid {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            Ok(None)
        }
        2 => {
            // RangeRecord: startGlyphID, endGlyphID, startCoverageIndex; sorted and non-overlapping.
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let rec = off + 4 + 6 * mid;
                let start = u16_at(d, rec)?;
                let end = u16_at(d, rec + 2)?;
                if gid < start {
                    hi = mid;
                } else if gid > end {
                    lo = mid + 1;
                } else {
                    return Ok(Some(u16_at(d, rec + 4)?.wrapping_add(gid - start)));
                }
            }
            Ok(None)
        }
        _ => Err(Error::BadFormat("coverage format")),
    }
}

/// ClassDef table (OpenType "Class Definition Table"): class of `gid`; glyphs not listed are class 0.
/// An offset of 0 means "no class definition", which also yields class 0.
fn class_of(d: &[u8], off: usize, gid: u16) -> Result<u16> {
    if off == 0 {
        return Ok(0);
    }
    let format = u16_at(d, off)?;
    match format {
        1 => {
            let start = u16_at(d, off + 2)?;
            let count = u16_at(d, off + 4)? as usize;
            if gid < start || (gid - start) as usize >= count {
                return Ok(0);
            }
            u16_at(d, off + 6 + 2 * (gid - start) as usize)
        }
        2 => {
            let count = u16_at(d, off + 2)? as usize;
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let rec = off + 4 + 6 * mid;
                let start = u16_at(d, rec)?;
                let end = u16_at(d, rec + 2)?;
                if gid < start {
                    hi = mid;
                } else if gid > end {
                    lo = mid + 1;
                } else {
                    return u16_at(d, rec + 4);
                }
            }
            Ok(0)
        }
        _ => Err(Error::BadFormat("classdef format")),
    }
}

/// Size in bytes of a ValueRecord with the given valueFormat (one 2-byte field per set bit).
fn value_record_size(format: u16) -> usize {
    2 * format.count_ones() as usize
}

/// XAdvance from a ValueRecord at `off` (0 if the format does not include XAdvance, bit 0x0004).
/// Device-table offsets are present in the record but are not applied (see docs/otshape.md).
fn x_advance(d: &[u8], off: usize, format: u16) -> Result<i16> {
    if format & 0x0004 == 0 {
        return Ok(0);
    }
    // Fields before XAdvance: XPlacement (0x1), YPlacement (0x2).
    i16_at(d, off + 2 * (format & 0x0003).count_ones() as usize)
}

// ---- layout tables (GPOS / GSUB share the header and lookup list) --------------------------------------

/// Lookup indices of the `tag` feature for the Latin script (`latn`, else `DFLT`), default language system.
/// Follows ScriptList -> Script -> default LangSys -> FeatureIndex -> Feature -> lookupListIndex (OpenType
/// "Script List Table", "Language System Table"). If the font has no ScriptList, every `tag` feature is used.
fn feature_lookups(layout: &[u8], tag: &[u8; 4]) -> Result<Vec<u16>> {
    let feature_list = u16_at(layout, 6)? as usize;
    if feature_list == 0 {
        return Ok(Vec::new());
    }
    let base = feature_list;
    let count = u16_at(layout, base)? as usize;
    let script_list = u16_at(layout, 4)? as usize;
    let wanted: Option<Vec<u16>> = if script_list == 0 {
        None
    } else {
        script_feature_indices(layout, script_list)?
    };
    let mut out = Vec::new();
    for i in 0..count {
        if let Some(w) = &wanted {
            if !w.contains(&(i as u16)) {
                continue;
            }
        }
        let rec = base + 2 + 6 * i;
        if slice(layout, rec, 4)? != tag {
            continue;
        }
        let feature = base + u16_at(layout, rec + 4)? as usize;
        let n = u16_at(layout, feature + 2)? as usize;
        for k in 0..n {
            out.push(u16_at(layout, feature + 4 + 2 * k)?);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// FeatureList indices enabled by the default LangSys of `latn` (or `DFLT`). None if neither script exists.
fn script_feature_indices(layout: &[u8], script_list: usize) -> Result<Option<Vec<u16>>> {
    let n = u16_at(layout, script_list)? as usize;
    let mut found = None;
    for want in [b"latn", b"DFLT"] {
        for i in 0..n {
            let rec = script_list + 2 + 6 * i;
            if slice(layout, rec, 4)? == want {
                found = Some(script_list + u16_at(layout, rec + 4)? as usize);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let Some(script) = found else {
        return Ok(None);
    };
    let default_lang = u16_at(layout, script)? as usize;
    if default_lang == 0 {
        return Ok(Some(Vec::new()));
    }
    let lang = script + default_lang;
    let required = u16_at(layout, lang + 2)?;
    let count = u16_at(layout, lang + 4)? as usize;
    let mut idx = Vec::with_capacity(count + 1);
    if required != 0xFFFF {
        idx.push(required);
    }
    for k in 0..count {
        idx.push(u16_at(layout, lang + 6 + 2 * k)?);
    }
    Ok(Some(idx))
}

/// Offset of lookup `idx` from the start of the layout table (LookupList, OpenType "Lookup List Table").
fn lookup_offset(layout: &[u8], idx: u16) -> Result<usize> {
    let list = u16_at(layout, 8)? as usize;
    let count = u16_at(layout, list)?;
    if idx >= count {
        return Err(Error::BadFormat("lookup index out of range"));
    }
    Ok(list + u16_at(layout, list + 2 + 2 * idx as usize)? as usize)
}

/// Subtable offsets of a lookup with extension lookups (type 9 in GPOS, type 7 in GSUB) resolved to the real
/// subtable and its real lookup type. `ext_type` is 9 for GPOS, 7 for GSUB.
fn subtables(layout: &[u8], lookup: usize, ext_type: u16) -> Result<(u16, Vec<usize>)> {
    let kind = u16_at(layout, lookup)?;
    let n = u16_at(layout, lookup + 4)? as usize;
    let mut out = Vec::with_capacity(n);
    let mut real_kind = kind;
    for i in 0..n {
        let mut sub = lookup + u16_at(layout, lookup + 6 + 2 * i)? as usize;
        if kind == ext_type {
            // ExtensionPosFormat1 / ExtensionSubstFormat1: format, extensionLookupType, extensionOffset (32-bit).
            if u16_at(layout, sub)? != 1 {
                return Err(Error::BadFormat("extension format"));
            }
            real_kind = u16_at(layout, sub + 2)?;
            sub += u32_at(layout, sub + 4)? as usize;
        }
        out.push(sub);
    }
    Ok((real_kind, out))
}

// ---- GPOS pair adjustment (lookup type 2) --------------------------------------------------------------

/// XAdvance of the first glyph from one PairPos subtable at `sub`; None if the subtable does not apply.
fn pair_pos_subtable(gpos: &[u8], sub: usize, left: u16, right: u16) -> Result<Option<i16>> {
    let format = u16_at(gpos, sub)?;
    let cov = sub + u16_at(gpos, sub + 2)? as usize;
    let Some(cov_idx) = coverage_index(gpos, cov, left)? else {
        return Ok(None);
    };
    let vf1 = u16_at(gpos, sub + 4)?;
    let vf2 = u16_at(gpos, sub + 6)?;
    let rec_size = 2 + value_record_size(vf1) + value_record_size(vf2);
    match format {
        // PairPosFormat1: pairSet per covered glyph; PairValueRecords sorted by secondGlyph.
        1 => {
            let idx = cov_idx as usize;
            let set = sub + u16_at(gpos, sub + 10 + 2 * idx)? as usize;
            let count = u16_at(gpos, set)? as usize;
            let (mut lo, mut hi) = (0usize, count);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let rec = set + 2 + mid * rec_size;
                let second = u16_at(gpos, rec)?;
                if second == right {
                    return Ok(Some(x_advance(gpos, rec + 2, vf1)?));
                } else if second < right {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            Ok(None)
        }
        // PairPosFormat2: class-pair matrix.
        2 => {
            let cd1 = u16_at(gpos, sub + 8)? as usize;
            let cd2 = u16_at(gpos, sub + 10)? as usize;
            let class1_count = u16_at(gpos, sub + 12)? as usize;
            let class2_count = u16_at(gpos, sub + 14)? as usize;
            let c1 = class_of(gpos, if cd1 == 0 { 0 } else { sub + cd1 }, left)? as usize;
            let c2 = class_of(gpos, if cd2 == 0 { 0 } else { sub + cd2 }, right)? as usize;
            if c1 >= class1_count || c2 >= class2_count {
                return Err(Error::BadFormat("pair class out of range"));
            }
            let cell = sub + 16 + (c1 * class2_count + c2) * (value_record_size(vf1) + value_record_size(vf2));
            Ok(Some(x_advance(gpos, cell, vf1)?))
        }
        _ => Err(Error::BadFormat("pair pos format")),
    }
}

// ---- legacy kern table ---------------------------------------------------------------------------------

/// Horizontal kern pair value from the Microsoft `kern` table (format 0 subtables), or None.
fn kern_table_pair(kern: &[u8], left: u16, right: u16) -> Result<Option<i16>> {
    if u16_at(kern, 0)? != 0 {
        return Err(Error::Unsupported("kern table version"));
    }
    let n_tables = u16_at(kern, 2)? as usize;
    let mut off = 4usize;
    for _ in 0..n_tables {
        // Subtable header: version, length, coverage (bit 0 horizontal; high byte = format).
        let length = u16_at(kern, off + 2)? as usize;
        let coverage = u16_at(kern, off + 4)?;
        let format = coverage >> 8;
        let horizontal = coverage & 1 == 1;
        if format == 0 && horizontal {
            let npairs = u16_at(kern, off + 6)? as usize;
            let key = ((left as u32) << 16) | right as u32;
            let (mut lo, mut hi) = (0usize, npairs);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let rec = off + 14 + 6 * mid;
                let k = ((u16_at(kern, rec)? as u32) << 16) | u16_at(kern, rec + 2)? as u32;
                if k == key {
                    return Ok(Some(i16_at(kern, rec + 4)?));
                } else if k < key {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            return Ok(None);
        }
        off += length;
    }
    Ok(None)
}

impl<'a> Font<'a> {
    /// Horizontal kerning of (left, right) glyph ids, in font units (XAdvance of the left glyph).
    ///
    /// If the font has GPOS `kern` lookups they are the only source (as in the OpenType layout model);
    /// otherwise the legacy `kern` table is used. Returns 0 when no pair value applies.
    pub fn pair_advance(&self, left: u16, right: u16) -> Result<i16> {
        if let Some(gpos) = self.gpos {
            let lookups = feature_lookups(gpos, b"kern")?;
            if !lookups.is_empty() {
                for idx in lookups {
                    let (kind, subs) = subtables(gpos, lookup_offset(gpos, idx)?, 9)?;
                    if kind != 2 {
                        continue;
                    }
                    for sub in subs {
                        if let Some(v) = pair_pos_subtable(gpos, sub, left, right)? {
                            return Ok(v);
                        }
                    }
                }
                return Ok(0);
            }
        }
        if let Some(kern) = self.kern {
            return Ok(kern_table_pair(kern, left, right)?.unwrap_or(0));
        }
        Ok(0)
    }

    /// Apply the `liga` feature (GSUB ligature substitution, lookup type 4) to a glyph sequence.
    ///
    /// Lookups run in ascending index order. At each position the first ligature (in set order) whose
    /// components match the following glyphs is taken; the output glyph is not re-scanned by the same lookup.
    pub fn ligate(&self, glyphs: &[u16]) -> Result<Vec<u16>> {
        let Some(gsub) = self.gsub else {
            return Ok(glyphs.to_vec());
        };
        let mut seq = glyphs.to_vec();
        for idx in feature_lookups(gsub, b"liga")? {
            let (kind, subs) = subtables(gsub, lookup_offset(gsub, idx)?, 7)?;
            if kind != 4 {
                continue;
            }
            for sub in subs {
                seq = ligature_subtable(gsub, sub, &seq)?;
            }
        }
        Ok(seq)
    }
}

/// One LigatureSubstFormat1 subtable applied to a whole sequence (OpenType "Ligature Substitution Subtable").
fn ligature_subtable(gsub: &[u8], sub: usize, seq: &[u16]) -> Result<Vec<u16>> {
    if u16_at(gsub, sub)? != 1 {
        return Err(Error::BadFormat("ligature subst format"));
    }
    let cov = sub + u16_at(gsub, sub + 2)? as usize;
    let set_count = u16_at(gsub, sub + 4)? as usize;
    let mut out = Vec::with_capacity(seq.len());
    let mut i = 0usize;
    while i < seq.len() {
        let mut taken = None;
        if let Some(k) = coverage_index(gsub, cov, seq[i])? {
            if (k as usize) < set_count {
                let set = sub + u16_at(gsub, sub + 6 + 2 * k as usize)? as usize;
                let lig_count = u16_at(gsub, set)? as usize;
                for j in 0..lig_count {
                    let lig = set + u16_at(gsub, set + 2 + 2 * j)? as usize;
                    let lig_glyph = u16_at(gsub, lig)?;
                    let comp_count = u16_at(gsub, lig + 2)? as usize;
                    if comp_count == 0 {
                        return Err(Error::BadFormat("ligature with zero components"));
                    }
                    if i + comp_count > seq.len() {
                        continue;
                    }
                    let matches = (1..comp_count).all(|c| u16_at(gsub, lig + 4 + 2 * (c - 1)).map(|g| g == seq[i + c]).unwrap_or(false));
                    if matches {
                        taken = Some((lig_glyph, comp_count));
                        break;
                    }
                }
            }
        }
        match taken {
            Some((glyph, n)) => {
                out.push(glyph);
                i += n;
            }
            None => {
                out.push(seq[i]);
                i += 1;
            }
        }
    }
    Ok(out)
}
