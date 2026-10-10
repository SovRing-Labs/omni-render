#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Print CSS rules of a frozen corpus page whose selector matches a regex (optionally whose body matches a 2nd).
usage: tools/rules.py <page-substring> <selector-regex> [body-regex]"""
import glob, re, sys

def rules(css):
    css = re.sub(r'/\*.*?\*/', '', css, flags=re.S)
    stack, start = [], 0
    for i, ch in enumerate(css):
        if ch == '{':
            stack.append(css[start:i].strip()); start = i + 1
        elif ch == '}':
            if stack:
                sel = stack.pop()
                if not sel.startswith('@'):
                    yield ' '.join(p for p in stack if p.startswith('@')), sel, css[start:i].strip()
            start = i + 1
        elif ch == ';' and not stack:
            start = i + 1

page, sel_re = sys.argv[1], re.compile(sys.argv[2])
body_re = re.compile(sys.argv[3]) if len(sys.argv) > 3 else None
d = [p for p in glob.glob('corpus/*') if page in p][0]
html = open(d + '/page.html', errors='replace').read()
texts = [(s.split('/')[-1], open(s, errors='replace').read()) for s in sorted(glob.glob(d + '/res/*.css'))]
texts += [('<style>', t) for t in re.findall(r'<style[^>]*>(.*?)</style>', html, flags=re.S)]
for src, t in texts:
    for at, sel, body in rules(t):
        if sel_re.search(sel) and (body_re is None or body_re.search(body)):
            print(f"{src}: {at + ' ' if at else ''}{sel[:200]} {{ {body[:int(__import__("os").environ.get("RW","300"))]} }}")
