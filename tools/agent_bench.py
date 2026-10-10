#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""agent_bench.py — RENDER step 4: omni-render vs Lightpanda vs headless Chrome on the same replayed pages.

Each engine runs as its own transient systemd unit (cgroup), so memory peak = everything that engine spawned
(Chrome's renderer/GPU/utility processes included). Two shapes:
  seq   one worker loads all pages in order
  par25 25 concurrent workers, one page each, until all pages are done
Output: results/agent_bench.json + a table on stdout. Requires tools/replay_server.py on :8931.
"""
import json, re, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
URLS = [u for u in (ROOT / "results/agent-urls.txt").read_text().split() if u]
OMR = str(ROOT / "target/release/omni-render")
LP = str(Path.home() / "bin/lightpanda")
CHROME = "google-chrome"
N_PAR = 25


CG = Path("/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice")


def run(name, cmd):
    """Start cmd in a transient unit; poll its cgroup memory.peak (systemd's --wait summary misreads it here)."""
    unit = f"omr-bench-{name}.service"
    subprocess.run(["systemctl", "--user", "reset-failed", unit], capture_output=True)
    t = time.time()
    cap = "5G" if "chrome" in name else "4G"
    subprocess.run(["systemd-run", "--user", "--collect", f"--unit={unit}", "-p", f"MemoryMax={cap}", "-p", "MemorySwapMax=0",
                    "sh", "-c", cmd], check=True, capture_output=True)
    cg = None
    peak = 0
    while True:
        st = subprocess.run(["systemctl", "--user", "show", unit, "-p", "ActiveState", "-p", "ControlGroup"], capture_output=True, text=True).stdout
        state = re.search(r"ActiveState=(\w+)", st).group(1)
        g = re.search(r"ControlGroup=(\S+)", st)
        if g and g.group(1):
            cg = Path("/sys/fs/cgroup") / g.group(1).lstrip("/")
        if cg and cg.exists():
            try:
                peak = max(peak, int((cg / "memory.peak").read_text()))
            except OSError:
                pass
        if state not in ("active", "activating", "deactivating"):
            break
        time.sleep(0.02)
    return time.time() - t, peak, "", ""


def main():
    lst = ROOT / "results/agent-urls.txt"
    urls = " ".join(URLS)
    out = ROOT / "results/agent-out"
    for e in ("omr", "lp", "chrome"):
        (out / e).mkdir(parents=True, exist_ok=True)
    jobs = {
        "omr-seq": f"{OMR} agent @{lst} --out {out}/omr",
        "lp-seq": f"{LP} fetch --dump markdown --json {urls} > {out}/lp/all.json 2>/dev/null",
        "chrome-seq": "for u in " + urls + f"; do {CHROME} --headless=new --disable-gpu --no-first-run --user-data-dir=/tmp/omr-chrome-prof --dump-dom \"$u\" > /dev/null 2>&1; done",
        "omr-par25": f"cat {lst} | xargs -P {N_PAR} -I{{}} {OMR} agent {{}} > /dev/null 2>&1",
        "lp-par25": f"cat {lst} | xargs -P {N_PAR} -I{{}} {LP} fetch --dump markdown {{}} > /dev/null 2>&1",
        "chrome-par25": f"cat {lst} | xargs -P {N_PAR} -I{{}} sh -c '{CHROME} --headless=new --disable-gpu --no-first-run --user-data-dir=$(mktemp -d) --dump-dom \"{{}}\" > /dev/null 2>&1'",
    }
    only = sys.argv[1:] or list(jobs)
    res = {}
    for name in only:
        wall, peak, so, se = run(name, jobs[name])
        st = subprocess.run(["systemctl", "--user", "show", f"omr-bench-{name}.service", "-p", "Result"], capture_output=True, text=True).stdout.strip()
        res[name] = {"result": st, "wall_s": wall, "pages": len(URLS), "pages_per_s": len(URLS) / wall, "mem_peak_mb": (peak or 0) / (1 << 20)}
        print(f"{name:<14} {wall:7.1f} s  {len(URLS)/wall:7.2f} pages/s  peak {res[name]['mem_peak_mb']:8.1f} MB  {st}", flush=True)
    prev = {}
    f = ROOT / "results/agent_bench.json"
    if f.exists():
        prev = json.loads(f.read_text())
    prev.update(res)
    f.write_text(json.dumps(prev, indent=1))


if __name__ == "__main__":
    main()
