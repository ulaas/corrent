// Names the service worker's cache after this build, so a new build starts a fresh cache and
// activate drops the old one with its stale hashed chunks. File names carry content hashes, so
// their list identifies the build.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { Plugin } from 'vite';

const PLACEHOLDER = '__BUILD_HASH__';

export function swHashPlugin(): Plugin {
  return {
    name: 'corrent-sw-hash',
    apply: 'build',
    writeBundle(options, bundle) {
      const hash = createHash('sha256').update(Object.keys(bundle).sort().join('\n')).digest('hex').slice(0, 12);
      const file = join(options.dir ?? resolve('dist'), 'sw.js');
      const text = readFileSync(file, 'utf8');
      if (!text.includes(PLACEHOLDER)) throw new Error(`${PLACEHOLDER} not found in ${file}`);
      writeFileSync(file, text.replaceAll(PLACEHOLDER, hash));
    },
  };
}
