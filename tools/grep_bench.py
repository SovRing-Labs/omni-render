# SPDX-License-Identifier: MIT OR Apache-2.0
import os, re, subprocess, sys, time
ROOT = "docs"; BIN = "target/release/omni-render"
files = sorted(os.path.join(d, f) for d, _, fs in os.walk(ROOT) for f in fs)
texts = [open(f, "rb").read().decode("utf-8", "replace") for f in files]
pats = [("omnid", ""), (r"\d{3,}-\d+", ""), ("error|fail|warn", "i"), ("sift.*kernel", ""), (r"^#+ ", ""), (r"[A-Z][a-z]+ing\b", "")]
print(f"corpus: {len(files)} files, {sum(len(t) for t in texts)/1e6:.1f} MB")
for p, fl in pats:
    rx = re.compile(p, re.I if "i" in fl else 0)
    t = time.perf_counter(); n = sum(1 for tx in texts for line in tx.split("\n") if rx.search(line)); py = (time.perf_counter() - t) * 1e3
    args = [BIN, "grep", "-c"] + (["-i"] if "i" in fl else []) + [p, ROOT]
    r = subprocess.run(args, capture_output=True, text=True); ours_n = sum(int(l.rsplit(":", 1)[1]) for l in r.stdout.splitlines() if l)
    info = r.stderr.strip().splitlines()[-1]; ms = float(info.split("·")[-1].split()[0]); eng = info.split("engine ")[1].split(" ·")[0][:22]
    t = time.perf_counter(); rg = subprocess.run(["rg", "-c", "--no-filename", "-uuu"] + (["-i"] if "i" in fl else []) + [p, ROOT], capture_output=True, text=True); rgms = (time.perf_counter() - t) * 1e3
    rg_n = sum(int(x) for x in rg.stdout.split())
    print(f"/{p}/{fl:<2} python {n:>6} lines {py:7.1f} ms | ours ({eng:<11}) {ours_n:>6} {ms:7.1f} ms = {py/ms:5.1f}x python | rg {rg_n:>6} {rgms:6.1f} ms (process)")
