// SPDX-License-Identifier: MIT OR Apache-2.0
// match-test.js — the /g/ vs non-/g/ halves of String.prototype.match, plus test/replace/split.
// match() is the one method that was legitimately all-matches; pin it so the a logged finding
// exec fix cannot quietly turn match back into a single match.
const s = "a1 b22 c333";
const EMPTY_G = new RegExp("", "g");   // a literal //g would lex as a line comment

console.log("match g", s.match(/\d+/g).join(","));
console.log("match non-g", s.match(/(\d)(\d)/)[0], "idx", s.match(/(\d)(\d)/).index);
console.log("miss g", s.match(/zz/g));
console.log("miss non-g", s.match(/zz/));
console.log("groups", JSON.stringify(s.match(/(a)(\d)/)));
console.log("empty pattern", s.match(EMPTY_G).length);
console.log("test", /b/.test(s), /zz/.test(s), /b/g.test(s));
console.log("test ignores li", (() => { const r = /b/; r.lastIndex = 5; const t = r.test(s); return t + "," + r.lastIndex; })());
console.log("replace all vs one", s.replace(/\d+/g, "#"), s.replace(/\d+/, "#"));
console.log("replace fn", s.replace(/\d+/g, (m) => "[" + m.length + "]"));
console.log("replace groups", s.replace(/([a-z])(\d)/g, "$2$1"));
console.log("split", s.split(/\s+/).join("|"), "a,b,,c".split(/,/).join("|"));