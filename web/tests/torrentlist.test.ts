import { describe, expect, it } from 'vitest';
import { filterTorrents, parseLinks, pieceBuckets, sortTorrents, statusCounts } from '../src/lib/torrentlist';
import { row } from './fixtures';

const list = [
  row(1, { name: 'Ubuntu 24.04', state: 'downloading', down_bps: 100, category: 'Linux', tags: ['iso'] }),
  row(2, { name: 'Sintel', state: 'seeding', progress: 1, up_bps: 50, category: 'Movies', tags: ['open-movie'] }),
  row(3, { name: 'Big Buck Bunny', state: 'paused', progress: 0.2 }),
  row(4, { name: 'broken', state: 'error', error: 'disk full' }),
];

describe('torrent list', () => {
  it('filters by status, category, tag and text', () => {
    const f = (o: object) => filterTorrents(list, { status: 'all', category: null, tag: null, text: '', ...o }).map((t) => t.name);
    expect(f({ status: 'downloading' })).toEqual(['Ubuntu 24.04']);
    expect(f({ status: 'seeding' })).toEqual(['Sintel']);
    expect(f({ status: 'active' })).toEqual(['Ubuntu 24.04', 'Sintel']);
    expect(f({ status: 'errored' })).toEqual(['broken']);
    expect(f({ category: '' })).toEqual(['Big Buck Bunny', 'broken']);
    expect(f({ category: 'Movies' })).toEqual(['Sintel']);
    expect(f({ tag: 'iso' })).toEqual(['Ubuntu 24.04']);
    expect(f({ text: 'buck BIG' })).toEqual(['Big Buck Bunny']);
  });

  it('sorts with a stable tie-break and natural numbers', () => {
    expect(sortTorrents(list, 'name', false).map((t) => t.queue_pos)).toEqual([3, 4, 2, 1]);
    expect(sortTorrents(list, 'down', true)[0].name).toBe('Ubuntu 24.04');
    expect(sortTorrents(list, 'queue', true).map((t) => t.queue_pos)).toEqual([4, 3, 2, 1]);
  });

  it('counts each status', () => {
    const c = statusCounts(list);
    expect(c.all).toBe(4);
    expect(c.completed).toBe(1);
    expect(c.inactive).toBe(2);
  });

  it('parses magnet links, URLs and info hashes', () => {
    const r = parseLinks(`magnet:?xt=urn:btih:abc\n\nhttps://x.org/a.torrent\n${'a'.repeat(40)}\nnot a link\nmagnet:?xt=urn:btih:abc\n${'A'.repeat(32)}`);
    expect(r.ok).toEqual(['magnet:?xt=urn:btih:abc', 'https://x.org/a.torrent', 'a'.repeat(40), `magnet:?xt=urn:btih:${'A'.repeat(32)}`]);
    expect(r.bad).toEqual(['not a link']);
  });

  it('turns an MSB-first bitfield into buckets', () => {
    // 10 pieces: first 8 done (0xff), then piece 8 done, 9 missing (0b10000000)
    expect(pieceBuckets([0xff, 0x80], 10, 10)).toEqual([1, 1, 1, 1, 1, 1, 1, 1, 1, 0]);
    expect(pieceBuckets([0xf0], 8, 2)).toEqual([1, 0]);
    expect(pieceBuckets(undefined, 8, 2)).toEqual([0, 0]);
  });
});
