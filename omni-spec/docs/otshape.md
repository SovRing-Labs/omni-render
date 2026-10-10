# otshape — OpenType shaping subset (kerning + ligatures)

- **Spec:** Microsoft OpenType specification: "OpenType Font File" (ch. 2, table directory); GPOS lookup type 2 Pair Adjustment (formats 1, 2); GSUB lookup type 4 Ligature Substitution; extension lookups (GPOS 9 / GSUB 7); Common Table Formats (Coverage, Class Definition, Script/Feature/LookupList); legacy `kern` table format 0. Pages: https://learn.microsoft.com/en-us/typography/opentype/spec/chapter2 , …/gpos , …/gsub , …/kern
- **Code:** `src/otshape.rs` (`parse`, `Font::pair_advance`, `Font::ligate`). Bounds-checked reads; malformed input returns `Err`, never panics.
- **Conformance:**
  - `tests/otshape.rs` synthetic fonts built from the spec layouts: GSUB ligature (format 1), GPOS class pair kerning (format 2), legacy kern (format 0), malformed input — all pass.
  - HarfBuzz oracle (`tools/oracle_otshape.py` → `tests/data/otshape-oracle.tsv`, black-box process via uharfbuzz): 275 kern pairs and 20 ligature strings on DejaVuSerif.ttf and LiberationSerif-Regular.ttf (read in place from /usr/share/fonts) — all match.
- **Oracle note:** HarfBuzz can split a pair value between the first glyph's x_advance and the second glyph's x_offset (seen on Liberation "11": advance −38, offset −38; total −76). The oracle therefore compares the pen position of the following glyph, not raw x_advance.

## API
- `parse(&[u8]) -> Result<Font>` — sfnt (0x00010000 / OTTO / true); `ttcf` rejected.
- `Font::pair_advance(left_gid, right_gid) -> Result<i16>` — XAdvance of the left glyph; 0 if no pair applies. If the font has GPOS `kern` lookups they are used exclusively; otherwise the legacy `kern` table.
- `Font::ligate(&[gid]) -> Result<Vec<u16>>` — applies `liga` lookups in index order; first matching ligature in set order wins; a ligature output is not re-scanned by the same lookup.

## Known gaps (not in scope of this module)
- Features are selected through the `latn` script (else `DFLT`) default LangSys only. Other scripts/languages and `DFLT`-only fonts with `kern` under other scripts are not selected per run.
- lookupFlag skipping (IgnoreMarks/IgnoreLigatures, mark filtering sets) and GDEF classes are not applied; pairs are looked up by glyph pair, so this matters only for sequences containing marks.
- Device/Variation tables in ValueRecords are ignored (variable-font deltas not applied).
- `kern` table: only format 0 horizontal subtables; Apple 'kern' version 1 is rejected (`Unsupported`).
- Lookup types other than pair (GPOS 2) and ligature (GSUB 4) are skipped; no contextual lookups, no mark positioning.
- Multi-lookup accumulation for pairs: the first lookup/subtable that yields a value wins (HarfBuzz oracle agreed on the tested fonts).
- Glyph ids in, glyph ids out: cmap (character → glyph) is the job of the `sfnt` module (wave step 7).
