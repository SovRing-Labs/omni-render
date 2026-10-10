// SPDX-License-Identifier: MIT OR Apache-2.0
// Parity oracle for src/jsparse.rs (BROWSER-BODY-0 step 8): each file → acorn estree, stripped exactly as tools/js2ir.mjs
// strips it, as one JSON line {file, module, ir}. module = parses as a module (allowAwaitOutsideFunction, like js2ir);
// otherwise a classic script; ir = null when neither parses. acorn is the oracle only (its output, not its code).
// usage: node tools/js_oracle.mjs <acorn.mjs> <list-of-files.txt> > oracle.jsonl
import { readFileSync } from 'node:fs';
const [,, acornPath, list] = process.argv;
const acorn = await import(acornPath);
const strip = (n) => {
  if (Array.isArray(n)) return n.map(strip);
  if (!n || typeof n !== 'object') return n;
  if (n instanceof RegExp) return { pattern: n.source, flags: n.flags };
  const o = {};
  for (const [k, v] of Object.entries(n)) if (!['start', 'end', 'loc', 'range', 'raw'].includes(k)) o[k] = (k === 'value' && v instanceof RegExp) ? null : strip(v);
  if (n.type === 'Literal' && typeof n.value === 'bigint') o.value = Number(n.value);
  return o;
};
for (const file of readFileSync(list, 'utf8').split('\n').filter(Boolean)) {
  const src = readFileSync(file, 'utf8');
  let ir = null, module = true;
  try { ir = strip(acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'module', allowAwaitOutsideFunction: true })); }
  catch (e) { module = false; try { ir = strip(acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'script' })); } catch (e2) { ir = null; } }
  process.stdout.write(JSON.stringify({ file, module, ir }) + '\n');
}
