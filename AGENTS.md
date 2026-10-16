# AGENTS.md — omni-render

For coding assistants (and people) setting this repository up. Plain steps; every command runs from the repo root.

**Status: pre-alpha, under development.** Interfaces, numbers and file layout can change without notice. Read `STATUS.md` first: what works today (each line names its test and evidence file), what is not yet tested, the known failures and the corrections.

## What it is
Our own web rendering engine in Rust: packed DOM, own CSS engine, integer layout, 2-bit raster, a program runtime that emits x86, a JavaScript interpreter on its own IR, and a self-repair core for layout.

## Install and check (3 commands)
```bash
git clone --depth 1 https://github.com/SovRing-Labs/omni-render && cd omni-render
# nothing to install beyond: cargo
cargo test --release   # expect: test result: ok
```
Needs: cargo.
Whole suite instead: `bash install.sh --no-gui` (python check → venv → every runnable `suite.json` test, headless; without `--no-gui` it then opens the local results page on 127.0.0.1).
Note: a full Rust release build first (several minutes of CPU).

## What the tests mean
`suite.json` lists every claim this repository makes with its verify command, the expected output, what passing means, how the number was obtained and how to use it in your own work (13 tests, in `suite.json` at the repository root). `STATUS.md` says what works today, what is not yet tested and the known failures; the umbrella's `STATUS.md` has one row per repository.

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
