// SPDX-License-Identifier: MIT OR Apache-2.0
// exec-nonGlobal.js — a non-global exec must ignore lastIndex AND leave it untouched.
const re = /b/;
console.log("first", re.exec("abc").index, "li", re.lastIndex);
console.log("again", re.exec("abc").index, "li", re.lastIndex);
re.lastIndex = 99;
console.log("ignores lastIndex", re.exec("abc").index, "li", re.lastIndex);
console.log("miss", /zz/.exec("abc") === null);
console.log("fresh li", /a/g.lastIndex);
console.log("span", /[\s\S]+/.exec("日本")[0].length);
console.log("no input throws", (() => { try { /a/.exec(); return "no-throw"; } catch (e) { return e.constructor.name; } })());
console.log("groups", /(\d)(\d)/.exec("ab12")[1] + re.exec("abc")[0]);
console.log("index prop", /b/.exec("abc").index);