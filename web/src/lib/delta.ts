// Applying the server's torrent list deltas (mirrors corrent_proto::torrent::apply).
import type { Torrent, TorrentPatch, TorrentsDelta } from './types';

const FAST = [
  'state',
  'done',
  'progress',
  'uploaded',
  'downloaded',
  'down_bps',
  'up_bps',
  'ratio',
  'peers_live',
  'peers_seen',
  'peers_connecting',
  'last_activity',
  'seeding_secs',
  'queue_pos',
] as const;

/** Applies `d` to `map` (hash → torrent) in place; returns whether the set of rows changed. */
export function applyDelta(map: Map<string, Torrent>, d: TorrentsDelta): boolean {
  let structural = false;
  for (const h of d.removed ?? []) structural = map.delete(h) || structural;
  for (const t of d.upserted ?? []) {
    if (!map.has(t.hash)) structural = true;
    map.set(t.hash, normalize(t));
  }
  for (const p of d.patched ?? []) {
    const t = map.get(p.hash);
    if (!t) continue;
    map.set(p.hash, patch(t, p));
  }
  return structural;
}

/** A new object with the patch's fields applied (rows are immutable for cheap change checks). */
export function patch(t: Torrent, p: TorrentPatch): Torrent {
  const n: Torrent = { ...t };
  const rec = n as unknown as Record<string, unknown>;
  for (const f of FAST) {
    const v = p[f];
    if (v !== undefined) rec[f] = v;
  }
  if ('eta_secs' in p) n.eta_secs = p.eta_secs ?? undefined;
  return n;
}

/** Fills fields serde skips when empty. */
export function normalize(t: Torrent): Torrent {
  t.tags ??= [];
  t.trackers ??= [];
  return t;
}
