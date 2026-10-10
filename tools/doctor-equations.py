#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""DOCTOR — symbolic-equation binding probe (J 2026-10-06, readback confirmed: "use symbols and equations instead of variables /
parameters"). Kill test from the plan: do analogous sections (table width distribution ↔ grid grow-to-limit) become the SAME
equation under a consistent symbol binding, with no use of names?

Method (clean-room; SSA / dataflow equations, canonical forms as in term rewriting): each distribution statement in src/layout.rs is
read from the source, its Rust expression parsed (casts stripped, `x[k]` → indexed symbol x_k, an iterator `.sum()` over k → Σ),
let-bindings it uses are substituted, and the result is put in canonical form with sympy. A section matches the DISTRIBUTION pattern
    out_k = B_k + S · W_k / Σ_j W_j          (base + a scalar share split in proportion to a per-track weight)
when some B_k and W_k make (out_k − B_k) · Σ W / W_k independent of k. The binding is reported as roles (base, share, weight) — the
symbols are bound by their ROLE in the equation, never by their names (patent design-around in the DRX CONVERGE card).
"""
import re, sys
import sympy as sp

SRC = open(sys.argv[1] if len(sys.argv) > 1 else "src/layout.rs").read().split("\n")
k = sp.Symbol("k", integer=True)
SUM = sp.Function("SUM")          # Σ_j over tracks of an indexed expression (opaque, canonical by its argument)

def line_of(pat):
    for i, l in enumerate(SRC, 1):
        if pat in l: return i, l
    raise SystemExit(f"pattern not found: {pat}")

def idx(name):                     # an indexed symbol x_k
    return sp.Function(name)(k)

def parse(expr, scalars, indexed, alias=None):
    """Rust arithmetic subset → sympy. `as T` casts and `.max(1)` guards are stripped (guards are not the computation)."""
    e = re.sub(r"\bas\s+(i64|i32|f64|f32|usize)\b", "", expr)
    e = re.sub(r"\(([^()]*)\)\.max\(1\)", r"(\1)", e)
    for a, b in (alias or {}).items(): e = re.sub(rf"\b{a}\b", b, e)
    e = re.sub(r"(\w+)\[k\]", r"\1__k", e)
    loc = {s: sp.Symbol(s) for s in scalars}
    loc.update({f"{n}__k": idx(n) for n in indexed})
    return sp.sympify(e, locals=loc)

def classify(name, out, lines):
    out = sp.simplify(out)
    atoms = sorted({a for a in out.atoms(sp.Function) if a.args == (k,)}, key=str)
    sums = sorted(out.atoms(SUM), key=str)
    for B in atoms:
        rest = sp.simplify(out - B)
        for S_ in sums:
            W = S_.args[0]                                   # the per-track weight the Σ is taken over
            share = sp.simplify(rest * S_ / W)
            if not share.has(k) and share != 0:
                print(f"  {name:28} L{lines}: out_k = {B} + ({share}) · {W} / Σ({W})")
                return {"base": B, "share": share, "weight": W}
    print(f"  {name:28} L{lines}: no DISTRIBUTION form found ({out})"); return None

print("doctor equations probe — sections as SSA equations, matched by structure, bound by role")
res = {}
# 1. table: extra space to auto columns in proportion to max-content (layout.rs, auto_mx = Σ mx over auto columns)
l1, _ = line_of("mx[k] + (extra * mx[k] as i64 / auto_mx)")
l1b, _ = line_of("let auto_mx: i64 = (0..ncol).filter(|&k| !fixed[k]).map(|k| mx[k] as i64).sum()")
res["table auto-columns"] = classify("table auto-columns", parse("mx[k] + (extra * mx[k] / AUTO)", ["extra"], ["mx"]).subs(sp.Symbol("AUTO"), SUM(idx("mx"))), f"{l1}(+{l1b})")
# 2. table: between min- and max-content, in proportion to (max − min)  (d = smx − smn = Σ(mx − mn); closure a = mn_k, b = mx_k)
l2, _ = line_of("a + ((b - a) as i64 * extra as i64 / d as i64)")
res["table min→max"] = classify("table min→max", parse("mn[k] + ((mx[k] - mn[k]) * extra / D)", ["extra"], ["mn", "mx"]).subs(sp.Symbol("D"), SUM(idx("mx") - idx("mn"))), f"{l2}")
# 3. grid: non-fr tracks grow toward their limits  (want = Σ(limit − w); give = min(free, want))
l3, _ = line_of("w[k] += ((limit[k] - w[k]) as i64 * give as i64 / want as i64)")
l3b, _ = line_of("let want: i32 = (0..ncols).filter(|&k| fr[k] == 0).map(|k| limit[k] - w[k]).sum()")
res["grid grow-to-limit"] = classify("grid grow-to-limit", parse("w[k] + ((limit[k] - w[k]) * give / WANT)", ["give"], ["w", "limit"]).subs(sp.Symbol("WANT"), SUM(idx("limit") - idx("w"))), f"{l3}(+{l3b})")

print("\nunification (same equation shape; bindings by role):")
names = [n for n in res if res[n]]
for i in range(len(names)):
    for j in range(i + 1, len(names)):
        a, b = res[names[i]], res[names[j]]
        # role binding: map a's symbols onto b's by the role they play (base→base, weight→weight, share→share)
        sub = {}
        for role in ("base", "weight"):
            fa, fb = a[role].atoms(sp.Function) - {SUM}, b[role].atoms(sp.Function) - {SUM}
            fa = sorted([f for f in fa if f.args == (k,)], key=str); fb = sorted([f for f in fb if f.args == (k,)], key=str)
        # structural test: is a's weight an affine image of b's (weight = limit − base in the grid, max − base in min→max)?
        wa_minus_base = sp.simplify(a["weight"] + a["base"]); wb_minus_base = sp.simplify(b["weight"] + b["base"])
        same_shape = (a["weight"] == a["base"]) == (b["weight"] == b["base"]) and \
                     (sp.simplify(a["weight"] - (wa_minus_base - a["base"])) == 0) == (sp.simplify(b["weight"] - (wb_minus_base - b["base"])) == 0)
        cap_a = None if a["weight"] == a["base"] else wa_minus_base; cap_b = None if b["weight"] == b["base"] else wb_minus_base
        kind = "IDENTICAL (base → cap, share ∝ cap − base)" if cap_a is not None and cap_b is not None else \
               ("IDENTICAL (share ∝ base)" if cap_a is None and cap_b is None else "same family, different weight")
        print(f"  {names[i]:20} ↔ {names[j]:20}: {kind}"
              + (f"  binding: base {a['base']}↔{b['base']} · cap {cap_a}↔{cap_b} · share {a['share']}↔{b['share']}" if cap_a is not None and cap_b is not None else ""))
