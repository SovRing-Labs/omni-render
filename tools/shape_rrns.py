#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_rrns.py — SOFT-RRNS triangulation (DOCTOR-KB-MINING #1; L7 soft evidence, RING-10 soft RRNS decoding).
The five anchored triangles (shape_tri.py v3) are five redundant VIEWS of one box, each reading it from a different reference:
  △ frame width + top-after-previous · ◸ previous sibling · ◹ next sibling · ◺ Chrome's target frame bottom · ⊿ the frame's own bottom.
A broken reference spoils exactly the views that use it (its SYNDROME). Each source hypothesis H predicts a misfit set M(H);
the decode sums per-view log-likelihoods (fit: residual ~ N(0, σ) with σ = the feathered tolerance; misfit: a broad
half-normal of width σ_m) and normalises over H (softmax) — the max-likelihood soft decode of a redundant residue code, which
CORRECTS a single bad view instead of only detecting it. Commit only when the posterior margin clears --margin (L17), else abstain.
Verify (no Chrome needed beyond the dump): a named reference must itself be broken more often than chance —
  prev → the previous sibling fails; next → the next sibling fails; frame bottom → another child of the frame fails in height.
Usage: shape_rrns.py TRI.jsonl DUMP.jsonl [--sigma 2] [--sigma-miss 60] [--margin 0.5]"""
import argparse, json, math
from collections import Counter, defaultdict

V = ["△", "◸", "◹", "◺", "⊿"]
HYP = {  # source hypothesis → views it spoils (syndrome)
    "carried (fits every view)": set(),
    "the box itself": set(V),
    "previous sibling": {"△", "◸"},
    "next sibling": {"◹"},
    "frame's own bottom (height from elsewhere)": {"⊿"},
    "frame vs Chrome's target height": {"◺"},
    "from above (frame top / previous)": {"△", "◸", "◹"},
    "from below (frame bottom)": {"◺", "⊿"},
}


def decode(view, sigma, sigma_m):
    def ll_fit(r): return -0.5 * (r / sigma) ** 2 - math.log(sigma)
    def ll_miss(r): return -0.5 * (r / sigma_m) ** 2 - math.log(sigma_m)
    sc = {h: sum(ll_miss(view[v]) if v in m else ll_fit(view[v]) for v in V if v in view) for h, m in HYP.items()}
    mx = max(sc.values()); z = sum(math.exp(x - mx) for x in sc.values())
    post = {h: math.exp(x - mx) / z for h, x in sc.items()}
    top = sorted(post.items(), key=lambda x: -x[1])
    return top[0][0], top[0][1], top[0][1] - top[1][1], post


def main():
    a = argparse.ArgumentParser(); a.add_argument("tri"); a.add_argument("dump")
    a.add_argument("--sigma", type=float, default=2.0); a.add_argument("--sigma-miss", type=float, default=60.0)
    a.add_argument("--margin", type=float, default=0.5); a.add_argument("--out")
    o = a.parse_args()
    rows = {(r["page"], r["node"]): r for r in map(json.loads, open(o.dump))}
    kids = defaultdict(list)
    for r in rows.values(): kids[(r["page"], r["parent"])].append(r["node"])
    nxt = {(r["page"], r["prev"]): r["node"] for r in rows.values() if r["prev"] >= 0}
    tri = [t for t in map(json.loads, open(o.tri)) if not t["agree"] and "view" in t]
    def failing(page, node): r = rows.get((page, node)); return r is not None and not r["agree"]
    def h_failing(page, node):
        r = rows.get((page, node));
        if r is None or r["agree"]: return False
        c, ou = r["rect_c"][3], r["rect_o"][3]; return abs(ou - c) > max(2, 0.05 * abs(c))
    out = []; named = Counter(); hits = Counter(); base = Counter(); by_cls = defaultdict(Counter)
    for t in tri:
        h, p, m, post = decode(t["view"], o.sigma, o.sigma_miss)
        commit = m >= o.margin
        r = rows[(t["page"], t["node"])]
        key = h if commit else "ABSTAIN"
        by_cls[t["cls"]][key] += 1
        if commit:
            named[h] += 1
            pg = t["page"]
            # does the named reference itself look broken?
            if h == "previous sibling": ok = r["prev"] >= 0 and failing(pg, r["prev"])
            elif h == "next sibling": nn = nxt.get((pg, t["node"])); ok = nn is not None and failing(pg, nn)
            elif h.startswith("frame's own bottom"): ok = any(h_failing(pg, k) for k in kids[(pg, r["parent"])] if k != t["node"])
            else: ok = None
            if ok is not None: hits[h] += int(ok); base[h] += 1
        out.append({**{k: t[k] for k in ("page", "node", "tag", "cls", "tri")}, "source": key, "p": round(p, 3), "margin": round(m, 3)})
    n = len(tri)
    print(f"soft-RRNS triangulation: {n} failing boxes · σ {o.sigma}px σ_miss {o.sigma_miss}px · margin {o.margin}")
    for cls in ("SPLIT", "DEFECT", "FIT"):
        c = by_cls[cls]; tot = sum(c.values())
        if tot: print(f"  {cls:6s} {tot:5d} → committed {tot - c['ABSTAIN']:5d} ({(tot - c['ABSTAIN']) / tot:.0%}) · " + " · ".join(f"{k} {v}" for k, v in c.most_common(6)))
    # VERIFY each named source by the one measurement it predicts (Chrome's dump), against the same measurement on all failing boxes
    def tol(c): return max(2.0, 0.05 * abs(c))
    def gap_err(t):
        r = rows[(t["page"], t["node"])]; q = rows.get((t["page"], r["prev"]))
        if q is None: return None
        go = r["rect_o"][1] - (q["rect_o"][1] + q["rect_o"][3]); gc = r["rect_c"][1] - (q["rect_c"][1] + q["rect_c"][3]); return abs(go - gc) > 2.0
    def next_gap_err(t):
        r = rows[(t["page"], t["node"])]; nn = nxt.get((t["page"], t["node"])); q = rows.get((t["page"], nn)) if nn is not None else None
        if q is None: return None
        go = q["rect_o"][1] - (r["rect_o"][1] + r["rect_o"][3]); gc = q["rect_c"][1] - (r["rect_c"][1] + r["rect_c"][3]); return abs(go - gc) > 2.0
    def parent_h_err(t):
        r = rows[(t["page"], t["node"])]; q = rows.get((t["page"], r["parent"]))
        if q is None: return None
        return abs(q["rect_o"][3] - q["rect_c"][3]) > tol(q["rect_c"][3])
    def own_size_err(t):
        r = rows[(t["page"], t["node"])]; return abs(r["rect_o"][2] - r["rect_c"][2]) > tol(r["rect_c"][2]) or abs(r["rect_o"][3] - r["rect_c"][3]) > tol(r["rect_c"][3])
    def own_size_ok(t): return not own_size_err(t)
    probe = {"previous sibling": ("gap after the previous sibling differs", gap_err),
             "next sibling": ("gap before the next sibling differs", next_gap_err),
             "frame's own bottom (height from elsewhere)": ("frame height differs", parent_h_err),
             "frame vs Chrome's target height": ("frame height differs", parent_h_err),
             "from below (frame bottom)": ("frame height differs", parent_h_err),
             "from above (frame top / previous)": ("gap after the previous sibling differs", gap_err),
             "the box itself": ("own width or height differs", own_size_err),
             "carried (fits every view)": ("own size right (only displaced)", own_size_ok)}
    dec = {(x["page"], x["node"]): x["source"] for x in out}
    print("  VERIFY — the measurement each named source predicts, named boxes vs all failing boxes:")
    for h, (what, f) in probe.items():
        sel = [t for t in tri if dec[(t["page"], t["node"])] == h]
        a = [f(t) for t in sel]; a = [x for x in a if x is not None]
        b = [f(t) for t in tri]; b = [x for x in b if x is not None]
        if a: print(f"    {h:44s} n {len(sel):5d} · {what}: {sum(a) / len(a):.0%} (all failing {sum(b) / max(1, len(b)):.0%})")
    if o.out:
        with open(o.out, "w") as f:
            for x in out: f.write(json.dumps(x) + "\n")


if __name__ == "__main__":
    main()
