import { describe, expect, it } from 'vitest';
import { filterParams, isFiltered, nextSort, readFilter, writeFilter } from '../src/lib/filters';

describe('filter buttons', () => {
  it('reads a filter from the page query and writes it back', () => {
    const f = readFilter(new URLSearchParams('watch=in_progress&fav=1&genre=Sci-Fi&decade=1990&sort=year'));
    expect(f).toEqual({ watch: 'in_progress', favorite: true, genre: 'Sci-Fi', decade: 1990 });
    expect(writeFilter(f)).toEqual({ filter: null, watch: 'in_progress', fav: '1', genre: 'Sci-Fi', decade: '1990' });
    expect(filterParams(f)).toEqual({ watch: 'in_progress', favorite: true, genre: 'Sci-Fi', decade: 1990 });
    expect(isFiltered(f)).toBe(true);
  });

  it('ignores what it does not know', () => {
    const f = readFilter(new URLSearchParams('watch=sometimes&decade=soon&fav=0'));
    expect(isFiltered(f)).toBe(false);
    expect(writeFilter(f)).toEqual({ filter: null, watch: null, fav: null, genre: null, decade: null });
    // Links from before the buttons still filter.
    expect(readFilter(new URLSearchParams('filter=unplayed')).watch).toBe('unplayed');
    expect(readFilter(new URLSearchParams('filter=favorites')).favorite).toBe(true);
  });

  it('a sort button turns the current order around, another starts its useful way', () => {
    expect(nextSort({ sort: 'title', desc: false }, 'title')).toEqual({ sort: 'title', desc: true });
    expect(nextSort({ sort: 'title', desc: true }, 'added')).toEqual({ sort: 'added', desc: true });
    expect(nextSort({ sort: 'added', desc: true }, 'title')).toEqual({ sort: 'title', desc: false });
  });
});
