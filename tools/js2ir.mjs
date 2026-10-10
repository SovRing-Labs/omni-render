// SPDX-License-Identifier: MIT OR Apache-2.0
// JS front-end: a .js file → estree IR JSON (positions stripped) for src/jsi.rs. acorn is a build-time parser here.
// usage: node tools/js2ir.mjs <file.js> <acorn module path> [out.json]
import { readFileSync, writeFileSync } from 'node:fs';
const [,, file, acornPath, out] = process.argv;
const acorn = await import(acornPath);
const src = readFileSync(file, 'utf8');
const ast = acorn.parse(src, { ecmaVersion: 'latest', sourceType: 'module', allowAwaitOutsideFunction: true });
const strip = (n) => {
  if (Array.isArray(n)) return n.map(strip);
  if (!n || typeof n !== 'object') return n;
  if (n instanceof RegExp) return { pattern: n.source, flags: n.flags };
  const o = {};
  for (const [k, v] of Object.entries(n)) if (!['start', 'end', 'loc', 'range', 'raw'].includes(k)) o[k] = (k === 'value' && v instanceof RegExp) ? null : strip(v);
  if (n.type === 'Literal' && typeof n.value === 'bigint') o.value = Number(n.value);
  return o;
};
const json = JSON.stringify(strip(ast));
if (out) writeFileSync(out, json); else process.stdout.write(json);
