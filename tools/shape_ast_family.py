#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_ast_family.py — box triangle ↔ code triangle at STATEMENT level, ranked inside one geometric family (J 10-06).
Each box carries its source lines (cov(line!()) markers) plus path.* branches and terms; its code triangle is the union of
self:, parent: and gparent: elements (the code that ran for the box, for the container that laid it out, and one level up).
Within a family (the boxes a fix later repaired, i.e. what the agent had selected), Ochiai ranks elements against GOOD boxes
on the same page with the same frame display (matched controls).
PRE-REGISTERED: the line enclosing the later fix ranks top 3 (any of self/parent/gparent) for ≥ 3 of the 4 testable fixes.
Usage: shape_ast_family.py OLD_DUMP NEW_DUMP EXPECTED.json"""
import json, math, sys
from collections import Counter

old = {(r["page"], r["node"]): r for r in map(json.loads, open(sys.argv[1]))}
new = {(r["page"], r["node"]): r for r in map(json.loads, open(sys.argv[2]))}
exp = json.load(open(sys.argv[3]))
pages = {"flex_col_basis": "theguardian-com-international", "anon_cell": "en-cppreference-com-w-cpp-container-vector",
         "line_clamp": "allrecipes-com-recipe-10813-best-chocolate-chip-cookies", "track_math": "blog-codinghorror-com"}


def els(r):
    s = set()
    for pre, b in (("self", r), ("parent", old.get((r["page"], r["parent"]))), ):
        if b: s |= {f"{pre}:{c}" for c in b["code"]}
    p = old.get((r["page"], r["parent"]))
    g = old.get((r["page"], p["parent"])) if p else None
    if g: s |= {f"gparent:{c}" for c in g["code"]}
    return s


hits = 0; tested = 0
for fix, page in pages.items():
    lines = exp.get(fix) or []
    if not lines: print(f"{fix}: no enclosing marker line — not testable"); continue
    tested += 1
    fam = [r for k, r in old.items() if k[0] == page and not r["agree"] and new.get(k, {}).get("agree")]
    pds = Counter(r["pdisplay"] for r in fam)
    ctrl = [r for k, r in old.items() if k[0] == page and r["agree"] and r["pdisplay"] in pds]
    F = len(fam)
    fc = Counter(); [fc.update(els(r)) for r in fam]
    pc = Counter(); [pc.update(els(r)) for r in ctrl]
    och = sorted(((e, fc[e] / math.sqrt(F * (fc[e] + pc[e]))) for e in fc), key=lambda x: -x[1])
    order = [e for e, _ in och]
    want = {f"{pre}:L{l}" for l in lines for pre in ("self", "parent", "gparent")}
    rank = min([order.index(w) + 1 for w in want if w in order] or [None], key=lambda v: v or 10**9)
    hit = rank is not None and rank <= 3; hits += hit
    print(f"\n{fix} on {page[:28]}: family {F} boxes (frames {dict(pds.most_common(3))}) vs {len(ctrl)} good controls · fix line(s) {lines} → best rank {rank} {'HIT' if hit else ''}")
    for e, s in och[:6]: print(f"   {s:.3f} {e:30s} {fc[e]:4d}/{pc[e]:5d}")
print(f"\nPRE-REGISTERED: fix line in the family top 3 for {hits}/{tested} (bar ≥ 3 of 4) → {'PASS' if hits >= 3 else 'FAIL'}")
