> **Status: under development — pre-alpha.** Interfaces, numbers and file layout can change without notice.

# omni-render — status

Last checked: 2026-10-16, Linux laptop (Intel i5-8365U, 8 threads), on a clean copy of this repo. 9 of 13 suite tests work today; 11 of 13 can run without special hardware. Each line below names its test id in `suite.json` and the evidence file it rests on.

## Works today

- **R1** Layout agrees with Chrome on 85.7 % of boxes (49 frozen pages) — PASS 2026-10-16 (`24,004 of 28,010 (85.7 %)`); evidence: `DOSSIER.md`.
- **R2** 99.0 % of computed CSS values equal Chrome's — PASS 2026-10-16 (`1,110,636 of 1,121,947`); evidence: `TECHNICAL-DISCLOSURE.md`.
- **R3** Agent mode: 28.5 pages/s at 36 MB — PASS 2026-10-16 (`28.5 pages/s at 36 MB`); evidence: `TECHNICAL-DISCLOSURE.md`.
- **R4** Sandbox: 5/5 escape probes denied; fuzzer 0 findings — PASS 2026-10-16 (`5/5 escape probes denied; 242k executions`); evidence: `TECHNICAL-DISCLOSURE.md`.
- **R5** Own x86 emitter: 0 diffs vs objdump; ternary heads and sift twin 0 mismatches — PASS 2026-10-16 (`0 diffs; 0/45 mismatches`); evidence: `TECHNICAL-DISCLOSURE.md`, `src/jit/x86.rs`, `src/jit/ternary.rs`.
- **R7** JavaScript on our IR: 29/29 laws agree with node; fleet launcher 73.7 % box agreement; 9x slower than node — PASS 2026-10-16 (`29/29 laws; 2,150 / 2,918; 9x slower`); evidence: `TECHNICAL-DISCLOSURE.md`.
- **R9** Doctor: clean-room rebuild 27/32 functions, 256/256 seeded leaks refused, fix loop 565 -> 220 defects — PASS 2026-10-16 (`27 of 32; 256/256; 565 -> 220`); evidence: `DOSSIER.md`.
- **R10** JS-compile gates: js_mod 0 mismatches / 20,000,441; probe 106/106; outputs equal node — PASS 2026-10-16 (`0 mismatches / 20,000,441; 106/106`); evidence: `tests/js-compile-0/INDEX.md`.
- **R11** Regex/String test cases run under node (reference side only) — PASS 2026-10-16 (`ok 7`); evidence: `tests/re/exec-lastIndex.js`, `tests/re/matchAll.js`, `tests/re/match-test.js`, `tests/re/search.js`, `tests/re/date.js`, `tests/re/error-surface.js`, `tests/re/exec-nonGlobal.js`.

## Not yet tested

- **R12** Regex/String parity gate: node vs omni-render output byte for byte — external: needs something the test laptop does not have; needs: cargo (Rust 1.8x, edition 2024), node >= 18, acorn (npm package, for tools/js2ir.mjs), a release build: cargo build --release
- **R13** Rust unit tests (x86 emitter, ternary, cookies, net, paint, css, page, jsi) — external: needs something the test laptop does not have; needs: cargo (Rust 1.8x, edition 2024), a C toolchain
- Quick test `for f in tests/re/*.js; do node "$f" >/dev/null || exit 1; done; echo ok` — not run on the laptop (long Rust or model build).

## Known failures and honest negatives

- **R6** Disclosed correction: the 2.76x faster claim was against the wrong baseline. Only the corrected outcome is stated; no harness output for either measurement is in the repo. Recorded and re-checked 2026-10-16 (PASS); evidence: `TECHNICAL-DISCLOSURE.md`
- **R8** Honest negatives: what the self-repair core has not solved. Counts are the project's own tally; the core's source (src/doctor*.rs, src/jit/t25.rs) is PolyForm-licensed and built only with --features t25. Recorded and re-checked 2026-10-16 (PASS); evidence: `TECHNICAL-DISCLOSURE.md`, `DOSSIER.md`
- Rust is needed for the engine's own tests (R13); the quick test was not run on the laptop (long cargo build).

## Corrections

- **C09** — Doctor 565 → 24 defects, 98.0 → 99.91 % pristine: reproduced exactly (reclassified, not an error); a second counter reads a different baseline and is explained before both are quoted.

## How to re-check

- One file, any python3 >= 3.8: download [`sovring-suite.pyz`](https://github.com/SovRing-Labs/SovRing/releases/latest/download/sovring-suite.pyz), then `python3 sovring-suite.pyz --run omni-render` (headless) or `python3 sovring-suite.pyz` (local page on 127.0.0.1).
- From this checkout: `bash install.sh` (checks python, prepares a venv, runs the suite headless, then opens the page; `--no-gui` to skip the page).
- Quick test only: `for f in tests/re/*.js; do node "$f" >/dev/null || exit 1; done; echo ok`.

## Defensive publication

- `TECHNICAL-DISCLOSURE.md` — on public GitHub since 2026-10-10
- DOI: [10.5281/zenodo.23282731](https://doi.org/10.5281/zenodo.23282731) (Zenodo, see `CITATION.cff`)
- Licence: MIT OR Apache-2.0 for the renderer; PolyForm Small Business 1.0.0 OR PolyForm Noncommercial 1.0.0 for the doctor and T25 core (cargo feature t25, off by default; PolyForm Free Trial for evaluation); text CC BY 4.0 (LICENSING.md). Quicktest runs the node reference cases only; the engine's own tests need a cargo build (R13).
