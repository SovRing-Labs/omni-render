#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# JS-COMPILE-0 bench runner: each bench through node (oracle + floor) and an omni-render build; outputs must match.
# usage: run.sh <omni-render binary> <label>   -> results/<label>.tsv  (bench  node_user_s  omni_user_s  ratio  match)
# Machine is loaded (school training): compare user-CPU seconds and ratios, same session, never absolute wall time.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); bin=$1; label=$2
acorn=$HOME/projects/opencode-launcher/ui/node_modules/acorn/dist/acorn.mjs
front=$HOME/ai-stack/projects/omni-render/tools/js2ir.mjs
mkdir -p "$here/.ir" "$here/results"; out="$here/results/$label.tsv"
rev=$(git -C "$(dirname "$(dirname "$(dirname "$bin")")")" rev-parse --short HEAD 2>/dev/null || echo ?)
echo -e "# $label  bin=$bin  rev=$rev  $(date -Is)  load=$(cut -d' ' -f1 /proc/loadavg)" > "$out"
echo -e "bench\tnode_user_s\tomni_user_s\tratio\tmatch" >> "$out"
for f in "$here"/bench/b*.js; do
  b=$(basename "$f" .js); ir="$here/.ir/$b.ir.json"
  [ "$ir" -nt "$f" ] || timeout 60 node "$front" "$f" "$acorn" "$ir"
  no=$( { /usr/bin/time -f "%U" timeout 120 node "$f" > "$here/.ir/$b.node.out"; } 2>&1 | tail -1)
  oo=$( { /usr/bin/time -f "%U" timeout 300 "$bin" js "$ir" > "$here/.ir/$b.omni.out"; } 2>&1 | tail -1)
  m=$(cmp -s "$here/.ir/$b.node.out" "$here/.ir/$b.omni.out" && echo ok || echo DIFF)
  r=$(awk -v a="$oo" -v b="$no" 'BEGIN{printf "%.1f", (b>0? a/b : 0)}')
  echo -e "$b\t$no\t$oo\t$r\t$m" | tee -a "$out"
done
