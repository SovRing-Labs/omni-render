# LICENSING — omni-render

Two licences, per file; every source file starts with an SPDX line saying which.

| Files | Licence |
|---|---|
| everything not listed below: the parser, style, layout, raster, networking, script runtime, tools, tests | `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`) |
| `src/oracle_laws.rs` (laws generated from Chrome black-box answers by `tools/emit_laws.py`) | `MIT OR Apache-2.0` |
| the doctor and T25 core: `src/doctor.rs`, `src/doctor_abc.rs`, `src/doctor_fix.rs`, `src/doctor_shape.rs`, `src/doctor_spec.rs`, `src/repair.rs`, `src/jit/t25.rs` | `PolyForm-Small-Business-1.0.0 OR PolyForm-Noncommercial-1.0.0` (`LICENSE.md`, `LICENSE-NONCOMMERCIAL.md`) |
| text of the technical disclosure and documentation | CC BY 4.0 |

The PolyForm files are compiled only with the cargo feature `t25` (off by default). A default build (`cargo build --release`) contains the
no-op stand-in `src/jit/t25_stub.rs` (MIT OR Apache-2.0) and none of the PolyForm code. You may use either licence of a dual pair.
Small Business and Noncommercial terms are those of polyformproject.org; for other terms, contact the author.

Published as dated prior art: the authors claim no patent and ask that none be granted on what is disclosed.

## Evaluation and commercial use (added 2026-10-14)
- **Evaluate first, free:** any organisation, of any size, may evaluate this repository's PolyForm-licensed files (the doctor and the T25 core; everything else is MIT OR Apache-2.0) under the **PolyForm Free Trial License 1.0.0**
  (`LICENSE-FREE-TRIAL.md`) for less than 32 consecutive calendar days.
- **Commercial licence:** use beyond the free licences (a larger company using it for a commercial purpose after evaluation) is
  available under a separate, paid commercial licence agreement with the licensor named in the Required Notice. To start one,
  open an issue in this repository titled "Commercial licence".
