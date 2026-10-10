#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# One SHAPE round (author 10-06 pipeline): census → angles → triangulation → soft-RRNS sources → cause-ordered families.
# Usage: tools/doctor_round.sh OUTDIR   (run from the omni-render worktree; binary = $OMR_BIN or target-doctor)
set -euo pipefail
OUT=${1:?outdir}; mkdir -p "$OUT"; B=${OMR_BIN:-../omni-render/target-doctor/release/omni-render}
OMR_PROGRAM=results/omr.program.json $B mismatches corpus "$OUT/mm.jsonl" 2>&1 | tail -1
# replay pages only: the holdout is a canary, never a problem source
$B core corpus --abc shape --top 0 --dump "$OUT/dump.jsonl" --out "$OUT/deformed.jsonl" 2>&1 | head -1
python3 tools/shape_tri.py "$OUT/dump.jsonl" --mode anchored --delta 3 --tau 4 --out "$OUT/tri.jsonl" | sed -n 2p
python3 tools/shape_rrns.py "$OUT/tri.jsonl" "$OUT/dump.jsonl" --sigma 4 --sigma-miss 60 --margin 0.5 --out "$OUT/rrns.jsonl" > "$OUT/rrns.log"
python3 - "$OUT" <<'PY'
import json, sys, collections as C
sys.path.insert(0, 'tools'); import shape_key as K
out = sys.argv[1]
rows = {(r['page'], r['node']): r for r in map(json.loads, open(f'{out}/dump.jsonl'))}
src = {(x['page'], x['node']): x['source'] for x in map(json.loads, open(f'{out}/rrns.jsonl'))}
fam = C.defaultdict(list)
for k, s in src.items():
    if s != 'the box itself': continue
    f = K.fracture(rows[k], rows)
    if not f: continue
    e, t = f; c = next((i for i in (0, 1, 2, 3) if t[i] != 0), None)
    if c is None: continue
    fam[(rows[k]['pdisplay'], K.AX[c], t[c])].append((k, e[c]))
res = []
for key, v in sorted(fam.items(), key=lambda x: -len(x[1])):
    # sub-families: per page × frame tag (one component = one likely cause)
    sub = C.Counter((k[0], rows[(k[0], rows[k]['parent'])]['tag'] if (k[0], rows[k]['parent']) in rows else '?') for k, _ in v)
    res.append({'key': key, 'n': len(v), 'members': [list(k) for k, _ in v], 'sub': [[a, b, n] for (a, b), n in sub.most_common(8)]})
json.dump(res, open(f'{out}/families.json', 'w'))
print('"box itself" families:', sum(r['n'] for r in res), 'boxes in', len(res), 'keys')
for r in res[:12]: print(f"  {r['n']:5d} {str(tuple(r['key'])):26s} top components {[(a[:18], b, n) for a, b, n in r['sub'][:3]]}")
PY
