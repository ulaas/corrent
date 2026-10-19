import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { tokensPlugin } from './plugins/tokens.ts';
import { i18nPlugin } from './plugins/i18n.ts';

// The dev server proxies the API to a local correntd (plain HTTP, see docs/SERVER.md).
const target = process.env.CORRENT_SERVER ?? 'http://127.0.0.1:28443';

export default defineConfig({
  plugins: [tokensPlugin(), i18nPlugin(), svelte()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    assetsInlineLimit: 0,
    reportCompressedSize: true,
    chunkSizeWarningLimit: 400,
  },
  server: {
    port: 5173,
    proxy: { '/api': { target, ws: true, changeOrigin: false } },
  },
  test: { include: ['tests/**/*.test.ts'], environment: 'node' },
});
