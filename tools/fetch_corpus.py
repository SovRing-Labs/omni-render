#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""fetch_corpus.py — freeze a page corpus for OMNI-RENDER benches (offline, deterministic).

For each URL, with system Chrome via Playwright, JavaScript OFF (the document engine's fair baseline):
  corpus/<slug>/page.html        raw HTML response (pre-JS)
  corpus/<slug>/res/<n>.<ext>    every CSS / image / font response, with manifest.json mapping url -> file
  corpus/<slug>/chrome.png       Chrome screenshot 1280x800 viewport (full page, capped 8000 px)
  corpus/<slug>/chrome-snap.json DOMSnapshot with computed styles (ground truth for the style table)
  corpus/<slug>/chrome-mem.json  Chrome JS-off renderer metrics (JSHeap, Nodes, LayoutCount)
Corpus is web content: marked .no-train.json (clean-room). Usage: fetch_corpus.py [urls.txt] [--only slug]
"""
import json, hashlib, os, re, sys, time
from pathlib import Path
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ["OMR_CORPUS_OUT"]) if os.environ.get("OMR_CORPUS_OUT") else ROOT / "corpus"  # a second corpus (e.g. corpus-top100) leaves the frozen yardstick alone
STYLES = ["display", "position", "float", "width", "height", "margin-top", "margin-right", "margin-bottom",
          "margin-left", "padding-top", "padding-right", "padding-bottom", "padding-left", "color",
          "background-color", "font-family", "font-size", "font-weight", "font-style", "line-height",
          "text-align", "text-decoration-line", "white-space", "border-top-width", "border-top-color",
          "list-style-type", "visibility", "overflow-x", "box-sizing", "vertical-align"]
EXT = {"stylesheet": "css", "image": "img", "font": "font"}


def slug(url):
    s = re.sub(r"^https?://(www\.)?", "", url)
    s = re.sub(r"[^A-Za-z0-9]+", "-", s).strip("-")[:60]
    return s


def fetch(pw_browser, url, d: Path):
    d.mkdir(parents=True, exist_ok=True)
    (d / "res").mkdir(exist_ok=True)
    ctx = pw_browser.new_context(java_script_enabled=False, viewport={"width": 1280, "height": 800},
                                 user_agent="Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36")
    page = ctx.new_page()
    manifest, main = {}, {}

    def on_resp(r):
        try:
            rt = r.request.resource_type
            if r.request.is_navigation_request() and r.request.frame == page.main_frame:
                if r.status < 300:
                    main["html"] = r.body()
                    main["url"] = r.url
                return
            if rt in EXT and r.status < 300:
                b = r.body()
                if len(b) > 4_000_000:
                    return
                fn = f"{len(manifest):03d}.{EXT[rt]}"
                (d / "res" / fn).write_bytes(b)
                manifest[r.url] = {"file": fn, "type": rt, "ctype": r.headers.get("content-type", ""), "bytes": len(b)}
        except Exception:
            pass

    page.on("response", on_resp)
    t0 = time.time()
    page.goto(url, wait_until="load", timeout=45000)
    page.wait_for_timeout(800)
    load_ms = (time.time() - t0) * 1000
    (d / "page.html").write_bytes(main.get("html") or page.content().encode())
    cdp = ctx.new_cdp_session(page)
    cdp.send("Performance.enable")
    snap = cdp.send("DOMSnapshot.captureSnapshot", {"computedStyles": STYLES})
    (d / "chrome-snap.json").write_text(json.dumps({"styles": STYLES, "snap": snap}))
    mets = {m["name"]: m["value"] for m in cdp.send("Performance.getMetrics")["metrics"]}
    (d / "chrome-mem.json").write_text(json.dumps({"load_ms": load_ms, **mets}))
    h = min(page.evaluate("document.documentElement.scrollHeight") or 800, 8000)
    page.set_viewport_size({"width": 1280, "height": 800})
    page.screenshot(path=str(d / "chrome.png"), full_page=True, clip={"x": 0, "y": 0, "width": 1280, "height": h})
    (d / "manifest.json").write_text(json.dumps({"url": url, "final_url": main.get("url", url), "res": manifest}, indent=1))
    ctx.close()


def main():
    urls_file = Path(sys.argv[1]) if len(sys.argv) > 1 and not sys.argv[1].startswith("--") else ROOT / "tools" / "urls.txt"
    only = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None
    urls = [u.strip() for u in urls_file.read_text().splitlines() if u.strip() and not u.startswith("#")]
    OUT.mkdir(exist_ok=True)
    (OUT / ".no-train.json").write_text(json.dumps({"train": False, "why": "third-party web pages, bench only (clean-room)"}))
    ok = bad = 0
    with sync_playwright() as pw:
        b = pw.chromium.launch(channel="chrome", headless=True)
        for u in urls:
            s = slug(u)
            if only and s != only:
                continue
            if (OUT / s / "manifest.json").exists():
                ok += 1
                continue
            try:
                fetch(b, u, OUT / s)
                ok += 1
                print("ok ", s, flush=True)
            except Exception as e:
                bad += 1
                print("ERR", s, repr(e)[:160], flush=True)
        b.close()
    print(f"corpus: {ok} ok, {bad} failed")


if __name__ == "__main__":
    main()
