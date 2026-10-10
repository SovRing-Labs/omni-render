#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# JS-COMPILE-0 steps 1+2: record each site's exact network bytes into a doctor-format bundle (corpus-js/<slug>, OMR_RECORD),
# then replay it twice with the virtual clock (OMR_REPLAY + OMR_CLOCK=virtual) and check both runs do identical work.
# usage: corpus_js.sh <bin> [sites.txt] [--replay-only]  -> results/corpus-js-check.tsv
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); bin=$1; sites=${2:-$here/sites.txt}; mode=${3:-}
root=$(cd "$here/../.." && pwd)/corpus-js; mkdir -p "$root" "$here/results"
out="$here/results/corpus-js-check.tsv"
echo -e "# $(date -Is) bin=$bin@$(git -C "$(dirname "$bin")/../.." rev-parse --short HEAD)" > "$out"
echo -e "slug\ttitle\tscripts_ran\tfailed\tsteps_r1\tsteps_r2\tnodes_r1\tnodes_r2\tmisses\tsame" | tee -a "$out"
meta() { grep '^meta' | cut -c6- | python3 -c 'import json,sys
try:
  m=json.load(sys.stdin); j=m.get("js") or {}; print((m.get("title") or "-").replace("\t"," ")[:40], j.get("scripts_ran",0), j.get("scripts_failed",0), j.get("steps",0), m.get("dom_nodes",0), sep="\t")
except Exception: print("-\t0\t0\t0\t0")'; }
while read -r url; do
  [ -z "$url" ] && continue
  slug=$(echo "$url" | sed -E 's#https?://##; s#[^A-Za-z0-9]+#-#g; s#-+$##' | cut -c1-60); d="$root/$slug"
  if [ "$mode" != "--replay-only" ]; then rm -rf "$d"; OMR_RECORD="$d" timeout 150 "$bin" page "$url" > /dev/null 2>&1; fi
  [ -f "$d/exchanges.jsonl" ] || { echo -e "$slug\tNOT-RECORDED" | tee -a "$out"; continue; }
  r1=$(OMR_REPLAY="$d" OMR_CLOCK=virtual timeout 150 "$bin" page "$url" 2>"$d.r1.err" | meta)
  r2=$(OMR_REPLAY="$d" OMR_CLOCK=virtual timeout 150 "$bin" page "$url" 2>/dev/null | meta)
  miss=$(grep -c "replay miss" "$d.r1.err"); rm -f "$d.r1.err"
  IFS=$'\t' read -r t1 sr1 f1 s1 n1 <<< "$r1"; IFS=$'\t' read -r t2 sr2 f2 s2 n2 <<< "$r2"
  same=$([ "$s1" = "$s2" ] && [ "$n1" = "$n2" ] && echo yes || echo NO)
  echo -e "$slug\t$t1\t$sr1\t$f1\t$s1\t$s2\t$n1\t$n2\t$miss\t$same" | tee -a "$out"
done < "$sites"
echo CORPUS-JS-DONE
