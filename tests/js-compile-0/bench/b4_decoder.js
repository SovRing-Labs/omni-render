// SPDX-License-Identifier: MIT OR Apache-2.0
// b4: protobuf-style decoder: varints, 10-deep if/else on the tag, property writes (C4 inline cache)
const N = 40000, buf = [];
for (let i = 0; i < N; i++) { const tag = (i % 10) + 1; buf.push((tag << 3) | 0); let v = (i * 2654435761) >>> 0 & 0x3fff; while (v > 127) { buf.push((v & 127) | 128); v >>>= 7; } buf.push(v); }
function Msg() { this.a = 0; this.b = 0; this.c = 0; this.d = 0; this.e = 0; this.f = 0; this.g = 0; this.h = 0; this.i = 0; this.j = 0; }
function decode(b) {
  const m = new Msg(); const r = { pos: 0 };
  function varint() { let x = 0, sh = 0, by; do { by = b[r.pos++]; x |= (by & 127) << sh; sh += 7; } while (by & 128); return x >>> 0; }
  while (r.pos < b.length) {
    const t = varint() >>> 3; const v = varint();
    if (t === 1) m.a += v; else if (t === 2) m.b += v; else if (t === 3) m.c += v; else if (t === 4) m.d += v; else if (t === 5) m.e += v;
    else if (t === 6) m.f += v; else if (t === 7) m.g += v; else if (t === 8) m.h += v; else if (t === 9) m.i += v; else m.j += v;
  }
  return m;
}
let m; for (let k = 0; k < 3; k++) m = decode(buf);
console.log("b4", m.a, m.e, m.j);
