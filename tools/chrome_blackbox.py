#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""chrome_blackbox.py — the Chrome BLACK-BOX knowledge base (J 2026-10-06: "then yes, blackbox").

We observe Chrome's OUTPUT only (boxes + computed styles via the DevTools DOMSnapshot), never its source: clean-room safe.
Every case becomes a bundle in the corpus format (page.html + manifest.json + chrome-snap.json + chrome-boxes.json), so the
engine's own checks run on it unchanged: `omni-render style blackbox/<suite>` (computed-style agreement per property) and
`omni-render mismatches blackbox/<suite> out.jsonl` (layout boxes).

Suites:
  --ua                         one page per HTML element / form control (standards mode): Chrome's default styles and sizes
  --cases F.jsonl SUITE        rows {"name", "html"} → blackbox/SUITE/ (oracle cases cut from failing shapes)
  --gather FILE.md [FILE.md…]  every ```html block in clean-room gather specs (an unpublished file) — Chrome's real boxes
                               replace the gatherers' hand-derived expectations
Output: projects/omni-render/blackbox/<suite>/<nnn-slug>/ ; index blackbox/<suite>/index.jsonl (case → tag summary + boxes).
Viewport 1280×800, JavaScript off (the engine's baseline, as fetch_corpus.py). Usage:
  tools/chrome_blackbox.py --ua
  tools/chrome_blackbox.py --gather ../../an unpublished file*.md
"""
import json, re, sys
from pathlib import Path
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "blackbox"
STYLES = ["display", "position", "float", "width", "height", "margin-top", "margin-right", "margin-bottom",
          "margin-left", "padding-top", "padding-right", "padding-bottom", "padding-left", "color",
          "background-color", "font-family", "font-size", "font-weight", "font-style", "line-height",
          "text-align", "text-decoration-line", "white-space", "border-top-width", "border-top-color",
          "list-style-type", "visibility", "overflow-x", "box-sizing", "vertical-align"]   # same list as fetch_corpus.py

# one sample per element; `x` = the element under test (carries id="x"); wrappers give it its required parent
UA_CASES = [
    ("a", '<a id="x" href="#">link</a>'), ("abbr", '<abbr id="x" title="t">abbr</abbr>'), ("address", '<address id="x">addr</address>'),
    ("article", '<article id="x">art</article>'), ("aside", '<aside id="x">aside</aside>'), ("b", '<b id="x">bold</b>'),
    ("bdi", '<bdi id="x">bdi</bdi>'), ("bdo", '<bdo id="x" dir="rtl">bdo</bdo>'), ("blockquote", '<blockquote id="x">quote</blockquote>'),
    ("button", '<button id="x">Button</button>'), ("button-w100", '<div style="width:300px"><button id="x" style="width:100%;padding:10px">B</button></div>'),
    ("caption", '<table><caption id="x">cap</caption><tr><td>c</td></tr></table>'), ("cite", '<cite id="x">cite</cite>'),
    ("code", '<code id="x">code</code>'), ("dd", '<dl><dt>t</dt><dd id="x">d</dd></dl>'), ("del", '<del id="x">del</del>'),
    ("details", '<details id="x"><summary>sum</summary>body</details>'), ("details-open", '<details id="x" open><summary>sum</summary>body</details>'),
    ("dfn", '<dfn id="x">dfn</dfn>'), ("dialog-open", '<dialog id="x" open>dlg</dialog>'), ("div", '<div id="x">div</div>'),
    ("dl", '<dl id="x"><dt>t</dt><dd>d</dd></dl>'), ("dt", '<dl><dt id="x">t</dt><dd>d</dd></dl>'), ("em", '<em id="x">em</em>'),
    ("fieldset", '<fieldset id="x"><legend>leg</legend>field</fieldset>'), ("legend", '<fieldset><legend id="x">leg</legend>f</fieldset>'),
    ("figure", '<figure id="x"><figcaption>fc</figcaption></figure>'), ("figcaption", '<figure><figcaption id="x">fc</figcaption></figure>'),
    ("footer", '<footer id="x">foot</footer>'), ("form", '<form id="x">form</form>'),
    ("h1", '<h1 id="x">H1</h1>'), ("h2", '<h2 id="x">H2</h2>'), ("h3", '<h3 id="x">H3</h3>'), ("h4", '<h4 id="x">H4</h4>'),
    ("h5", '<h5 id="x">H5</h5>'), ("h6", '<h6 id="x">H6</h6>'), ("h1-in-section", '<section><h1 id="x">H1</h1></section>'),
    ("h1-in-article-section", '<article><section><h1 id="x">H1</h1></section></article>'),
    ("header", '<header id="x">head</header>'), ("hgroup", '<hgroup id="x"><h1>a</h1><p>b</p></hgroup>'), ("hr", '<hr id="x">'),
    ("i", '<i id="x">ital</i>'), ("iframe", '<iframe id="x"></iframe>'), ("img-broken", '<img id="x" src="missing.png">'),
    ("img-broken-alt", '<img id="x" src="missing.png" alt="alt text">'),
    ("input-text", '<input id="x" type="text">'), ("input-size5", '<input id="x" type="text" size="5">'),
    ("input-checkbox", '<input id="x" type="checkbox">'), ("input-radio", '<input id="x" type="radio">'),
    ("input-submit", '<input id="x" type="submit" value="Go">'), ("input-button", '<input id="x" type="button" value="Btn">'),
    ("input-reset", '<input id="x" type="reset">'), ("input-range", '<input id="x" type="range">'), ("input-color", '<input id="x" type="color">'),
    ("input-date", '<input id="x" type="date">'), ("input-number", '<input id="x" type="number">'), ("input-file", '<input id="x" type="file">'),
    ("input-search", '<input id="x" type="search">'), ("input-email", '<input id="x" type="email">'), ("input-password", '<input id="x" type="password">'),
    ("input-w100", '<div style="width:300px"><input id="x" type="text" style="width:100%"></div>'),
    ("ins", '<ins id="x">ins</ins>'), ("kbd", '<kbd id="x">kbd</kbd>'), ("label", '<label id="x">label</label>'),
    ("li-ul", '<ul><li id="x">item</li></ul>'), ("li-ol", '<ol><li id="x">item</li></ol>'), ("main", '<main id="x">main</main>'),
    ("mark", '<mark id="x">mark</mark>'), ("menu", '<menu id="x"><li>m</li></menu>'), ("meter", '<meter id="x" value="0.5"></meter>'),
    ("nav", '<nav id="x">nav</nav>'), ("ol", '<ol id="x"><li>a</li></ol>'), ("ol-nested", '<ol><li><ol id="x"><li>a</li></ol></li></ol>'),
    ("output", '<output id="x">out</output>'), ("p", '<p id="x">para</p>'), ("pre", '<pre id="x">pre  text</pre>'),
    ("progress", '<progress id="x" value="0.5"></progress>'), ("q", '<q id="x">quote</q>'),
    ("ruby", '<ruby id="x">漢<rt>kan</rt></ruby>'), ("s", '<s id="x">strike</s>'), ("samp", '<samp id="x">samp</samp>'),
    ("search", '<search id="x">search</search>'), ("section", '<section id="x">sec</section>'),
    ("select", '<select id="x"><option>one</option><option>longer option</option></select>'),
    ("select-multiple", '<select id="x" multiple><option>a</option><option>b</option></select>'),
    ("select-size3", '<select id="x" size="3"><option>a</option><option>b</option></select>'),
    ("small", '<small id="x">small</small>'), ("span", '<span id="x">span</span>'), ("strong", '<strong id="x">strong</strong>'),
    ("sub", '<sub id="x">sub</sub>'), ("sup", '<sup id="x">sup</sup>'), ("summary", '<details><summary id="x">sum</summary>b</details>'),
    ("table", '<table id="x"><tr><td>c</td></tr></table>'), ("td", '<table><tr><td id="x">cell</td></tr></table>'),
    ("th", '<table><tr><th id="x">head</th></tr></table>'), ("thead", '<table><thead id="x"><tr><td>c</td></tr></thead></table>'),
    ("textarea", '<textarea id="x"></textarea>'), ("textarea-rows", '<textarea id="x" rows="5" cols="30"></textarea>'),
    ("time", '<time id="x">12:00</time>'), ("u", '<u id="x">under</u>'), ("ul", '<ul id="x"><li>a</li></ul>'),
    ("ul-nested", '<ul><li><ul id="x"><li>a</li></ul></li></ul>'), ("var", '<var id="x">var</var>'), ("video", '<video id="x"></video>'),
    ("canvas", '<canvas id="x"></canvas>'), ("svg-noattr", '<svg id="x"></svg>'), ("svg-viewbox", '<svg id="x" viewBox="0 0 24 24"></svg>'),
    ("object", '<object id="x"></object>'), ("embed", '<embed id="x">'),
]


def doc(body: str) -> str:
    if re.search(r"<!doctype|<html", body, re.I):
        return body
    return "<!doctype html>\n<html><head><meta charset=\"utf-8\"></head><body>\n" + body + "\n</body></html>\n"


def boxes(snap: dict) -> list:
    """element boxes from the DOMSnapshot layout tree: tag, id, class, [x, y, w, h]"""
    s = snap["strings"]; d = snap["documents"][0]; nd = d["nodes"]; lay = d["layout"]
    names = nd["nodeName"]; attrs = nd.get("attributes", [])
    out = []
    for li, ni in enumerate(lay["nodeIndex"]):
        if nd["nodeType"][ni] != 1:
            continue
        a = attrs[ni] if ni < len(attrs) else []
        kv = {s[a[i]]: s[a[i + 1]] for i in range(0, len(a) - 1, 2)}
        out.append({"tag": s[names[ni]].lower(), "id": kv.get("id", ""), "class": kv.get("class", ""), "box": [round(v, 3) for v in lay["bounds"][li]]})
    return out


def capture(browser, html: str, d: Path) -> dict:
    d.mkdir(parents=True, exist_ok=True)
    (d / "page.html").write_text(html)
    url = (d / "page.html").resolve().as_uri()
    ctx = browser.new_context(java_script_enabled=False, viewport={"width": 1280, "height": 800})
    page = ctx.new_page()
    page.goto(url, wait_until="load", timeout=20000)
    cdp = ctx.new_cdp_session(page)
    snap = cdp.send("DOMSnapshot.captureSnapshot", {"computedStyles": STYLES})
    (d / "chrome-snap.json").write_text(json.dumps({"styles": STYLES, "snap": snap}))
    bx = boxes(snap)
    (d / "chrome-boxes.json").write_text(json.dumps(bx, indent=0))
    (d / "manifest.json").write_text(json.dumps({"url": url, "final_url": url, "res": {}}))
    ctx.close()
    return {"boxes": bx}


def gather_cases(files):
    for f in files:
        txt = Path(f).read_text(errors="replace")
        stem = Path(f).stem.replace("BUGLIB-GATHER-", "").lower()
        for k, m in enumerate(re.finditer(r"```html\s*\n(.*?)```", txt, re.S)):
            # the nearest preceding "## Topic" heading names the case
            head = txt.rfind("## Topic", 0, m.start())
            topic = txt[head:txt.find("\n", head)].replace("## Topic:", "").strip() if head >= 0 else ""
            yield f"{stem}-{k:02d}", m.group(1), {"source": str(f), "topic": topic}


def main():
    a = sys.argv[1:]
    if "--ua" in a:
        suite, cases = "ua", [(n, b, {"element": n}) for n, b in UA_CASES]
    elif "--gather" in a:
        suite, cases = "gather", list(gather_cases([x for x in a if x != "--gather"]))
    elif "--cases" in a:  # --cases F.jsonl SUITE: rows {"name", "html"} (the doctor's oracle cases: shapes cut from failing boxes)
        f, suite = a[a.index("--cases") + 1], a[a.index("--cases") + 2]
        cases = [(r["name"], r["html"], {k: v for k, v in r.items() if k != "html"}) for r in map(json.loads, Path(f).read_text().splitlines()) if r]
    else:
        sys.exit(__doc__)
    base = OUT / suite
    base.mkdir(parents=True, exist_ok=True)
    (OUT / ".no-train.json").write_text(json.dumps({"train": False, "why": "Chrome observations (black-box), bench + doctor needs only"}))
    idx = []
    with sync_playwright() as pw:
        b = pw.chromium.launch(channel="chrome", headless=True)
        for i, (name, body, meta) in enumerate(cases):
            slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
            d = base / f"{i:03d}-{slug}"
            try:
                r = capture(b, doc(body), d)
                x = next((e for e in r["boxes"] if e["id"] == "x"), None)
                idx.append({"case": d.name, **meta, "x": x, "n_boxes": len(r["boxes"])})
                print("ok ", d.name, x["box"] if x else "", flush=True)
            except Exception as e:
                idx.append({"case": d.name, **meta, "error": repr(e)[:200]})
                print("ERR", d.name, repr(e)[:160], flush=True)
        b.close()
    (base / "index.jsonl").write_text("".join(json.dumps(r) + "\n" for r in idx))
    print(f"{len(idx)} cases → {base}")


if __name__ == "__main__":
    main()
