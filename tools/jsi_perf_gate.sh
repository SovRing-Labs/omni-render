#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# JSI-PERF-1 verification gate — verbatim from spawn-briefs/active/JSI-PERF-1-dev.md.
# Written as a file so the awk quoting survives verbatim (no bash -c re-escaping layer).
cd 

python3 tools/re_parity.py >/dev/null 2>&1 && ./target/release/omni-render sv-run results/launcher-ir App /tmp/jsi-perf-gate --js results/launcher-js --css ui/src/app.css > /tmp/jsi-perf-gate.txt 2>&1
grep -q "0 errors" /tmp/jsi-perf-gate.txt && awk "/render/ {sub(/.*render /,\"\"); sub(/ ms.*/,\"\"); exit (\$1+0 < 7000) ? 0 : 1}" /tmp/jsi-perf-gate.txt
GATE=$?

echo "GATE_EXIT=$GATE"
grep -o "step0:.*" /tmp/jsi-perf-gate.txt | head -1
exit $GATE