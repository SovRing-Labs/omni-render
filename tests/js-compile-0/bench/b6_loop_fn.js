// SPDX-License-Identifier: MIT OR Apache-2.0
// b6: b1 inside a function (page bundles run in function scope, not at script top level)
function main() { let s = 0; for (let i = 0; i < 2000000; i++) { s = (s + i * 3) % 1000003; } return s; }
console.log("b6", main());
