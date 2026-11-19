import { describe, expect, it } from 'vitest';
import { filterParams, isFiltered, nextSort, readFilter, writeFilter } from '../src/lib/filters';
import { arriving, remembering, viewPart } from '../src/lib/views';

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

  it('a page opens with the view it had last, unless its address names one', () => {
    let saved = remembering('/library/3', '?watch=played&sort=added&desc=1&r=2', {});
    saved = remembering('/', '?fav=1', saved);
    expect(saved).toEqual({ '/library/3': 'watch=played&sort=added&desc=1', '/': 'fav=1' });
    expect(arriving('/library/3', '', saved)).toBe('?watch=played&sort=added&desc=1');
    expect(arriving('/library/3/', '', saved)).toBe('?watch=played&sort=added&desc=1');
    expect(arriving('/', '', saved)).toBe('?fav=1');
    // Another library, another page, and a link that says what to show.
    expect(arriving('/library/4', '', saved)).toBe('');
    expect(arriving('/item/3', '', saved)).toBe('');
    expect(arriving('/library/3', '?sort=year', saved)).toBe('?sort=year');
    expect(viewPart('?x=1&genre=Sci-Fi+%26+Fantasy')).toBe('genre=Sci-Fi+%26+Fantasy');
    // "Clear filters" and the first sort: nothing is kept, the page opens plain again.
    saved = remembering('/library/3', '', saved);
    expect(saved).toEqual({ '/': 'fav=1' });
    expect(arriving('/library/3', '', saved)).toBe('');
    // Only Home and the library pages have such a view.
    expect(remembering('/search', '?sort=size', saved)).toBe(saved);
  });

  it('a sort button turns the current order around, another starts its useful way', () => {
    expect(nextSort({ sort: 'title', desc: false }, 'title')).toEqual({ sort: 'title', desc: true });
    expect(nextSort({ sort: 'title', desc: true }, 'added')).toEqual({ sort: 'added', desc: true });
    expect(nextSort({ sort: 'added', desc: true }, 'title')).toEqual({ sort: 'title', desc: false });
  });
});
