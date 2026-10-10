#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""BROWSER-BODY-0 step 8 verify: every Omniglyph law with a `js` form, on its own generated cases (2 seeds, base scope),
run three ways — node (the reference engine), acorn → `omni-render js` (the omniir path Omniglyph cements), and
jsparse → `omni-render js-src` (the run-time front-end page scripts use, inside the sandbox). A law holds on the run-time
path when js-src prints exactly what node prints for every case; parity = js-src output equals the acorn path's output.
Reads bin/omniglyph as a module; writes nothing to the Omniglyph ledger. Output: results/browser-body-0/omniglyph-jsparse.json"""
import importlib.machinery, importlib.util, json, os, subprocess, sys, tempfile
from pathlib import Path
HERE = Path(__file__).resolve().parents[1]
BIN = str(HERE / "target/release/omni-render")
loader = importlib.machinery.SourceFileLoader("og", os.path.expanduser("~/ai-stack/bin/omniglyph"))
og = importlib.util.module_from_spec(importlib.util.spec_from_loader("og", loader)); loader.exec_module(og)
ACORN = og.OMNIIR_FRONT[1]
def runp(argv):
    try:
        r = subprocess.run(argv, capture_output=True, text=True, timeout=60)
        return r.stdout.strip() if r.returncode == 0 else f"ERR {r.returncode}: {r.stderr.strip()[-160:]}"
    except subprocess.TimeoutExpired: return "TIMEOUT"
rows = []
tmp = tempfile.mkdtemp(prefix="ogjs-")
laws = [e for e in og.entries() if "js" in (e.get("forms") or e.get("code") or {}) and not e.get("gen")]
for e in laws:
    for seed in (1, 2):
        cases = og.inputs(e, seed, 300)
        src = og.program(e, "js", cases)
        p = f"{tmp}/q.js"; open(p, "w").write(src)
        node = runp(["node", p])
        subprocess.run(["node", og.OMNIIR_FRONT[0], p, ACORN, f"{tmp}/q.ir.json"], capture_output=True)
        acorn = runp([BIN, "js", f"{tmp}/q.ir.json"])
        ours = runp([BIN, "js-src", p])
        rows.append({"id": e["id"], "domain": e["domain"], "seed": seed, "cases": len(cases),
                     "node_eq_ours": node == ours, "acorn_eq_ours": acorn == ours, "node_eq_acorn": node == acorn,
                     **({} if node == ours else {"ours_head": ours[:120], "node_head": node[:120]})})
out = {"laws": len(laws), "runs": len(rows), "ours_eq_acorn_path": sum(r["acorn_eq_ours"] for r in rows),
       "ours_eq_node": sum(r["node_eq_ours"] for r in rows), "acorn_path_eq_node": sum(r["node_eq_acorn"] for r in rows), "rows": rows}
(HERE / "results").mkdir(exist_ok=True)
dst = Path(os.environ.get("OUT", str(HERE / "results/omniglyph-jsparse.json"))); dst.write_text(json.dumps(out, indent=1))
print(json.dumps({k: v for k, v in out.items() if k != "rows"}))
for r in rows:
    if not r["acorn_eq_ours"] or not r["node_eq_ours"]: print("  ", r["id"], r["seed"], "acorn==ours" if r["acorn_eq_ours"] else "ACORN≠OURS", "node==ours" if r["node_eq_ours"] else "NODE≠OURS", r.get("ours_head", "")[:80])
