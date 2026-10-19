#!/usr/bin/env node
// Headless screenshots of the web client for visual checks (never drives the real desktop).
// CORRENT_URL=http://127.0.0.1:28443 CORRENT_USER=admin CORRENT_PASS=… SHOTS_DIR=/tmp/shots \
//   node scripts/screenshots.mjs [route ...]
import { chromium } from 'playwright-core';
import { mkdirSync } from 'node:fs';

const base = process.env.CORRENT_URL ?? 'http://127.0.0.1:28443';
const out = process.env.SHOTS_DIR ?? './shots';
const user = process.env.CORRENT_USER ?? 'admin';
const pass = process.env.CORRENT_PASS ?? '';
const chrome = process.env.CHROME ?? '/usr/bin/google-chrome';
const routes = process.argv.slice(2).length ? process.argv.slice(2) : ['/'];
const sizes = (process.env.SIZES ?? 'desktop,phone').split(',');
const themes = (process.env.THEMES ?? 'dark,light').split(',');
const VIEW = { desktop: { width: 1440, height: 900 }, phone: { width: 390, height: 844 }, tablet: { width: 900, height: 1100 } };

mkdirSync(out, { recursive: true });
const browser = await chromium.launch({ executablePath: chrome, headless: true, args: ['--autoplay-policy=no-user-gesture-required'] });
const errors = [];
for (const size of sizes) {
  for (const theme of themes) {
    const ctx = await browser.newContext({ viewport: VIEW[size], deviceScaleFactor: size === 'phone' ? 2 : 1, colorScheme: theme, isMobile: size === 'phone', hasTouch: size === 'phone' });
    if (pass) {
      const r = await ctx.request.post(`${base}/api/v1/auth/login`, { data: { username: user, password: pass } });
      if (!r.ok()) throw new Error(`login failed: ${r.status()} ${await r.text()}`);
    }
    await ctx.addInitScript((th) => {
      const p = JSON.parse(localStorage.getItem('corrent.prefs') || '{}');
      p.theme = th;
      localStorage.setItem('corrent.prefs', JSON.stringify(p));
      if (localStorage.getItem('corrent.lang') === null && navigator.language) localStorage.setItem('corrent.lang', 'en');
    }, theme);
    const page = await ctx.newPage();
    page.on('console', (m) => m.type() === 'error' && errors.push(`${m.text()}`));
    page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
    for (const r of routes) {
      const [path, action] = r.split('#');
      await page.goto(base + path, { waitUntil: 'networkidle' }).catch(() => {});
      await page.waitForTimeout(Number(process.env.WAIT ?? 900));
      if (action) {
        for (const step of action.split('&')) {
          const [kind, arg] = step.split('=');
          if (kind === 'click') await page.click(decodeURIComponent(arg)).catch((e) => errors.push(`click ${arg}: ${e.message}`));
          if (kind === 'wait') await page.waitForTimeout(+arg);
          if (kind === 'key') await page.keyboard.press(arg);
        }
        await page.waitForTimeout(600);
      }
      const name = `${(path === '/' ? 'home' : path.slice(1).replace(/[/?=&]/g, '_'))}${action ? '-' + action.replace(/[^a-z0-9]+/gi, '_').slice(0, 30) : ''}-${size}-${theme}.png`;
      await page.screenshot({ path: `${out}/${name}`, fullPage: process.env.FULL === '1' });
      console.log(name);
    }
    await ctx.close();
  }
}
await browser.close();
if (errors.length) console.log('console errors:\n  ' + [...new Set(errors)].join('\n  '));
