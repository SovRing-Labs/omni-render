#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""REPAIR-CORE plan v3 steps I + J (v0).
I: every agent commit that touched src/layout.rs becomes a agent-fix row (commit, message, hunks) — the head's verified traces.
J: Getafix-style mining (Bader et al., OOPSLA 2019): each changed line pair (removed → added) is anti-unified at token level —
   identifiers become ?id, numbers ?n, the unchanged context stays, the edit is kept as the template body; templates seen in
   >= 2 different commits are fix templates the PATCH station can apply. Usage: tools/seatfix.py [--since 2026-10-03]."""
import json, re, subprocess, sys, difflib, collections
since = sys.argv[sys.argv.index("--since") + 1] if "--since" in sys.argv else "2026-10-03"
git = lambda *a: subprocess.run(["git", *a], capture_output=True, text=True, check=True).stdout
TOK = re.compile(r"[A-Za-z_][A-Za-z0-9_]*|\d+|==|!=|<=|>=|&&|\|\||->|=>|::|[^\sA-Za-z0-9_]")
KW = set("if else let mut match for in while loop return fn self Some None true false as continue break max min clamp abs unwrap_or".split())
def gen(tok):
    if tok in KW: return tok
    if tok[0].isdigit(): return "?n"
    if tok[0].isalpha() or tok[0] == "_": return "?id"
    return tok
def genseq(ts):
    """generalise locals only: names after '.' or '::' (methods, fields, enum variants) are the semantics and stay."""
    return [t if (k > 0 and ts[k - 1] in (".", "::")) else gen(t) for k, t in enumerate(ts)]
def anti(a, b):
    """token-level anti-unification of a removed line a and its added line b → (shape of the edit)."""
    ta, tb = TOK.findall(a), TOK.findall(b)
    ga, gb = genseq(ta), genseq(tb)
    sm = difflib.SequenceMatcher(a=ga, b=gb, autojunk=False)
    out = []
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op == "equal": out.append("…" if not out or out[-1] != "…" else None)
        else: out.append("[" + " ".join(ga[i1:i2]) + " → " + " ".join(gb[j1:j2]) + "]")
    return " ".join(x for x in out if x)
rows, tmpl = [], collections.defaultdict(list)
for line in git("log", f"--since={since}", "--format=%h\t%s", "--", "src/layout.rs").splitlines():
    h, msg = line.split("\t", 1)
    diff = git("show", "--format=", "-U0", h, "--", "src/layout.rs")
    hunks, rem, add = [], [], []
    def flush():
        if rem or add: hunks.append((list(rem), list(add)))
        rem.clear(); add.clear()
    for l in diff.splitlines():
        if l.startswith("@@"): flush()
        elif l.startswith("-") and not l.startswith("---"): rem.append(l[1:].strip())
        elif l.startswith("+") and not l.startswith("+++"): add.append(l[1:].strip())
    flush()
    shapes = []
    for r, a in hunks:
        pairs = []
        for x in r:                                               # pair each removed line with its most similar added line
            best = max(a, key=lambda y: difflib.SequenceMatcher(a=x, b=y, autojunk=False).ratio(), default=None)
            if best is not None and best != x and difflib.SequenceMatcher(a=x, b=best, autojunk=False).ratio() > 0.6: pairs.append((x, best))
        for r0, a0 in pairs:
            r, a = [r0], [a0]
            s = anti(r[0], a[0])
            body = re.sub(r"[…\[\]→,:?a-z ]", "", s.replace("?id", "").replace("?n", "").replace("mut", ""))
            core = " ".join(re.findall(r"\[[^\]]*\]", s))          # the edit itself, context dropped (the template body)
            if 0 < len(core) < 120 and body: shapes.append(s); tmpl[core].append((h, r[0][:100], a[0][:100]))
        if not r and a:
            for x in a:
                if x.startswith("if ") or " if " in x[:40]: shapes.append("[+ guard] " + " ".join(gen(t) for t in TOK.findall(x))[:120])
    rows.append({"commit": h, "msg": msg, "hunks": len(hunks), "lines_removed": sum(len(r) for r, _ in hunks), "lines_added": sum(len(a) for _, a in hunks), "edit_shapes": shapes[:40]})
with open("results/omr.program.seatfix.jsonl", "w") as f:
    for r in rows: f.write(json.dumps(r) + "\n")
mined = sorted(((s, sorted({x[0] for x in c})) for s, c in tmpl.items() if len({x[0] for x in c}) >= 2), key=lambda x: -len(x[1]))
json.dump([{"template": s, "commits": c, "uses": len(tmpl[s]), "examples": [{"commit": x[0], "before": x[1], "after": x[2]} for x in tmpl[s][:4]]} for s, c in mined], open("results/omr.templates.json", "w"), indent=1)
print(f"agent-fix rows {len(rows)} · single-line edits {sum(len(v) for v in tmpl.values())} · templates (>= 2 commits) {len(mined)}")
for s, c in mined[:15]: print(f"  {len(c)} commits · {len(tmpl[s])} uses · {s[:150]}")
# coarse template classes (the PATCH station instantiates these on ledger terms; the exact shapes above are their instances)
CLS = [("clamp: wrap a term in max/min/clamp", re.compile(r"→[^\]]*\.\s*(max|min|clamp)\s*\(")),
       ("guard: add or narrow a condition", re.compile(r"→[^\]]*(\bif\b|&&|\|\||matches)")),
       ("term: add an additive term", re.compile(r"\[\s*→\s*[+-]\s")),
       ("swap: read a different field / variant", re.compile(r"\[\s*\.\s*\w+[^\]]*→\s*\.\s*\w+")),
       ("parent: read the parent / containing block", re.compile(r"→[^\]]*(parent|cb_node|cbh|pos_stack)")),
       ("zero/skip: drop a contribution", re.compile(r"→[^\]]*(\b0\b|continue|unwrap_or\s*\(\s*0)"))]
cls = collections.defaultdict(set); ex = collections.defaultdict(list)
for r in rows:
    for s in r["edit_shapes"]:
        for name, rx in CLS:
            if rx.search(s): cls[name].add(r["commit"]); ex[name].append(s[:110])
out = [{"class": n, "commits": sorted(cls[n]), "n_commits": len(cls[n]), "examples": ex[n][:3]} for n, _ in CLS]
json.dump(out, open("results/omr.template-classes.json", "w"), indent=1)
print("template classes (commits that used each):")
for o in out: print(f"  {o['n_commits']:2d} · {o['class']}")
