// SPDX-License-Identifier: MIT OR Apache-2.0
// SVELTE-T0 probe: inventory a Svelte app for translation through the Omniglyph IR → OmniUI / WASM back-ends.
// usage: node tools/svelte_probe.mjs <app/src dir> <path to node_modules/svelte/compiler>
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
const [,, root, compilerPath] = process.argv;
const { parse } = await import(compilerPath);
const files = [];
(function walk(d) { for (const f of readdirSync(d)) { const p = join(d, f); if (statSync(p).isDirectory()) walk(p); else if (p.endsWith('.svelte')) files.push(p); } })(root);
const count = {}; const add = (k, n = 1) => { count[k] = (count[k] || 0) + n; };
const per = [];
for (const f of files) {
  const src = readFileSync(f, 'utf8');
  let ast; try { ast = parse(src, { modern: true }); } catch (e) { add('parse_error'); continue; }
  const c = { file: f.replace(root + '/', ''), lines: src.split('\n').length, nodes: 0, dynamic: 0 };
  (function visit(n) {
    if (!n || typeof n !== 'object') return;
    if (Array.isArray(n)) { n.forEach(visit); return; }
    if (n.type) {
      c.nodes++;
      switch (n.type) {
        case 'RegularElement': add('el:' + n.name); break;
        case 'Component': add('component'); add('comp:' + n.name); break;
        case 'IfBlock': case 'EachBlock': case 'AwaitBlock': case 'KeyBlock': case 'SnippetBlock': add('block:' + n.type); c.dynamic++; break;
        case 'ExpressionTag': add('expr'); c.dynamic++; break;
        case 'OnDirective': add('dir:on'); break;
        case 'BindDirective': add('dir:bind:' + n.name); break;
        case 'ClassDirective': add('dir:class'); break;
        case 'StyleDirective': add('dir:style'); break;
        case 'UseDirective': add('dir:use(action)'); break;
        case 'TransitionDirective': add('dir:transition'); break;
        case 'Attribute': if (/^on[a-z]/.test(n.name)) add('attr:event'); break;
        case 'RenderTag': add('render'); break;
      }
    }
    for (const k of Object.keys(n)) if (k !== 'parent' && k !== 'metadata') visit(n[k]);
  })(ast.fragment);
  const script = ast.instance ? src.slice(ast.instance.start, ast.instance.end) : '';
  for (const r of ['$state', '$derived', '$effect', '$props', '$bindable']) { const m = script.split(r + '(').length - 1 + script.split(r + '.').length - 1; if (m) add('rune:' + r, m); }
  for (const api of ['fetch(', 'localStorage', 'EventSource', 'WebSocket', 'getContext(', 'requestAnimationFrame', 'setInterval', 'setTimeout', 'invoke(', 'document.', 'window.', 'new Worker']) { const m = script.split(api).length - 1; if (m) add('api:' + api, m); }
  const imports = [...script.matchAll(/from\s+['"]([^'"]+)['"]/g)].map(m => m[1]);
  for (const i of imports) if (!i.startsWith('.')) add('import:' + i);
  c.script_lines = script.split('\n').length;
  per.push(c);
}
const top = Object.entries(count).sort((a, b) => b[1] - a[1]);
const els = top.filter(([k]) => k.startsWith('el:'));
const tot = (p) => top.filter(([k]) => k.startsWith(p)).reduce((s, [, v]) => s + v, 0);
console.log(JSON.stringify({
  components: files.length, parse_errors: count.parse_error || 0,
  template_lines: per.reduce((s, c) => s + c.lines - c.script_lines, 0), script_lines: per.reduce((s, c) => s + c.script_lines, 0),
  elements: tot('el:'), distinct_elements: els.length, top_elements: els.slice(0, 20),
  blocks: top.filter(([k]) => k.startsWith('block:')), directives: top.filter(([k]) => k.startsWith('dir:') || k.startsWith('attr:')),
  expressions: count.expr || 0, components_used: count.component || 0, top_components: top.filter(([k]) => k.startsWith('comp:')).slice(0, 12),
  runes: top.filter(([k]) => k.startsWith('rune:')), browser_apis: top.filter(([k]) => k.startsWith('api:')), imports: top.filter(([k]) => k.startsWith('import:')),
  biggest: per.sort((a, b) => b.lines - a.lines).slice(0, 8).map(c => [c.file, c.lines, c.script_lines]),
}, null, 1));
