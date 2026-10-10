#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""cmp.py <page> [h] — side-by-side ours | chrome, top h px, to /tmp/claude-1000/cmp.png"""
import sys
from PIL import Image
n=sys.argv[1]; h=int(sys.argv[2]) if len(sys.argv)>2 else 800
a=Image.open(f'results/render/{n}.png').crop((0,0,1280,h)); b=Image.open(f'corpus/{n}/chrome.png').convert('RGB').crop((0,0,1280,h))
c=Image.new('RGB',(2560,h),'white'); c.paste(a,(0,0)); c.paste(b,(1280,0)); c.save('/tmp/claude-1000/cmp.png')
