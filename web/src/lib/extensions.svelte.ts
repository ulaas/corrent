// Web plugins: modules an administrator installed (runtime = "web") that add buttons to item
// pages and pages to Settings. They run in this page with the signed-in person's rights, through
// `window.corrent` (version 1):
//
//   corrent.addItemAction({ id, label, run(item) })      // a button on item pages
//   corrent.addSettingsPage({ id, title, render(el) })    // a page under Settings → Extensions
//   corrent.api.get|post|put|del(path, body?)             // /api/v1, as this person
//   corrent.toast(text, 'success' | 'error' | 'info')
import { api, errorText } from './api';
import { toast } from './toast.svelte';
import type { ItemDetail } from './types';

export interface ItemAction {
  id: string;
  label: string;
  run(item: ItemDetail): void | Promise<void>;
}

export interface ExtensionPage {
  id: string;
  title: string;
  /** Fills `el`; may return a function that cleans up. */
  render(el: HTMLElement): void | (() => void);
}

export const extensions = $state<{ itemActions: ItemAction[]; pages: ExtensionPage[]; errors: string[] }>({
  itemActions: [],
  pages: [],
  errors: [],
});

let loaded = false;

function safeId(s: unknown): string {
  return String(s ?? '').replace(/[^a-z0-9-]/gi, '').slice(0, 64);
}

let current: string | null = null;

/** `<plugin>.<id>`, or '' when the id has nothing usable. */
function scopedId(id: unknown): string {
  const own = safeId(id);
  return own ? `${safeId(current ?? 'ext')}.${own}` : '';
}

export async function loadExtensions() {
  if (loaded) return;
  loaded = true;
  let list: { id: string; name: string; entry: string }[] = [];
  try {
    list = await api.get('/plugins/web');
  } catch {
    return;
  }
  if (!list.length) return;
  (window as unknown as { corrent: unknown }).corrent = {
    version: 1,
    addItemAction(a: ItemAction) {
      const id = scopedId(a?.id);
      if (a && typeof a.run === 'function' && id && !extensions.itemActions.some((x) => x.id === id))
        extensions.itemActions.push({ id, label: String(a.label ?? ''), run: a.run });
    },
    addSettingsPage(p: ExtensionPage) {
      const id = scopedId(p?.id);
      if (p && typeof p.render === 'function' && id && !extensions.pages.some((x) => x.id === id))
        extensions.pages.push({ id, title: String(p.title ?? ''), render: p.render });
    },
    api: {
      get: (p: string) => api.get(p),
      post: (p: string, b?: unknown) => api.post(p, b),
      put: (p: string, b?: unknown) => api.put(p, b),
      del: (p: string) => api.del(p),
    },
    toast: (text: string, kind: 'success' | 'error' | 'info' = 'info') => toast(String(text), kind),
  };
  for (const p of list) {
    // Registrations while a plugin's module runs are named after it, so two plugins can't
    // take each other's ids (later ones, e.g. after an await, fall back to "ext").
    current = p.id;
    try {
      await import(/* @vite-ignore */ p.entry);
    } catch (e) {
      extensions.errors.push(`${p.name}: ${errorText(e)}`);
    }
    current = null;
  }
}
