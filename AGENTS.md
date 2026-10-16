# AGENTS.md — omni-render

For coding assistants (and people) setting this repository up. Plain steps; every command runs from the repo root.

## What it is
A web rendering engine for the computers people already own: speed across the whole experience, small memory, safety and privacy. Packed DOM (16 bytes per node, 3.9x less RAM than an `Rc` DOM over html5ever on the bench corpus), own CSS engine, integer fixed-point layout (1/64 px), 2-bit glyph masks, a JavaScript runtime that compiles to its own IR, and a self-repair ("doctor") core for layout. Licence: MIT OR Apache-2.0 for the renderer; the doctor and T25 core are PolyForm Small Business 1.0.0 OR PolyForm Noncommercial 1.0.0 and are built only with `--features t25` (`LICENSING.md`). …

## Install and check (3 commands)
```bash
git clone --depth 1 https://github.com/SovRing-Labs/omni-render && cd omni-render
# nothing to install beyond: cargo
cargo test --release   # expect: test result: ok
```
Needs: cargo.
Note: a full Rust release build first (several minutes of CPU).

## What the tests mean
`suite.json` lists every claim this repository makes with its verify command, the expected output, what passing means, how the number was obtained and how to use it in your own work (being written). `STATUS.md` says what works today, what is not yet tested and the known failures; the umbrella's `STATUS.md` has one row per repository.

## Where the evidence is
- `DOSSIER.md` — history, metrics and limits
- `TECHNICAL-DISCLOSURE.md` — the method, as dated prior art
- `CITATION.cff` — DOI 10.5281/zenodo.23282731

## Honest limits (from DOSSIER.md)
Three fault-localisation probes failed their bars (doc 35); the reader reads Rust and JavaScript only. Pristine fixes move boxes away from Chrome where Chrome draws the same defect. Script-heavy pages are not light (Omni Web census, doc 33).

## Licence
MIT OR Apache-2.0 (renderer); doctor and T25 core PolyForm Small Business OR Noncommercial. Details: `LICENSING.md`.

## Guided setup through MCP
The SovRing umbrella ships an MCP server that explains, checks readiness, clones and runs these tests: https://github.com/SovRing-Labs/SovRing (README → "Paste this to your assistant"). Zero-install docs endpoint: https://gitmcp.io/SovRing-Labs/omni-render
