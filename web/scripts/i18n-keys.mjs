// Shared by the Vite i18n plugin and check-i18n.mjs: finds the translation keys used in web/src
// (`t("…")`, `t('…')` and the marker `tk("…")` for strings translated later) and reads the
// Turkish catalogs.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { parse } from 'smol-toml';

const here = import.meta.dirname;
export const SRC = resolve(here, '../src');
export const CATALOGS = [resolve(here, '../../assets/i18n/tr.toml'), resolve(here, '../../assets/i18n/web/tr.toml')];
export const WEB_CATALOG = CATALOGS[1];

const CALL = /(?<![\w$.])tk?\(\s*(['"])((?:\\.|(?!\1)[^\\\n])*)\1\s*[,)]/g;

function unquote(q, body) {
  if (q === '"') return JSON.parse(`"${body}"`);
  return JSON.parse(`"${body.replace(/\\'/g, "'").replace(/"/g, '\\"')}"`);
}

export function keysIn(text) {
  const out = [];
  for (const m of text.matchAll(CALL)) out.push(unquote(m[1], m[2]));
  return out;
}

function walk(dir, acc) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, acc);
    else if (/\.(ts|svelte)$/.test(name) && !name.endsWith('.d.ts')) acc.push(p);
  }
  return acc;
}

/** Map of key → files using it. */
export function usedKeys(dir = SRC) {
  const keys = new Map();
  for (const f of walk(dir, [])) {
    for (const k of keysIn(readFileSync(f, 'utf8'))) {
      if (!keys.has(k)) keys.set(k, new Set());
      keys.get(k).add(f);
    }
  }
  return keys;
}

/** The merged Turkish catalog (the web file wins over the shared one). */
export function catalog(files = CATALOGS) {
  const out = {};
  for (const f of files) {
    let text = '';
    try {
      text = readFileSync(f, 'utf8');
    } catch {
      continue;
    }
    Object.assign(out, parse(text));
  }
  return out;
}

export function placeholders(s) {
  return [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}
