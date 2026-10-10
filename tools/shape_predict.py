#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_predict.py — approximate the shape of the failing regions from the angularity of KNOWN-GOOD points
(J 10-06: "approximate the shape based on the angularity of known good points and in that way scan and fix the rest";
"the Pythagorean theorem comes to mind").

Known-good points = boxes that agree with Chrome. For every failing box whose FRAME (parent) is good, the expected shape is
read off its peers — good boxes of the same structural key (tag, class, display, frame display, frame tag/class; backing off to
coarser keys) — as their angles in their own good frames: width share w/wp (or the absolute width when that is tighter), offset
share, gap after a good previous sibling, height. Per axis the tighter of share / absolute wins (lower dispersion = the angle the
peers actually hold). The predicted corners are the right-angle legs (x-leg, y-leg) from the good anchors; the error to Chrome is
the hypotenuse √(dx² + dy²) of each corner — Pythagoras.

PRE-REGISTERED BAR (before the run): on eligible failing boxes (good frame, ≥ 3 peers) — predicted box within Chrome's tolerance
on ≥ 50 % (our engine is 0 % there by definition) AND median corner hypotenuse below our engine's.
Chrome is read ONLY to score, never to predict. Usage: shape_predict.py DUMP.jsonl [--min-peers 3] [--out F]"""
import argparse, json, math, statistics as st
from collections import defaultdict, Counter


def tol(c): return max(2.0, 0.05 * abs(c))


def main():
    a = argparse.ArgumentParser(); a.add_argument("dump"); a.add_argument("--min-peers", type=int, default=3); a.add_argument("--out")
    o = a.parse_args()
    rows = {(r["page"], r["node"]): r for r in map(json.loads, open(o.dump))}
    def parent(r): return rows.get((r["page"], r["parent"]))
    def prev(r): return rows.get((r["page"], r["prev"]))
    def keys(r):
        p = parent(r); pt = p["tag"] if p else "-"; pc = p.get("class", "") if p else ""
        return [("K1", r["tag"], r.get("class", ""), r["display"], r["pdisplay"], pt, pc),
                ("K2", r["tag"], r.get("class", ""), r["display"], r["pdisplay"]),
                ("K3", r["tag"], r["display"], r["pdisplay"], pt)]
    # learn: relations of good boxes in good frames (their own rects; good = agrees with Chrome)
    rel = defaultdict(lambda: defaultdict(list))
    for r in rows.values():
        if not r["agree"]: continue
        p = parent(r)
        if not p or not p["agree"]: continue
        x, y, w, h = r["rect_o"]; xp, yp, wp, hp = p["rect_o"]
        q = prev(r)
        gap = (y - (q["rect_o"][1] + q["rect_o"][3])) if q and q["agree"] else None
        for k in keys(r):
            R = rel[k]
            R["ws"].append(w / max(wp, 1)); R["wa"].append(w); R["xs"].append((x - xp) / max(wp, 1)); R["xa"].append(x - xp)
            R["h"].append(h); R["top"].append(y - yp)
            if gap is not None: R["gap"].append(gap)
            R["n"].append(1)
    def pick(R, share, absl, base):
        # the tighter relation (lower relative spread) is the angle the peers hold
        if not R[share] and not R[absl]: return None
        ms, ma = st.median(R[share]), st.median(R[absl])
        ds = st.pstdev(R[share]) * base if len(R[share]) > 1 else 0.0
        da = st.pstdev(R[absl]) if len(R[absl]) > 1 else 0.0
        return ms * base if ds <= da else ma
    out = []; elig = 0; skipped = Counter()
    for k0, r in rows.items():
        if r["agree"]: continue
        p = parent(r)
        if not p or not p["agree"]: skipped["frame not good"] += 1; continue
        R = None; lvl = None
        for k in keys(r):
            if len(rel[k]["n"]) >= o.min_peers: R = rel[k]; lvl = k[0]; break
        if R is None: skipped["no peers"] += 1; continue
        elig += 1
        xp, yp, wp, hp = p["rect_o"]
        w = pick(R, "ws", "wa", wp); xo = pick(R, "xs", "xa", wp)
        q = prev(r)
        if q and q["agree"] and R["gap"]: y = q["rect_o"][1] + q["rect_o"][3] + st.median(R["gap"])
        elif not q and R["top"]: y = yp + st.median(R["top"])
        else: y = None
        h = st.median(R["h"]) if R["h"] else None
        pred = [xp + xo if xo is not None else None, y, w, h]
        c = r["rect_c"]; ours = r["rect_o"]
        def corners(b): return [(b[0], b[1]), (b[0] + b[2], b[1] + b[3])]
        def hyp(b): return max(math.hypot(u[0] - v[0], u[1] - v[1]) for u, v in zip(corners(b), corners(c)))
        full = [pv if pv is not None else ov for pv, ov in zip(pred, ours)]          # missing legs fall back to ours
        ok_axis = [pred[i] is not None and abs(pred[i] - c[i]) <= (2.0 if i < 2 else tol(c[i])) for i in range(4)]
        ok_all = all(abs(full[i] - c[i]) <= (2.0 if i < 2 else tol(c[i])) for i in range(4))
        out.append({"page": r["page"], "node": r["node"], "tag": r["tag"], "class": r.get("class", ""), "level": lvl, "peers": len(R["n"]),
                    "pred": [None if v is None else round(v, 2) for v in pred], "ours": ours, "chrome": c, "ok": ok_all, "ok_axis": ok_axis,
                    "hyp_pred": round(hyp(full), 2), "hyp_ours": round(hyp(ours), 2)})
    n = len(out); okn = sum(x["ok"] for x in out)
    hp_, ho = [x["hyp_pred"] for x in out], [x["hyp_ours"] for x in out]
    print(f"failing boxes {sum(not r['agree'] for r in rows.values())} · eligible (good frame, ≥ {o.min_peers} good peers) {elig} · skipped {dict(skipped)}")
    print(f"predicted box within Chrome's tolerance: {okn}/{n} = {okn / max(1, n):.1%}   (our engine on these: 0 % by definition)")
    print(f"median corner hypotenuse √(dx²+dy²): predicted {st.median(hp_):.1f} px · ours {st.median(ho):.1f} px · prediction closer on {sum(a < b for a, b in zip(hp_, ho))}/{n}")
    for i, ax in enumerate("xywh"):
        have = [x for x in out if x["pred"][i] is not None]; good = sum(x["ok_axis"][i] for x in have)
        print(f"  axis {ax}: predicted for {len(have)} · within tolerance {good} ({good / max(1, len(have)):.0%})")
    print("  by key level:", {l: f"{sum(x['ok'] for x in out if x['level'] == l)}/{sum(1 for x in out if x['level'] == l)}" for l in ("K1", "K2", "K3")})
    bar = okn / max(1, n) >= 0.5 and st.median(hp_) < st.median(ho)
    print("PRE-REGISTERED BAR:", "PASS" if bar else "FAIL")
    if o.out:
        with open(o.out, "w") as f:
            for x in out: f.write(json.dumps(x) + "\n")


if __name__ == "__main__":
    main()
