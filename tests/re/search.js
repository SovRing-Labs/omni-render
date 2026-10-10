// SPDX-License-Identifier: MIT OR Apache-2.0
// search.js — String.prototype.search returns an index or -1. Before a logged finding it
// was aliased to match(), so it returned an array of matches or null instead.
const s = "the quick brown fox";
const EMPTY = new RegExp("");   // a literal // would lex as a comment, not an empty pattern

console.log("hit", s.search(/quick/));
console.log("miss", s.search(/zebra/));
console.log("start", s.search(/^the/));
console.log("non-global ignores flag", s.search(/o/g));
console.log("empty pattern", s.search(EMPTY));
console.log("string arg", s.search("brown"));
console.log("first of many", s.search(/o/), s.indexOf("o"));
console.log("groups discarded", s.search(/(qu)ick/));
console.log("does not move lastIndex", (() => { const r = /o/g; r.lastIndex = 2; s.search(r); return r.lastIndex; })());
console.log("empty string", "".search(/x/), "".search(EMPTY));
console.log("after end", s.search(/zz/), s.search(/o/g));