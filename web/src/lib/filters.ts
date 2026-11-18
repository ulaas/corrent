// The filter and sort buttons of library pages and Home (components/FilterBar.svelte): what is
// chosen lives in the page's query string, so Back and a shared link keep it. The desktop app has
// the same buttons (src/app/media/filters.rs).
import type { SortBy } from './types';

export type Watch = 'unplayed' | 'in_progress' | 'played';

export interface ItemFilter {
  watch?: Watch;
  favorite?: boolean;
  genre?: string;
  /** The first year of a decade (2010 = 2010–2019). */
  decade?: number;
}

const WATCH: Watch[] = ['unplayed', 'in_progress', 'played'];

/** The filter in a page's query string (`?watch=played&fav=1&genre=Drama&decade=2010`). */
export function readFilter(q: URLSearchParams): ItemFilter {
  const watch = WATCH.find((w) => w === q.get('watch'));
  const decade = Number(q.get('decade'));
  return {
    watch,
    favorite: q.get('fav') === '1' || undefined,
    genre: q.get('genre') || undefined,
    decade: Number.isInteger(decade) && decade > 0 ? decade : undefined,
  };
}

/** The filter as page query values for `setQuery` (null removes one). */
export function writeFilter(f: ItemFilter): Record<string, string | null> {
  return {
    watch: f.watch ?? null,
    fav: f.favorite ? '1' : null,
    genre: f.genre ?? null,
    decade: f.decade ? String(f.decade) : null,
  };
}

/** The filter as parameters of `GET /items` and `GET /home`. */
export function filterParams(f: ItemFilter): Record<string, string | number | boolean | undefined> {
  return { watch: f.watch, favorite: f.favorite || undefined, genre: f.genre, decade: f.decade };
}

export function isFiltered(f: ItemFilter): boolean {
  return !!(f.watch || f.favorite || f.genre || f.decade);
}

/** The order after pressing a sort button: the current one turns around, another starts in its
 * useful direction (newest first for dates). */
export function nextSort(cur: { sort: SortBy; desc: boolean }, clicked: SortBy): { sort: SortBy; desc: boolean } {
  if (clicked === cur.sort) return { sort: cur.sort, desc: !cur.desc };
  return { sort: clicked, desc: clicked === 'added' || clicked === 'played' || clicked === 'year' };
}
