# omni-render — Technical Disclosure, Part 6 (2026-10-04): our rendering engine, its program runtime, JS on our IR, and a self-repair core

> Copy notice. Licence of this text: CC BY 4.0. Software that this Part describes carries its own licence where it is released.

Dated prior art. Covers omni-render work measured 2026-10-03 → 2026-10-04 (no earlier part disclosed omni-render). Laptop = i5-8365U
(4 cores / 8 threads, AVX2, L3 6 MB), shared during most runs with a concurrent CPU training job (load 6–12), so wall times are noisy.
Ground truth = Chrome: a frozen corpus of 49 public pages captured with their Chrome DOMSnapshot; a box "agrees" when x, y, width and
height are each within max(2 px, 5%). Failures are disclosed as measured. Field terms throughout (the author's own vocabulary): "render" = both the
visual engine and emitting code into memory at run time. Evidence status: the item-level run records, the system manual and development log, and the plan files behind these numbers are private and are not published, and a claim-to-evidence index for this Part has not been published yet; a figure that does not name a harness and a result file is a figure recorded by the project, not an independently reproducible one.

## 1. The visual engine
- **Packed DOM + own CSS engine + integer layout + 2-bit raster:** html5ever drives our tree sink into flat columns (16 B/node,
  5.2× less RAM than an Rc DOM); own cascade (selectors, media queries 4, @import, @supports evaluated as Chrome, custom properties,
  full calc/min/max/clamp math): 1,110,636 of 1,121,947 computed values equal Chrome (99.0%); layout in 1/64 px integers
  (block, inline, float, table, flex, grid incl. auto-flow column, abs/fixed/sticky, margin collapsing with clearance, declarative
  shadow DOM); glyphs as 2-bit masks (≤ 0.002 SSIM cost) in content-addressed palette tiles; mean SSIM 0.815 (viewport) / 0.763 (full).
- **Agent mode + safety:** 28.5 pages/s at 36 MB (same harness: Lightpanda 23.3 @ 94 MB, Chrome 0.78 @ 551 MB); Landlock + seccomp
  sandbox 5/5 escape probes denied; fuzzer 242k executions, 0 findings after fixes; TLS 1.3/1.2 through rustls with webpki roots
  (badssl expired / self-signed / wrong-host / untrusted-root refused).
- **Layout explained, then algebra:** a derivation recorder (`why`) reports for one watched box every term that built it (intrinsic
  min/max, flow gap from collapsed margins, each flex pass, abs containing block, every later move) plus the winning declaration per
  property with its stylesheet rule; zero cost when off (one thread-local load). Check rings R0–R3 detect CSS invariant violations
  and pair each finding with Chrome's verdict on that box, so a ring's false-alarm rate is measured. Inline min-content became a
  segment monoid (leading/trailing unbreakable widths fuse across element boundaries): +180 boxes, 0 pages worse; one shrink-to-fit
  rule replaced 4 hand paths, one subtree move replaced 6 shift sites.
- **Agreement over time (hand fixes):** 14,156 / 27,019 (run 21) → 17,391 / 28,010 (run 35; the comparable-box count grew with
  recaptures). Since 10-03, 26 of 44 renderer commits were hand fixes of one missed size/position term — the motivation for §4.

## 2. The program runtime (render = emit code)
- **Own x86 emitter** (VEX2/VEX3 + ModRM, ~35 instructions) writing W^X pages; every encoding checked against GNU objdump (0 diffs).
- **Ternary heads compiled away:** +1 → add, −1 → sub, 0 → nothing; 8-lane form `vpaddd/vpsubd ymm, [rdi+disp8]`; 0 mismatches on 16
  trained matrices + random 4 KB–4 MB; 1.6–3.4× the packed-trit kernel while the code fits L3.
- **Sift twin rendered from IR** (pshufb LUT form, d unrolled): 0/45 mismatches vs the hand AVX2 library, rendered in 30 µs.
  **Disclosed failure:** the IR v0 tiled schedule was first reported 2.76× faster than the hand kernel — the baseline was the untiled
  kernel; against the hand register-blocked kernel it is 1.4–2.5× SLOWER (corrected record).
- **A core inside the frame loop:** each frame renders a query from the hovered node's packed columns, sifts the page's element marks
  (d 1,024), runs a ternary head as code and one ring-core beat: 428 µs p50 / 1.42 ms p99 (2.6% / 8.5% of a 60 Hz frame). Open: the
  head costs 52–65 µs in the loop vs 18.5 µs hot (its 0.9 MB of code exceeds L2).
- **WASM apps calling our cores as intrinsics** on their own linear memory (wasmtime sandbox: fuel, no ambient I/O): a 4.5 KB Rust app
  loads a ternary index and ranks through host top-k in place.

## 3. JavaScript on our IR, and an app translated instead of ported
- **jsi:** a JS interpreter over a typed AST lowered from an ESTree build-time parse: closures, classes, async, destructuring, try/catch,
  ES modules, own regex engine, WTF-16 strings with lone surrogates, fetch through the sandbox broker. Verified as Omniglyph language
  `omniir`: 29/29 laws with a JS form agree with node. Speed: a microbench 792 → ~370 ms (node 40 ms) after typed lowering,
  capture-aware per-iteration scopes, unscoped blocks and a fast hasher — 9× slower than node, disclosed.
- **Svelte → IR:** components translated at build time; a Svelte runtime on jsi (instance scopes, prop ownership, `$:` with dependency
  tracking, stores, lifecycle after render, element handles, events). The fleet launcher (65 components, 968 KB of JS) on our engine
  vs Chrome with JS on: 22.2% → 57.3% → **73.7% box agreement (2,150 / 2,918)**, SSIM 0.699, 0 script errors, 12 live requests,
  1.6 MB resident (Chrome heap 16.4 MB).
- **Ternary selector matching (SEL-T0):** selectors as a ternary dictionary matched against element feature vectors; matched rule sets
  IDENTICAL to the interpreter on every element of 6 heavy pages (speed not claimed yet; the JIT form is the next step).

## 4. A self-repair core for the layout engine (no model heads)
The author's statement (readback confirmed): an automated worker that takes the problems one at a time, compares the error to the proof,
tries combination after combination until complete, and gets better as it succeeds, like a sieve. Built from the parts of our
reasoning generators (Part 5 §2), mapped one to one:
| Generator part | In the core |
|---|---|
| strategy as data | the engine's **program vector**: ternary weights {−1, 0, +1} per (decision point, context), read by layout at run time; absent = the hand rule |
| independent judge, ring 0 | Chrome's boxes + the scorer + the gate; the core can never write them |
| frozen replay + holdout | 37 problem pages + 12 gate-only pages, hash-pinned on first use |
| gate (100% verify, verdict class kept) | every box that agreed with Chrome still agrees, on replay AND holdout |
| quarantine with the killing row | every refused move with the page + box it broke; history = rollback breadcrumb |
| abstain naming the missing piece | open problems name the layout stage with no decision point, or why they stay open |
| solvers ported as stations | causal.dag → CAUSAL, science.factorial → FACTORIAL, sat.dpll → SAT, code.debug → DEBUG |
| trace ISA | every problem leaves a trace in the 16-op ISA (GOAL OBSERVE PREDICT INTERVENE DERIVE CHECK EMIT ABSTAIN) |
- **Problems:** origin mismatches (the box disagrees, its parent agrees) clustered by error kind × display × parent display × text.
- **Term ledger (reach):** every additive layout term is written through one call that records (node, term, value) and weighs it by
  the program's knob for (term, the node's display context) — 44 terms (box frame/content, flow gap and margins, line-box strut/text/
  atomic extents, table spacing/extra/valign/cell min-max, grid gaps/rows/stretch/offsets, flex gaps/base/margins/offsets, relative/
  translate) and 12 **rule switches** (a hand rule as a knob: margin collapse first/last/empty, table shrink-to-fit, max-width,
  auto-margin centring, quirks empty line, replaced width, flex automatic minimum, grid grow-to-limit, table cell specified width,
  table fill; counts from the source, 2026-10-04). With no knobs set the corpus is box-for-box unchanged.
- **Lanes (FACTORIAL, deterministic):** layout is deterministic, so the factorial generator's required-runs rule gives one intervention
  per setting; each problem's fix is first PREDICTED from its boxes' term values (Δ = −v·(1 − w)) for all instances at once, and only
  the top predictions are re-laid out (286 → 120 interventions); each screen covers every page, so it is also its own gate.
- **CAUSAL:** a fix is direct when the box is repaired while its parent did not move; repaired only by moving the parent = the cause
  sits in the parent's term (commit through the parent if nothing is lost); climb to the grandparent for table/grid causes.
- **SAT conditions:** for a fix that repairs boxes but breaks others, positives = the term's owners on the repaired boxes, negatives =
  the nearest owner with a nonzero term above each broken box; DPLL set cover (most-constrained-first, as sat.dpll) finds the smallest
  conjunction of ≤ 3 structural node features (display, tag, parent, quirks, position, float, overflow, box-sizing, white-space,
  vertical-align, text, set sizes, frame sides, margin signs, first/last child, child count…, never class names) true on every positive
  and false on every negative; the knob then fires only where the condition holds; screened and gated again.
- **DEBUG pairs:** problems where single settings close only part of the error get pairs screened as one compound fix.
- **Firing core:** one worker pool with TYPED tasks (baseline, watch.perf, watch.rings, cluster, predict, screen, decide, sat, debug)
  — different code sections for different problems run in the same tick (MIMD), lanes inside screens (SIMD); later stations drain
  first, each type's share grows with its queue depth (elastic replicas); a governor yields while the load average is above a cap;
  cycles repeat while a cycle still solves.
- **The core's own speed (WebKit selector-JIT lesson applied):** load (parse + cascade + fonts) measured at 70% of a screen, so
  it runs once per worker and only layout repeats: a warm screen 7.8× faster, a full cycle 52 s vs 316–341 s (6.1–6.6×), results
  identical. The first cached version was not exact (17,014 vs 17,391 boxes): a per-document calc() table kept in thread-local
  state leaked between cached pages — disclosed and fixed by restoring it per page. Knob clauses are compiled once (feature
  indices, most specific first) and must equal the interpreter: 456,043 / 456,043 identical.
- **Field ideas implemented clean-room (from their published descriptions; no code taken):** demand-driven incremental screens
  (Adapton / Salsa dirty marking: a knob re-lays out only pages where its term fired in its context — 4,439 of 9,996 page-screens
  skipped, 0 mismatches with every skipped page re-checked; cycle 52 → 41 s); feature-wise bitmask clause evaluation (QuickScorer)
  with a base-context prefilter (Hyperscan) and copy-and-patch machine-code stencils for knob clauses (Xu & Kjolstad) — all four
  forms (interpreter, compiled, bitmask, stencil) identical on 456,043 / 456,043 checks, but at today's 10 clauses the lazily
  compiled form is fastest (272–282 ms vs 376–457 ms warm: interning every feature costs more than 10 clauses) — kept for large
  programs; a per-layout knob memo (Truffle-style specialisation on observed constants); a fused lean screen pass (HyPer) —
  exact, no measurable gain (layout dominates); a program verifier (eBPF verifier idea) whose valid keys are read from the layout
  source at compile time, plus a SipHash-2-4 keyed signature: forged weight, wrong key and malformed programs all refused, the
  engine then runs its hand rules. Not built: Stylo-style layout sharing between identical subtrees; incremental Rete matching
  across cycles; partial evaluation of the layout code itself.
- **Results (all 0 boxes lost, every commit PROPOSED with history):** total **+318 agreeing boxes** over the hand-fixed engine: 17,391 →
  17,709 of 28,010 (62.1% → 63.2%) — core-1 +70 (three inline-intrinsic weights); term ledger +147 (`box.border_bottom[TableCell]=0`
  +128, `table.valign_dy[TableCell]=0` +15, `box.pad_bottom[Table]=−1` +4); SAT/parent +37 (`table.spacing[Table&tag=div]=0` +21 —
  the core found by itself that the HTML `<table>` 2 px cellspacing default does not apply to `display: table` divs;
  `box.border_bottom[Table]=−1` +11; `line.strut_below[Block&nkids=2&pt=1]=0` +5); climb run +30 (`grid.stretch_cols[Grid&tag=ul]=0` +27, `grid.stretch_cols[Grid&first=1&pos=0]=0` +3); all-stations run +34 (`table.spacing[Table&first=0&ptag=div&w_set=0]=0` +33). Generalisation: an AGM challenge on the launcher, never learned from, sustained 0 of 8 clauses and the largest rule adds +44 boxes there. The run-by-run log with every variation is a private record and is not published.
- **Mined from our own history:** 28 commits by interactive assistant sessions on the layout file → session-fix rows; anti-unified line edits (Getafix-style) give 1
  exact template seen twice (clamp a term with `.max`); by class, 14 of 28 commits added or narrowed a CONDITION (term added 4, clamp
  3, parent read 1, contribution dropped 1) — evidence that the SAT condition station is the core's most needed move.
- **DOCTOR core, first code-level repair (2026-10-05, PROPOSED):** reverse partitioned-IFS donor search — the layout file is cut at
  its own joints into 5,263 sections, each with a shape vector (identifiers abstracted, called method names kept) and a subject;
  for a broken range the whole file is searched for the domain section plus identifier map that reproduces it (sift shortlist
  over 16-phase hypervectors, then an exact LCS collage). Blind, it named grid track sizing as the analogue of table column
  distribution (top-1 on 4/4 ranges) and the table branch of intrinsic sizing as the analogue of the table min/max pass. A port
  of that donor's spanning-cell widening fired but changed no box (gate: no gain) — kept as negative knowledge. Tracing the
  largest "no knob" card showed 18 of its 21 cells inside `table-layout: fixed` tables, a property the engine never parsed;
  fixed table layout written from CSS 2.2 §17.5.2.1 behind a dormant switch passed the gate: **+72 boxes, 0 lost** (replay
  14,935 → 15,007, holdout unchanged), compiled = interpreter on 521,192 checks. Held as a proposed change for the author's review. Lesson: shape search finds candidates; a missing FEATURE has no donor and needs its own diagnosis station.
- **DOCTOR missing-feature station (2026-10-06):** the doctor reads, for every broken box and its ancestors, the CSS the page
  declares and subtracts what the engine's style code handles (read from its own source), ranking the remainder by how much more
  often it sits on broken than on agreeing boxes. Its first list named logical insets, `aspect-ratio`, `overflow-wrap`, grid
  alignment and border shorthands. Written from the specs and gated by a binary gate (agreeing-box snapshot of the old engine vs
  the new): logical insets +10, `aspect-ratio` +285 (holdout +124) with 0 lost — the first `aspect-ratio` try lost 2 boxes under an
  inline-flex parent and was conditioned away, the SAT station's move applied to a feature. Agreement 17,709 → 18,076 of 28,010.
- **Condition search for ported features (2026-10-06):** a feature that gains on most pages but loses on a few is put behind a
  dormant switch and the doctor gates it under every node-feature literal of its context, then pairs, keeping the best clause with
  zero loss — the SAT station's move, applied to code instead of knobs. First run: CSS border-spacing / border-collapse,
  unconditioned +132 / −14 → `<table>` elements without a set width: +76, 0 lost (311 gates, 8 min). `justify-items` passed after
  three traced losses, each an older bug (nested-grid max-content, margins ignored when centring, negative margins).
  Agreement 18,186 of 28,010. Field map for the next steps (papers only): ssFix / SimFix / GenProg donor search, CodePhage
  cross-application transfer, Getafix / Prophet learned patterns, Ochiai fault localisation, delta debugging, XFix layout repair.
- **Code index, review panel and equations (2026-10-06):** the doctor's structural index now spans 17 of our apps (Rust, Python,
  JS/TS/Svelte, C/C++): 31 MB, full build 12.6 s, incremental ~1.4 s, refusing any file with a foreign copyright header and any snapshot
  of another repo. A 9-cell oracle panel (3 lenses × 3 altitudes) caught two over-claims — the first "cross-app" donor was a pinned
  snapshot of the renderer itself, and 1,853 indexed sections were third-party headers — both fixed; with forks excluded, the renderer has
  no cross-app donor: its donors are its own code and a standards library. Following the author's direction, code sections were rewritten as symbolic
  equations bound by ROLE, not names: the table's min→max distribution and grid's grow-to-limit come out as the same equation
  (base + share·(cap − base)/Σ(cap − base)); as a transplant generator the equation form has not yet produced a candidate (0 bindable
  units over 3,958 pairs). Repair-candidate ranking by name text or parameter position is avoided by design.
- **Standards-built library (2026-10-06):** omni-spec — 12 modules written by one agent from the specifications alone (Unicode line
  breaking 19,346/19,346, bidi 770,241 + 91,707, graphemes, OpenType kerning and ligatures, inflate, PNG, Brotli, font tables, JSON,
  GIF, JPEG, lossless WebP), each passing its official conformance data; POSIX utilities split between two builders under a
  differential gate (10,000/10,000 vs GNU per tool). Released free to the community under Apache-2.0, pairing with the doctor as its donor library.

## 5. What has not borne fruit yet, and what is promising
- **Not yet:** 43 of ~63 problems (≈ 223 boxes) remain "no knob moves these boxes" (table column widths, flex item widths, grid
  placement); SAT abstains when 0–5 positives face ~500 negatives (rule switches act too broadly, e.g. margin collapse off fixes the
  largest problem, 25 boxes, but breaks ~500); DEBUG pairs: 0 solves so far; template mining yields classes, not yet applicable
  patches; firing core mixes task types in only 2–9% of ticks (phases still wait on each other); `box.border_bottom[TableCell]=0`
  most likely stands in for the unimplemented `border-collapse` and must become the real rule.
- **Promising:** reach grows mechanically (every recorded term or rule is a knob for free); conditions from DPLL rediscover real CSS
  rules and are readable; the core's run is 2–5× cheaper per problem than v0 while diagnosing every problem; traces in the generator
  ISA make the core's work the rows of a future `layout.repair` generator (practice for our seed); next: the climb to the table level,
  per-page overlap of baseline/screen, finer SAT features, a PATCH station applying mined templates on a clone, and cementing a knob
  into code once it holds on fresh captures.

## 6. Provenance (borrowed / ours)
The field's: html5ever, rustls/ring/webpki roots, wasmtime, acorn (build-time parse), Svelte (the translated app's framework), Chrome
DevTools DOMSnapshot (ground truth), Landlock/seccomp, SSIM, CSS 2.2 / Flexbox / Grid / Box Alignment specifications, SPR (Long &
Rinard, FSE 2015: transformation schemas, target value search, condition synthesis), Getafix (Bader et al., OOPSLA 2019: fix templates
by anti-unification), the plastic surgery hypothesis (Barr et al., FSE 2014), WebKit's CSS selector JIT (Poulain 2014),
Adapton (Hammer et al., PLDI 2014) / Salsa, Rete (Forgy), QuickScorer (Lucchese et al., SIGIR 2015), Hyperscan (Wang et al., NSDI
2019), copy-and-patch (Xu & Kjolstad 2021), Truffle/Graal (Würthinger et al.), HyPer (Neumann, VLDB 2011), eBPF verifier, SipHash
(Aumasson & Bernstein), Formal Concept Analysis (Ganter & Wille), max-plus algebra, AGM belief revision, Cassius (Panchekha & Torlak, OOPSLA 2016) and Medea (Liu, Bodik et al., PLDI 2023) for layout as a formal
spec, DPLL, Fisher factorial designs, Wald SPRT, Pearl's backdoor criterion, segment monoids, W^X JIT practice. Ours: the packed DOM
+ 2-bit content-addressed raster engine; the derivation recorder paired with Chrome-verdict check rings; the program vector read by a
layout engine; the term ledger (every term and hand rule a knob, zero-cost when unset); deterministic lanes prediction + one shared
intervention screen as its own gate; parent-unmoved direct-effect test; SAT-synthesised feature conditions on layout knobs; a typed
MIMD firing core with depth-following replicas and a training-yield governor; the generator-part mapping (ring-0 judge, frozen
replay/holdout, quarantine with killing box, ISA traces) applied to engine repair; JS verified as an Omniglyph language; Svelte apps
translated through our IR onto our engine. None claimed as first without a wider prior-art search; all dated here.
