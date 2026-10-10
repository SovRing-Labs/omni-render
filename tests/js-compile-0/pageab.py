#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""JS-COMPILE-0 step 3: page A/B on the doctor-format JS bundles (corpus-js, OMR_RECORD) under OMR_REPLAY + OMR_CLOCK=virtual.
Every build runs every site K times, interleaved (min per build = least disturbed run on a loaded box). A run's fingerprint
(title, dom nodes, display items, page height, links, steps, scripts ran/failed, timers, js errors) must be identical across
all runs of all builds — same work, same result — or the row says DIFF. Speed = min js.ms (renderer JS time) and min user
CPU of the whole page command (broker + renderer).
usage: pageab.py --label L [--k 2] [--only slug,...] name=/path/to/omni-render ...   -> results/pageab-<label>.tsv"""
import argparse, json, os, re, resource, subprocess, sys, time
HERE = os.path.dirname(os.path.abspath(__file__)); ROOT = os.path.abspath(os.path.join(HERE, "../.."))
CORPUS = os.path.join(ROOT, "corpus-js")

def site_url(d):
    m = json.load(open(os.path.join(d, "manifest.json"))); return m.get("url")

def run(bin_, d, url):
    env = dict(os.environ, OMR_REPLAY=d, OMR_CLOCK="virtual")
    r0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    p = subprocess.run(["timeout", "300", bin_, "page", url], env=env, capture_output=True, text=True)
    r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    user = (r1.ru_utime - r0.ru_utime)
    meta = next((l[5:] for l in p.stdout.splitlines() if l.startswith("meta ")), None)
    if not meta: return None, user, None
    m = json.loads(meta); j = m.get("js") or {}
    fp = (m.get("title"), m.get("dom_nodes"), m.get("items"), m.get("page_h"), m.get("links"), j.get("steps"), j.get("scripts_ran"),
          j.get("scripts_failed"), j.get("timers_ran"), json.dumps(j.get("errors"), sort_keys=True))
    return fp, user, j.get("ms")

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--label", required=True); ap.add_argument("--k", type=int, default=2); ap.add_argument("--only", default="")
    ap.add_argument("bins", nargs="+"); a = ap.parse_args()
    bins = [b.split("=", 1) for b in a.bins]
    sites = sorted(x for x in os.listdir(CORPUS) if os.path.isfile(os.path.join(CORPUS, x, "exchanges.jsonl")))
    if a.only: sites = [s for s in sites if s in a.only.split(",")]
    out = os.path.join(HERE, "results", f"pageab-{a.label}.tsv"); os.makedirs(os.path.dirname(out), exist_ok=True)
    revs = {n: subprocess.run(["git", "-C", os.path.dirname(os.path.dirname(os.path.dirname(b))), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip() for n, b in bins}
    hdr = ["site", "steps", "same"] + [f"{n}_js_ms" for n, _ in bins] + [f"{n}_user_s" for n, _ in bins] + [f"js_x_{n}" for n, _ in bins[1:]]
    with open(out, "w") as f:
        f.write(f"# pageab {a.label} {time.strftime('%Y-%m-%dT%H:%M:%S')} k={a.k} load={os.getloadavg()[0]:.1f} " + " ".join(f"{n}={b}@{revs[n]}" for n, b in bins) + "\n")
        f.write("\t".join(hdr) + "\n"); print("\t".join(hdr), flush=True)
        tot = {n: 0.0 for n, _ in bins}
        for s in sites:
            d = os.path.join(CORPUS, s); url = site_url(d)
            best = {n: (1e18, 1e18) for n, _ in bins}; fps = set()
            for _ in range(a.k):
                for n, b in bins:
                    fp, user, ms = run(b, d, url)
                    fps.add(fp)
                    if ms is not None: best[n] = (min(best[n][0], ms), min(best[n][1], user))
            if any(v[0] >= 1e18 for v in best.values()):
                row = [s, "-", "NO-META"]
            else:
                steps = next(iter(fps))[5] if len(fps) == 1 and None not in fps else "-"
                same = "yes" if len(fps) == 1 and None not in fps else "DIFF"
                base_ms = best[bins[0][0]][0]
                for n, _ in bins: tot[n] += best[n][0]
                row = [s, str(steps), same] + [f"{best[n][0]:.0f}" for n, _ in bins] + [f"{best[n][1]:.2f}" for n, _ in bins] + [f"{base_ms / max(best[n][0], 1e-9):.2f}" for n, _ in bins[1:]]
            f.write("\t".join(row) + "\n"); f.flush(); print("\t".join(row), flush=True)
        b0 = tot[bins[0][0]]
        row = ["TOTAL", "", ""] + [f"{tot[n]:.0f}" for n, _ in bins] + ["" for _ in bins] + [f"{b0 / max(tot[n], 1e-9):.2f}" for n, _ in bins[1:]]
        f.write("\t".join(row) + "\n"); print("\t".join(row))
    print("PAGEAB-DONE", out)

if __name__ == "__main__": main()
