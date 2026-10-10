#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""t25_families.py — the doctor's FIX QUEUE: failing roots grouped by their triangle-twist key on T25 (J 10-06, triangles placed).

The key is the shape of the fault, not its size: the display pair (frame, box) + one trit per coordinate (x0, y0, w, h) of the
triangle twist ΔB − ΔP (box error minus frame error), 0 inside the agreement tolerance, ±1 past it. Families come out largest
first — the order the doctor works them (oracle → fix → gate). With --old, validates on a pre-fix dump: the roots a later fix
repaired should fall into few families (theguardian 26 repaired roots → 3 families on the 10-06 run).
Usage: t25_families.py DUMP [--old PRE_FIX_DUMP] [--top 12] [--page SUBSTR]"""
import argparse, collections, os, sys
sys.path.insert(0, os.path.dirname(__file__)); import t25_ifs as T


def tri_key(rows, r):
    P = rows.get((r['page'], r['parent'])); eB = T.twist(rows, r); eP = T.twist(rows, P)
    rel = {g: eB[g] - eP[g] for g in ('x0', 'y0', 'w', 'h')}
    c = r['rect_c']; cc = {'x0': c[0], 'y0': c[1], 'w': c[2], 'h': c[3]}
    tol = lambda g: max(2.0, 0.05 * abs(cc[g])) if g in ('w', 'h') else 2.0
    return (r['pdisplay'], r['display']) + tuple(0 if abs(rel[g]) <= tol(g) else (1 if rel[g] > 0 else -1) for g in ('x0', 'y0', 'w', 'h'))


def families(rows):
    fam = collections.defaultdict(list)
    for r in T.roots(rows): fam[tri_key(rows, r)].append(r)
    return fam


def main():
    a = argparse.ArgumentParser(); a.add_argument('dump'); a.add_argument('--old'); a.add_argument('--top', type=int, default=12)
    a.add_argument('--page', default='')
    o = a.parse_args()
    rows = T.load(o.dump); fam = families(rows)
    if o.page: fam = {k: [r for r in v if o.page in r['page']] for k, v in fam.items()}; fam = {k: v for k, v in fam.items() if v}
    n = sum(len(v) for v in fam.values())
    print(f"{n} roots → {len(fam)} triangle-twist families (display pair, Δx0 Δy0 Δw Δh trits)")
    for k, v in sorted(fam.items(), key=lambda x: -len(x[1]))[:o.top]:
        ex = v[0]
        print(f"   {len(v):3d} {k} {collections.Counter(r['page'][:16] for r in v).most_common(3)} · e.g. {ex['page'][:24]} node {ex['node']} <{ex.get('tag','')}.{(ex.get('class') or '')[:30]}>")
    if o.old:
        old = T.load(o.old); fo = families(old)
        of = {(r['page'], r['node']): k for k, v in fo.items() for r in v}
        fixed = [k for k in of if rows.get(k, {}).get('agree')]
        bp = collections.defaultdict(list)
        for k in fixed: bp[k[0]].append(of[k])
        print(f"pre-fix: {len(of)} roots → {len(fo)} families; repaired since: {len(fixed)}")
        for pg, fs in sorted(bp.items(), key=lambda x: -len(x[1]))[:8]:
            c = collections.Counter(fs)
            print(f"   {pg[:28]:28s} {len(fs):3d} repaired roots in {len(c)} families · largest {c.most_common(1)[0][1]} {c.most_common(1)[0][0]}")


if __name__ == '__main__':
    main()
