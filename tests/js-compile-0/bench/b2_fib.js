// SPDX-License-Identifier: MIT OR Apache-2.0
// b2: recursive calls (call overhead, activation frames)
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
console.log("b2", fib(25));
