#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_tri.py — 3-point TRIANGULATION of a shape, ours vs Chrome (J 10-06, readback confirmed: "first match the shape, then
put a larger triangle over the shape, rotate it slightly in each direction ◢ ▲ ◣ … superimposing the shape difference, you will
see what fits and what does not").

Per box: a larger triangle is laid over its shape, anchored on the frame it sits in (the parent's left edge and width, the top
at the previous sibling's bottom or the parent top — the inputs that flow into the box, never the box itself). The box's four
corners are read in barycentric coordinates of OUR triangle turned by θ ∈ {−δ, 0, +δ} about its centroid and placed into
CHROME's untilted triangle (superimposition = an affine map triangle → triangle; Procrustes with rotational jitter). A corner
FITS when it lands within τ of Chrome's corner; fuzzy membership = the best fit over the three turns.
  FIT        — all four corners fit at θ = 0
  DISPLACED  — all four fit only under a slight turn (text / formatting displacement, not a defect)
  DEFECT     — no turn fits: the corners that never fit are what is wrong
Usage: shape_tri.py DUMP.jsonl [--delta 3] [--tau 2] [--out F] [--truth PRE.deformed POST.deformed] [--top N]
"""
import argparse, json, math
from collections import Counter
import numpy as np
PIVOT = "base"
MODE = "corners"


def tri(anchor_x, anchor_y, s):
    """the larger triangle ▲ over a frame: apex above, base below, 3s wide — contains any box inside the frame"""
    cx = anchor_x + s / 2
    return np.array([[cx, anchor_y - s], [cx - 1.5 * s, anchor_y + s], [cx + 1.5 * s, anchor_y + s]])


def turn(T, th, pivot=None):
    c = T.mean(0) if pivot is None else T[pivot]; R = np.array([[math.cos(th), -math.sin(th)], [math.sin(th), math.cos(th)]])
    return (T - c) @ R.T + c


def bary(T, P):
    """barycentric coordinates of points P (k×2) in triangle T (3×2)"""
    A = np.array([[T[0, 0] - T[2, 0], T[1, 0] - T[2, 0]], [T[0, 1] - T[2, 1], T[1, 1] - T[2, 1]]])
    l12 = np.linalg.solve(A, (P - T[2]).T).T
    return np.column_stack([l12, 1 - l12.sum(1)])


def corners(r):
    x, y, w, h = r
    return np.array([[x, y], [x + w, y], [x, y + h], [x + w, y + h]])


def frame(rows, key, r, eng):
    """anchor (x, y) and size s of the frame's triangle in engine `eng` (rect_o / rect_c): parent left + width, top = previous
    sibling's bottom (y flows sideways) or the parent top"""
    p = key.get((r["page"], r["parent"]))
    if p is None: return None
    pr = rows[p][eng]
    q = key.get((r["page"], r["prev"]))
    top = rows[q][eng][1] + rows[q][eng][3] if q is not None else pr[1]
    return pr[0], top, max(pr[2], 1.0)


def frame_rect(rows, key, r, eng):
    """the frame as a rectangle: parent left/right, top = previous sibling's bottom or the parent top, bottom = parent bottom"""
    p = key.get((r["page"], r["parent"]))
    if p is None: return None
    pr = rows[p][eng]; q = key.get((r["page"], r["prev"]))
    top = rows[q][eng][1] + rows[q][eng][3] if q is not None else pr[1]
    return pr[0], top, pr[0] + max(pr[2], 1.0), max(pr[1] + pr[3], top + 1.0)


def corner_tris(fr):
    """J's five triangles over the frame: △ isosceles, and a right triangle with its right angle at each frame corner
    ◸ (top-left) ◹ (top-right) ◺ (bottom-left) ⊿ (bottom-right); legs 2× the frame so the triangle covers the shape"""
    x0, y0, x1, y1 = fr; w, h = x1 - x0, y1 - y0
    return {"△": np.array([[(x0 + x1) / 2, y0 - h], [x0 - w, y1 + h * 0.01], [x1 + w, y1 + h * 0.01]]),
            "◸": np.array([[x0, y0], [x0 + 2 * w, y0], [x0, y0 + 2 * h]]),
            "◹": np.array([[x1, y0], [x1 - 2 * w, y0], [x1, y0 + 2 * h]]),
            "◺": np.array([[x0, y1], [x0 + 2 * w, y1], [x0, y1 - 2 * h]]),
            "⊿": np.array([[x1, y1], [x1 - 2 * w, y1], [x1, y1 - 2 * h]])}


def classify_corners(rows, key, r, delta, tau):
    """each triangle tried exact, then with a slight angle variation at each of its corners (pivot the triangle about that
    corner by ±δ): 7 variants per triangle. A triangle FITS when all four box corners land within τ in Chrome's triangle."""
    fo, fc = frame_rect(rows, key, r, "rect_o"), frame_rect(rows, key, r, "rect_c")
    if fo is None or fc is None: return None
    To, Tc = corner_tris(fo), corner_tris(fc)
    Po, Pc = corners(r["rect_o"]), corners(r["rect_c"])
    sig = {}; best_res = None; view = {}
    for k in To:
        exact = None; varied = False; bres = None
        for pv in (None, 0, 1, 2):
            for th in ((0.0,) if pv is None else (-math.radians(delta), math.radians(delta))):
                T = To[k] if pv is None else turn(To[k], th, pv)
                try: pred = bary(T, Po) @ Tc[k]
                except np.linalg.LinAlgError: continue
                res = np.linalg.norm(pred - Pc, axis=1)
                ok = bool((res <= tau).all())
                if pv is None: exact = ok
                elif ok: varied = True
                if bres is None or res.max() < bres.max(): bres = res
        sig[k] = "=" if exact else ("~" if varied else "x")
        if best_res is None or bres.max() < best_res.max(): best_res = bres
    cls = "FIT" if "=" in sig.values() else ("DISPLACED" if "~" in sig.values() else "DEFECT")
    return {"cls": cls, "tri": "".join(f"{k}{v}" for k, v in sig.items()), "misfit": [i for i in range(4) if best_res[i] > tau],
            "res_px": [round(float(x), 1) for x in best_res]}


NEXT = {}


def anchored_tris(rows, key, r, eng):
    """the five triangles on SEPARATE references (J 10-06: break out of the single frame rectangle) — each reads the box from a
    different set of neighbours, so they disagree where the error comes from:
      △  the frame's width only: anchor (parent left, top after the previous sibling), size = parent width (no heights at all)
      ◸  parent top-left · parent top-right · previous sibling's bottom-left           (reads from above / the sibling before)
      ◹  parent top-right · parent top-left · next sibling's top-right                 (reads from the sibling after)
      ◺  parent top-left · Chrome's target bottom (parent top + Chrome's parent height) left · right   (the hole it must fit)
      ⊿  parent's OWN bottom-right · bottom-left · parent top-right                    (reads from the frame's own output)"""
    p = key.get((r["page"], r["parent"]))
    if p is None: return None
    pr, pc = rows[p][eng], rows[p]["rect_c"]
    x0, y0, w, h = pr; x1 = x0 + max(w, 1.0); w = max(w, 1.0)
    q = key.get((r["page"], r["prev"])); nx = NEXT.get((r["page"], r["node"]))
    top = rows[q][eng][1] + rows[q][eng][3] if q is not None else y0
    sb = (rows[q][eng][0], rows[q][eng][1] + rows[q][eng][3]) if q is not None and rows[q][eng][3] > 0.5 else (x0, y0 + w)
    if sb[1] <= y0 + 0.5: sb = (x0, y0 + w)
    nt = (rows[nx][eng][0] + rows[nx][eng][2], rows[nx][eng][1]) if nx is not None else (x1, y0 + w)
    if nt[1] <= y0 + 0.5: nt = (x1, y0 + w)
    yb_target = y0 + max(pc[3], 1.0)                  # Chrome's frame height: the hole, the same in both engines
    yb_own = y0 + max(h, 1.0)
    cx = x0 + w / 2
    return {"△": np.array([[cx, top - w], [cx - 1.5 * w, top + w], [cx + 1.5 * w, top + w]]),
            "◸": np.array([[x0, y0], [x1, y0], list(sb)]),
            "◹": np.array([[x1, y0], [x0, y0], list(nt)]),
            "◺": np.array([[x0, yb_target], [x1, yb_target], [x0, y0]]),
            "⊿": np.array([[x1, yb_own], [x0, yb_own], [x1, y0]])}


def classify_anchored(rows, key, r, delta, tau):
    To, Tc = anchored_tris(rows, key, r, "rect_o"), anchored_tris(rows, key, r, "rect_c")
    if To is None or Tc is None: return None
    Po, Pc = corners(r["rect_o"]), corners(r["rect_c"])
    sig = {}; best_res = None; view = {}
    for k in To:
        exact = None; varied = False; bres = None
        for pv in (None, 0, 1, 2):
            for th in ((0.0,) if pv is None else (-math.radians(delta), math.radians(delta))):
                T = To[k] if pv is None else turn(To[k], th, pv)
                try: pred = bary(T, Po) @ Tc[k]
                except np.linalg.LinAlgError: continue
                res = np.linalg.norm(pred - Pc, axis=1)
                ok = bool((res <= tau).all())
                if pv is None: exact = ok
                elif ok: varied = True
                if bres is None or res.max() < bres.max(): bres = res
        if bres is None: continue
        sig[k] = "=" if exact else ("~" if varied else "x")
        view[k] = float(bres.max())
        if best_res is None or bres.max() < best_res.max(): best_res = bres
    if best_res is None: return None
    vals = set(sig.values())
    cls = "FIT" if vals == {"="} else ("DEFECT" if vals == {"x"} else "SPLIT")   # SPLIT: the triangles disagree = the source is located
    return {"cls": cls, "view": {k: round(v, 3) for k, v in view.items()}, "tri": "".join(f"{k}{v}" for k, v in sig.items()), "misfit": [i for i in range(4) if best_res[i] > tau],
            "res_px": [round(float(x), 1) for x in best_res]}


def classify(rows, key, r, delta, tau):
    if MODE == "anchored": return classify_anchored(rows, key, r, delta, tau)
    if MODE == "corners": return classify_corners(rows, key, r, delta, tau)
    fo, fc = frame(rows, key, r, "rect_o"), frame(rows, key, r, "rect_c")
    if fo is None or fc is None: return None
    To, Tc = tri(*fo), tri(*fc)
    Po, Pc = corners(r["rect_o"]), corners(r["rect_c"])
    tol = np.maximum(tau, 0.05 * np.abs(np.array(r["rect_c"])[[2, 2, 2, 2]]) * 0)  # τ px per corner (fuzzy edge below)
    best = None
    # ▲ untilted · ◣ leaned on its left base corner · ◢ leaned on its right base corner (--pivot base); --pivot centroid = spin
    for th, pv in ((0.0, None), (-math.radians(delta), 1 if PIVOT == "base" else None), (math.radians(delta), 2 if PIVOT == "base" else None)):
        pred = bary(turn(To, th, pv), Po) @ Tc          # our corners superimposed into Chrome's triangle
        res = np.linalg.norm(pred - Pc, axis=1)
        fit = res <= tol
        mu = np.clip(1 - (res - tol) / (2 * tau), 0, 1)  # fuzzy membership: 1 inside τ, fading over 2τ
        cand = (int(fit.sum()), float(mu.mean()), th, res)
        if best is None or cand[:2] > best[:2]: best = cand
        if th == 0.0: fit0 = int(fit.sum())
    nfit, mu, th, res = best
    cls = "FIT" if fit0 == 4 else ("DISPLACED" if nfit == 4 else "DEFECT")
    return {"cls": cls, "theta_deg": round(math.degrees(th), 2), "corners_fit": nfit, "fuzzy": round(mu, 3),
            "misfit": [i for i in range(4) if res[i] > tau], "res_px": [round(float(x), 1) for x in res]}


def main():
    a = argparse.ArgumentParser()
    a.add_argument("dump"); a.add_argument("--delta", type=float, default=3.0); a.add_argument("--tau", type=float, default=2.0)
    a.add_argument("--out"); a.add_argument("--truth", nargs=2); a.add_argument("--top", type=int, default=12)
    a.add_argument("--pivot", default="base", choices=["base", "centroid"])
    a.add_argument("--mode", default="anchored", choices=["anchored", "corners", "spin"])
    o = a.parse_args(); global PIVOT, MODE; PIVOT = o.pivot; MODE = o.mode
    rows = [json.loads(l) for l in open(o.dump)]
    key = {(r["page"], r["node"]): i for i, r in enumerate(rows)}
    for i, r in enumerate(rows):
        if r["prev"] >= 0: NEXT[(r["page"], r["prev"])] = i
    out = []
    for r in rows:
        if r["agree"] and r["mu"] < 0.5: continue        # tier (b) feeds tier (c): failing or deformed boxes only
        c = classify(rows, key, r, o.delta, o.tau)
        if c: out.append({"page": r["page"], "node": r["node"], "tag": r["tag"], "display": r["display"], "pdisplay": r["pdisplay"],
                          "mu": r["mu"], "agree": r["agree"], **c})
    def tally(sel, name):
        cnt = Counter(x["cls"] for x in sel); n = max(1, len(sel))
        print(f"  {name:38s} n {len(sel):5d} · FIT {cnt['FIT']:5d} · DISPLACED {cnt['DISPLACED']:5d} · SPLIT {cnt['SPLIT']:5d} ({cnt['SPLIT'] / n:.0%}) · DEFECT {cnt['DEFECT']:5d} ({cnt['DEFECT'] / n:.0%})")
    print(f"triangulation δ ±{o.delta}° τ {o.tau}px over {len(out)} boxes (failing or deformed)")
    tally([x for x in out if not x["agree"]], "failing boxes")
    tally([x for x in out if x["mu"] >= 0.5], "deformed (angle station μ ≥ ½)")
    tally([x for x in out if x["mu"] >= 0.5 and not x["agree"]], "deformed and failing")
    if MODE in ("corners", "anchored"):
        print("  which triangles fit (= exact · ~ with a slight corner angle · x no), failing boxes:", Counter(x["tri"] for x in out if not x["agree"]).most_common(10))
    print("  misfit corners in DEFECT (0 TL · 1 TR · 2 BL · 3 BR):", Counter(tuple(x["misfit"]) for x in out if x["cls"] == "DEFECT").most_common(6))
    print("  DEFECT by page:", Counter(x["page"][:24] for x in out if x["cls"] == "DEFECT" and x["mu"] >= 0.5).most_common(6))
    if o.truth:
        pre = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[0])) if r["mu"] >= 0.5}
        post = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[1])) if r["mu"] >= 0.5}
        F = pre - post; idx = {(x["page"], x["node"]): x for x in out}
        fd = [idx[k] for k in F if k in idx]
        tally(fd, "TRUTH: deformed boxes the §8.3 fix repaired")
        tally([x for x in fd if x["pdisplay"] == "Grid"], "TRUTH direct: grid items it repaired")
    if o.out:
        with open(o.out, "w") as f:
            for x in out: f.write(json.dumps(x) + "\n")


if __name__ == "__main__":
    main()
