#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Merge gate for browser/body-0 <- jsperf/work (JS-COMPILE-0 step 6), the shared recipe (omni-ui results/browser-functional-0/
# merge-gate.sh) with OLD = browser/body-0's build and NEW = this branch's build. Run under tw. Prints MERGE-GATE-DONE <dir>.
set -u
OLD=${OLD:-target/release/omni-render}
NEW=${NEW:-target/release/omni-render}
R=results/browser-functional-0
OUT=$(cd "$(dirname "$0")" && pwd)/results/merge-gate.$(date +%Y%m%d-%H%M); mkdir -p "$OUT"
echo "OLD $OLD @$(git -C "$(dirname "$OLD")/../.." rev-parse --short HEAD)  NEW $NEW @$(git -C "$(dirname "$NEW")/../.." rev-parse --short HEAD)" | tee "$OUT/bins.txt"
cd  || exit 1
P="--program results/omr.program.json"
echo "== M2a baseline (old vs new must be equal)"
$OLD core corpus $P --max 0 > "$OUT/base-old.txt" 2>&1; tail -2 "$OUT/base-old.txt"
$NEW core corpus $P --max 0 > "$OUT/base-new.txt" 2>&1; tail -2 "$OUT/base-new.txt"
echo "== M2b check-compiled (0 different)"
$NEW core corpus $P --check-compiled > "$OUT/check-compiled.txt" 2>&1; tail -2 "$OUT/check-compiled.txt"
echo "== M2c binary gate (0 lost)"
$OLD core corpus $P --agree-snapshot "$OUT/agree-old.json" 2>&1 | tail -1
$NEW core corpus $P --binary-gate "$OUT/agree-old.json" > "$OUT/binary-gate.json" 2>"$OUT/binary-gate.err"; tail -1 "$OUT/binary-gate.err"
echo "== M3 browser gates (server :8793 must be up)"
printf 'jsprobe title: '; timeout 120 $NEW page http://127.0.0.1:8793/index.html 2>/dev/null | grep '^meta' | python3 -c 'import json,sys; print(json.loads(sys.stdin.read()[5:]).get("title",""))'
sed 's/8771/8793/' $R/conn.sh > "$OUT/conn8793.sh"; BIN=$NEW sh "$OUT/conn8793.sh" > "$OUT/conn.txt" 2>&1; echo "conn.sh fails=$?"; grep -c '^PASS' "$OUT/conn.txt"
echo "MERGE-GATE-DONE $OUT"
