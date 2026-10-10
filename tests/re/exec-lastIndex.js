// SPDX-License-Identifier: MIT OR Apache-2.0
// exec-lastIndex.js — a logged finding regression: the global exec drain loop must terminate.
// TriadWorkspace.svelte mdToHtml() has exactly this shape over a 1.59 MB transcript and spun
// forever when exec ignored lastIndex. Everything prints primitives, because the engine's
// console.log formats containers differently from node's — parity is about values, not layout.
const src = "a ```js\ncode 1\n``` b ```py\ncode 2\n``` c tail *em* **strong**";
const FENCE = /```([^\n]*)\n?([\s\S]*?)(?:```|$)/g;

// the canonical shape: drain a /g/ regex with exec() until null
const found = [];
let m;
let guard = 0;
FENCE.lastIndex = 0;
while ((m = FENCE.exec(src)) !== null) {
  if (++guard > 50) { console.log("FAIL exec did not terminate"); break; }
  found.push([m.index, m[0], m[1], m[2]].join("~"));
}
console.log("fence count", found.length);
for (const f of found) console.log("fence", f);

// lastIndex must actually move, and reset to 0 once the drain hits null
FENCE.lastIndex = 0;
const first = FENCE.exec(src);
console.log("first", first.index, "li", FENCE.lastIndex);
const second = FENCE.exec(src);
console.log("second", second.index, "li", FENCE.lastIndex);
console.log("distinct", first.index !== second.index);
console.log("third", FENCE.exec(src));
console.log("li after null", FENCE.lastIndex);
console.log("restarts after null", FENCE.exec(src).index);

// lastIndex is per-regex-object, not global engine state
const R1 = /a/g, R2 = /a/g;
R1.exec("aaa");
console.log("independent", R1.lastIndex, R2.lastIndex);
