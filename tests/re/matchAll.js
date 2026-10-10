// SPDX-License-Identifier: MIT OR Apache-2.0
// matchAll.js — String.prototype.matchAll enumerates every match from lastIndex.
// Before a logged finding it was aliased to match(), so matchAll === match. Values only.
const s = "a1 b22 c333";
console.log("all", [...s.matchAll(/([a-z])(\d+)/g)].map((m) => [m[0], m[1], m[2], m.index].join("~")).join(" | "));
console.log("empty", [...s.matchAll(/zz/g)].length);
console.log("non-global throws", (() => { try { s.matchAll(/([a-z])/); return "no-throw"; } catch (e) { return e.constructor.name; } })());
const re = /\d+/g;
re.lastIndex = 2;
console.log("from lastIndex", [...s.matchAll(re)].map((m) => m[0]).join(","));
console.log("leaves li alone", re.lastIndex);
console.log("vs match", s.match(/\d+/g).join(","));
