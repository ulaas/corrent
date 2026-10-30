import { describe, expect, it } from 'vitest';
import { KEEP, points, sample, speeds } from '../src/lib/speedhistory.svelte';
import { live } from '../src/lib/torrents.svelte';
import type { Torrent } from '../src/lib/types';

describe('speed history', () => {
  it('draws the newest sample at the right edge', () => {
    expect(points([0, 50, 100], 100, 299, 100)).toBe('297.0,100.0 298.0,50.0 299.0,0.0');
    expect(points([5], 10, 100, 100)).toBe('');
  });
  it('keeps the session and torrents that moved data, at most KEEP samples', () => {
    live.stats = { down_bps: 10, up_bps: 2 } as typeof live.stats;
    live.list = [{ hash: 'a', down_bps: 5, up_bps: 0 }, { hash: 'b', down_bps: 0, up_bps: 0 }] as Torrent[];
    for (let i = 0; i < KEEP + 5; i++) sample();
    expect(speeds.all.length).toBe(KEEP);
    expect(speeds.all.at(-1)).toEqual([10, 2]);
    expect(Object.keys(speeds.per)).toEqual(['a']);
    live.list = [];
    sample();
    expect(speeds.per).toEqual({});
  });
});
