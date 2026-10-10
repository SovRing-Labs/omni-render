#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""make_t15.py — the ARC reactor's address tiers as pages for omni-render (J 10-06: "render the 15t in the trit browser … it's
toroids all the way up"; then "A, B, then add a 20t").

Each prime modulus m is one ring (a circle = a 1-torus) of m positions; a tier is the product of its rings, a torus T^k:
  T5 = {3,5,7,13,17} ⊂ T10 = {3..31} ⊂ T15 = {3..53} ⊂ T20 = {3..73}
The rings nest, so one integer coordinate lives on every tier at once (CRT: each lower tier is an exact projection of the
next). T15's period ≈ 1.63e19 fits a u64; T20's ≈ 2.04e28 needs a u128 — a T15 address is already a point on T20.
Modes (each frame = one corpus-format bundle DIR/<name>/{page.html,manifest.json}; render with `omni-render render DIR`):
  still  [--x X]                one event, its next tick in white
  orbit  [--x X] [--frames N]   A: the tick orbit x → x+1 → … (every ring steps at once; a fading trail of past ticks)
  stream --events F.jsonl       B: the LIVE reactor stream (arcd receipts): each frame lights the newest event, older ones fade
Pure CSS (absolute boxes) so the engine renders it without SVG or JS.  Usage: make_t15.py MODE OUTDIR [...]"""
import argparse, hashlib, json, math, os

T5 = [3, 5, 7, 13, 17]
T10 = [3, 5, 7, 11, 13, 17, 19, 23, 29, 31]
T15 = T10 + [37, 41, 43, 47, 53]
T20 = T15 + [59, 61, 67, 71, 73]
def tier(m): return "T5" if m in T5 else "T10" if m in T10 else "T15" if m in T15 else "T20"
COL = {"T5": ("#f5a623", "#7a5418"), "T10": ("#2ec4d6", "#1d6a74"), "T15": ("#a78bfa", "#54467e"), "T20": ("#4ade80", "#1f6a3c")}
P = {t: math.prod(v) for t, v in (("T5", T5), ("T10", T10), ("T15", T15), ("T20", T20))}
ORDER = T5 + [m for m in T10 if m not in T5] + [m for m in T15 if m not in T10] + [m for m in T20 if m not in T15]
CX, CY, R0, DR = 410, 450, 62, 17.5


def mix(c, f):  # fade colour c toward the background by factor f (1 = full)
    c = c.lstrip("#"); r, g, b = (int(c[i:i + 2], 16) for i in (0, 2, 4)); bg = (11, 11, 16)
    return "#%02x%02x%02x" % tuple(int(bg[i] + (v - bg[i]) * f) for i, v in enumerate((r, g, b)))


def rings(lit):
    """lit: list of (x, strength 0..1, white?) — the newest first; every ring lights x mod m for each"""
    out = []
    for k, m in enumerate(ORDER):
        r = R0 + k * DR; on, off = COL[tier(m)]
        level = {}
        for x, s, white in lit:
            j = x % m
            if s > level.get(j, (0, False))[0]: level[j] = (s, white)
        for j in range(m):
            a = 2 * math.pi * j / m - math.pi / 2
            px, py = CX + r * math.cos(a), CY + r * math.sin(a)
            s, white = level.get(j, (0, False))
            size = 4 + 6 * s
            col = (mix("#ffffff", s) if white else mix(on, 0.35 + 0.65 * s)) if s > 0 else off
            out.append(f'<div style="position:absolute;left:{px - size / 2:.1f}px;top:{py - size / 2:.1f}px;width:{size:.0f}px;height:{size:.0f}px;background:{col}"></div>')
        out.append(f'<div style="position:absolute;left:{CX + r + 5:.0f}px;top:{CY - 6}px;font-size:9px;color:{on}">{m}</div>')
    return "\n".join(out)


def panel(x, title, extra=""):
    res = lambda ms: " ".join(f"{m}:{x % m}" for m in ms)
    return f"""<div style="position:absolute;left:840px;top:84px;width:420px">
    <div style="color:#fff;font-weight:bold">{title}</div>
    <div style="color:#4ade80;font-weight:bold;margin-top:8px">T²⁰ · 20 rings · period {P['T20']:,} (needs u128)</div>
    <div>{res(T20[15:])}</div>
    <div style="color:#a78bfa;font-weight:bold;margin-top:6px">T¹⁵ bus · period {P['T15']:,} (&lt; 2⁶⁴)</div>
    <div>{res(T15[10:])} <span style="color:#6a6a80">(port rings)</span></div>
    <div style="color:#2ec4d6;font-weight:bold;margin-top:6px">T¹⁰ hologram core · period {P['T10']:,}</div>
    <div>{res([m for m in T10 if m not in T5])}</div>
    <div style="color:#f5a623;font-weight:bold;margin-top:6px">T⁵ rendered core · period {P['T5']:,}</div>
    <div>{res(T5)}</div>
    <div style="margin-top:8px;color:#9a9ab0">x = {x:,}<br>on T⁵: {x % P['T5']:,} · on T¹⁰: {x % P['T10']:,}</div>
    {extra}</div>"""


HEAD = "T²⁰ ⊃ T¹⁵ ⊃ T¹⁰ ⊃ T⁵ · toroids all the way up"


def page(body, sub):
    return f"""<!doctype html><html><head><meta charset="utf-8"><title>T20 — toroids all the way up</title></head>
<body style="margin:0;background:#0b0b10;color:#d8d8e0;font-family:DejaVu Sans Mono,monospace;font-size:12px;line-height:1.45">
<div style="position:relative;width:1280px;height:900px">
  <div style="position:absolute;left:24px;top:14px;font-weight:bold;font-size:21px;color:#fff">{HEAD}</div>
  <div style="position:absolute;left:24px;top:44px;width:800px;color:#9a9ab0">{sub}</div>
{body}
</div></body></html>"""


def write(out, name, html):
    d = os.path.join(out, name); os.makedirs(d, exist_ok=True)
    open(os.path.join(d, "page.html"), "w").write(html)
    url = "file://" + os.path.abspath(os.path.join(d, "page.html"))
    json.dump({"url": url, "final_url": url, "res": {}}, open(os.path.join(d, "manifest.json"), "w"))


def main():
    a = argparse.ArgumentParser(); a.add_argument("mode", choices=["still", "orbit", "stream"]); a.add_argument("out")
    a.add_argument("--x", type=int, default=None); a.add_argument("--frames", type=int, default=24); a.add_argument("--events")
    o = a.parse_args()
    x0 = o.x if o.x is not None else int.from_bytes(hashlib.sha256(b"15t").digest(), "big") % P["T15"]
    if o.mode == "still":
        write(o.out, "t20", page(rings([(x0, 1.0, False), (x0 + 1, 0.8, True)]) + panel(x0, "one event (white = next tick)"),
                                 "Each ring is one prime modulus. A tier is the product of its rings — a torus — and each tier is the exact projection of the next."))
    elif o.mode == "orbit":
        for t in range(o.frames):
            lit = [(x0 + t - k, 1.0 - k / 6, False) for k in range(6) if t - k >= 0]
            write(o.out, f"orbit{t:03d}", page(rings(lit) + panel(x0 + t, f"A · tick orbit  t = {t}"),
                  "The tick x → x+1 moves EVERY ring one step at once: a line winding the torus, visiting every address once per period (Weyl orbit, law R8). Trail = the last ticks."))
    else:
        ev = [json.loads(l) for l in open(o.events)]
        for t in range(len(ev)):
            lit = [(ev[t - k]["address"], 1.0 - k / 5, False) for k in range(5) if t - k >= 0]
            e = ev[t]; pt = e.get("port") or {}
            extra = f"""<div style="margin-top:10px;color:#fff;font-weight:bold">live reactor event {t + 1}/{len(ev)} (arcd receipt)</div>
    <div>engine {e['domain']} · {e['spoke']} · {e['verdict']}</div>
    <div>port: genr {pt.get('genr')} · stage {pt.get('stage')} · station {pt.get('station')} · phase {pt.get('phase')} · tick {pt.get('tick')}</div>
    <div style="color:#9a9ab0">the 5 port rings (37…53) carry WHO/WHAT; the 10 clock rings carry WHEN</div>"""
            write(o.out, f"stream{t:03d}", page(rings(lit) + panel(e["address"], "B · live reactor stream", extra),
                  "Real events from the arc reactor service: each receipt's T¹⁵ address lit on every ring; older events fade. On T²⁰ the same integer simply has 5 more residues."))


if __name__ == "__main__":
    main()
