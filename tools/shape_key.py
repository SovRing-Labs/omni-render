#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_key.py — REVERSE IFS, exact key first (DOCTOR-KB-MINING #2; CODE-ABC: exact hash 0.98 vs polygon 0.77 when the pieces
are exact; MATH-INDEX #141 exact lookups ≫ similarity). Domain = the boxes soft-RRNS triangulation names as "the box itself".
Each box's fracture in its frame, as TRITS (−1 / 0 / +1 past τ): width share, x offset share, height, gap after the previous
sibling — ours vs Chrome, in the frame's own scale (shift- and scale-free, like the angles). Keys, coarse → exact:
  K1 = (frame display, display) · K2 = K1 + the trit of the query's dominant size axis · K3 = K1 + all four trits.
The range (one faulty box) retrieves every domain box with the same key = every place the faulty shape repeats.
--truth PRE.deformed POST.deformed scores against the boxes a known fix repaired."""
import argparse, json
from collections import Counter

AX = ["w", "x", "h", "y"]


def fracture(r, rows):
    p = rows.get((r["page"], r["parent"]))
    if p is None: return None
    po, pc, o, c = p["rect_o"], p["rect_c"], r["rect_o"], r["rect_c"]
    wpo, wpc = max(po[2], 1.0), max(pc[2], 1.0)
    e_w = (o[2] / wpo - c[2] / wpc) * wpc
    e_x = ((o[0] - po[0]) / wpo - (c[0] - pc[0]) / wpc) * wpc
    e_h = o[3] - c[3]
    q = rows.get((r["page"], r["prev"]))
    if q is not None: e_y = (o[1] - (q["rect_o"][1] + q["rect_o"][3])) - (c[1] - (q["rect_c"][1] + q["rect_c"][3]))
    else: e_y = (o[1] - po[1]) - (c[1] - pc[1])
    tol = [max(2.0, 0.05 * abs(c[2])), 2.0, max(2.0, 0.05 * abs(c[3])), 2.0]
    e = [e_w, e_x, e_h, e_y]
    return e, tuple(0 if abs(v) <= t else (1 if v > 0 else -1) for v, t in zip(e, tol))


def main():
    a = argparse.ArgumentParser(); a.add_argument("dump"); a.add_argument("rrns"); a.add_argument("--query", required=True, nargs="+")
    a.add_argument("--truth", nargs=2); a.add_argument("--all-domain", action="store_true"); a.add_argument("--out")
    o = a.parse_args()
    rows = {(r["page"], r["node"]): r for r in map(json.loads, open(o.dump))}
    src = {(x["page"], x["node"]): x["source"] for x in map(json.loads, open(o.rrns))}
    dom = [k for k, s in src.items() if o.all_domain or s == "the box itself"]
    feat = {}
    for k in dom:
        f = fracture(rows[k], rows)
        if f: feat[k] = f
    F = set()
    if o.truth:
        pre = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[0])) if r["mu"] >= 0.5}
        post = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[1])) if r["mu"] >= 0.5}
        F = {k for k in pre - post if rows.get(k, {}).get("pdisplay") == "Grid"}
    print(f"domain: {len(feat)} boxes named 'the box itself' (soft-RRNS) · truth (grid items the fix repaired): {len(F)} · in domain {len(F & set(feat))}")
    res = []
    for q in o.query:
        page, node = q.rsplit(":", 1); kq = (page, int(node))
        if kq not in feat: print(f"  {q}: not in the domain ({src.get(kq)})"); continue
        e, t = feat[kq]; r = rows[kq]
        dom_ax = max((0, 2), key=lambda i: abs(e[i]))
        k1 = lambda k: (rows[k]["pdisplay"], rows[k]["display"])
        # CAUSE ORDER (width flows down, height follows content): the key keeps the earliest turned axis of the dataflow —
        # width if it is cracked, else x, else height, else gap — and the FRAME it sits in; own display and the axes downstream
        # of the cause are consequences, not the fault
        cause = next((i for i in (0, 1, 2, 3) if t[i] != 0), 0)
        keys = {f"KC frame + cause ({AX[cause]} trit)": lambda k: rows[k]["pdisplay"] == r["pdisplay"] and feat[k][1][cause] == t[cause],
                "K1 frame+display": lambda k: k1(k) == k1(kq),
                f"K2 +{AX[dom_ax]} trit": lambda k: k1(k) == k1(kq) and feat[k][1][dom_ax] == t[dom_ax],
                "K3 +all trits": lambda k: k1(k) == k1(kq) and feat[k][1] == t}
        print(f"  range {q} <{r['tag']}> {r['display']}<{r['pdisplay']} fracture w {e[0]:+.1f} x {e[1]:+.1f} h {e[2]:+.1f} y {e[3]:+.1f} → trits {t}")
        for name, f in keys.items():
            S = {k for k in feat if f(k)}; tp = len(S & F)
            pages = Counter(k[0][:18] for k in S).most_common(4)
            line = f"    {name:16s} repeats {len(S):5d}"
            if F: line += f" · truth recall {tp}/{len(F)} = {tp / max(1, len(F)):.3f} · precision {tp / max(1, len(S)):.3f}"
            print(line + f" · pages {pages}")
            res.append({"query": q, "key": name, "n": len(S), "tp": tp, "F": len(F)})
    if o.out: json.dump(res, open(o.out, "w"), indent=1)


if __name__ == "__main__":
    main()
