// A small history-API router: `route.path`, `route.params`, `route.query`, and `go()`.

export interface Match {
  name: string;
  params: Record<string, string>;
}

const PATTERNS: [string, RegExp, string[]][] = [];

/** Registers a route; patterns use `:name` segments. */
export function define(name: string, pattern: string) {
  const keys: string[] = [];
  const re = new RegExp(
    '^' +
      pattern.replace(/\/:(\w+)/g, (_, k: string) => {
        keys.push(k);
        return '/([^/]+)';
      }) +
      '/?$',
  );
  PATTERNS.push([name, re, keys]);
}

export function match(path: string): Match {
  for (const [name, re, keys] of PATTERNS) {
    const m = re.exec(path);
    if (m) {
      const params: Record<string, string> = {};
      keys.forEach((k, i) => (params[k] = decodeURIComponent(m[i + 1])));
      return { name, params };
    }
  }
  return { name: 'notfound', params: {} };
}

function read() {
  return { path: location.pathname, query: new URLSearchParams(location.search), key: history.state?.key ?? 0 };
}

export const route = $state(read());
let seq = Date.now();

export function go(to: string, opts: { replace?: boolean } = {}) {
  const u = new URL(to, location.origin);
  if (u.pathname + u.search === location.pathname + location.search && !opts.replace) return;
  const key = ++seq;
  if (opts.replace) history.replaceState({ key }, '', u.pathname + u.search + u.hash);
  else history.pushState({ key }, '', u.pathname + u.search + u.hash);
  Object.assign(route, read());
}

/** Updates the query string without a new history entry. */
export function setQuery(q: Record<string, string | null>) {
  const u = new URL(location.href);
  for (const [k, v] of Object.entries(q)) {
    if (v === null || v === '') u.searchParams.delete(k);
    else u.searchParams.set(k, v);
  }
  history.replaceState(history.state, '', u.pathname + u.search);
  Object.assign(route, read());
}

export function back(fallback = '/') {
  if (history.length > 1 && history.state?.key) history.back();
  else go(fallback);
}

addEventListener('popstate', () => Object.assign(route, read()));

/** `<a href use:link>`: client-side navigation for internal links. */
export function link(node: HTMLAnchorElement) {
  const onClick = (e: MouseEvent) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const href = node.getAttribute('href');
    if (!href || !href.startsWith('/') || node.target) return;
    e.preventDefault();
    go(href);
  };
  node.addEventListener('click', onClick);
  return { destroy: () => node.removeEventListener('click', onClick) };
}
