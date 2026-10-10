#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# JS-COMPILE-0 real-page check: YouTube home through `omni-render page`. One warm-up run (parse cache filled), then
# base / new alternating x2 with OMR_PROF=1, then the new build under OMR_SLOTCHECK=1 (panics on any slot / by-name
# disagreement). Outputs: results/yt-<label>-<side><n>.err, results/yt-<label>-check.err, summary on stdout.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); base=$1; new=$2; label=$3; url=${4:-https://www.youtube.com/}
mkdir -p "$here/results"
echo "== warm-up"; timeout 400 "$new" page "$url" > /dev/null 2>&1 || echo "exit $?"
for n in 1 2; do for side in base new; do
  bin=${!side}; f="$here/results/yt-$label-$side$n.err"
  echo "== $side$n $bin load=$(cut -d' ' -f1 /proc/loadavg)"
  /usr/bin/time -f "user %U s  wall %e s  maxrss %M KB" env OMR_PROF=1 timeout 400 "$bin" page "$url" > /dev/null 2> "$f" || echo "exit $?"
  grep -m1 "^prof:" "$f" || true; grep -E "<anon@4191[8]>|<anon@41888>|<anon@41952>|<force:total>" "$f" | head -4 || true; tail -1 "$f"
done; done
echo "== check"; OMR_SLOTCHECK=1 timeout 400 "$new" page "$url" > /dev/null 2> "$here/results/yt-$label-check.err" || echo "exit $?"
echo "slotcheck lines: $(grep -c "SLOTCHECK" "$here/results/yt-$label-check.err" || true)  panics: $(grep -c "panicked" "$here/results/yt-$label-check.err" || true)"
echo YT-DONE
