// What Home and each library page were last filtered and sorted by, kept in this browser: a page
// opened without a choice in its address (the sidebar, a new launch) shows the one it had. An
// address that carries one (a shared link, Home's "Recently added") is shown as it is and doesn't
// replace it; only the buttons do. The desktop app keeps the same in its preferences
// (src/app/media/filters.rs).
const KEY = 'corrent.views';
/** The parts of a page's query that are its view (`filter` is what old links carry). */
const VIEW_KEYS = ['view', 'watch', 'fav', 'genre', 'decade', 'sort', 'desc', 'filter'];
/** Home and the library pages. */
const PAGE = /^\/(?:library\/\d+)?$/;

function pageOf(path: string): string | undefined {
  const p = path.length > 1 ? path.replace(/\/$/, '') : path;
  return PAGE.test(p) ? p : undefined;
}

/** The view in a query string, as one (`''`: the default view). */
export function viewPart(search: string): string {
  const q = new URLSearchParams(search);
  const out = new URLSearchParams();
  for (const k of VIEW_KEYS) {
    const v = q.get(k);
    if (v) out.set(k, v);
  }
  return out.toString();
}

/** The query a page is opened with: its own when it names a view, else with the saved one. */
export function arriving(path: string, search: string, saved: Record<string, string>): string {
  const page = pageOf(path);
  const kept = page === undefined ? undefined : saved[page];
  if (!kept || viewPart(search)) return search;
  const q = new URLSearchParams(search);
  for (const [k, v] of new URLSearchParams(kept)) q.set(k, v);
  return `?${q}`;
}

/** The saved views after a page's buttons changed its query (the default view isn't kept). */
export function remembering(path: string, search: string, saved: Record<string, string>): Record<string, string> {
  const page = pageOf(path);
  if (page === undefined) return saved;
  const { [page]: _old, ...rest } = saved;
  const view = viewPart(search);
  return view ? { ...rest, [page]: view } : rest;
}

function load(): Record<string, string> {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return v && typeof v === 'object' && !Array.isArray(v) ? v : {};
  } catch {
    return {};
  }
}

/** After a filter, sort or view button changed the address: this page's view is kept. */
export function rememberView() {
  try {
    localStorage.setItem(KEY, JSON.stringify(remembering(location.pathname, location.search, load())));
  } catch {
    /* private mode */
  }
}

/** For the router's `onArrive`: a page comes up with its saved view. */
export function recallView(u: URL) {
  u.search = arriving(u.pathname, u.search, load());
}
