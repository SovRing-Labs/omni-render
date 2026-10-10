# omni-render — Dossier

*History, specification, metrics, tests and limits. Companion to Technical Disclosure Part 6 and Part 7 §3–4 and `README.md`. The
research record: github.com/SovRing-Labs/omniring-research (docs 32–37, essays 14–15).*

## What it is

Our own rendering engine, in Rust: a packed DOM, its own CSS engine, integer layout and a 2-bit raster, plus a program runtime that
renders code into memory and runs JavaScript on our own IR. Inside it lives the **doctor**, a core that repairs the engine's own layout
code. In the stack it is the **eyes** — and, through Omni Web, the window. Stage (essay 07): system → primitive.

## Creation history

| Date (2026) | Milestone |
|---|---|
| 10-03 – 10-04 | Engine, program runtime, JavaScript on our IR and a self-repair core measured (Part 6): 99.0 % of computed values equal Chrome; 28.5 pages/s at 36 MB in agent mode |
| 10-06 | The doctor named as a core; standards wired into text layout (line breaking, kerning) from our standards library |
| 10-07 | Code as an alphabet (name-free equations, anti-unification, search by function 24/24); Chrome as a black-box oracle (nine laws); the two-team clean room |
| 10-07 – 10-08 | The gate series: agreement with Chrome 19,008 → 24,004 of 28,010 boxes, 0 lost at every merge |
| 10-07 | The T25 box kernel rendered as x86 and fired inside every layout pass (0.90 % of layout time) |
| 10-08 | Pristine geometry: the unattended fix loop (565 → 220 defects, 0 pristine boxes lost); an overlap-blind judge found and fixed |
| 10-09 | Licence split decided: renderer MIT OR Apache-2.0; the doctor and the T25 core PolyForm, behind a feature off by default |

## Specification (summary)

* Box agreement yardstick: x, y, width, height each within max(2 px, 5 %) of Chrome on 49 frozen pages.
* The doctor: shape census, angle measurement, black-box law synthesis, dormant switches gated with 0 boxes lost, the T25 box kernel,
  the pristine map and fix loop, the reader / clean-room gate / implementer pipeline. Part 6 §4, Part 7 §4.
* Merge gate before every engine pin: baseline unchanged, compiled = interpreted, binary gate 0 lost, JS probe, connectivity.

## Metrics

| Property | Result |
|---|---|
| Boxes agreeing with Chrome (49 pages) | 24,004 of 28,010 (85.7 %) |
| Fixes merged in the gate series | every one with 0 boxes lost |
| T25 kernel inside layout | 0.90 % of layout time |
| Clean room on our own layout file | 27 of 32 functions rebuilt from the spec alone; 256/256 seeded leaks refused |
| test262 (language suite) | 54.5 % → 56.85 % of executed tests |

## Limits

* Three fault-localisation probes failed their bars (doc 35); the reader reads Rust and JavaScript only.
* Pristine fixes move boxes away from Chrome where Chrome draws the same defect.
* Script-heavy pages are not light (Omni Web census, doc 33).

## Lineage and credits

html5ever, rustls / ring / webpki roots, wasmtime; SPR, Getafix, egg, babble, Stitch, DreamCoder, CEGIS, XFix, Cassius, Ochiai — full list in
Part 6 §6 and Part 7 §8; `PROVENANCE.md`. Updates: weekly, recorded here with dates.
