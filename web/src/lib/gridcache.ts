// What grids and the photo viewer fetched, so Back shows it at once. Everything here is this
// user's: it is dropped at sign-out.
import type { Item } from './types';

export interface GridPages {
  pages: Record<number, Item[]>;
  total: number;
}

const MAX_GRIDS = 5;
const grids = new Map<string, GridPages>();

export function cachedGrid(key: string): GridPages | undefined {
  return grids.get(key);
}

/** Stores (and marks as the newest) a grid's pages; the oldest of more than five goes. */
export function rememberGrid(key: string, g: GridPages) {
  grids.delete(key);
  grids.set(key, g);
  if (grids.size > MAX_GRIDS) {
    const oldest = grids.keys().next().value;
    if (oldest !== undefined) grids.delete(oldest);
  }
}

/** A photo's siblings by the query that lists them (up to 2000 items: asked for once). */
export const photoSiblings = new Map<string, Item[]>();

export function forgetGrids() {
  grids.clear();
  photoSiblings.clear();
}
