# omni-render changelog

## 0.1.0 (2026-10-10)
omni-render 0.1.0, first public release.

- The engine as measured in Technical Disclosure Part 6 (2026-10-04), plus the work of 2026-10-05 to 2026-10-09 (JavaScript compiled to an IR,
  the script platform, browser memory and connectivity gates; research corpus docs 30-37).
- Licence split: renderer MIT OR Apache-2.0; doctor and T25 core PolyForm (cargo feature `t25`, off by default). A default build contains
  no PolyForm code: `src/jit/t25_stub.rs` stands in for the T25 hooks, and the doctor's page-load hooks are compiled out. `src/oracle_laws.rs`
  (generated from Chrome black-box answers) is MIT OR Apache-2.0.
- Checked: default and `t25` builds both compile; on the 50-page check corpus they print identical output; 15 unit tests pass (default build).
  Not checked: the full layout-agreement gate on the default build (the gate needs the `t25` build).
- Not shipped: the third-party page corpus, Chrome black-box result files, run results.
