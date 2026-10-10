# omni-render

A web rendering engine for the computers people already own: speed across the whole experience, small memory, safety and privacy.
Packed DOM (16 bytes per node, 3.9x less RAM than an `Rc` DOM over html5ever on the bench corpus), own CSS engine, integer fixed-point
layout (1/64 px), 2-bit glyph masks, a JavaScript runtime that compiles to its own IR, and a self-repair ("doctor") core for layout.
**Licence:** MIT OR Apache-2.0 for the renderer; the doctor and T25 core are PolyForm Small Business 1.0.0 OR PolyForm Noncommercial 1.0.0
and are built only with `--features t25` (`LICENSING.md`). **Disclosure:** published as dated prior art, `TECHNICAL-DISCLOSURE.md` (Part 6);
later work is Part 7 in the research corpus (https://github.com/SovRing-Labs/omniring-research, docs 30-37).

## Build and run
```bash
cargo build --release                      # renderer only (MIT OR Apache-2.0)
cargo build --release --features t25       # adds the doctor and T25 core (PolyForm)
cargo test --release                       # unit tests (default build)
./target/release/omni-render bench <corpus-dir>   # DOM bytes per node vs html5ever RcDom
./target/release/omni-render check <corpus-dir>   # layout self-checks
```
The bench corpus is third-party web pages and is not shipped: `python3 tools/fetch_corpus.py` freezes one (needs Chrome and Playwright).
Subcommands that need the doctor (`core ...`, `t25-bus-bench`) exist only in the `t25` build.

## What is here
`src/` the engine (parse, style, layout, raster, network, scripts); `src/jit/` the program runtime; `tools/` corpus, oracle and measurement
scripts; `tests/` JavaScript and regular-expression tests and the A/B harness; `apps/` two small demonstrators. Measured results with their
harnesses: `TECHNICAL-DISCLOSURE.md`. History and limits: `DOSSIER.md`. Sources and what was not copied: `PROVENANCE.md`.
Layout agreement with Chrome on the frozen 49-page corpus is reported in the disclosure, including the failures.
