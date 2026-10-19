#!/usr/bin/env node
// Fails when a key used in web/src has no Turkish entry or the entry's {placeholders} differ.
import { catalog, placeholders, usedKeys, WEB_CATALOG } from './i18n-keys.mjs';
import { relative } from 'node:path';

const cat = catalog();
const used = usedKeys();
const problems = [];
for (const [k, files] of used) {
  const where = [...files].map((f) => relative(process.cwd(), f)).join(', ');
  const tr = cat[k];
  if (typeof tr !== 'string' || !tr.trim()) {
    problems.push(`missing: ${JSON.stringify(k)}  (${where})`);
    continue;
  }
  const a = placeholders(k).join(','), b = placeholders(tr).join(',');
  if (a !== b) problems.push(`placeholders differ: ${JSON.stringify(k)} {${a}} vs {${b}}`);
}
if (problems.length) {
  console.error(`i18n: ${problems.length} problem(s); add entries to ${relative(process.cwd(), WEB_CATALOG)}:`);
  for (const p of problems) console.error('  ' + p);
  process.exit(1);
}
console.log(`i18n: ${used.size} keys, all translated`);
