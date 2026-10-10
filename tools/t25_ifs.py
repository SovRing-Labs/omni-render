#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""t25_ifs.py — the doctor's reverse IFS + sift on T25 points, and the triangles on T25 (J 10-06: "run it, then see how the
triangles fit into this, I've a feeling we found their place").

PART 1 · reverse IFS on T25. Every failing box is the pair (ours, Chrome) of T25 points; its TWIST = the per-ring residue gaps
(25 angles) — five exact coordinate errors (x0, y0, w, h, ry), decoded by CRT. SIEVE = exact lookup: boxes whose twists are
identical (same error vector, 1/64 px) are one family. SIFT = soft read: the 25 angles as a unit phasor vector, cosine ≥ θ merges
near-equal twists (Δh 20 vs 21). Validation on the pre-fix dump: the roots a later fix repaired should fall into few families.

PART 2 · triangles on T25. A triangle is three points: the box B, its frame P, its previous sibling S. On every ring the triangle's
shape is the pair of residue differences (B − P, B − S) — the box's place relative to its anchors, exact. Ours vs Chrome: the
triangle's twist = ΔB − ΔP (and ΔB − ΔS). If it is zero on all rings the box sits exactly where its anchors say (carried);
otherwise the group that twists names the coordinate that is the box's own fault. Translation invariance is the subtraction of
the anchor point — on the torus it is free.
Usage: t25_ifs.py DUMP [--old OLD_DUMP --new NEW_DUMP] [--theta 0.98]"""
import argparse, json, math, sys, time, os
from collections import Counter, defaultdict
sys.path.insert(0, os.path.dirname(__file__)); import box_t25 as bt


def load(p): return {(r["page"], r["node"]): r for r in map(json.loads, open(p))}


def point(rows, r, k):
    p = rows.get((r["page"], r["parent"]))
    return bt.encode(bt.coords(r[k], p[k] if p else None))


def twist(rows, r):
    return bt.errors(point(rows, r, "rect_o"), point(rows, r, "rect_c"))


def angles(rows, r):
    Bo, Bc = point(rows, r, "rect_o"), point(rows, r, "rect_c")
    return [2 * math.pi * ((Bo - Bc) % m) / m for g in ("x0", "y0", "w", "h", "ry") for m in bt.G[g]]


def roots(rows):
    return [r for r in rows.values() if not r["agree"] and (rows.get((r["page"], r["parent"])) or {}).get("agree")]


def families(rows, rs, theta):
    t0 = time.time()
    exact = defaultdict(list)
    for r in rs:
        e = twist(rows, r); exact[tuple(round(e[g] * 64) for g in ("x0", "y0", "w", "h", "ry"))].append(r)
    t_sieve = time.time() - t0
    # sift: greedy leader clustering of the exact families by cosine of their 25-angle phasor vectors
    t1 = time.time()
    keys = list(exact)
    vec = {k: angles(rows, exact[k][0]) for k in keys}
    def cos(a, b): return sum(math.cos(x - y) for x, y in zip(a, b)) / len(a)
    leaders = []; fam = {}
    for k in sorted(keys, key=lambda k: -len(exact[k])):
        for L in leaders:
            if cos(vec[k], vec[L]) >= theta: fam[k] = L; break
        else: leaders.append(k); fam[k] = k
    t_sift = time.time() - t1
    merged = defaultdict(list)
    for k, rs_ in exact.items(): merged[fam[k]] += rs_
    return exact, merged, t_sieve, t_sift


def main():
    a = argparse.ArgumentParser(); a.add_argument("dump"); a.add_argument("--old"); a.add_argument("--new"); a.add_argument("--theta", type=float, default=0.98)
    o = a.parse_args()
    rows = load(o.dump); rs = roots(rows)
    exact, merged, ts, tf = families(rows, rs, o.theta)
    print(f"PART 1 · {len(rs)} roots → SIEVE {len(exact)} exact twist families ({ts * 1000:.1f} ms) → SIFT θ {o.theta}: {len(merged)} families ({tf * 1000:.1f} ms)")
    for k, v in sorted(merged.items(), key=lambda x: -len(x[1]))[:10]:
        pages = Counter(r["page"][:18] for r in v).most_common(2)
        print(f"   {len(v):3d} boxes · twist (x0,y0,w,h,ry)/64px {tuple(round(x / 64, 1) for x in k)} · {pages}")
    if o.old and o.new:
        old, new = load(o.old), load(o.new)
        ors = roots(old)
        _, m_old, _, _ = families(old, ors, o.theta)
        fam_of = {(r["page"], r["node"]): k for k, v in m_old.items() for r in v}
        fixed = [(r["page"], r["node"]) for r in ors if new.get((r["page"], r["node"]), {}).get("agree")]
        by_page = defaultdict(list)
        for k in fixed: by_page[k[0]].append(fam_of[k])
        print(f"   validation (pre-fix dump): {len(ors)} roots in {len(m_old)} families; {len(fixed)} roots a later fix repaired")
        for pg, fs in sorted(by_page.items(), key=lambda x: -len(x[1]))[:8]:
            c = Counter(fs); print(f"     {pg[:28]:28s} {len(fs):3d} repaired roots in {len(c)} families · largest family holds {c.most_common(1)[0][1]}")
    # PART 2 — triangles on T25
    fails = [r for r in rows.values() if not r["agree"]]
    cls = Counter(); own_groups = Counter(); n = 0
    for r in fails:
        P = rows.get((r["page"], r["parent"]))
        if not P: continue
        S = rows.get((r["page"], r["prev"]))
        eB = twist(rows, r); eP = twist(rows, P)
        relP = {g: eB[g] - eP[g] for g in ("x0", "y0", "w", "h")}          # the triangle's twist against the frame vertex
        relS = {g: eB[g] - twist(rows, S)[g] for g in ("x0", "y0")} if S else None
        tol = lambda g, c: max(2.0, 0.05 * abs(c)) if g in ("w", "h") else 2.0
        c_ = r["rect_c"]; cc = {"x0": c_[0], "y0": c_[1], "w": c_[2], "h": c_[3]}
        bad = [g for g in relP if abs(relP[g]) > tol(g, cc[g])]
        if S and relS is not None and "y0" in bad and abs(relS["y0"]) <= 2.0: bad.remove("y0")   # its sibling vertex explains the drop
        n += 1
        if not bad: cls["CARRIED — the triangle (frame, sibling, box) keeps its shape"] += 1
        else: cls["OWN — the triangle deforms"] += 1; own_groups[tuple(bad)] += 1
    print(f"\nPART 2 · triangles on T25 over {n} failing boxes (vertices: box, frame, previous sibling):")
    for k, v in cls.most_common(): print(f"   {v:5d} {k}")
    print("   the coordinates that deform the triangle:", own_groups.most_common(8))


if __name__ == "__main__":
    main()
