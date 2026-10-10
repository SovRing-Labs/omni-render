// SPDX-License-Identifier: MIT OR Apache-2.0
// b8: semantics the slot resolver must keep (output compared with node): error text naming resolved locals, closures
// per iteration, shadowing in blocks / catch / switch, hoisting, named function expressions, var-in-block, arguments
function g() {
  const x = {}; const out = [];
  try { x.foo(); } catch (e) { out.push(e.message); }
  try { let y; y(); } catch (e) { out.push(e.message); }
  try { new x(); } catch (e) { out.push(e.message); }
  const fs = []; for (let i = 0; i < 3; i++) fs.push(() => i); out.push(fs.map(f => f()).join());
  let a = 1; { let a = 2; out.push(a); } out.push(a);
  try { throw 7; } catch (a) { out.push(a); } out.push(a);
  switch (a) { case 1: { let a = 'sw'; out.push(a); } }
  out.push(typeof h, h()); function h() { return 'hoisted'; }
  const fe = function me(n) { return n ? me(n - 1) + 1 : 0; }; out.push(fe(4));
  { var vb = 'var-in-block'; } out.push(vb);
  out.push((function () { return arguments.length; })(1, 2, 3));
  let c = 0; const inc = () => ++c; inc(); inc(); out.push(c);
  for (const [k, v] of [[1, 2], [3, 4]]) out.push(k + v);
  for (const k in { p: 1, q: 2 }) out.push(k);
  let z = 5; z += 2; z **= 2; z ??= 9; z ||= 1; out.push(z);
  return out.join('|');
}
console.log("b8", g());
