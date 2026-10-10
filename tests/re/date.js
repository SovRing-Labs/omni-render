// SPDX-License-Identifier: MIT OR Apache-2.0
// date.js — Date instance surface parity. src/jsi_date.rs added real calendar maths because every feed
// row in the launcher formats a timestamp; before it, `new Date(x).toISOString()` was a TypeError
// (12 in AgentManagerPopout, 13 in ControlCenter, 1 in FlightMatrix). Values here avoid wall-clock and
// locale-dependent formatting, which cannot be asserted portably — formatting is pinned separately in
// format.js against a fixed offset.
const T = Date.UTC(2026, 8, 5, 14, 7, 9, 42);          // 2026-09-05T14:07:09.042Z
const d = new Date(T);

console.log("getTime", d.getTime() === T);
console.log("valueOf", d.valueOf() === T);
console.log("iso", d.toISOString());
console.log("json", d.toJSON());
console.log("utc y/m/d", d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
console.log("utc h/m/s/ms", d.getUTCHours(), d.getUTCMinutes(), d.getUTCSeconds(), d.getUTCMilliseconds());
console.log("getDay sun=0", new Date(Date.UTC(2026, 8, 6)).getDay(), new Date(Date.UTC(2026, 8, 5)).getDay());
console.log("epoch", new Date(0).toISOString());
console.log("negative", new Date(-1).toISOString());
console.log("pre-epoch year", new Date(Date.UTC(1969, 11, 31, 23, 59, 59)).toISOString());
console.log("leap day", new Date(Date.UTC(2024, 1, 29)).toISOString());
console.log("non-leap day", new Date(Date.UTC(2023, 1, 29)).toISOString());
console.log("month overflow", new Date(Date.UTC(2026, 12, 1)).toISOString());
console.log("day 0 clamps", new Date(Date.UTC(2026, 8, 0)).toISOString());

// parse forms Date.parse must accept
for (const s of ["2026-09-05", "2026-09-05T14:07:09.042Z", "2026-09-05T14:07:09Z", "2026-09-05T00:00:00+02:00", "2026-09-05 14:07:09"]) {
  console.log("parse", s, Date.parse(s));
}
console.log("parse bad", Number.isNaN(Date.parse("not a date")));

// static vs multi-arg constructor: Date.UTC is UTC, new Date(y,m,...) is local
console.log("UTC static", Date.UTC(2026, 8, 5) === new Date(Date.UTC(2026, 8, 5)).getTime());
console.log("multi-arg ms", new Date(2026, 8, 5, 14, 7, 9, 42).getMilliseconds());

// a numeric string is Invalid Date (not a timestamp) -- both engines agree, via the throw
console.log("numeric string invalid", (() => { try { new Date(String(T)).toISOString(); return "parsed"; } catch (e) { return e.name; } })());
console.log("no-arg is number", typeof Date.now());
console.log("roundtrip", new Date(d.toISOString()).getTime() === T);