#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""shape_ifs.py — REVERSE IFS of a faulty shape over the whole corpus, the omniring way (PIFS-RES / CORE-ROUTE; J 10-06:
"reverse IFS the original faulty shape and replace with correct shape").

Input: `omni-render core corpus --abc shape --all --dump D.jsonl` (every box: its four snapped angles ours + Chrome, parent).
1 ENCODE   every box's local shape as one phasor row (FHRR product encoding, fractional power encoding for the angles —
           math index #58/#59): the box's angles ours + Chrome, the turn (ours − Chrome) per axis, its parent's angles and
           turns, its display and the parent's display. A shape is compared, never named (R1).
2 PARTITION the D dims into geometric blocks (route.partition_blocks: TRI residue classes by default).
3 SIFT     per block, the faulty shape (the range) against every box in the corpus (the domains): 16-phase lattice inner
           product (route's block read).
4 COLLAGE  each block votes for the domains whose block score reaches θ of the range's own; majority over blocks (Chernoff).
5 CHECK    re-encode exactly (unquantised) and cosine against the range ≥ κ.
6 PEEL     accept, mark, continue down the collage list until a check fails `--patience` times in a row.
Output: every place the faulty shape repeats. `--truth PRE POST` scores it against the boxes a known fix repaired
(deformed in PRE, not in POST).
Usage: shape_ifs.py DUMP.jsonl --query PAGE:NODE [--truth PRE.deformed POST.deformed] [--D 512] [--part TRI] ...
"""
import argparse, json, sys, time
from collections import Counter
import numpy as np
sys.path.insert(0, "packages/omni-ring")
from omni_ring.route import partition_blocks  # noqa: E402

AXES = 4


def load(path):
    rows = [json.loads(l) for l in open(path)]
    key = {(r["page"], r["node"]): i for i, r in enumerate(rows)}
    return rows, key


def features(rows, key):
    """per box: numeric features (deg) with NaN = absent, and categorical ids"""
    n = len(rows)
    num = np.full((n, 4 * AXES), np.nan)   # ours[4] chrome[4] turn[4] parent turn[4]
    for i, r in enumerate(rows):
        o = [np.nan if v is None else v for v in r["ours"]]; c = [np.nan if v is None else v for v in r["chrome"]]
        num[i, 0:4] = o; num[i, 4:8] = c; num[i, 8:12] = np.subtract(o, c)
    for i, r in enumerate(rows):
        j = key.get((r["page"], r["parent"]))
        if j is not None: num[i, 12:16] = num[j, 8:12]
    cats = [(r["display"], r["pdisplay"]) for r in rows]
    return num, cats


def encode(num, cats, D, bw, rng):
    """phase φ(n) = Σ_f value_f · base_f (Gaussian base → Gaussian kernel of width 1/bw, random Fourier features) +
    Σ categorical codewords; an absent value contributes its own fixed codeword (absent ≠ 0)"""
    n, F = num.shape
    base = rng.normal(0, 1, (F, D)) * np.array(bw)[:, None]
    absent = rng.uniform(0, 2 * np.pi, (F, D))
    vals = np.nan_to_num(num, nan=0.0)
    phi = vals @ base + np.isnan(num) @ absent
    book = {}
    for slot in range(2):
        for i, c in enumerate(cats):
            k = (slot, c[slot])
            if k not in book: book[k] = rng.uniform(0, 2 * np.pi, D)
            phi[i] += book[k]
    return phi


# X Y W H index the angle columns: ours 0-3, Chrome 4-7, turn 8-11 (dump order x y w h)
# the shape's PARTS (the geometric partition of the shape itself, the author's PIFS: partition the signal, each block finds its own match):
# one part per angle (ours, Chrome, turn), the parent's turns, the box's display, the frame's display
PARTS = [("W", [2, 6, 10]), ("X", [0, 4, 8]), ("Y", [1, 5, 9]), ("H", [3, 7, 11]), ("parent", [12, 13, 14, 15]), ("display", ["c0"]), ("frame", ["c1"])]


def encode_parts(num, cats, D, bw, rng):
    """every part in its own block of dims (concatenation = the partition), product-encoded inside its block"""
    d = D // len(PARTS); out = np.zeros((num.shape[0], d * len(PARTS))); spans = []
    for k, (_, feats) in enumerate(PARTS):
        sl = slice(k * d, (k + 1) * d); spans.append(np.arange(k * d, (k + 1) * d))
        for f in feats:
            if isinstance(f, str):
                slot = int(f[1]); book = {}
                for i, c in enumerate(cats):
                    if c[slot] not in book: book[c[slot]] = rng.uniform(0, 2 * np.pi, d)
                    out[i, sl] += book[c[slot]]
            else:
                base = rng.normal(0, 1, d) * bw[f]; absent = rng.uniform(0, 2 * np.pi, d)
                col = num[:, f]; out[:, sl] += np.outer(np.nan_to_num(col), base) + np.outer(np.isnan(col), absent)
    return out, spans


def main():
    a = argparse.ArgumentParser()
    a.add_argument("dump"); a.add_argument("--query", required=True)
    a.add_argument("--truth", nargs=2); a.add_argument("--D", type=int, default=512); a.add_argument("--part", default="TRI")
    a.add_argument("--B", type=int, default=32); a.add_argument("--theta", type=float, default=0.6); a.add_argument("--kappa", type=float, default=0.5)
    a.add_argument("--bw-angle", type=float, default=1 / 20); a.add_argument("--bw-turn", type=float, default=1 / 10)
    a.add_argument("--patience", type=int, default=25); a.add_argument("--faulty", action="store_true"); a.add_argument("--seed", type=int, default=7); a.add_argument("--out")
    o = a.parse_args()
    t0 = time.time()
    rows, key = load(o.dump)
    num, cats = features(rows, key)
    bw = [o.bw_angle] * 8 + [o.bw_turn] * 8
    if o.part == "PARTS":
        phi, blocks = encode_parts(num, cats, o.D, bw, np.random.default_rng(o.seed))
    else:
        phi = encode(num, cats, o.D, bw, np.random.default_rng(o.seed)); blocks = None
    q16 = (np.round(phi / (2 * np.pi / 16)).astype(np.int64) % 16).astype(np.uint8)    # the 16-phase lattice (alias)
    cos16 = np.cos(2 * np.pi * np.arange(16) / 16)
    page, node = o.query.rsplit(":", 1); i0 = key[(page, int(node))]
    blocks = blocks if blocks is not None else partition_blocks(o.part, o.D, o.B)
    if o.part == "PARTS" and o.faulty:
        # the RANGE is the faulty shape: only the parts that turned (|turn| ≥ 1°) plus the frame they sit in
        tq = num[i0, 8:12]; keep = [k for k, (nm, _) in enumerate(PARTS) if (nm in "XYWH" and len(nm) == 1 and abs(np.nan_to_num(tq["XYWH".index(nm)])) >= 1.0) or nm == "frame"]
        print("range parts:", [PARTS[k][0] for k in keep]); blocks = [blocks[k] for k in keep]
        # the frame is the DOMAIN CLASS (the map applies inside it), not a vote: gate on it; the votes are the turned parts.
        # ORACLE CHECK: a repeat must be turned on the same parts, the same way (sign), past the outline (|turn| ≥ 1°)
        fi = keep.pop(); frame_gate = np.array([c[1] == cats[i0][1] for c in cats]); blocks = blocks[:-1]
        sgn = np.sign(tq); turned = [ "XYWH".index(PARTS[k][0]) for k in keep]
        part_ok = [(np.sign(np.nan_to_num(num[:, 8 + a])) == sgn[a]) & (np.abs(np.nan_to_num(num[:, 8 + a])) >= 1.0) for a in turned]
        oracle = frame_gate
    else:
        oracle = np.ones(len(rows), bool); part_ok = None
    # 3 SIFT per block + 4 COLLAGE
    votes = np.zeros(len(rows))
    for bi, b in enumerate(blocks):
        s = cos16[(q16[:, b].astype(np.int16) - q16[i0, b]) % 16].sum(1)
        v = s >= o.theta * s[i0]
        if part_ok is not None: v = v & part_ok[bi]    # the part votes only where the oracle sees it turned the same way
        votes += v
    collage = votes / len(blocks) * oracle
    # 5 CHECK (exact, unquantised) + 6 PEEL down the collage order
    v0 = np.exp(1j * phi[i0]); o.D = phi.shape[1]
    order = np.argsort(-collage)
    found, miss = [], 0
    for i in order:
        if collage[i] < 0.5: break
        # CHECK: exact re-encode, per block cosine, majority of blocks ≥ κ (the check obeys the same partition)
        c = float(np.mean([np.real(np.vdot(v0[b], np.exp(1j * phi[i, b]))) / len(b) >= o.kappa for b in blocks]))
        if c >= 0.5: found.append((int(i), float(collage[i]), c)); miss = 0
        else:
            miss += 1
            if miss >= o.patience: break
    secs = time.time() - t0
    r0 = rows[i0]
    print(f"range {page}:{node} <{r0['tag']}> {r0['display']}<{r0['pdisplay']} ours {r0['ours']} chrome {r0['chrome']}")
    print(f"reverse IFS: {len(rows)} domains · {len(blocks)} {o.part} blocks · D {o.D} · {len(found)} repeats (collage ≥ ½, check ≥ {o.kappa}) · {secs:.2f}s")
    pages = Counter(rows[i]["page"] for i, _, _ in found)
    print("  by page:", dict(pages.most_common(8)))
    deformed = [i for i, _, _ in found if rows[i]["mu"] >= 0.5]
    print(f"  of them deformed (μ ≥ ½): {len(deformed)} · agreeing boxes matched: {sum(rows[i]['agree'] for i, _, _ in found)}")
    for i, cl, c in found[:12]:
        r = rows[i]; print(f"   {r['page'][:28]:28s} n{r['node']:<5} <{r['tag']}> {r['display']}<{r['pdisplay']} collage {cl:.2f} check {c:.2f} μ {r['mu']:.2f} turn {[None if x is None or y is None else round(x - y, 1) for x, y in zip(r['ours'], r['chrome'])]}")
    res = {"query": o.query, "found": [{"page": rows[i]["page"], "node": rows[i]["node"], "collage": cl, "check": c, "mu": rows[i]["mu"]} for i, cl, c in found],
           "params": vars(o), "secs": secs}
    if o.truth:
        pre = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[0])) if r.get("mu", 1) >= 0.5}
        post = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[1])) if r.get("mu", 1) >= 0.5}
        F = pre - post
        S = {(rows[i]["page"], rows[i]["node"]) for i in deformed}
        tp = len(S & F)
        fam = {(r["page"], r["node"]) for r in map(json.loads, open(o.truth[0])) if r.get("mu", 1) >= 0.5 and r["sig"] == next(x["sig"] for x in map(json.loads, open(o.truth[0])) if (x["page"], x["node"]) == (page, int(node)))}
        print(f"  TRUTH (deformed before the fix, fixed after): {len(F)} · found {tp} → recall {tp / max(1, len(F)):.3f} precision {tp / max(1, len(S)):.3f}")
        fr = {(r["page"], r["node"]): r["pdisplay"] for r in rows}
        Fd = {k for k in F if fr.get(k) == "Grid"}; tpd = len(S & Fd); Sd = {k for k in S if fr.get(k) == "Grid"}
        print(f"  TRUTH direct (grid items the fix repaired): {len(Fd)} · found {tpd} → recall {tpd / max(1, len(Fd)):.3f} · precision among found grid items {tpd / max(1, len(Sd)):.3f}")
        res["truth_direct"] = {"F": len(Fd), "tp": tpd, "recall": tpd / max(1, len(Fd))}
        print(f"  baseline flat family sig: {len(fam)} boxes → recall {len(fam & F) / max(1, len(F)):.3f} precision {len(fam & F) / max(1, len(fam)):.3f}")
        res["truth"] = {"F": len(F), "tp": tp, "found_deformed": len(S), "recall": tp / max(1, len(F)), "precision": tp / max(1, len(S)),
                        "baseline_family": {"n": len(fam), "recall": len(fam & F) / max(1, len(F)), "precision": len(fam & F) / max(1, len(fam))}}
    if o.out: json.dump(res, open(o.out, "w"), indent=1)


if __name__ == "__main__":
    main()
