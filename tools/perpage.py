#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""perpage.py <log> [<baseline log>] — per-page layout agreement from a render log; with a baseline, the diff."""
import re, sys
def load(f):
    d = {}
    for l in open(f):
        m = re.match(r'^(\S+)\s+layout .*SSIM vp ([0-9.]+).*?layout\s+([0-9.]+)% of (\d+)', l)
        if m: d[m.group(1)] = (float(m.group(3)), int(m.group(4)), float(m.group(2)))
    return d
a = load(sys.argv[1])
if len(sys.argv) > 2:
    b = load(sys.argv[2])
    rows = sorted(((a[k][0] - b[k][0], k) for k in a if k in b), reverse=True)
    for dlt, k in rows:
        if abs(dlt) >= 0.5: print(f"{dlt:+6.1f}  {k[:44]:<44} {b[k][0]:5.1f} -> {a[k][0]:5.1f}  (n={a[k][1]})")
else:
    for k, v in sorted(a.items(), key=lambda x: x[1][0]): print(f"{v[0]:5.1f}% n={v[1]:<5} ssim {v[2]:.3f}  {k}")
