#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""DOCTOR step 3 — transplant v2 with SYMBOLIC-EQUATION binding (J 2026-10-06, readback confirmed).
Code sections → SSA equations; donor ↔ range equations matched by a NAME-FREE structural signature; ONE consistent symbol binding
derived by walking matched trees together (role binding); donor equations with no partner in the range = missing → a transplant unit
only when every input symbol is bound; emitted as code behind `crate::layout::patch_on("patch.e<id>")` at a statement boundary;
`cargo check` → patch gate → ledger. Names are used ONLY to print / emit after the binding is fixed — never to rank or bind
(patent design-around, DRX CONVERGE card: no name-text or per-position similarity).
Clean-room: SSA / dataflow equations and canonical forms are textbook compiler ideas; no code from other projects.
usage: tools/doctor-eqtransplant.py PAIRS.jsonl [--max-err 0.45] [--top 50] [--dry-run] [--gate]
"""
import json, os, re, subprocess, sys, time, hashlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WT = os.path.join(os.path.dirname(ROOT), "omni-render-doctor")
TARGET = os.path.join(ROOT, "target-doctor")
LEDGER = os.path.join(ROOT, "results", "doctor.transplants.jsonl")

# ── lexer over source text with byte offsets ──────────────────────────────────────────────────────────────────────────────────
TOK = re.compile(r"\s+|//[^\n]*|/\*.*?\*/|r#?\"(?:[^\"\\]|\\.)*\"#?|\"(?:[^\"\\]|\\.)*\"|'(?:[^'\\]|\\.)'|[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*|\d[\w.]*|\.\.=?|::|->|=>|&&|\|\||[+\-*/%]=|==|!=|<=|>=|<<|>>|[^\s]", re.S)
def lex(src, a, b):
    out = []
    for m in TOK.finditer(src, a, b):
        t = m.group(0)
        if t.isspace() or t.startswith("//") or t.startswith("/*"): continue
        out.append((t, m.start(), m.end()))
    return out

# ── expression trees (name-free signature + leaves) ───────────────────────────────────────────────────────────────────────────
# node = (op, children) ; leaf = ("sym", name) | ("num", value) | ("idx", name)  [name[K] inside a loop over K]
COMM = {"+", "*", "max", "min", "==", "!=", "&&", "||"}
class P:
    def __init__(s, toks, loopvar=None): s.t, s.i, s.lv = toks, 0, loopvar
    def peek(s, o=0): return s.t[s.i + o][0] if s.i + o < len(s.t) else None
    def eat(s, x=None):
        t = s.peek()
        if t is None: raise SyntaxError("eof")
        if x is not None and t != x: raise SyntaxError(f"want {x} got {t}")
        s.i += 1; return t
    def expr(s): return s.bin(0)
    PREC = [("||",), ("&&",), ("==", "!=", "<", ">", "<=", ">="), ("+", "-"), ("*", "/", "%")]
    def bin(s, lvl):
        if lvl == len(s.PREC): return s.unary()
        l = s.bin(lvl + 1)
        while s.peek() in s.PREC[lvl]:
            op = s.eat(); r = s.bin(lvl + 1)
            l = ("-", [l, r]) if op == "-" else (op, [l, r])
        return l
    def unary(s):
        if s.peek() == "-": s.eat(); return ("neg", [s.unary()])
        if s.peek() == "!": s.eat(); return ("not", [s.unary()])
        if s.peek() in ("&", "*"): s.eat(); return s.unary()
        return s.postfix(s.primary())
    def args(s):
        s.eat("("); a = []
        while s.peek() != ")":
            if s.peek() is None: raise SyntaxError("eof")
            a.append(s.expr())
            if s.peek() == ",": s.eat()
        s.eat(")"); return a
    def postfix(s, e):
        while True:
            t = s.peek()
            if t == "as": s.eat(); s.eat(); continue                         # casts are not the computation
            if t == "[":
                s.eat(); i = s.expr(); s.eat("]")
                if e[0] == "sym" and i == ("sym", s.lv): e = ("idx", e[1])
                else: e = ("at", [e, i])
                continue
            if t == "." and s.peek(1) and re.match(r"[A-Za-z_]", s.peek(1)):
                s.eat(); name = s.eat()
                if s.peek() == "::":                                         # turbofish: .sum::<i64>()
                    while s.peek() not in ("(", None): s.eat()
                if s.peek() == "(" and name in ("map", "filter"):           # elementwise: (0..n).map(|k| …), a.iter().zip(&b).map(|(&x, &y)| …)
                    e = s.closure_call(name, e); continue
                if s.peek() == "(":
                    a = s.args()
                    if name in ("iter", "iter_mut", "into_iter", "cloned", "copied", "collect", "clone", "to_vec") and not a: continue
                    if name == "zip": e = ("zip", [e] + a); continue
                    if name == "sum" and not a: e = ("sum", [e[1][0]] if e[0] == "vec" else [e]); continue
                    if name == "term" and len(a) == 3: e = a[2]; continue        # Layout::term(n, key, v) = v · knob(key): the value is v
                    e = (name if name in ("max", "min", "abs", "clamp") else "m." + name, [e] + a)
                else:
                    e = ("sym", (e[1] + "." + name)) if e[0] == "sym" else ("fld." + name, [e])
                continue
            return e
    def closure_call(s, name, recv):
        s.eat("("); s.eat("|"); pat = []
        while s.peek() != "|":
            t = s.eat()
            if re.match(r"[A-Za-z_]", t) and t != "mut": pat.append(t)
        s.eat("|")
        # body: parse to the closing paren of the call (with the loop variable known when the receiver is a range)
        depth = 1; start = s.i
        while depth:
            t = s.eat(); depth += (t in "([{") - (t in ")]}")
        body_toks = s.t[start:s.i - 1]
        lv = pat[0] if recv[0] == "range" and pat else None
        q = P(body_toks, lv); body = q.expr()
        if q.i != len(body_toks): raise SyntaxError("closure body")
        # bind pattern names to elements of the receiver: iter(a) → a[K]; zip(a, b) → a[K], b[K]
        srcs = recv[1] if recv[0] == "zip" else ([recv] if recv[0] != "range" else [])
        sub = {}
        for p_, r in zip(pat, srcs):
            if r[0] == "sym": sub[p_] = ("idx", r[1])
            elif r[0] == "vec": sub[p_] = r[1][0]
        body = subst(body, sub)
        if recv[0] == "vec": body = subst(body, {pat[0]: recv[1][0]}) if pat else body
        if name == "filter": return ("vec", [("guard", [body, recv[1][0] if recv[0] == "vec" else (recv if recv[0] != "range" else ("sym", "_k"))])])
        return ("vec", [body])
    def primary(s):
        t = s.peek()
        if t is None: raise SyntaxError("eof")
        if t == "(":
            s.eat(); e = s.expr()
            if s.peek() in ("..", "..="):
                s.eat(); hi = s.expr(); s.eat(")"); return ("range", [e, hi])
            if s.peek() == ",": raise SyntaxError("tuple")
            s.eat(")"); return e
        if t == "if":
            s.eat(); c = s.expr_nostruct(); th = s.block(); s.eat("else")
            el = s.primary() if s.peek() == "if" else s.block()
            return ("sel", [c, th, el])
        if t == "{": return s.block()
        if re.match(r"\d", t): s.eat(); v = t.rstrip("i32u64f64us").replace("_", ""); return ("num", v)
        if t.startswith('"') or t.startswith("r#") or t.startswith("r\""): s.eat(); return ("num", "str")
        if re.match(r"[A-Za-z_]", t):
            s.eat()
            if s.peek() == "(": return ("call." + t.split("::")[-1], s.args())
            if s.peek() == "!": raise SyntaxError("macro")
            return ("sym", t)
        raise SyntaxError(f"unexpected {t}")
    def expr_nostruct(s): return s.expr()
    def block(s):
        s.eat("{"); depth = 1; start = s.i
        while depth:
            t = s.eat()
            depth += (t == "{") - (t == "}")
        inner = s.t[start:s.i - 1]
        env = {}
        while True:                                   # leading `let x = e;` statements are inlined into the tail (SSA)
            tt = [x[0] for x in inner]
            if tt[:1] != ["let"] or ";" not in tt: break
            j = 2 if tt[1:2] == ["mut"] else 1; name = tt[j]; k = tt.index("=", j); end = k
            d = 0
            while end < len(tt) and not (d == 0 and tt[end] == ";"): d += (tt[end] in "([{") - (tt[end] in ")]}"); end += 1
            q = P(inner[k + 1:end], s.lv); v = q.expr()
            if q.i != end - k - 1: raise SyntaxError("let rhs")
            env[name] = subst(v, env); inner = inner[end + 1:]
        q = P(inner, s.lv)
        e = q.expr()
        if q.i != len(inner): raise SyntaxError("block with statements")
        return subst(e, env)

def subst(e, sub):
    if e[0] == "sym": return sub.get(e[1], e)
    if e[0] in ("idx", "num"): return e
    return (e[0], [subst(c, sub) for c in e[1]])

def subtrees(e):
    yield e
    if e[0] not in ("sym", "idx", "num"):
        for c in e[1]: yield from subtrees(c)

def sig(e):
    """Name-free structural signature: leaves → kind only; commutative ops sort children by THEIR signatures (never by names)."""
    k = e[0]
    if k == "sym": return "S"
    if k == "idx": return "I"
    if k == "num": return "N" + (e[1] if e[1] in ("0", "1", "2") else "")
    cs = [sig(c) for c in e[1]]
    if k in COMM: cs = sorted(cs)
    return k + "(" + ",".join(cs) + ")"

def ordered(e):
    """Children in signature order for commutative ops — the walk order used to pair leaves across two equal-signature trees."""
    if e[0] in ("sym", "idx", "num"): return e
    cs = [ordered(c) for c in e[1]]
    if e[0] in COMM: cs = sorted(cs, key=sig)
    return (e[0], cs)

def leaves(e):
    if e[0] in ("sym", "idx"): return [e]
    if e[0] == "num": return []
    return [l for c in e[1] for l in leaves(c)]

# ── statements → equations ────────────────────────────────────────────────────────────────────────────────────────────────────
def statements(src, a, b, loopvar=None):
    """Split [a, b) into statements at depth 0; descend into `for K in … { }` and `if c { }` bodies. Yields dicts with byte spans."""
    toks = lex(src, a, b); i = 0; out = []
    def scan_to_end(i):
        d = 0
        while i < len(toks):
            t = toks[i][0]
            if t in "([{": d += 1
            elif t in ")]}": d -= 1
            if d == 0 and t == ";": return i
            if d == 0 and t == "}" and i + 1 < len(toks) and toks[i + 1][0] not in ("else", ".", "?", ")", ",", "as", ";", "=>"):
                return i
            if d < 0: return i - 1
            i += 1
        return len(toks) - 1
    while i < len(toks):
        t = toks[i][0]
        if t == "for" and i + 2 < len(toks) and toks[i + 2][0] == "in":
            lv = toks[i + 1][0]; j = i
            while j < len(toks) and toks[j][0] != "{": j += 1
            d = 0; k = j
            while k < len(toks):
                d += (toks[k][0] == "{") - (toks[k][0] == "}")
                if d == 0: break
                k += 1
            if j >= len(toks) or k >= len(toks): break      # loop body runs past the span: stop here
            out += statements(src, toks[j][2], toks[k][1], lv)
            i = k + 1; continue
        if t == "if":
            j = i
            while j < len(toks) and toks[j][0] != "{": j += 1
            d = 0; k = j
            while k < len(toks):
                d += (toks[k][0] == "{") - (toks[k][0] == "}")
                if d == 0: break
                k += 1
            if k < len(toks) and (k + 1 >= len(toks) or toks[k + 1][0] != "else"):
                out += statements(src, toks[j][2], toks[k][1], loopvar)
                i = k + 1; continue
        e = scan_to_end(i)
        if e >= i:
            out.append({"toks": toks[i:e + 1], "a": toks[i][1], "b": toks[e][2], "lv": loopvar, "text": src[toks[i][1]:toks[e][2]]})
        i = max(e + 1, i + 1)   # always advance (a closing brace of the enclosing block ends the scan at i − 1)
    return out

def equation(st):
    """`let [mut] x = e;` · `x = e;` · `x op= e;` · `v[K] (op)= e;` → (lhs leaf, rhs tree); anything else → None (opaque)."""
    t = [x[0] for x in st["toks"]]
    if t and t[-1] == ";": t = t[:-1]; toks = st["toks"][:-1]
    else: toks = st["toks"]
    try:
        if t[:1] == ["let"]:
            j = 2 if t[1:2] == ["mut"] else 1
            if j + 1 >= len(t) or not re.match(r"[A-Za-z_]", t[j]): return None
            k = j + 1
            if t[k] == ":":
                while k < len(t) and t[k] != "=": k += 1
            if k >= len(t) or t[k] != "=": return None
            p = P(toks[k + 1:], st["lv"]); rhs = p.expr()
            if p.i != len(toks) - k - 1: return None
            return (("sym", t[j]), rhs, "let")
        for op in ("+=", "-=", "*=", "/=", "="):
            if op in t:
                k = t.index(op)
                lp = P(toks[:k], st["lv"]); lhs = lp.postfix(lp.primary())
                if lp.i != k or lhs[0] not in ("sym", "idx"): return None
                p = P(toks[k + 1:], st["lv"]); rhs = p.expr()
                if p.i != len(toks) - k - 1: return None
                if op != "=": rhs = ({"+=": "+", "-=": "-", "*=": "*", "/=": "/"}[op], [lhs, rhs])
                return (lhs, rhs, "assign")
    except (SyntaxError, IndexError):
        return None
    return None

# ── role unification ──────────────────────────────────────────────────────────────────────────────────────────────────────────
def unify(range_eqs, donor_eqs):
    """Pair equations with equal signatures (whole equation: lhs kind + rhs); walk the pair in signature order to bind donor leaves
    to range leaves; keep the binding consistent (a donor symbol bound twice to different range symbols is a conflict)."""
    bind, conflicts, matched_d, pairs = {}, 0, set(), []
    rsub = {}
    for i, (st, eq) in enumerate(range_eqs):
        for t in subtrees(eq[1]):
            if t[0] not in ("sym", "idx", "num") and len(leaves(t)) >= 2: rsub.setdefault(sig(t), []).append((i, t))
    for j, (st, eq) in enumerate(donor_eqs):
        # the donor rhs, or (for an in-place update x op= f) its update term f
        targets = [eq[1]] + ([eq[1][1][1]] if eq[2] == "assign" and eq[1][0] in ("+", "-") and eq[1][1][0] == eq[0] else [])
        hit = None
        for tgt in targets:
            c = rsub.get(sig(tgt))
            if c: hit = (c[0], tgt); break
        if not hit: continue
        (i, rt), dt = hit
        matched_d.add(j); pairs.append((i, j))
        rl = leaves(ordered(rt)); dl = leaves(ordered(dt))
        for r, d in zip(rl, dl):
            if r[0] != d[0]: conflicts += 1; continue
            if d[1] in bind and bind[d[1]] != r[1]: conflicts += 1; continue
            bind[d[1]] = r[1]
    return bind, conflicts, matched_d, pairs

def rename_text(text, bind):
    """Emit donor text under the binding (applied only after binding is fixed structurally). Longest names first; `a.b` fields too."""
    for d in sorted(bind, key=len, reverse=True):
        if d == bind[d]: continue
        text = re.sub(r"(?<![\w.])" + re.escape(d) + r"(?![\w])", "\x00" + bind[d] + "\x00", text)
    return text.replace("\x00", "")

def main():
    pairs_path = sys.argv[1]
    max_err = float(sys.argv[sys.argv.index("--max-err") + 1]) if "--max-err" in sys.argv else 0.45
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 50
    dry = "--dry-run" in sys.argv
    src = open(os.path.join(ROOT, "src/layout.rs")).read()
    line_off = [0]
    for ln in src.split("\n"): line_off.append(line_off[-1] + len(ln) + 1)
    span = lambda l0, l1: (line_off[l0 - 1], line_off[l1])
    pairs = [json.loads(l) for l in open(pairs_path)]
    pairs = [p for p in pairs if p["err"] < max_err]
    units, stats = [], {"pairs": len(pairs), "parsed_r": 0, "parsed_d": 0, "eqs_r": 0, "eqs_d": 0, "matched": 0, "missing": 0, "unbound": 0, "units": 0, "conflicts": 0}
    seen = set()
    for p in pairs:
        ra, rb = p.get("range_bytes") or span(*p["range"]); da, db = p.get("donor_bytes") or span(*p["donor"])
        try: rs = statements(src, ra, rb); ds = statements(src, da, db)
        except (IndexError, SyntaxError): stats["span_errors"] = stats.get("span_errors", 0) + 1; continue
        req = [(st, e) for st in rs if (e := equation(st))]; deq = [(st, e) for st in ds if (e := equation(st))]
        stats["eqs_r"] += len(req); stats["eqs_d"] += len(deq)
        if not req or not deq: continue
        bind, conf, matched, prs = unify(req, deq)
        stats["conflicts"] += conf; stats["matched"] += len(matched)
        if conf or not prs: continue
        range_syms = {l[1] for st, e in req for l in [e[0]] + leaves(e[1])}
        for j, (st, eq) in enumerate(deq):
            if j in matched: continue
            stats["missing"] += 1
            ins = [l[1] for l in leaves(eq[1])] + ([eq[0][1]] if eq[2] == "assign" else [])
            if eq[2] == "let" or any(x not in bind for x in ins):
                stats["unbound"] += 1; continue
            # insertion: after the range statement paired with the nearest matched donor statement before j
            prev = [i for (i, jj) in prs if jj < j]
            if not prev: stats["unbound"] += 1; continue
            host = req[max(prev)][0]
            text = rename_text(st["text"], bind)
            key = (host["b"], text)
            if key in seen: continue
            seen.add(key)
            uid = "e" + hashlib.sha1(f"{p['donor']}:{st['a']}:{host['b']}".encode()).hexdigest()[:8]
            units.append({"id": uid, "range": p["range"], "donor": p["donor"], "err": p["err"], "insert_at": host["b"],
                          "insert_line": src.count("\n", 0, host["b"]) + 1, "text": text, "donor_text": st["text"],
                          "binding": {d: bind[d] for d in sorted(bind) if d in ins}, "sig": sig(eq[1])})
    stats["units"] = len(units)
    print("eq-transplant:", json.dumps(stats))
    units.sort(key=lambda u: u["err"])
    for u in units[:8]: print(f"  {u['id']} L{u['insert_line']} err {u['err']:.3f} | {u['donor_text'].strip()[:70]!r} → {u['text'].strip()[:70]!r}")
    if dry: return
    run_units(units[:top])

def run_units(units):
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET)
    sh = lambda c, cwd, t=900: subprocess.run(c, cwd=cwd, env=env, shell=True, capture_output=True, text=True, timeout=t)
    built = 0
    for u in units:
        t0 = time.time()
        sh("git checkout -q -B doctor/eq-transplant main && git checkout -q -- src/layout.rs", WT)
        s = open(os.path.join(WT, "src/layout.rs")).read()
        body = f'\n        // DOCTOR eq-transplant {u["id"]} (step 3, dormant): donor L{u["donor"][0]}-{u["donor"][1]}, bound by equation roles\n        if crate::layout::patch_on("patch.{u["id"]}") {{ {u["text"].strip()} }}'
        open(os.path.join(WT, "src/layout.rs"), "w").write(s[:u["insert_at"]] + body + s[u["insert_at"]:])
        c = sh("cargo check --release --message-format short 2>&1 | grep -E '^src/.*error' | head -3", WT)
        row = {k: u[k] for k in ("id", "range", "donor", "err", "insert_line", "binding", "sig")}
        row.update(method="eq", text=u["text"].strip()[:300], date=time.strftime("%Y-%m-%dT%H:%M:%S"))
        if c.stdout.strip():
            row.update(outcome="build-fail", compiled=False, error=c.stdout.strip().split("\n")[0][:220])
        else:
            built += 1; row["compiled"] = True
            if "--gate" in sys.argv:
                b = sh("nice -n 10 cargo build --release 2>&1 | grep -E '^error' | head -2", WT)
                g = sh(f"{TARGET}/release/omni-render core corpus --patch-gate --program results/omr.program.json --clause 'patch.{u['id']}[*]=-1'", ROOT)
                try:
                    j = json.loads(g.stdout.strip().split("\n")[-1]); lost, gained = sum(j["lost"]), sum(j["gained"])
                    row.update(lost=lost, gained=gained, outcome="pass" if j["pass"] else ("lossy" if lost and gained else ("no-gain" if not gained else "loss")))
                except Exception as e:
                    row.update(outcome="gate-error", error=(g.stderr or str(e))[-200:])
            else:
                row["outcome"] = "compiled"
        row["secs"] = round(time.time() - t0, 1)
        with open(LEDGER, "a") as f: f.write(json.dumps(row) + "\n")
        print(f"  {u['id']} → {row['outcome']:10} {row.get('gained', '')}/{row.get('lost', '')} {row.get('error', '')[:110]} ({row['secs']}s)")
    sh("git checkout -q -- src/layout.rs", WT)
    print(f"eq-transplant: {built}/{len(units)} compiled")

if __name__ == "__main__":
    main()
