#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""oracle_synth.py — the doctor learns Chrome's layout equations from Chrome itself, as a black box (J 2026-10-06, readback
confirmed: "have the doctor just use the blackbox itself, and find the equation and symbol that it takes instead of the code,
and rebuild it").

Oracle-guided inductive synthesis (Jha et al., ICSE 2010 "oracle-guided component-based program synthesis"; system
identification): a TARGET is an HTML template whose CSS / attribute values are symbols a0, a1 …; Chrome (headless, JS off,
1280×800) is the oracle — we observe only the output box of one element (DevTools DOMSnapshot), in LayoutUnits (1/64 px).
Probes per target: 8 examples (the builder's training set) + 40 oracle answers the CEGIS loop draws counterexamples from +
40 validation answers the builder never sees. The equation is found by the doctor's frame-first builder
(`omni-render core --abc need --build-all --ff`), verified by rustc on every answer, then checked on the unseen 40.
Clean-room: Chrome is observed, never read.

Usage: tools/oracle_synth.py [target …] [--list]      results → results/oracle/<target>.json + needs file
"""
import json, random, subprocess, sys, os
from pathlib import Path
from playwright.sync_api import sync_playwright
sys.path.insert(0, str(Path(__file__).resolve().parent))
from chrome_blackbox import boxes, doc   # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "results" / "oracle"
OMR = os.environ.get("OMR_BIN", str(ROOT / "target-doctor" / "release" / "omni-render"))
AXIS = {"x": 0, "y": 1, "w": 2, "h": 3}

# name: (symbols with ranges, html template, (element id or tag, axis))
TARGETS = {
    "meter-width":      ([("font_px", 6, 48)], '<meter id="x" value="0.5" style="font-size:{a0}px"></meter>', ("x", "w")),
    "meter-height":     ([("font_px", 6, 48)], '<meter id="x" value="0.5" style="font-size:{a0}px"></meter>', ("x", "h")),
    "progress-width":   ([("font_px", 6, 48)], '<progress id="x" value="0.5" style="font-size:{a0}px"></progress>', ("x", "w")),
    "progress-height":  ([("font_px", 6, 48)], '<progress id="x" value="0.5" style="font-size:{a0}px"></progress>', ("x", "h")),
    "textarea-width":   ([("cols", 1, 120)], '<textarea id="x" cols="{a0}"></textarea>', ("x", "w")),
    "textarea-height":  ([("rows", 1, 40)], '<textarea id="x" rows="{a0}"></textarea>', ("x", "h")),
    "input-size-width": ([("size", 1, 120)], '<input id="x" type="text" size="{a0}">', ("x", "w")),
    "body-collapse-y":  ([("body_margin", 0, 40), ("child_margin_top", -30, 60)],
                         '<body style="margin:{a0}px"><div id="x" style="margin-top:{a1}px;height:10px"></div></body>', ("body", "y")),
    "legend-width":     ([("pad_px", 0, 40)], '<fieldset><legend id="x" style="padding:0 {a0}px">L</legend>f</fieldset>', ("x", "w")),
    # BUGLIB tables-01 (colspan distribution, not fully specified by CSS 2.2 §17.5.2.2): two columns whose max-content widths are
    # w1, w2 and a spanning cell of max-content S; no padding / spacing so the law is the distribution alone
    "table-span-col1":  ([("w1", 10, 200), ("w2", 10, 200), ("span", 20, 800)],
                         '<table style="border-spacing:0"><tr><td id="x" style="padding:0"><span style="display:inline-block;width:{a0}px"></span></td>'
                         '<td style="padding:0"><span style="display:inline-block;width:{a1}px"></span></td></tr><tr><td colspan="2" style="padding:0">'
                         '<span style="display:inline-block;width:{a2}px"></span></td></tr></table>', ("x", "w")),
}


def measure(browser, html, key):
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "p.html"; p.write_text(doc(html) if "<body" not in html else "<!doctype html><html><head><meta charset=\"utf-8\"></head>" + html + "</html>")
        ctx = browser.new_context(java_script_enabled=False, viewport={"width": 1280, "height": 800})
        pg = ctx.new_page(); pg.goto(p.as_uri(), wait_until="load")
        snap = ctx.new_cdp_session(pg).send("DOMSnapshot.captureSnapshot", {"computedStyles": []})
        ctx.close()
    who, ax = key
    for e in boxes(snap):
        if e["id"] == who or (who in ("body", "html") and e["tag"] == who):
            return round(e["box"][AXIS[ax]] * 64)
    return None


def measure_ours(html, key):
    """the same probe through OUR engine (omni-render explain --json on a one-page bundle), in 1/64 px"""
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        d = Path(td); (d / "page.html").write_text(doc(html) if "<body" not in html else "<!doctype html><html><head><meta charset=\"utf-8\"></head>" + html + "</html>")
        (d / "manifest.json").write_text(json.dumps({"url": (d / "page.html").as_uri(), "final_url": (d / "page.html").as_uri(), "res": {}}))
        who, ax = key; sel = "#" + who if who not in ("body", "html") else who
        r = subprocess.run([OMR, "explain", str(d), sel, "--json"], capture_output=True, text=True,
                           env={**os.environ, "OMR_PROGRAM": str(ROOT / "results" / "omr.program.json")})
        try: o = json.loads(r.stdout)[0]["ours"]
        except Exception: return None
        return None if o is None else round(o[AXIS[ax]] * 64)


def run(name, browser, seed=1):
    syms, tpl, key = TARGETS[name]
    R = random.Random(seed)
    pts, seen = [], set()
    space = 1
    for _, lo, hi in syms: space *= hi - lo + 1
    if len(syms) == 1 and space <= 200:
        # small domain: the oracle answers EVERY input (exhaustive) — 8 train, the rest the CEGIS pool; the law is then checked
        # on its whole domain by rustc (no sampling gap: textarea 321/40 vs 634/79 differ only from cols 79)
        (_, lo, hi), = syms
        xs = list(range(lo, hi + 1)); R.shuffle(xs)
        pts = [[[x], measure(browser, tpl.format(a0=x), key)] for x in xs]
        pts = [p for p in pts if p[1] is not None]
        need = {"need": name, "args": ["int"], "symbols": [syms[0][0]], "examples": pts[:8], "holdout": pts[8:], "validate": [],
                "template": tpl, "measured": f"{key[0]}.{key[1]} in 1/64 px (Chrome LayoutUnit)", "exhaustive": len(pts)}
        return finish(name, need, pts)
    cache = {}
    while len(pts) < 88:
        a = [R.randint(lo, hi) for _, lo, hi in syms]
        if tuple(a) in seen and len(seen) < min(space, 88): continue     # distinct points until the symbol space is used up
        seen.add(tuple(a))
        if tuple(a) in cache: pts.append([a, cache[tuple(a)]]); continue
        v = measure(browser, tpl.format(**{f"a{i}": x for i, x in enumerate(a)}), key)
        if v is not None: pts.append([a, v]); cache[tuple(a)] = v
    need = {"need": name, "args": ["int"] * len(syms), "symbols": [s for s, _, _ in syms], "examples": pts[:8], "holdout": pts[8:48], "validate": pts[48:88],
            "template": tpl, "measured": f"{key[0]}.{key[1]} in 1/64 px (Chrome LayoutUnit)"}
    return finish(name, need, pts)


def finish(name, need, pts):
    # OUR engine on the very same probes: its own law, so CODE-ABC compares two equations instead of two lists of numbers
    ours = [[a, measure_ours(need["template"].format(**{f"a{i}": x for i, x in enumerate(a)}), TARGETS[name][2])] for a, _ in pts]
    agree = sum(1 for (a, v), (_, o) in zip(pts, ours) if o is not None and abs(o - v) <= 64)
    need_o = dict(need, need=name + "--ours", examples=[p for p in ours[:8] if p[1] is not None], holdout=[p for p in ours[8:48] if p[1] is not None],
                  validate=[p for p in ours[48:88] if p[1] is not None])
    if need.get("exhaustive"): need_o["holdout"], need_o["validate"] = [p for p in ours[8:] if p[1] is not None], []
    ours_law = solve(name + "--ours", need_o)
    res = finish_chrome(name, need, pts)
    res.update({"ours_equation_1_64px": ours_law, "probes_agree_within_1px": agree, "probes": len(pts)})
    (OUT / f"{name}.json").write_text(json.dumps(res, indent=1))
    return res


def solve(tag, need):
    OUT.mkdir(parents=True, exist_ok=True)
    nf = OUT / f"{tag}.need.jsonl"; nf.write_text(json.dumps(need) + "\n")
    subprocess.run([OMR, "core", "--abc", "need", "--src", str(ROOT / "results/doctor-runs/2026-10-06/abc/pair.rs"), "--needs", str(nf), "--build-all", "--ff",
                    "--build-dir", str(OUT / "build"), "--out", str(OUT / f"{tag}.out.jsonl")], capture_output=True, text=True,
                   env={**os.environ, "PATH": os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"]})
    try: b = json.loads((OUT / f"{tag}.out.jsonl").read_text().splitlines()[0]).get("built") or {}
    except Exception: return None
    eq = b.get("rust") if b.get("verified") else None
    if eq:
        for i, sname in enumerate(need["symbols"]): eq = eq.replace(f"a{i}", sname)
    return eq


def finish_chrome(name, need, pts):
    OUT.mkdir(parents=True, exist_ok=True)
    nf = OUT / f"{name}.need.jsonl"; nf.write_text(json.dumps(need) + "\n")
    r = subprocess.run([OMR, "core", "--abc", "need", "--src", str(ROOT / "results/doctor-runs/2026-10-06/abc/pair.rs"), "--needs", str(nf),
                        "--build-all", "--ff", "--build-dir", str(OUT / "build"), "--out", str(OUT / f"{name}.out.jsonl")],
                       capture_output=True, text=True, env={**os.environ, "PATH": os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"]})
    row = json.loads((OUT / f"{name}.out.jsonl").read_text().splitlines()[0])
    b = row.get("built") or {}
    eq = b.get("rust")
    if eq:
        for i, s in enumerate(need["symbols"]): eq = eq.replace(f"a{i}", s)
    res = {"target": name, "symbols": need["symbols"], "equation_1_64px": eq, "verified_rustc": b.get("verified"), "cegis_rounds": b.get("cegis_rounds"),
           "validate": b.get("validate") or ({"agree": len(pts), "of": len(pts), "rustc": b.get("verified"), "exhaustive": True} if need.get("exhaustive") and b.get("verified") else None), "work": b.get("work"), "how": b.get("how"), "synth_secs": b.get("synth_secs"), "sample": pts[:4]}
    (OUT / f"{name}.json").write_text(json.dumps(res, indent=1))
    return res


def main():
    a = [x for x in sys.argv[1:] if not x.startswith("--")]
    if "--list" in sys.argv: print("\n".join(TARGETS)); return
    names = a or list(TARGETS)
    with sync_playwright() as pw:
        br = pw.chromium.launch(channel="chrome", headless=True)
        for i, n in enumerate(names):
            print(f"@tw step={i} total={len(names)} phase={n}", flush=True)   # tritwatch progress line
            r = run(n, br)
            v = r["validate"] or {}
            print(f"{n:18} CHROME {r['equation_1_64px'] or 'NO EQUATION'}\n{'':18} OURS   {r.get('ours_equation_1_64px') or 'NO EQUATION'}\n{'':18} probes agree {r.get('probes_agree_within_1px')}/{r.get('probes')} · chrome law unseen {v.get('agree', '-')}/{v.get('of', '-')} rustc {v.get('rustc', '-')}", flush=True)
        br.close()


if __name__ == "__main__":
    main()
