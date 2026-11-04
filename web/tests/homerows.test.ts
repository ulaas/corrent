import { describe, expect, it } from 'vitest';
import { rowFilter } from '../src/lib/homerows';
import type { Item } from '../src/lib/types';

const item = (extra: Partial<Item> = {}): Item => ({
  id: 1,
  library_id: 1,
  kind: 'movie',
  title: 't',
  state: { position_ms: 0, played: false, play_count: 0, favorite: false },
  added_at: 0,
  ...extra,
});

describe('home rows in full', () => {
  it('asks the documented filters', () => {
    expect(rowFilter('favorites')?.query.favorite).toBe(true);
    expect(rowFilter('continue')?.query.in_progress).toBe(true);
    expect(rowFilter('next_up')?.query.next_up).toBe(true);
    expect(rowFilter('recent:3')).toBeNull();
  });
  it('sees through a server that ignores the filter', () => {
    const unwatched = item({ kind: 'movie' });
    expect(rowFilter('continue')?.fits(unwatched)).toBe(false);
    expect(rowFilter('favorites')?.fits(unwatched)).toBe(false);
    expect(rowFilter('next_up')?.fits(unwatched)).toBe(false);
    expect(rowFilter('continue')?.fits(item({ state: { ...unwatched.state, position_ms: 9000 } }))).toBe(true);
    expect(rowFilter('next_up')?.fits(item({ kind: 'episode' }))).toBe(true);
  });
});
