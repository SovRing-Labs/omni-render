// SPDX-License-Identifier: MIT OR Apache-2.0
// b7: b3 inside a function — isolates the closure-call cost from top-level (global) lookups
function main() {
  function mk() { let c = 0; const k = 7; return function (x) { c = (c + x * k) | 0; return c; }; }
  const f = mk(); let t = 0;
  for (let i = 0; i < 500000; i++) { t = f(i); }
  return t;
}
console.log("b7", main());
