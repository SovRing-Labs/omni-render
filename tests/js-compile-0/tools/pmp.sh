#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Poor-man's sampling profiler (perf is blocked: perf_event_paranoid=4). Runs `<bin> js <ir>` under gdb; SIGINT every
# <ms> stops it, gdb prints the top frames. Output: frame histogram (innermost omni_render frame per sample).
# usage: pmp.sh <bin> <ir.json> [samples=60] [ms=25]
set -euo pipefail
bin=$1; ir=$2; n=${3:-60}; ms=${4:-25}; tmp=$(mktemp -d)
{ echo "set pagination off"; echo "set print frame-arguments none"; echo "handle SIGINT stop nopass"; echo "run js $ir > /dev/null";
  for i in $(seq "$n"); do echo "thread apply all bt 14"; echo "continue"; done; echo "kill"; } > "$tmp/cmds"
timeout 300 gdb -q -batch -x "$tmp/cmds" "$bin" > "$tmp/out" 2>&1 &
g=$!; sleep 1.5
for i in $(seq "$n"); do p=$(pgrep -n -f "$bin js $ir" | head -1 || true); [ -n "$p" ] && kill -INT "$p" 2>/dev/null; sleep "$(echo "scale=3; $ms/1000" | bc)"; done
wait $g || true
echo "samples: $(grep -c '^#0' "$tmp/out")"
echo "-- innermost frame"; grep '^#0' "$tmp/out" | sed -E 's/^#0 +(0x[0-9a-f]+ in )?//; s/ \(.*//' | sort | uniq -c | sort -rn | head -25
echo "-- first omni_render frame"; awk '/^Thread /{done=0} /^#[0-9]+ /{ if(!done && $0 ~ /omni_render/){ sub(/^#[0-9]+ +(0x[0-9a-f]+ in )?/,""); sub(/ \(.*/,""); print; done=1 } }' "$tmp/out" | sort | uniq -c | sort -rn | head -25
cp "$tmp/out" "${PMP_OUT:-/dev/null}" 2>/dev/null || true; rm -rf "$tmp"
