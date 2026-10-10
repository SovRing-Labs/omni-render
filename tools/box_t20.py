#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""box_t20.py — map layout boxes EXACTLY onto the T20 torus (J 10-06: "map the points on the chrome boxes mathematically onto the 20t").

The 20 primes of T20 split into 4 groups of 5 (greedy balance, each group's product ≥ 7.2e6):
  x0 ↦ {11,13,41,43,73} · y0 ↦ {5,17,37,47,71} · x1 ↦ {7,19,31,53,67} · y1 ↦ {3,23,29,59,61}
A box corner coordinate is an exact integer in 1/64 px (Chrome's LayoutUnit), offset by 16,384 px so off-screen boxes stay
positive; it is written as its residues on its group's 5 rings. The 20 residues together are ONE integer B < P20 (CRT) — the
box as a single point on T20 — and B decodes back to the same box exactly.
Comparing ours with Chrome is comparing two points: on every ring the residue difference δ = (r_ours − r_chrome) mod m is an
ANGLE 2πδ/m, and the CRT over a group's 5 angles recovers that coordinate's error exactly (signed, in 1/64 px).
  translation (shape right, displaced):  Δx0 = Δx1 and Δy0 = Δy1  — both corners twist by the same amount
  deformation (the shape is wrong):      the corners twist differently (Δw = Δx1 − Δx0, Δh = Δy1 − Δy0 ≠ 0)
Usage: box_t20.py DUMP.jsonl [--render OUTDIR]   (DUMP = `core corpus --abc shape --dump`: rect_o / rect_c per box)"""
import argparse, json, math, os, sys
from collections import Counter

T20 = [3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73]
G = {"x0": [11, 13, 41, 43, 73], "y0": [5, 17, 37, 47, 71], "x1": [7, 19, 31, 53, 67], "y1": [3, 23, 29, 59, 61]}
P20 = math.prod(T20); OFF = 16384 * 64; U = 64
COEF = {m: (P20 // m) * pow(P20 // m % m, -1, m) % P20 for m in T20}


def crt(res, mods):
    Pm = math.prod(mods); return sum(r * (Pm // m) * pow(Pm // m % m, -1, m) for r, m in zip(res, mods)) % Pm


def coords(rect):
    x, y, w, h = rect
    return {"x0": round(x * U) + OFF, "y0": round(y * U) + OFF, "x1": round((x + w) * U) + OFF, "y1": round((y + h) * U) + OFF}


def encode(rect):
    c = coords(rect)
    return sum((c[k] % m) * COEF[m] for k, ms in G.items() for m in ms) % P20


def decode(B):
    v = {k: crt([B % m for m in ms], ms) for k, ms in G.items()}
    x0, y0, x1, y1 = ((v[k] - OFF) / U for k in ("x0", "y0", "x1", "y1"))
    return [x0, y0, x1 - x0, y1 - y0]


def twist(Bo, Bc):
    """per ring the angle of ours relative to Chrome (signed fraction of a turn), and the exact error per coordinate from the angles"""
    ang = {m: ((Bo % m - Bc % m + m // 2) % m - m // 2) / m for m in T20}
    err = {}
    for k, ms in G.items():
        Pm = math.prod(ms); d = crt([(Bo - Bc) % m for m in ms], ms)
        err[k] = (d - Pm if d > Pm // 2 else d) / U
    return ang, err


def main():
    a = argparse.ArgumentParser(); a.add_argument("dump"); a.add_argument("--render"); o = a.parse_args()
    rows = [json.loads(l) for l in open(o.dump)]
    # 1. exactness: every Chrome box and every one of ours → one T20 point → back
    exact = sum(1 for r in rows for rect in (r["rect_c"], r["rect_o"]) if all(abs(p - q) < 1e-9 for p, q in zip(decode(encode(rect)), [round(v * U) / U for v in rect])))
    print(f"boxes {len(rows)} · {2 * len(rows)} rectangles → T20 points → back: exact {exact}/{2 * len(rows)} (in 1/64 px)")
    # 2. the error read off the torus angles == the direct error, and the translation / deformation split
    tol = lambda c: max(2.0, 0.05 * abs(c))
    cls = Counter(); same = 0; fails = [r for r in rows if not r["agree"]]
    for r in fails:
        Bo, Bc = encode(r["rect_o"]), encode(r["rect_c"])
        _, e = twist(Bo, Bc)
        ro, c = r["rect_o"], r["rect_c"]
        direct = {"x0": ro[0] - c[0], "y0": ro[1] - c[1], "x1": ro[0] + ro[2] - c[0] - c[2], "y1": ro[1] + ro[3] - c[1] - c[3]}
        same += all(abs(e[k] - round(direct[k] * U) / U) < 1.0 / U + 1e-9 for k in e)
        dw, dh = e["x1"] - e["x0"], e["y1"] - e["y0"]
        cls["translation (same twist on both corners)" if abs(dw) <= tol(c[2]) and abs(dh) <= tol(c[3]) else "deformation (corners twist differently)"] += 1
    print(f"failing boxes {len(fails)} · error recovered from the T20 angles == direct error: {same}/{len(fails)}")
    for k, v in cls.most_common(): print(f"  {v:5d}  {k}")
    if o.render:
        sys.path.insert(0, os.path.dirname(__file__)); import make_t15 as mk
        pick = {"translation": None, "deformation": None}
        for r in fails:
            _, e = twist(encode(r["rect_o"]), encode(r["rect_c"]))
            kind = "translation" if abs(e["x1"] - e["x0"]) <= tol(r["rect_c"][2]) and abs(e["y1"] - e["y0"]) <= tol(r["rect_c"][3]) else "deformation"
            if pick[kind] is None and r["tag"] not in ("html", "body") and max(abs(v) for v in e.values()) > 8: pick[kind] = (r, e)
        for kind, (r, e) in pick.items():
            Bo, Bc = encode(r["rect_o"]), encode(r["rect_c"])
            grp = " · ".join(f"{k} rings {{{','.join(map(str, ms))}}}: Δ {e[k]:+.2f}px" for k, ms in G.items())
            extra = f"""<div style="margin-top:10px;color:#fff;font-weight:bold">{kind}: {r['page'][:30]} &lt;{r['tag']}&gt; node {r['node']}</div>
    <div>Chrome box {[round(v, 2) for v in r['rect_c']]} → B = {Bc:,}</div>
    <div>ours {[round(v, 2) for v in r['rect_o']]} → B = {Bo:,}</div>
    <div style="color:#9a9ab0">{grp}</div>
    <div style="color:#9a9ab0">bright = Chrome's point, white = ours; each ring's angle between them is that ring's share of the error</div>"""
            mk.write(o.render, f"box-{kind}", mk.page(mk.rings([(Bc, 1.0, False), (Bo, 0.85, True)]) + mk.panel(Bc, f"a layout box as one T²⁰ point ({kind})", extra),
                     "Corners x0 · y0 · x1 · y1 → 4 groups of 5 prime rings → one CRT integer B < P20. Ours vs Chrome = two points; per ring the residue gap is an angle, and the angles give back the exact error."))
        print("rendered:", ", ".join(f"box-{k}" for k in pick))


if __name__ == "__main__":
    main()
