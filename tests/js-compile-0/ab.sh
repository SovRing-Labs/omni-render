#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# JS-COMPILE-0 A/B: each bench run base / new interleaved K times (default 3); min user-CPU per side (the least
# disturbed run on a shared, loaded machine); outputs of every run compared with node. -> results/ab-<label>.tsv
# usage: ab.sh <base bin> <new bin> <label> [K] [bench-glob]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); base=$1; new=$2; label=$3; K=${4:-3}; glob=${5:-b*}
acorn=$HOME/projects/opencode-launcher/ui/node_modules/acorn/dist/acorn.mjs; front=$HOME/ai-stack/projects/omni-render/tools/js2ir.mjs
mkdir -p "$here/.ir" "$here/results"; out="$here/results/ab-$label.tsv"
rb=$(git -C "$(dirname "$base")/../.." rev-parse --short HEAD); rn=$(git -C "$(dirname "$new")/../.." rev-parse --short HEAD)
echo -e "# ab $label base=$base@$rb new=$new@$rn K=$K $(date -Is) load=$(cut -d' ' -f1 /proc/loadavg) env: ${ABENV:-}" > "$out"
echo -e "bench\tbase_min_s\tnew_min_s\tspeedup\toutputs" | tee -a "$out"
for f in "$here"/bench/$glob.js; do
  b=$(basename "$f" .js); ir="$here/.ir/$b.ir.json"
  [ "$ir" -nt "$f" ] || timeout 60 node "$front" "$f" "$acorn" "$ir"
  [ -s "$here/.ir/$b.node.out" ] && [ "$here/.ir/$b.node.out" -nt "$f" ] || timeout 120 node "$f" > "$here/.ir/$b.node.out"
  mb=999; mn=999; ok=ok
  for k in $(seq "$K"); do
    for side in base new; do
      bin=${!side}
      t=$( { /usr/bin/time -f "%U" env ${ABENV:-} timeout 300 "$bin" js "$ir" > "$here/.ir/$b.$side.out"; } 2>&1 | tail -1)
      cmp -s "$here/.ir/$b.node.out" "$here/.ir/$b.$side.out" || ok=DIFF-$side
      if [ $side = base ]; then mb=$(awk -v a=$mb -v b=$t 'BEGIN{print (b<a)?b:a}'); else mn=$(awk -v a=$mn -v b=$t 'BEGIN{print (b<a)?b:a}'); fi
    done
  done
  echo -e "$b\t$mb\t$mn\t$(awk -v a=$mb -v b=$mn 'BEGIN{printf "%.2f", (b>0)? a/b : 0}')\t$ok" | tee -a "$out"
done
