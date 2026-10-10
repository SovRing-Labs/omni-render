// SPDX-License-Identifier: MIT OR Apache-2.0
// zeroWidth-drain.js — ENGINE-ONLY invariant (no node reference: node does not terminate here).
//
// `while ((m = /(?:)/g.exec(s)) !== null)` never terminates under node/V8 — the zero-length match
// at end-of-input keeps succeeding. This engine advances past the end and returns null, which is the
// behaviour that keeps a drain loop from wedging. That divergence from V8 is deliberate: an unbounded
// loop in a renderer is worse than a spec corner, and a logged finding was exactly such a wedge.
//
// The assertion: the drain terminates. If this ever prints FAIL, the zero-width advance regressed.
const ZW = /(?:)/g;
let n = 0;
let m;
while ((m = ZW.exec("abc")) !== null) {
  if (++n > 100) { console.log("FAIL zero-width drain did not terminate"); break; }
}
console.log(n <= 100 ? "terminated after " + n + " steps" : "FAIL");

// same for a pattern that can match empty in the middle: /a*/g
const A = /a*/g;
let k = 0;
while (A.exec("bab") !== null) { if (++k > 100) { console.log("FAIL a* drain did not terminate"); break; } }
console.log(k <= 100 ? "terminated after " + k + " steps" : "FAIL");
