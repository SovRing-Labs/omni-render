// SPDX-License-Identifier: MIT OR Apache-2.0
// SVELTE-T1 front-end: one .svelte component → Omniglyph UI IR (JSON).
// usage: node tools/svelte2ir.mjs <component.svelte> <svelte compiler module path> [out.json]
// The svelte compiler is a build-time tool here (parse + scoped CSS only); nothing from it is shipped or executed at run time.
import { readFileSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';
const [,, file, compilerPath, out] = process.argv;
const { parse, compile } = await import(compilerPath);
const src = readFileSync(file, 'utf8');
const ast = parse(src, { modern: true });

// expressions: estree, minus positions (the IR evaluator in src/uiir.rs reads this subset)
const strip = (n) => {
  if (Array.isArray(n)) return n.map(strip);
  if (!n || typeof n !== 'object') return n;
  const o = {};
  for (const [k, v] of Object.entries(n)) if (!['start', 'end', 'loc', 'range', 'metadata', 'leadingComments', 'trailingComments'].includes(k)) o[k] = strip(v);
  return o;
};
const text = (n) => src.slice(n.start, n.end);

// scoped CSS exactly as Svelte scopes it (class svelte-<hash> on the component's elements)
let css = '', scope = null;
try {
  const r = compile(src, { filename: file, css: 'external', generate: 'client' });
  css = r.css?.code || '';
  scope = (css.match(/svelte-[a-z0-9]+/) || [null])[0];
} catch (e) { css = ''; }

const attrValue = (v) => {
  if (v === true) return { k: 'bool' };
  const parts = Array.isArray(v) ? v : [v];
  return { k: 'parts', parts: parts.map(p => p.type === 'Text' ? { t: p.data } : { e: strip(p.expression) }) };
};
const frag = (f) => (f?.nodes || []).map(node).filter(Boolean);
function node(n) {
  switch (n.type) {
    case 'Text': return { k: 'text', t: n.data };
    case 'ExpressionTag': return { k: 'expr', e: strip(n.expression) };
    case 'HtmlTag': return { k: 'html', e: strip(n.expression) };
    case 'Comment': return null;
    case 'RegularElement': case 'SvelteElement': {
      const el = { k: 'el', tag: n.name, attrs: [], cls: [], on: [], bind: [], style: [] };
      for (const a of n.attributes) {
        if (a.type === 'Attribute') {
          if (/^on[a-z]/.test(a.name)) { const v = Array.isArray(a.value) ? a.value[0] : a.value; el.on.push({ ev: a.name.slice(2), h: v && v.type === 'ExpressionTag' ? strip(v.expression) : null }); }
          else el.attrs.push({ name: a.name, v: attrValue(a.value) });
        }
        else if (a.type === 'ClassDirective') el.cls.push({ name: a.name, e: strip(a.expression) });
        else if (a.type === 'StyleDirective') el.style.push({ name: a.name, v: a.value === true ? null : attrValue(a.value) });
        else if (a.type === 'OnDirective') el.on.push({ ev: a.name, h: a.expression ? strip(a.expression) : null, mods: a.modifiers || [] });
        else if (a.type === 'BindDirective') el.bind.push({ name: a.name, e: strip(a.expression) });
        else if (a.type === 'SpreadAttribute') el.attrs.push({ spread: strip(a.expression) });
      }
      if (scope) el.attrs.push({ name: 'class', v: { k: 'parts', parts: [{ t: scope }] }, scope: true });
      el.children = frag(n.fragment);
      return el;
    }
    case 'IfBlock': {
      const branches = [];
      let cur = n;
      while (cur) {
        branches.push({ test: strip(cur.test), children: frag(cur.consequent) });
        const alt = cur.alternate;
        if (alt && alt.nodes?.length === 1 && alt.nodes[0].type === 'IfBlock' && alt.nodes[0].elseif) { cur = alt.nodes[0]; continue; }
        if (alt) branches.push({ test: null, children: frag(alt) });
        cur = null;
      }
      return { k: 'if', branches };
    }
    case 'EachBlock': return { k: 'each', list: strip(n.expression), item: n.context ? strip(n.context) : null, index: n.index || null, key: n.key ? strip(n.key) : null, children: frag(n.body), fallback: n.fallback ? frag(n.fallback) : [] };
    case 'KeyBlock': return { k: 'frag', children: frag(n.fragment) };
    case 'AwaitBlock': return { k: 'frag', children: frag(n.pending) };
    case 'SvelteComponent':
      return { k: 'dyncomp', this: strip(n.expression), props: n.attributes.filter(a => a.type === 'Attribute').map(a => ({ name: a.name, v: attrValue(a.value) })), children: frag(n.fragment) };
    case 'Component':
      return { k: 'comp', on: n.attributes.filter(a => a.type === 'OnDirective').map(a => ({ ev: a.name, h: a.expression ? strip(a.expression) : null })), name: n.name, props: n.attributes.filter(a => a.type === 'Attribute').map(a => ({ name: a.name, v: attrValue(a.value) })), children: frag(n.fragment) };
    case 'SnippetBlock': return null;
    case 'ConstTag': { const d = n.declaration.declarations[0]; return d.id.type === 'Identifier' ? { k: 'const', name: d.id.name, e: strip(d.init) } : { k: 'unsupported', what: 'ConstTag-pattern' }; }
    case 'SvelteWindow': case 'SvelteHead': case 'SvelteBody': case 'SvelteDocument': return null;
    case 'RenderTag': return { k: 'unsupported', what: 'render' };
    default: return { k: 'unsupported', what: n.type };
  }
}

// instance script → props / state / derived / stores / handlers
const ir = { component: basename(file, '.svelte'), file, css, scope, props: [], state: [], derived: [], handlers: [], imports: [], stores: new Set(), view: frag(ast.fragment),
  instance_body: strip(ast.instance?.content?.body || []), module_body: strip(ast.module?.content?.body || []) };
const body = ast.instance?.content?.body || [];
const runeOf = (init) => init?.type === 'CallExpression' && init.callee.type === 'Identifier' && init.callee.name.startsWith('$') ? init.callee.name : null;
for (const st of body) {
  if (st.type === 'ImportDeclaration') { ir.imports.push({ from: st.source.value, names: st.specifiers.map(s => s.local.name) }); continue; }
  if (st.type === 'ExportNamedDeclaration' && st.declaration?.type === 'VariableDeclaration') {
    for (const d of st.declaration.declarations) ir.props.push({ name: d.id.name, init: d.init ? strip(d.init) : null });
    continue;
  }
  if (st.type === 'VariableDeclaration') {
    for (const d of st.declarations) {
      const r = runeOf(d.init);
      if (r === '$props' && d.id.type === 'ObjectPattern') {
        for (const p of d.id.properties) if (p.type === 'Property') ir.props.push({ name: p.value.type === 'AssignmentPattern' ? p.value.left.name : p.key.name, init: p.value.type === 'AssignmentPattern' ? strip(p.value.right) : null });
      } else if (r === '$derived') ir.derived.push({ name: d.id.name, e: strip(d.init.arguments[0]) });
      else if (r === '$state') ir.state.push({ name: d.id.name, init: d.init.arguments[0] ? strip(d.init.arguments[0]) : null });
      else if (d.id.type === 'Identifier') ir.state.push({ name: d.id.name, init: d.init ? strip(d.init) : null, konst: st.kind === 'const' });
    }
    continue;
  }
  if (st.type === 'LabeledStatement' && st.label.name === '$' && st.body.type === 'ExpressionStatement' && st.body.expression.type === 'AssignmentExpression' && st.body.expression.left.type === 'Identifier') {
    ir.derived.push({ name: st.body.expression.left.name, e: strip(st.body.expression.right) }); continue;
  }
  if (st.type === 'FunctionDeclaration') { ir.handlers.push({ name: st.id.name, params: strip(st.params), body: strip(st.body), async: !!st.async }); continue; }
}
for (const m of src.matchAll(/\$([A-Za-z_][A-Za-z0-9_]*)/g)) if (!['state', 'derived', 'effect', 'props', 'bindable', 'inspect', 'host'].includes(m[1])) ir.stores.add(m[1]);
ir.stores = [...ir.stores];
const json = JSON.stringify(ir);
if (out) writeFileSync(out, json); else process.stdout.write(json);
