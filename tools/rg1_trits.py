#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Export the trained ternary rows of rg1 arm A (cal3.LM, BitLinear absmean) as golden heads for JITW-0.

Trits are produced exactly as ternary.BitLinear.export does (absmean, round, clamp to {-1,0,1}). Output (little endian):
  b"TRIT" u32 count, then per matrix: u32 rows, u32 cols, rows*cols int8.  Default -> results/rg1_trits.bin (not committed).
"""
import os
import struct
import sys

import torch

sys.path.insert(0, os.path.expanduser("~/ai-stack/an unpublished file"))
from ternary import ternary_weight  # noqa: E402

src = os.path.expanduser(sys.argv[1] if len(sys.argv) > 1 else "~/gpurun-out-1003/rg1/rg1.armA.pt")
dst = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "results", "rg1_trits.bin")
sd = torch.load(src, map_location="cpu", weights_only=False)
mats = [(k, v) for k, v in sd.items() if k.startswith("blocks.") and k.endswith(".weight") and v.dim() == 2]
with open(dst, "wb") as f:
    f.write(b"TRIT" + struct.pack("<I", len(mats)))
    zeros = total = 0
    for k, w in mats:
        t, _ = ternary_weight(w.float())
        t = t.to(torch.int8).contiguous()
        f.write(struct.pack("<II", *t.shape) + t.numpy().tobytes())
        zeros += int((t == 0).sum()); total += t.numel()
print(f"{len(mats)} matrices, {total} trits, zero fraction {zeros / total:.4f} -> {os.path.abspath(dst)}")
