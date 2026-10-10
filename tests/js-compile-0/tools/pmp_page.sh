#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Poor-man's sampler for `omni-render page` under replay: gdb follows the fork + exec into the renderer process (where
# the page's JS runs); SIGINT every <ms> to the renderer; frame histogram of the renderer's interpreter thread.
# usage: pmp_page.sh <bin> <bundle-dir> <url> [samples=150] [ms=40]   (env OMR_CLOCK=virtual set here)
set -euo pipefail
bin=$1; dir=$2; url=$3; n=${4:-150}; ms=${5:-40}; tmp=$(mktemp -d)
{ echo "set pagination off"; echo "set print frame-arguments none"; echo "set follow-fork-mode child"; echo "set detach-on-fork on"; echo "set follow-exec-mode same";
  echo "handle SIGINT stop nopass"; echo "handle SIGPIPE nostop noprint pass"; echo "handle SIGSYS nostop noprint pass"; echo "run page $url > /dev/null";
  for i in $(seq "$n"); do echo "thread apply all bt 14"; echo "continue"; done; echo "kill"; } > "$tmp/cmds"
OMR_REPLAY=$dir OMR_CLOCK=virtual timeout 600 gdb -q -batch -x "$tmp/cmds" "$bin" > "$tmp/out" 2>&1 &
g=$!; sleep 3
for i in $(seq "$n"); do p=$(pgrep -n -f "$bin page-renderer" || true); [ -n "$p" ] && kill -INT "$p" 2>/dev/null; sleep "$(echo "scale=3; $ms/1000" | bc)"; done
wait $g || true
echo "stops: $(grep -c '^Thread ' "$tmp/out")"
echo "-- first omni_render frame (per thread stack)"; awk '/^Thread /{done=0} /^#[0-9]+ /{ if(!done && $0 ~ /omni_render/){ sub(/^#[0-9]+ +(0x[0-9a-f]+ in )?/,""); sub(/ \(.*/,""); print; done=1 } }' "$tmp/out" | sort | uniq -c | sort -rn | head -40
cp "$tmp/out" "${PMP_OUT:-/dev/null}" 2>/dev/null || true; rm -rf "$tmp"
