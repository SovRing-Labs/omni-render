// SPDX-License-Identifier: MIT OR Apache-2.0
// b5: reads three scopes up + block-scoped lets (C1 depth/slot resolution)
function outer() {
  let a = 1, b = 2;
  function mid() {
    let c = 3;
    function inner(n) { let acc = 0; for (let i = 0; i < n; i++) { const d = i & 7; { let e = d + c; acc = (acc + a * e + b) % 65521; } } return acc; }
    return inner;
  }
  return mid();
}
console.log("b5", outer()(600000));
