import { describe, expect, it } from 'vitest';
import { applyDelta, patch } from '../src/lib/delta';
import { row } from './fixtures';

describe('applyDelta', () => {
  it('upserts, patches and removes like corrent_proto::torrent::apply', () => {
    const m = new Map([row(1), row(2), row(3)].map((t) => [t.hash, t]));
    const before = m.get(row(1).hash)!;
    const structural = applyDelta(m, {
      upserted: [row(4), row(2, { name: 'renamed' })],
      patched: [{ hash: row(1).hash, down_bps: 1234, eta_secs: 60, progress: 0.5 }],
      removed: [row(3).hash],
    });
    expect(structural).toBe(true);
    expect([...m.keys()].length).toBe(3);
    expect(m.get(row(2).hash)!.name).toBe('renamed');
    expect(m.get(row(1).hash)!.down_bps).toBe(1234);
    expect(m.get(row(1).hash)!.eta_secs).toBe(60);
    expect(m.get(row(1).hash)).not.toBe(before);
    expect(before.down_bps).toBe(0);
  });

  it('clears the ETA with null and ignores unknown hashes', () => {
    const m = new Map([[row(1).hash, row(1, { eta_secs: 5 })]]);
    expect(applyDelta(m, { patched: [{ hash: row(1).hash, eta_secs: null }, { hash: 'x'.repeat(40), down_bps: 1 }] })).toBe(false);
    expect(m.get(row(1).hash)!.eta_secs).toBeUndefined();
    expect(m.size).toBe(1);
  });

  it('leaves fields absent from a patch alone', () => {
    const t = row(1, { up_bps: 7, ratio: 1.5 });
    const p = patch(t, { hash: t.hash, down_bps: 9 });
    expect(p.up_bps).toBe(7);
    expect(p.ratio).toBe(1.5);
    expect(p.down_bps).toBe(9);
  });
});
