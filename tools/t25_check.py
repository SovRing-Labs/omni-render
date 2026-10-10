#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""t25_check.py — T25-CORE-JIT gate: the engine's T25 core (core corpus --abc t25 --out CORE.jsonl) against the Python golden
(box_t25 / t25_ifs / t25_families) on a shape dump of the same pages and program. PASS = every box's five exact errors equal,
the root sets equal, every root's triangle key equal, the family table equal.
Usage: t25_check.py DUMP.jsonl CORE.jsonl"""
import json, os, sys
from collections import Counter
sys.path.insert(0, os.path.dirname(__file__)); import t25_ifs as T, t25_families as F

rows = T.load(sys.argv[1])
core = {(r["page"], r["node"]): r for r in map(json.loads, open(sys.argv[2]))}
G = ("x0", "y0", "w", "h", "ry")
n = tw_ok = 0; bad = []
for k, r in rows.items():
    c = core.get(k)
    if c is None: bad.append(("missing in core", k)); continue
    n += 1
    e = T.twist(rows, r); py = [round(e[g] * 64) for g in G]
    if py == c["twist"]: tw_ok += 1
    elif len(bad) < 8: bad.append(("twist", k, py, c["twist"]))
extra = [k for k in core if k not in rows]
py_roots = {(r["page"], r["node"]) for r in T.roots(rows)}
core_roots = {k for k, c in core.items() if c["root"]}
key_ok = sum(1 for k in py_roots & core_roots if list(F.tri_key(rows, rows[k])) == core[k]["key"])
fam_py = Counter(F.tri_key(rows, rows[k]) for k in py_roots)
fam_core = Counter(tuple(core[k]["key"]) for k in core_roots)
ok = tw_ok == n == len(core) and not extra and py_roots == core_roots and key_ok == len(py_roots) and fam_py == fam_core
print(f"boxes {n} (core {len(core)}, extra {len(extra)}) · twists equal {tw_ok}/{n} · roots py {len(py_roots)} core {len(core_roots)} "
      f"(same {len(py_roots & core_roots)}) · keys equal {key_ok}/{len(py_roots)} · families py {len(fam_py)} core {len(fam_core)} "
      f"equal {fam_py == fam_core} → {'PASS' if ok else 'FAIL'}")
for b in bad[:8]: print("  ", b)
sys.exit(0 if ok else 1)
