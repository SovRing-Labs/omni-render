// SPDX-License-Identifier: MIT OR Apache-2.0
// b1: tight numeric loop (C3 closure compile, C6 x86 target)
let s = 0;
for (let i = 0; i < 2000000; i++) { s = (s + i * 3) % 1000003; }
console.log("b1", s);
