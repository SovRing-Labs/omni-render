#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""DOCTOR D8 — automatic transplant (ssFix-style, clean-room from Xin & Reiss 2017 / Le Goues GenProg 2012: reuse code that
already exists elsewhere in the program). For each unit from `omni-render core --donors --units-out U.jsonl`:
  1. on the worktree (branch doctor/transplants, reset to main each time) insert the renamed donor statement after
     `insert_after`, wrapped in a DORMANT switch: `if self.patch(n, "patch.<id>") { <unit> }`
  2. build (CARGO_TARGET_DIR=target-doctor); a compile error is an outcome (negative knowledge), not a crash
  3. gate: `core corpus --patch-gate --clause patch.<id>[*]=-1` (base program vs base + the switch on, lost 0 and gained > 0)
  4. append the outcome to results/doctor.transplants.jsonl (the SimFix-style prior reads it: build-fail / no-gain / lossy / pass)
Passing units stay committed on their own branch doctor/t-<id> for J to merge.
usage: tools/doctor-transplant.py U.jsonl [--top 6] [--classes guard,term-add,clamp,loop,other]
"""
import json, os, subprocess, sys, time

MAIN = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WT = os.path.join(os.path.dirname(MAIN), "omni-render-doctor")
TARGET = os.path.join(MAIN, "target-doctor")
LEDGER = os.path.join(MAIN, "results", "doctor.transplants.jsonl")


def sh(cmd, cwd, env=None, timeout=900):
    return subprocess.run(cmd, cwd=cwd, env=env, shell=True, capture_output=True, text=True, timeout=timeout)


def main():
    units_path = sys.argv[1]
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 6
    classes = set((sys.argv[sys.argv.index("--classes") + 1] if "--classes" in sys.argv else "guard,term-add,clamp,loop,other").split(","))
    units = [json.loads(l) for l in open(units_path) if l.strip()]
    done = {json.loads(l)["id"] for l in open(LEDGER)} if os.path.exists(LEDGER) else set()
    picked = [u for u in units if u["class"] in classes and u["id"] not in done and u["text"].rstrip().endswith((";", "}"))
              and not any(k in u["text"] for k in ("return", "break", "continue", "eprintln!", "println!", "dbg!"))][:top]
    print(f"transplant: {len(picked)} of {len(units)} units (classes {sorted(classes)}, {len(done)} already in the ledger)")
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET)
    for u in picked:
        t0 = time.time()
        sh("git checkout -q -B doctor/transplants main && git checkout -q -- src/layout.rs", WT)
        src = open(os.path.join(WT, "src/layout.rs")).read().split("\n")
        at = int(u["insert_after"])  # 1-based line; insert after it
        indent = len(src[at - 1]) - len(src[at - 1].lstrip())
        body = [f'{" " * indent}// DOCTOR transplant {u["id"]} (D8, dormant): donor L{u["unit"][0]}-{u["unit"][1]}, class {u["class"]}, collage err {u["err"]:.3f}',
                f'{" " * indent}if self.patch(n, "patch.{u["id"]}") {{'] + [" " * (indent + 4) + l.strip() for l in u["text"].split("\n")] + [f'{" " * indent}}}']
        open(os.path.join(WT, "src/layout.rs"), "w").write("\n".join(src[:at] + body + src[at:]))
        b = sh("nice -n 10 cargo build --release 2>&1 | grep -E '^error' -A3 | head -6", WT, env)
        row = {"id": u["id"], "class": u["class"], "range": u["range"], "donor": u["donor"], "unit": u["unit"], "insert_after": at, "err": u["err"],
               "date": time.strftime("%Y-%m-%dT%H:%M:%S")}
        if b.stdout.strip():
            row.update(outcome="build-fail", error=b.stdout.strip().split("\n")[0][:200])
        else:
            g = sh(f"{TARGET}/release/omni-render core corpus --patch-gate --program results/omr.program.json --clause 'patch.{u['id']}[*]=-1'", MAIN)
            try:
                j = json.loads(g.stdout.strip().split("\n")[-1])
                lost, gained = sum(j["lost"]), sum(j["gained"])
                row.update(lost=lost, gained=gained, outcome="pass" if j["pass"] else ("lossy" if lost and gained else ("no-gain" if not gained else "loss")))
            except Exception as e:  # noqa: BLE001 — a gate that cannot run is an outcome too
                row.update(outcome="gate-error", error=(g.stderr or str(e))[-200:])
            if row["outcome"] == "pass":
                sh(f"git checkout -q -B doctor/t-{u['id']} && git commit -qam 'doctor D8 transplant {u['id']} (dormant patch.{u['id']}): gate +{row['gained']}, 0 lost' "
                   "-m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'", WT)
        row["secs"] = round(time.time() - t0, 1)
        with open(LEDGER, "a") as f:
            f.write(json.dumps(row) + "\n")
        print(f"  {u['id']} {u['class']:8} → {row['outcome']:10} {row.get('gained', '')}/{row.get('lost', '')} {row.get('error', '')[:100]} ({row['secs']}s)")
    sh("git checkout -q -- src/layout.rs", WT)


if __name__ == "__main__":
    main()
