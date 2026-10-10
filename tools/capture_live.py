#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""capture_live.py — Chrome with JavaScript ON as ground truth for a JS app (the launcher), for SVELTE-T comparisons.
usage: capture_live.py <url> <out-dir> [settle_ms]  →  <out-dir>/chrome-snap.json (DOMSnapshot + computed styles), chrome.png
Same style list and viewport as fetch_corpus.py, so `omni-render render` scores our translated render against it."""
import json, sys, time
from pathlib import Path
from playwright.sync_api import sync_playwright
sys.path.insert(0, str(Path(__file__).resolve().parent))
from fetch_corpus import STYLES

url, out = sys.argv[1], Path(sys.argv[2]); settle = int(sys.argv[3]) if len(sys.argv) > 3 else 3000
out.mkdir(parents=True, exist_ok=True)
with sync_playwright() as pw:
    b = pw.chromium.launch(channel="chrome", headless=True)
    ctx = b.new_context(java_script_enabled=True, viewport={"width": 1280, "height": 800})
    page = ctx.new_page()
    t0 = time.time(); page.goto(url, wait_until="load", timeout=45000); page.wait_for_timeout(settle)
    cdp = ctx.new_cdp_session(page); cdp.send("Performance.enable")
    snap = cdp.send("DOMSnapshot.captureSnapshot", {"computedStyles": STYLES})
    (out / "chrome-snap.json").write_text(json.dumps({"styles": STYLES, "snap": snap}))
    mets = {m["name"]: m["value"] for m in cdp.send("Performance.getMetrics")["metrics"]}
    (out / "chrome-mem.json").write_text(json.dumps({"load_ms": (time.time() - t0) * 1000, **mets}))
    page.screenshot(path=str(out / "chrome.png"), full_page=False)
    print(json.dumps({k: mets.get(k) for k in ("JSHeapUsedSize", "Nodes", "LayoutCount", "ScriptDuration")}))
    b.close()
