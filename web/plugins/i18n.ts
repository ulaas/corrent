// `virtual:i18n/tr`: the Turkish catalog for the keys the web client uses, merged from
// assets/i18n/tr.toml (shared with the desktop app) and assets/i18n/web/tr.toml (web only).
import type { Plugin } from 'vite';
// @ts-expect-error plain JS helper shared with scripts/check-i18n.mjs
import { CATALOGS, catalog, usedKeys } from '../scripts/i18n-keys.mjs';

const ID = 'virtual:i18n/tr';

export function i18nPlugin(): Plugin {
  return {
    name: 'corrent-i18n',
    resolveId(id) {
      return id === ID ? '\0' + ID : null;
    },
    load(id) {
      if (id !== '\0' + ID) return null;
      for (const f of CATALOGS) this.addWatchFile(f);
      const all: Record<string, string> = catalog();
      const used: Map<string, Set<string>> = usedKeys();
      const out: Record<string, string> = {};
      for (const k of used.keys()) if (typeof all[k] === 'string') out[k] = all[k];
      return `export default ${JSON.stringify(out)};`;
    },
  };
}
