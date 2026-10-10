#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""box_t25.py — boxes on T25 (J 10-06: "go up to t25, see how the boxes map … it will show us what we haven't been able to see").

T25 = the first 25 odd primes 3…101, period P25 ≈ 1.16e38 ≈ 2^126.5 — the largest tier that fits a u128. Five groups of 5 rings
carry FIVE exact coordinates per box (1/64 px), one CRT integer per box:
  x0 {3,29,41,59,101} · y0 {5,31,37,67,89} · w {7,23,43,61,97} · h {11,19,47,71,83} · ry = y0 − frame y0 {13,17,53,73,79}
The fifth group is what T20 could not hold: the box's position INSIDE its frame. Ours vs Chrome = two points; per ring the
residue gap is an angle; the CRT over a group's angles is that coordinate's exact error. The point now splits a failing box three
ways:  SHAPE (w / h twist) · OWN PLACE (ry twists: wrong gap / offset in its frame) · CARRIED (absolute rings twist, frame-relative
and shape rings do not: moved by an ancestor, nothing to fix here).
Check: a carried box should sit under a failing frame far more often than a box with its own defect.
Usage: box_t25.py DUMP.jsonl [--render OUTDIR]"""
import argparse, json, math, os, sys
from collections import Counter

T25 = [3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97, 101]
G = {"x0": [3, 29, 41, 59, 101], "y0": [5, 31, 37, 67, 89], "w": [7, 23, 43, 61, 97], "h": [11, 19, 47, 71, 83], "ry": [13, 17, 53, 73, 79]}
P25 = math.prod(T25); U = 64
OFF = {"x0": 16384 * U, "y0": 16384 * U, "w": 0, "h": 0, "ry": math.prod(G["ry"]) // 2}
COEF = {m: (P25 // m) * pow(P25 // m % m, -1, m) % P25 for m in T25}


def crt(res, mods):
    Pm = math.prod(mods); return sum(r * (Pm // m) * pow(Pm // m % m, -1, m) for r, m in zip(res, mods)) % Pm


def coords(rect, frame):
    x, y, w, h = rect
    return {"x0": round(x * U), "y0": round(y * U), "w": round(w * U), "h": round(h * U), "ry": round((y - (frame[1] if frame else 0)) * U)}


def encode(c):
    return sum(((c[k] + OFF[k]) % m) * COEF[m] for k, ms in G.items() for m in ms) % P25


def decode(B):
    return {k: crt([B % m for m in ms], ms) - OFF[k] for k, ms in G.items()}


def errors(Bo, Bc):
    e = {}
    for k, ms in G.items():
        Pm = math.prod(ms); d = crt([(Bo - Bc) % m for m in ms], ms); e[k] = (d - Pm if d > Pm // 2 else d) / U
    return e


def classify(e, c_rect):
    tol = lambda v: max(2.0, 0.05 * abs(v))
    if abs(e["w"]) > tol(c_rect[2]) or abs(e["h"]) > tol(c_rect[3]): return "SHAPE"
    if abs(e["ry"]) > 2.0: return "OWN PLACE"
    if abs(e["x0"]) > 2.0 or abs(e["y0"]) > 2.0: return "CARRIED"
    return "within tolerance"


def main():
    a = argparse.ArgumentParser(); a.add_argument("dump"); a.add_argument("--render"); o = a.parse_args()
    rows = {(r["page"], r["node"]): r for r in map(json.loads, open(o.dump))}
    fr = lambda r, k: (rows.get((r["page"], r["parent"])) or {}).get(k)
    exact = 0; n = 0
    for r in rows.values():
        for rk in ("rect_c", "rect_o"):
            c = coords(r[rk], fr(r, rk)); n += 1; exact += decode(encode(c)) == c
    print(f"T25 · P25 = {P25:,} (2^{math.log2(P25):.2f}, fits u128) · {len(rows)} boxes · {n} rectangles → one T25 point each → back exact: {exact}/{n}")
    cls = Counter(); under_bad = Counter(); same = 0; ex = {}
    for k, r in rows.items():
        if r["agree"]: continue
        co, cc = coords(r["rect_o"], fr(r, "rect_o")), coords(r["rect_c"], fr(r, "rect_c"))
        e = errors(encode(co), encode(cc))
        same += all(abs(e[x] - (co[x] - cc[x]) / U) < 1e-9 for x in e)
        c = classify(e, r["rect_c"]); cls[c] += 1
        p = rows.get((r["page"], r["parent"]))
        under_bad[c] += int(p is not None and not p["agree"])
        if c not in ex and r["tag"] not in ("html", "body") and max(abs(v) for v in e.values()) > 6: ex[c] = (r, co, cc, e)
    tot = sum(cls.values())
    print(f"failing boxes {tot} · errors read off the T25 angles == direct: {same}/{tot}")
    for c, v in cls.most_common():
        print(f"  {c:18s} {v:5d} ({v / tot:.0%}) · frame also failing {under_bad[c]}/{v} = {under_bad[c] / max(1, v):.0%}")
    if o.render:
        sys.path.insert(0, os.path.dirname(__file__)); import make_t15 as mk
        mk.ORDER = [m for g in ("x0", "y0", "w", "h", "ry") for m in G[g]]          # rings grouped by coordinate, centre out
        mk.R0, mk.DR, mk.CY = 52, 15.5, 455
        col = {"x0": ("#f5a623", "#7a5418"), "y0": ("#2ec4d6", "#1d6a74"), "w": ("#a78bfa", "#54467e"), "h": ("#4ade80", "#1f6a3c"), "ry": ("#f472b6", "#7a2a58")}
        grp = {m: g for g, ms in G.items() for m in ms}
        mk.tier = lambda m: grp[m]; mk.COL = col; mk.HEAD = "T²⁵ ⊃ T²⁰ ⊃ T¹⁵ ⊃ T¹⁰ ⊃ T⁵ · a layout box as one point"
        for c, (r, co, cc, e) in ex.items():
            Bo, Bc = encode(co), encode(cc)
            rows_html = "".join(f'<div style="color:{col[g][0]}">{g:3s} rings {{{",".join(map(str, G[g]))}}}: Δ {e[g]:+.2f}px</div>' for g in G)
            body = mk.rings([(Bc, 1.0, False), (Bo, 0.85, True)])
            side = f"""<div style="position:absolute;left:840px;top:84px;width:420px">
    <div style="color:#fff;font-weight:bold">T²⁵ · one layout box = one point · {c}</div>
    <div style="color:#9a9ab0">P25 = {P25:,} (u128)</div>
    <div style="margin-top:8px">{r['page'][:34]} &lt;{r['tag']}&gt; node {r['node']}</div>
    <div>Chrome {[round(v, 1) for v in r['rect_c']]}</div><div>ours {[round(v, 1) for v in r['rect_o']]}</div>
    <div style="margin-top:8px">{rows_html}</div>
    <div style="margin-top:8px;color:#9a9ab0">rings grouped by coordinate from the centre: x0 (amber) · y0 (cyan) · w (violet) · h (green) · ry = place in its frame (pink). Bright = Chrome, white = ours: white appears only on the rings that carry the error.</div></div>"""
            name = "t25-" + c.lower().replace(" ", "-")
            mk.write(o.render, name, mk.page(body + side, "Five coordinates per box on 25 prime rings: absolute place, size, and place inside its frame. The twist pattern tells which one is wrong."))
        print("rendered:", ", ".join(ex))


if __name__ == "__main__":
    main()
