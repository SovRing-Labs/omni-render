// SPDX-License-Identifier: MIT OR Apache-2.0
// b3: closure-variable calls (C1/C2 slot scopes) — brief: 2 us/call vs node 0.3 us
function mk() { let c = 0; const k = 7; return function (x) { c = (c + x * k) | 0; return c; }; }
const f = mk(); let t = 0;
for (let i = 0; i < 500000; i++) { t = f(i); }
console.log("b3", t);
