#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""re_parity.py — RegExp/String surface parity gate: node vs src/jsi.rs.

Every case is written ONCE in plain JS (tests/re/*.js), then run twice:
  reference:  node <file>            (ground truth)
  ours:       node js2ir.mjs <file>  → omni-render js <file.ir.json>
`console.log` output must match byte for byte. Any diff is a parity failure.

This gate exists because of a logged finding: `RegExp.prototype.exec` was wired to
`String.prototype.match` semantics — with /g/ it returned *every* match and never read
`lastIndex`, so `while ((m = re.exec(s)) !== null)` never terminated. TriadWorkspace's
`mdToHtml` spun forever on a 1.59 MB transcript. No test covered exec/lastIndex, so the
bug shipped. Anything added to the regex surface belongs here.

Also runs tests/re/engine-only/*.js: invariants with no node reference (node itself misbehaves
there), executed under omni-render only — they must exit 0 and not time out.

usage: tools/re_parity.py [case-name ...]      (no args = all cases)
exit 0 = parity, 1 = a case diverged, 2 = harness/setup problem"""
import subprocess, sys, pathlib, shutil

ROOT = pathlib.Path(__file__).resolve().parent.parent
CASES = ROOT / "tests" / "re"
# js2ir output is a build artifact (gitignored); keep it out of the case directories
BUILD = ROOT / "tests" / ".build"
BIN = ROOT / "target" / "release" / "omni-render"
JS2IR = ROOT / "tools" / "js2ir.mjs"
# js2ir.mjs does `await import(acornPath)`, which needs the ESM entry file, not the package dir
ACORN_CANDIDATES = [
    "ui/node_modules/acorn/dist/acorn.mjs",
    "node_modules/acorn/dist/acorn.mjs",
    "ui/node_modules/acorn/dist/acorn.mjs",
    "node_modules/acorn/dist/acorn.mjs",
]


def find_acorn():
    for c in ACORN_CANDIDATES:
        if pathlib.Path(c).exists():
            return c
    hits = sorted(pathlib.Path("$HOME").glob("projects/*/node_modules/acorn/dist/acorn.mjs"))
    return str(hits[0]) if hits else None


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, timeout=120, **kw)


def main(argv):
    BUILD.mkdir(parents=True, exist_ok=True)
    if not BIN.exists():
        print(f"missing {BIN} — cargo build --release first", file=sys.stderr)
        return 2
    acorn = find_acorn()
    if not acorn or not shutil.which("node"):
        print("need node + an acorn install (tried " + ", ".join(ACORN_CANDIDATES) + ")", file=sys.stderr)
        return 2

    # engine-only invariants first: a wedged invariant is the a logged finding failure mode
    eo_fails = []
    for f in sorted((CASES / "engine-only").glob("*.js")) if (CASES / "engine-only").is_dir() else []:
        ir = BUILD / f"{f.stem}.ir.json"
        conv = run(["node", str(JS2IR), str(f), acorn, str(ir)])
        if conv.returncode != 0:
            eo_fails.append(f"{f.stem}: js2ir failed: {conv.stderr.strip()[:160]}")
            continue
        try:
            got = run([str(BIN), "js", str(ir)])
        except subprocess.TimeoutExpired:
            eo_fails.append(f"{f.stem}: DID NOT TERMINATE (invariant regression)")
            continue
        bad = got.returncode != 0 or "FAIL" in got.stdout
        print(("FAIL " if bad else "ok   ") + f"engine-only/{f.stem}" + (f"  {got.stdout.strip().splitlines()[-1]}" if got.stdout.strip() else ""))
        if bad:
            eo_fails.append(f"{f.stem}: {got.stdout.strip()[:200]} {got.stderr.strip()[:200]}")

    names = argv or sorted(p.stem for p in CASES.glob("*.js"))
    fails, errs = [], []
    for name in names:
        src = CASES / f"{name}.js"
        if not src.exists():
            errs.append(f"{name}: no such case")
            continue
        ir = BUILD / f"{name}.ir.json"
        conv = run(["node", str(JS2IR), str(src), acorn, str(ir)])
        if conv.returncode != 0:
            errs.append(f"{name}: js2ir failed: {conv.stderr.strip()[:200]}")
            continue
        want = run(["node", str(src)])
        got = run([str(BIN), "js", str(ir)])
        # node writes to stdout; the engine's console.log also lands on stdout via it.out
        if got.returncode != 0:
            fails.append(f"{name}: ours exited {got.returncode}\n{'-'*60}\n{got.stdout}{got.stderr}")
        elif got.stdout != want.stdout:
            fails.append(
                f"{name}: output diverges\n{'-'*60}\n"
                + diff(want.stdout, got.stdout)
            )
        else:
            n = len(want.stdout.splitlines())
            print(f"ok   {name}  ({n} lines agree)")

    for e in errs:
        print(f"ERR  {e}")
    for e in eo_fails:
        print(f"FAIL engine-only/{e}", file=sys.stderr)
    fails.extend(eo_fails)
    for f in fails:
        print(f"FAIL {f}", file=sys.stderr)
    if fails or errs:
        return 1 if fails else 2
    print(f"re_parity: {len(names)} case(s), node parity")
    return 0


def diff(want, got):
    import difflib
    return "\n".join(
        difflib.unified_diff(
            want.splitlines(), got.splitlines(),
            fromfile="node", tofile="omni-render", lineterm="", n=1,
        )
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))