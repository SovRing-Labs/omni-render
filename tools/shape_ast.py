#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_ast.py — box tree ↔ code AST, same parent-tree format (J 10-06, readback confirmed).
Each box is laid out by a code triangle: the PARENT's container branch (path.flow / flex / flex_column / grid / table / float …
recorded on the parent) — the edge — and the box's own terms (box.*, size.*, rule.*, flex.*, …). A root failure (failing box,
good parent) is scored against good boxes per code element with Ochiai (spectrum fault localisation, Abreu 2007), separately per
error LEG (Pythagoras: the corner error splits into a horizontal leg dx/dw and a vertical leg dy/dh; each leg indicts its own code).
Usage: shape_ast.py DUMP.jsonl [--top 15] [--expect key1,key2,…] (pre-registered check: where do the expected code elements rank?)"""
import argparse, json, math
from collections import Counter


def tol(c): return max(2.0, 0.05 * abs(c))


def main():
    a = argparse.ArgumentParser(); a.add_argument("dump"); a.add_argument("--top", type=int, default=15); a.add_argument("--expect", default="")
    o = a.parse_args()
    rows = {(r["page"], r["node"]): r for r in map(json.loads, open(o.dump))}
    def tri(r):
        p = rows.get((r["page"], r["parent"]))
        pc = [f"parent:{k}" for k in (p["code"] if p else []) if k.startswith("path.")]
        own = [f"self:{k}" for k in r["code"]]
        return set(pc + own + [f"edge:{r['pdisplay']}>{r['display']}"])
    good = [r for r in rows.values() if r["agree"]]
    roots = [r for r in rows.values() if not r["agree"] and (rows.get((r["page"], r["parent"])) or {}).get("agree")]
    def leg(r):
        d = [ro - rc for ro, rc in zip(r["rect_o"], r["rect_c"])]
        hz = math.hypot(d[0], d[2]); vt = math.hypot(d[1], d[3])
        return "horizontal" if hz >= vt else "vertical"
    passc = Counter(); [passc.update(tri(r)) for r in good]
    expect = [e for e in o.expect.split(",") if e]
    print(f"roots (failing box, good parent): {len(roots)} · good boxes {len(good)}")
    ranks = {}
    for L in ("horizontal", "vertical", "all"):
        rs = roots if L == "all" else [r for r in roots if leg(r) == L]
        failc = Counter(); [failc.update(tri(r)) for r in rs]
        F = len(rs)
        och = {e: failc[e] / math.sqrt(F * (failc[e] + passc[e])) for e in failc if F}
        top = sorted(och.items(), key=lambda x: -x[1])
        print(f"\n{L} leg: {F} roots — top code elements by Ochiai (fail / pass):")
        for e, s in top[:o.top]: print(f"  {s:.3f}  {e:40s} {failc[e]:4d} / {passc[e]:5d}")
        order = [e for e, _ in top]
        for e in expect: ranks.setdefault(e, {})[L] = (order.index(e) + 1) if e in order else None
    if expect:
        print("\nexpected code elements (fixes made later), rank per leg:")
        hits = 0
        for e, rk in ranks.items():
            best = min([v for v in rk.values() if v] or [999]); hits += best <= 5
            print(f"  {e:40s} {rk}  best {best}")
        print(f"PRE-REGISTERED: expected element in the top 5 for {hits}/{len(ranks)} (bar ≥ 3)")


if __name__ == "__main__":
    main()
