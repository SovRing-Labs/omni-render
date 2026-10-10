#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# JS-COMPILE-0 C3 real-page check: YouTube with every function compiled on first call (+SLOTCHECK) vs the JIT off.
# Compares the page's js summary (meta.js) and counts panics. -> results/ytjit-<label>-{jit,nojit}.{meta,err}
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd); bin=$1; label=$2; url=${3:-https://www.youtube.com/}
mkdir -p "$here/results"
for mode in nojit jit; do
  if [ $mode = jit ]; then envs="OMR_JIT_N=1 OMR_SLOTCHECK=1"; else envs="OMR_JIT=0"; fi
  echo "== $mode ($envs) load=$(cut -d' ' -f1 /proc/loadavg)"
  /usr/bin/time -f "user %U s wall %e s" env $envs timeout 400 "$bin" page "$url" 2> "$here/results/ytjit-$label-$mode.err" | grep '^meta' | cut -c6- > "$here/results/ytjit-$label-$mode.meta" || echo "exit $?"
  tail -1 "$here/results/ytjit-$label-$mode.err"
  python3 -c 'import json,sys; m=json.load(open(sys.argv[1])); j=m.get("js"); print("title:", m.get("title")); print("js:", json.dumps(j)[:600] if j is not None else None)' "$here/results/ytjit-$label-$mode.meta" || true
  echo "panics: $(grep -c panicked "$here/results/ytjit-$label-$mode.err" || true)  slotcheck: $(grep -c SLOTCHECK "$here/results/ytjit-$label-$mode.err" || true)"
done
echo YTJIT-DONE
