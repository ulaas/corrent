// Which `GET /items` filter shows a home row in full (docs/API.md). `fits` tells whether an answered
// item can belong to the row: a server that ignores the parameter answers with everything.
import type { Item } from './types';

export interface RowFilter {
  query: Record<string, string | number | boolean>;
  fits: (item: Item) => boolean;
}

export function rowFilter(id: string): RowFilter | null {
  switch (id) {
    case 'favorites':
      return { query: { favorite: true, sort: 'played', desc: true }, fits: (i) => i.state.favorite };
    case 'continue':
      return { query: { in_progress: true }, fits: (i) => i.state.position_ms > 0 && !i.state.played };
    case 'next_up':
      // Not paged on the server: at most 100 rows, all in the first answer.
      return { query: { next_up: true }, fits: (i) => i.kind === 'episode' && !i.state.played };
    default:
      return null;
  }
}
