// Pages loaded on demand. An import is always asynchronous, even for a module that's already here,
// so `{#await import(...)}` shows its spinner on every visit: once resolved, a component is handed
// out synchronously.
import type { Component } from 'svelte';
import { SvelteMap, SvelteSet } from 'svelte/reactivity';

export type Loader = () => Promise<{ default: Component<any> }>;

const resolved = new SvelteMap<string, Component<any>>();
const failed = new SvelteSet<string>();
const started = new Set<string>();

/** The component for `key`, or undefined while `load` runs (reactive: reads update when it's done). */
export function lazy(key: string, load: Loader): Component<any> | undefined {
  const c = resolved.get(key);
  if (!c && !started.has(key)) {
    started.add(key);
    load().then(
      (m) => resolved.set(key, m.default),
      () => {
        // A chunk that's gone (a new build) or no network: the next visit tries again.
        started.delete(key);
        failed.add(key);
      },
    );
  }
  return c;
}

export function lazyFailed(key: string): boolean {
  return failed.has(key);
}
