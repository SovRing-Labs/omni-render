# omni-render — Provenance

**Licence:** the renderer (DOM, style, layout, paint, JavaScript, network, program runtime) MIT OR Apache-2.0; the doctor and the T25 core
(`src/doctor*.rs`, `repair.rs`, `oracle_laws.rs`, `jit/t25.rs`) PolyForm Small Business 1.0.0 OR PolyForm Noncommercial 1.0.0, built only with
the `t25` / `doctor` features (`LICENSING.md`). **Disclosure:** published as dated prior art (Technical Disclosure Part 6; Part 7 §3–4); no
patents filed.

## How this code was written
From published specifications and our own measurements, not from any browser engine's source. Three clean-room forms apply, named per part:
**standard text only** for the HTML-adjacent, CSS, layout, network, cookie, font, image and JavaScript modules (each cites its specification
sections); **two-team** (reader, clean-room gate, implementer) for the doctor's specification pipeline; **idea level** for the doctor's repair
methods and the program runtime. The reference browser was observed only as a black box (rendered boxes and computed styles kept as data).

## Ideas this engine builds on (cited, not copied)
| Idea | Source |
|---|---|
| Clone vectors, partitioned IFS and the collage theorem for donor search | Jiang et al., ICSE 2007; Jacquin, 1992 |
| Equality saturation; anti-unification; library learning | Willsey et al., POPL 2021; Plotkin 1970, Reynolds 1970; babble and Stitch, POPL 2023 |
| Enumerative and counterexample-guided synthesis | Udupa et al., PLDI 2013; Solar-Lezama et al., ASPLOS 2006; λ², Myth, FlashMeta (2015) |
| Repair search against a visual oracle; overfitting in test-based repair | Mahajan et al., 2017 (XFix); Smith et al., FSE 2015 |
| Layout as a formal specification | Panchekha and Torlak, OOPSLA 2016 (Cassius) |
| Residue number systems and CRT decoding | Szabo and Tanaka, 1967; Garner, 1959 |
| Multi-process browser with a confined renderer | Grier, Tang and King, 2008; Reis and Gribble, 2009 |
| Ternary heads compiled to add/sub code | Unrolling Ternary Neural Networks (arXiv 1909.04509) |

## Used, not copied
Linked crates (MIT OR Apache-2.0 unless noted): html5ever, markup5ever_rcdom, ttf-parser, png, gif, zune-jpeg, image-webp, brotli-decompressor
(BSD-3-Clause OR MIT), miniz_oxide, rustls, ring (Apache-2.0 AND ISC), webpki-roots (CDLA-Permissive-2.0 data), memmap2, serde_json, libc,
wasmtime (Apache-2.0 WITH LLVM-exception), omni-spec (Apache-2.0). No GPL or MPL dependency (cargo metadata, 2026-10-09). Conformance suites
and frozen third-party pages are used as data, never shipped.

## Checks to run on the export tree (before release)
ScanCode with snippet matching and gitleaks on the exact export; `cargo metadata` licence list in `NOTICE`; the doctor's clone check of
rebuilt code against its sources (designed, not yet run).
