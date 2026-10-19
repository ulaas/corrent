// Turns design/tokens.json into CSS custom properties (`virtual:tokens.css`) and a small JS
// module with the preset and accent names (`virtual:tokens`).
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Plugin } from 'vite';
import { onColor } from '../src/lib/color.ts';

const FILE = resolve(import.meta.dirname, '../../design/tokens.json');
const CSS_ID = 'virtual:tokens.css';
const JS_ID = 'virtual:tokens';

type Colors = Record<string, string>;
interface Tokens {
  palettes: Record<string, Record<'dark' | 'light', Colors>>;
  accents: Record<string, Record<'dark' | 'light', string>>;
  type: {
    family: Record<string, string>;
    scale: Record<string, number>;
    weight: Record<string, number>;
    line_height: Record<string, number>;
  };
  radius: Record<string, number>;
  space: Record<string, number>;
  elevation: Record<string, { y: number; blur: number; alpha_dark: number; alpha_light: number }>;
  motion: { duration: Record<string, number>; easing: Record<string, string> };
  focus: { width: number; offset: number };
}

const kebab = (s: string) => s.replace(/_/g, '-');

export function tokensCss(t: Tokens): string {
  const out: string[] = [];
  const root: string[] = [
    '--radius-scale: 1;',
    '--density: 1;',
    '--motion: var(--motion-user, 1);',
  ];
  for (const [k, v] of Object.entries(t.type.family)) root.push(`--font-${k}: ${v};`);
  for (const [k, v] of Object.entries(t.type.scale)) root.push(`--fs-${k}: ${v}px;`);
  for (const [k, v] of Object.entries(t.type.weight)) root.push(`--fw-${k}: ${v};`);
  for (const [k, v] of Object.entries(t.type.line_height)) root.push(`--lh-${k}: ${v};`);
  for (const [k, v] of Object.entries(t.radius)) {
    root.push(k === 'pill' ? `--r-${k}: ${v}px;` : `--r-${k}: calc(${v}px * var(--radius-scale));`);
  }
  for (const [k, v] of Object.entries(t.space)) root.push(`--sp-${k}: calc(${v}px * var(--density));`);
  for (const [k, v] of Object.entries(t.motion.duration)) root.push(`--dur-${k}: calc(${v}ms * var(--motion));`);
  for (const [k, v] of Object.entries(t.motion.easing)) root.push(`--ease-${k}: ${v};`);
  root.push(`--focus-width: ${t.focus.width}px;`, `--focus-offset: ${t.focus.offset}px;`);
  out.push(`:root {\n  ${root.join('\n  ')}\n}`);
  out.push('@media (prefers-reduced-motion: reduce) {\n  :root { --motion: 0; }\n}');

  for (const scheme of ['dark', 'light'] as const) {
    const sh = Object.entries(t.elevation).map(
      ([k, e]) =>
        `--shadow-${k}: 0 ${e.y}px ${e.blur}px rgba(0, 0, 0, ${scheme === 'dark' ? e.alpha_dark : e.alpha_light});`,
    );
    out.push(`:root[data-scheme="${scheme}"] {\n  color-scheme: ${scheme};\n  ${sh.join('\n  ')}\n}`);
    for (const [name, a] of Object.entries(t.accents)) {
      out.push(
        `:root[data-scheme="${scheme}"][data-accent="${name}"] { --accent: ${a[scheme]}; --on-accent: ${onColor(a[scheme])}; }`,
      );
    }
    for (const [preset, p] of Object.entries(t.palettes)) {
      const c = p[scheme];
      const vars = Object.entries(c)
        .filter(([k]) => !k.startsWith('$') && k !== 'accent')
        .map(([k, v]) => `--${kebab(k)}: ${v};`);
      out.push(`:root[data-preset="${preset}"][data-scheme="${scheme}"] {\n  ${vars.join('\n  ')}\n}`);
      if (c.accent) {
        // Fixed accent: more specific than the accent rules (and the client skips custom accents).
        out.push(
          `:root[data-preset="${preset}"][data-scheme="${scheme}"][data-accent] { --accent: ${c.accent}; --on-accent: ${onColor(c.accent)}; }`,
        );
      }
    }
  }
  return out.join('\n') + '\n';
}

export function tokensJs(t: Tokens): string {
  const fixed = Object.entries(t.palettes)
    .filter(([, p]) => p.dark.accent)
    .map(([k]) => k);
  return (
    `export const presets = ${JSON.stringify(Object.keys(t.palettes))};\n` +
    `export const fixedAccentPresets = ${JSON.stringify(fixed)};\n` +
    `export const accents = ${JSON.stringify(t.accents)};\n` +
    `export const durations = ${JSON.stringify(t.motion.duration)};\n`
  );
}

export function tokensPlugin(): Plugin {
  const load = (): Tokens => JSON.parse(readFileSync(FILE, 'utf8'));
  return {
    name: 'corrent-tokens',
    resolveId(id) {
      if (id === CSS_ID) return '\0' + CSS_ID;
      if (id === JS_ID) return '\0' + JS_ID;
      return null;
    },
    load(id) {
      if (id === '\0' + CSS_ID) {
        this.addWatchFile(FILE);
        return tokensCss(load());
      }
      if (id === '\0' + JS_ID) {
        this.addWatchFile(FILE);
        return tokensJs(load());
      }
      return null;
    },
  };
}
