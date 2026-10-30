import { describe, expect, it } from 'vitest';
import { DEFAULT_COLUMNS, cell, fit, known, template, trackerHost } from '../src/lib/torrentcols';
import { sortTorrents } from '../src/lib/torrentlist';
import type { Torrent } from '../src/lib/types';

const tor = (o: Partial<Torrent>) => ({ name: 'x', tags: [], trackers: [], queue_pos: 0, ...o }) as Torrent;

describe('torrent columns', () => {
  it('keeps what fits, dropping from the end', () => {
    expect(fit(DEFAULT_COLUMNS, 2000)).toEqual(DEFAULT_COLUMNS);
    expect(fit(DEFAULT_COLUMNS, 600)).toEqual(['size', 'state']);
    expect(fit(DEFAULT_COLUMNS, 200)).toEqual([]);
  });
  it('builds the grid from the columns', () => {
    expect(template(['size', 'state'])).toBe('40px minmax(240px, 1fr) 84px 110px');
  });
  it('forgets unknown and repeated columns', () => {
    expect(known(['size', 'nope', 'size', 'tracker'])).toEqual(['size', 'tracker']);
  });
  it('shows queue positions, trackers and tags', () => {
    expect(cell(tor({ queue_pos: 0 }), 'queue')).toBe('1');
    expect(cell(tor({ pending: true }), 'queue')).toBe('–');
    expect(trackerHost('udp://tracker.example:80/announce')).toBe('tracker.example');
    expect(cell(tor({ trackers: ['https://t.example/announce'] }), 'tracker')).toBe('t.example');
    expect(cell(tor({ tags: ['a', 'b'] }), 'tags')).toBe('a, b');
  });
  it('sorts by the new columns', () => {
    const list = [tor({ name: 'a', uploaded: 5 }), tor({ name: 'b', uploaded: 9 })];
    expect(sortTorrents(list, 'uploaded', true).map((x) => x.name)).toEqual(['b', 'a']);
    const tr = [tor({ name: 'a', trackers: ['udp://z.example'] }), tor({ name: 'b', trackers: ['udp://a.example'] })];
    expect(sortTorrents(tr, 'tracker', false).map((x) => x.name)).toEqual(['b', 'a']);
  });
});
