// SPDX-License-Identifier: Apache-2.0
//! UAX #14 — Unicode Line Breaking Algorithm (Unicode 18.0.0).
//!
//! Spec: https://www.unicode.org/reports/tr14/ — implemented from §6.1 (non-tailorable rules LB1–LB31 as
//! given in the default tailoring §6.2), §5.1 (line break classes, Table 1), and the classification of
//! unresolved classes in LB1. Property data lives in `uax14_tables.rs`, generated from the UCD by
//! `tools/gen_uax14.py`. Rules are evaluated in order; the first rule that decides a break position wins.
//!
//! Positions are "gaps": gap `k` sits between the k-th and (k+1)-th character (k = 0 is sot, k = len is eot).
//! LB9 folds combining marks into their base, so breaks are only ever reported at unit boundaries.

use crate::uax14_tables::RUNS;

/// Line_Break property values (UAX #14 Table 1), in the same index order as the generated table.
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    AI, AK, AL, AP, AS, B2, BA, BB, BK, CB, CJ, CL, CM, CP, CR, EB, EM, EX, GL, H2, H3, HH, HL, HY,
    ID, IN, IS, JL, JT, JV, LF, NL, NS, NU, OP, PO, PR, QU, RI, SA, SG, SP, SY, VF, VI, WJ, XX, ZW, ZWJ,
}

const ALL: &[Class] = &[
    Class::AI, Class::AK, Class::AL, Class::AP, Class::AS, Class::B2, Class::BA, Class::BB, Class::BK,
    Class::CB, Class::CJ, Class::CL, Class::CM, Class::CP, Class::CR, Class::EB, Class::EM, Class::EX,
    Class::GL, Class::H2, Class::H3, Class::HH, Class::HL, Class::HY, Class::ID, Class::IN, Class::IS,
    Class::JL, Class::JT, Class::JV, Class::LF, Class::NL, Class::NS, Class::NU, Class::OP, Class::PO,
    Class::PR, Class::QU, Class::RI, Class::SA, Class::SG, Class::SP, Class::SY, Class::VF, Class::VI,
    Class::WJ, Class::XX, Class::ZW, Class::ZWJ,
];

// General_Category codes packed in the generated table (see tools/gen_uax14.py).
const GC_MN: u8 = 1;
const GC_MC: u8 = 2;
const GC_PI: u8 = 3;
const GC_PF: u8 = 4;
const GC_CN: u8 = 5;

/// A line break opportunity. `index` is a character index (not a byte offset) in `breaks_chars`,
/// and a byte offset in `break_opportunities`. `mandatory` marks BK/CR/LF/NL and end-of-text breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    pub index: usize,
    pub mandatory: bool,
}

/// Line_Break class, East_Asian_Width {F,W,H}, Extended_Pictographic, general category of one code point.
#[derive(Clone, Copy, Debug)]
pub struct Props {
    pub class: Class,
    pub fhw: bool,
    pub ext_pict: bool,
    pub gc: u8,
}

/// Look up the properties of a code point (UAX #14 §5.1 / LB1 inputs) before LB1 resolution.
pub fn props(cp: char) -> Props {
    let cp = cp as u32;
    let i = RUNS.partition_point(|&(start, _)| start <= cp) - 1;
    let p = RUNS[i].1;
    Props {
        class: ALL[(p & 0x3F) as usize],
        fhw: (p >> 8) & 1 == 1,
        ext_pict: (p >> 9) & 1 == 1,
        gc: ((p >> 10) & 0x7) as u8,
    }
}

/// LB1: resolve AI, SG, XX → AL; SA → CM (Mn/Mc) or AL; CJ → NS. Other classes are unchanged.
fn resolve(p: Props) -> Class {
    match p.class {
        Class::AI | Class::SG | Class::XX => Class::AL,
        Class::SA => {
            if p.gc == GC_MN || p.gc == GC_MC {
                Class::CM
            } else {
                Class::AL
            }
        }
        Class::CJ => Class::NS,
        c => c,
    }
}

/// One unit after LB9/LB10: a base character plus any CM/ZWJ it absorbs.
#[derive(Clone, Copy, Debug)]
struct Unit {
    class: Class,
    cp: u32,
    fhw: bool,
    ext_pict: bool,
    gc: u8,
    /// The last character absorbed into this unit is a ZWJ (LB8a).
    zwj_tail: bool,
    /// Character index of the first character of the unit.
    start: usize,
}

/// LB9 and LB10: fold each CM/ZWJ into the preceding unit unless that unit is BK, CR, LF, NL, SP or ZW.
fn units(text: &[char]) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::with_capacity(text.len());
    for (i, &ch) in text.iter().enumerate() {
        let p = props(ch);
        let class = resolve(p);
        let zwj = class == Class::ZWJ;
        if matches!(class, Class::CM | Class::ZWJ) {
            if let Some(last) = out.last_mut() {
                if !matches!(last.class, Class::BK | Class::CR | Class::LF | Class::NL | Class::SP | Class::ZW) {
                    last.zwj_tail = zwj;
                    continue;
                }
            }
            // LB10: a remaining CM/ZWJ behaves as an uppercase Latin A.
            out.push(Unit { class: Class::AL, cp: ch as u32, fhw: false, ext_pict: false, gc: 0, zwj_tail: zwj, start: i });
        } else {
            out.push(Unit { class, cp: ch as u32, fhw: p.fhw, ext_pict: p.ext_pict, gc: p.gc, zwj_tail: false, start: i });
        }
    }
    out
}

const YES: (bool, bool) = (true, false);
const MUST: (bool, bool) = (true, true);
const NO: (bool, bool) = (false, false);

/// Decide the gap `g` between units `u[g-1]` and `u[g]`. Returns (is_break, is_mandatory).
fn decide(u: &[Unit], g: usize) -> (bool, bool) {
    let n = u.len();
    // LB2: never break at the start of text.
    if g == 0 {
        return NO;
    }
    // LB3: always break at the end of text.
    if g == n {
        return MUST;
    }
    let p = u[g - 1].class;
    let nx = u[g].class;

    // Walk back over SP* from gap g; returns the index just after the last non-SP unit before g.
    let back_sp = |mut j: usize| {
        while j > 0 && u[j - 1].class == Class::SP {
            j -= 1;
        }
        j
    };
    // NU (SY | IS)* ending at unit index e (exclusive).
    let nu_seq_ends = |e: usize| {
        let mut j = e;
        while j > 0 && matches!(u[j - 1].class, Class::SY | Class::IS) {
            j -= 1;
        }
        j > 0 && u[j - 1].class == Class::NU
    };
    // Korean syllable block members, treated as ID (LB27).
    let idlike = |k: Class| matches!(k, Class::ID | Class::EB | Class::EM | Class::JL | Class::JV | Class::JT | Class::H2 | Class::H3);
    // AK | dotted circle | AS (LB28a).
    let ak = |k: usize| matches!(u[k].class, Class::AK | Class::AS) || u[k].cp == 0x25CC;

    // LB4–LB6: mandatory breaks and no break before them.
    if p == Class::BK {
        return MUST;
    }
    if p == Class::CR && nx == Class::LF {
        return NO;
    }
    if matches!(p, Class::CR | Class::LF | Class::NL) {
        return MUST;
    }
    if matches!(nx, Class::BK | Class::CR | Class::LF | Class::NL) {
        return NO;
    }
    // LB7: no break before SP or ZW.
    if matches!(nx, Class::SP | Class::ZW) {
        return NO;
    }
    // LB8: break after ZW SP*.
    let j = back_sp(g);
    if j > 0 && u[j - 1].class == Class::ZW {
        return YES;
    }
    // LB8a: no break after ZWJ.
    if u[g - 1].zwj_tail {
        return NO;
    }
    // LB11: word joiner.
    if nx == Class::WJ || p == Class::WJ {
        return NO;
    }
    // LB12: no break after GL.
    if p == Class::GL {
        return NO;
    }
    // LB12a: no break before GL unless after SP, HY or HH.
    if nx == Class::GL && !matches!(p, Class::SP | Class::HY | Class::HH) {
        return NO;
    }
    // LB13: no break before CL, CP, EX, SY.
    if matches!(nx, Class::CL | Class::CP | Class::EX | Class::SY) {
        return NO;
    }
    // LB14: no break after OP SP*.
    let j = back_sp(g);
    if j > 0 && u[j - 1].class == Class::OP {
        return NO;
    }
    // LB15a: no break after an unresolved initial quote (Pi) at the start, after SP, OP, QU, GL, ZW, BK, CR, LF, NL.
    if j > 0 {
        let q = j - 1;
        if u[q].class == Class::QU
            && u[q].gc == GC_PI
            && (q == 0 || matches!(u[q - 1].class, Class::BK | Class::CR | Class::LF | Class::NL | Class::OP | Class::QU | Class::GL | Class::SP | Class::ZW))
        {
            return NO;
        }
    }
    // LB15b: no break before an unresolved final quote (Pf) at the end of the line.
    if nx == Class::QU
        && u[g].gc == GC_PF
        && (g + 1 == n || matches!(u[g + 1].class, Class::SP | Class::GL | Class::WJ | Class::CL | Class::QU | Class::CP | Class::EX | Class::IS | Class::SY | Class::BK | Class::CR | Class::LF | Class::NL | Class::ZW))
    {
        return NO;
    }
    // LB15c: break before a decimal mark after SP, as in "subtract .5".
    if p == Class::SP && nx == Class::IS && g + 1 < n && u[g + 1].class == Class::NU {
        return YES;
    }
    // LB15d: no break before IS.
    if nx == Class::IS {
        return NO;
    }
    // LB16: no break between closing punctuation (and SP*) and NS.
    if j > 0 && matches!(u[j - 1].class, Class::CL | Class::CP) && nx == Class::NS {
        return NO;
    }
    // LB17: no break within B2 SP* B2.
    if j > 0 && u[j - 1].class == Class::B2 && nx == Class::B2 {
        return NO;
    }
    // LB18: break after SP.
    if p == Class::SP {
        return YES;
    }
    // LB19: no break around unresolved quotes (QU that are not Pi before, not Pf after).
    if nx == Class::QU && u[g].gc != GC_PI {
        return NO;
    }
    if p == Class::QU && u[g - 1].gc != GC_PF {
        return NO;
    }
    // LB19a: unless surrounded by East Asian characters, no break either side of an unresolved quote.
    if nx == Class::QU {
        if !u[g - 1].fhw {
            return NO;
        }
        if g + 1 == n || !u[g + 1].fhw {
            return NO;
        }
    }
    if p == Class::QU {
        if g < n && !u[g].fhw {
            return NO;
        }
        if g - 1 == 0 || !u[g - 2].fhw {
            return NO;
        }
    }
    // LB20: break before and after CB.
    if nx == Class::CB || p == Class::CB {
        return YES;
    }
    // LB20a: no break after a word-initial hyphen.
    if matches!(p, Class::HY | Class::HH)
        && matches!(nx, Class::AL | Class::HL)
        && (g - 1 == 0 || matches!(u[g - 2].class, Class::BK | Class::CR | Class::LF | Class::NL | Class::SP | Class::ZW | Class::CB | Class::GL))
    {
        return NO;
    }
    // LB21: no break before BA, HH, HY, NS; none after BB.
    if matches!(nx, Class::BA | Class::HH | Class::HY | Class::NS) || p == Class::BB {
        return NO;
    }
    // LB21a: no break after the hyphen in Hebrew + hyphen + non-Hebrew.
    if matches!(p, Class::HY | Class::HH) && g >= 2 && u[g - 2].class == Class::HL && nx != Class::HL {
        return NO;
    }
    // LB21b: no break between SY and Hebrew letters.
    if p == Class::SY && nx == Class::HL {
        return NO;
    }
    // LB22: no break before an ellipsis.
    if nx == Class::IN {
        return NO;
    }
    // LB23: no break between letters and digits.
    if (matches!(p, Class::AL | Class::HL) && nx == Class::NU) || (p == Class::NU && matches!(nx, Class::AL | Class::HL)) {
        return NO;
    }
    // LB23a: no break between numeric prefix and ideograph / ideograph and numeric postfix.
    if p == Class::PR && idlike(nx) {
        return NO;
    }
    if idlike(p) && nx == Class::PO {
        return NO;
    }
    // LB24: no break between numeric prefix/postfix and letters.
    if (matches!(p, Class::PR | Class::PO) && matches!(nx, Class::AL | Class::HL))
        || (matches!(p, Class::AL | Class::HL) && matches!(nx, Class::PR | Class::PO))
    {
        return NO;
    }
    // LB25: numbers. NU (SY | IS)* with CL/CP, PO, PR, OP, HY, IS, NU.
    if matches!(p, Class::CL | Class::CP) && matches!(nx, Class::PO | Class::PR) && nu_seq_ends(g - 1) {
        return NO;
    }
    if matches!(nx, Class::PO | Class::PR) && nu_seq_ends(g) {
        return NO;
    }
    if nx == Class::NU && nu_seq_ends(g) {
        return NO;
    }
    if p == Class::PO && nx == Class::OP && g + 1 < n && u[g + 1].class == Class::NU {
        return NO;
    }
    if p == Class::PO && nx == Class::OP && g + 2 < n && u[g + 1].class == Class::IS && u[g + 2].class == Class::NU {
        return NO;
    }
    if p == Class::PO && nx == Class::NU {
        return NO;
    }
    if p == Class::PR && nx == Class::OP && g + 1 < n && u[g + 1].class == Class::NU {
        return NO;
    }
    if p == Class::PR && nx == Class::OP && g + 2 < n && u[g + 1].class == Class::IS && u[g + 2].class == Class::NU {
        return NO;
    }
    if p == Class::PR && nx == Class::NU {
        return NO;
    }
    if matches!(p, Class::HY | Class::IS) && nx == Class::NU {
        return NO;
    }
    // LB26: Korean syllable blocks (Jamo and Hangul syllables).
    if p == Class::JL && matches!(nx, Class::JL | Class::JV | Class::H2 | Class::H3) {
        return NO;
    }
    if matches!(p, Class::JV | Class::H2) && matches!(nx, Class::JV | Class::JT) {
        return NO;
    }
    if matches!(p, Class::JT | Class::H3) && nx == Class::JT {
        return NO;
    }
    // LB27: Korean syllable block behaves as ID with PO and PR.
    if matches!(p, Class::JL | Class::JV | Class::JT | Class::H2 | Class::H3) && nx == Class::PO {
        return NO;
    }
    if p == Class::PR && matches!(nx, Class::JL | Class::JV | Class::JT | Class::H2 | Class::H3) {
        return NO;
    }
    // LB28: no break between alphabetics.
    if matches!(p, Class::AL | Class::HL) && matches!(nx, Class::AL | Class::HL) {
        return NO;
    }
    // LB28a: Brahmic orthographic syllables.
    if p == Class::AP && g < n && ak(g) {
        return NO;
    }
    if g >= 1 && ak(g - 1) && matches!(nx, Class::VF | Class::VI) {
        return NO;
    }
    if g >= 2 && p == Class::VI && ak(g - 2) && (nx == Class::AK || u[g].cp == 0x25CC) {
        return NO;
    }
    if g >= 1 && ak(g - 1) && g < n && ak(g) && g + 1 < n && u[g + 1].class == Class::VF {
        return NO;
    }
    // LB29: no break between numeric punctuation and alphabetics.
    if p == Class::IS && matches!(nx, Class::AL | Class::HL) {
        return NO;
    }
    // LB30: no break between letters/numbers and opening parentheses, or closing parentheses and letters/numbers.
    if matches!(p, Class::AL | Class::HL | Class::NU) && nx == Class::OP && !u[g].fhw {
        return NO;
    }
    if p == Class::CP && !u[g - 1].fhw && matches!(nx, Class::AL | Class::HL | Class::NU) {
        return NO;
    }
    // LB30a: break between regional indicator pairs only; no break inside an unpaired RI run.
    if p == Class::RI && nx == Class::RI {
        let mut k = 0;
        let mut j = g;
        while j > 0 && u[j - 1].class == Class::RI {
            k += 1;
            j -= 1;
        }
        if k % 2 == 1 {
            return NO;
        }
    }
    // LB30b: no break between an emoji base and an emoji modifier.
    if p == Class::EB && nx == Class::EM {
        return NO;
    }
    if u[g - 1].ext_pict && u[g - 1].gc == GC_CN && nx == Class::EM {
        return NO;
    }
    // LB31: break everywhere else.
    YES
}

/// Line break opportunities for a character slice, as character indices (gap positions).
/// Breaks are never reported at index 0 (LB2); the end of text (index `text.len()`) is always reported.
pub fn breaks_chars(text: &[char]) -> Vec<Break> {
    let u = units(text);
    let mut out = Vec::new();
    for g in 0..=u.len() {
        let (is_break, mandatory) = decide(&u, g);
        if is_break {
            let index = if g == u.len() { text.len() } else { u[g].start };
            out.push(Break { index, mandatory });
        }
    }
    out
}

/// Line break opportunities for a string, as byte offsets into `s`.
pub fn break_opportunities(s: &str) -> Vec<Break> {
    let chars: Vec<char> = s.chars().collect();
    let offsets: Vec<usize> = s.char_indices().map(|(b, _)| b).chain(std::iter::once(s.len())).collect();
    breaks_chars(&chars)
        .into_iter()
        .map(|b| Break { index: offsets[b.index], mandatory: b.mandatory })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uax14_tables::CLASS_NAMES;

    #[test]
    fn class_order_matches_generated_table() {
        assert_eq!(ALL.len(), CLASS_NAMES.len());
        for (c, name) in ALL.iter().zip(CLASS_NAMES) {
            assert_eq!(format!("{c:?}"), *name);
        }
    }

    #[test]
    fn simple_latin_and_space() {
        let b = break_opportunities("ab cd");
        assert_eq!(b, vec![Break { index: 3, mandatory: false }, Break { index: 5, mandatory: true }]);
    }
}
