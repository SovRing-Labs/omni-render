#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""replay_server.py — serve the frozen corpus over plain HTTP so every engine loads identical bytes, offline.

  GET /p/<slug>                 -> the page HTML with absolute http(s) URLs rewritten to /r/<host>/<path>
  GET /r/<host>/<path?query>    -> the corpus resource whose original URL is https://<host>/<path?query>
CSS responses get the same url(...) rewriting. Anything not in the corpus -> 404 (nothing leaves the machine).
Usage: replay_server.py [port=8931]
"""
import json, os, re, sys
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import urljoin

ROOT = Path(__file__).resolve().parent.parent / "corpus"
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8931
BASE = f"http://127.0.0.1:{PORT}"
INDEX = {}   # original url (no fragment) -> (file path, content-type)
PAGES = {}   # slug -> (html path, final url)
for d in sorted(ROOT.iterdir()):
    m = d / "manifest.json"
    if not m.exists():
        continue
    man = json.loads(m.read_text())
    PAGES[d.name] = (d / "page.html", man.get("final_url") or man["url"])
    for u, v in man["res"].items():
        INDEX[u.split("#")[0]] = (d / "res" / v["file"], v.get("ctype") or "application/octet-stream")

ABS = re.compile(rb"(https?:)?//([A-Za-z0-9.-]+\.[A-Za-z]{2,})(/[^\s\"'<>)]*)?")


def rewrite(body: bytes, base_url: str, css: bool) -> bytes:
    def rep(m):
        host = m.group(2).decode()
        path = (m.group(3) or b"/").decode()
        return f"{BASE}/r/{host}{path}".encode()
    body = ABS.sub(rep, body)
    if css:  # relative url(...) in stylesheets resolve against the stylesheet's own original URL
        def rel(m):
            u = m.group(2).decode().strip()
            if u.startswith(("data:", "http", "/r/")):
                return m.group(0)
            full = urljoin(base_url, u)
            h = re.match(r"https?://([^/]+)(/.*)?", full)
            return f"url({m.group(1).decode()}{BASE}/r/{h.group(1)}{h.group(2) or '/'}{m.group(1).decode()})".encode() if h else m.group(0)
        body = re.sub(rb"url\(\s*(['\"]?)([^'\")]+)\1\s*\)", rel, body)
    return body


CACHE = {}  # path -> (body, ctype): every response is rewritten once, at startup, so serving is a dict lookup


def build(p):
    if p.startswith("/p/"):
        slug = p[3:].split("?")[0]
        if slug not in PAGES:
            return None
        path, url = PAGES[slug]
        body = path.read_bytes()
        h = re.match(r"https?://([^/]+)(/.*)?", url)
        d = (h.group(2) or "/").rsplit("/", 1)[0] + "/"
        body = rewrite(body, url, False)
        body = re.sub(rb"(<head[^>]*>)", lambda m: m.group(1) + f'<base href="{BASE}/r/{h.group(1)}{d}">'.encode(), body, count=1, flags=re.I)
        return body, "text/html; charset=utf-8"
    if p.startswith("/r/"):
        host, _, path = p[3:].partition("/")
        for scheme in ("https://", "http://"):
            key = f"{scheme}{host}/{path}"
            if key in INDEX:
                f, ct = INDEX[key]
                body = f.read_bytes()
                if "css" in ct:
                    body = rewrite(body, key, True)
                return body, ct
    return None


def precompute():
    for slug in PAGES:
        CACHE[f"/p/{slug}"] = build(f"/p/{slug}")
    for key in INDEX:
        m = re.match(r"https?://([^/]+)(/.*)?", key)
        if m:
            p = f"/r/{m.group(1)}{m.group(2) or '/'}"
            CACHE.setdefault(p, build(p))


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"  # keep-alive, like a real server

    def log_message(self, *a):
        pass

    def send(self, code, body=b"", ctype="text/plain"):
        # one write per response: header + body together (no Nagle/delayed-ACK stall between two small writes)
        head = f"HTTP/1.1 {code} {'OK' if code == 200 else 'Not Found'}\r\nContent-Type: {ctype}\r\nContent-Length: {len(body)}\r\nConnection: keep-alive\r\n\r\n".encode()
        self.wfile.write(head + body)

    def do_GET(self):
        p = self.path
        if p in CACHE and CACHE[p] is not None:
            body, ct = CACHE[p]
            return self.send(200, body, ct)
        if p.startswith("/p/"):
            slug = p[3:].split("?")[0]
            if slug not in PAGES:
                return self.send(404)
            path, url = PAGES[slug]
            body = path.read_bytes()
            # page-relative resources: a <base> pointing at the replayed original directory
            h = re.match(r"https?://([^/]+)(/.*)?", url)
            d = (h.group(2) or "/").rsplit("/", 1)[0] + "/"
            body = rewrite(body, url, False)
            body = re.sub(rb"(<head[^>]*>)", lambda m: m.group(1) + f'<base href="{BASE}/r/{h.group(1)}{d}">'.encode(), body, count=1, flags=re.I)
            return self.send(200, body, "text/html; charset=utf-8")
        if p.startswith("/r/"):
            rest = p[3:]
            host, _, path = rest.partition("/")
            for scheme in ("https://", "http://"):
                key = f"{scheme}{host}/{path}"
                if key in INDEX:
                    f, ct = INDEX[key]
                    body = f.read_bytes()
                    if "css" in ct:
                        body = rewrite(body, key, True)
                    return self.send(200, body, ct)
            return self.send(404)
        if p == "/list":
            return self.send(200, "\n".join(PAGES).encode())
        self.send(404)


if __name__ == "__main__":
    precompute()
    print(f"replay: {len(CACHE)} responses precomputed", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
