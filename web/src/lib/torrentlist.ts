// Filtering, sorting and counting the torrent list (pure functions; unit-tested).
import { tk } from './i18n.svelte';
import type { Torrent, TorrentState } from './types';
import { trackerHost, type ColId } from './torrentcols';

export type StatusFilter = 'all' | 'downloading' | 'seeding' | 'completed' | 'paused' | 'active' | 'inactive' | 'errored';

export const STATUS_FILTERS: { id: StatusFilter; label: string }[] = [
  { id: 'all', label: tk('All') },
  { id: 'downloading', label: tk('Downloading') },
  { id: 'seeding', label: tk('Seeding') },
  { id: 'completed', label: tk('Completed') },
  { id: 'paused', label: tk('Paused') },
  { id: 'active', label: tk('Active') },
  { id: 'inactive', label: tk('Inactive') },
  { id: 'errored', label: tk('Errored') },
];

export const STATE_LABEL: Record<TorrentState, string> = {
  metadata: tk('Fetching metadata'),
  checking: tk('Checking'),
  downloading: tk('Downloading'),
  stalled: tk('Stalled'),
  seeding: tk('Seeding'),
  completed: tk('Completed'),
  paused: tk('Paused'),
  queued: tk('Queued'),
  error: tk('Error'),
};

/** CSS colour of a state (mirrors Palette::state_color). */
export function stateColor(s: TorrentState): string {
  switch (s) {
    case 'downloading':
      return 'var(--accent)';
    case 'metadata':
      return 'color-mix(in srgb, var(--accent) 80%, transparent)';
    case 'stalled':
      return 'color-mix(in srgb, var(--accent) 55%, var(--bg3))';
    case 'seeding':
      return 'var(--success)';
    case 'completed':
      return 'var(--completed)';
    case 'paused':
      return 'var(--paused)';
    case 'queued':
      return 'var(--queued)';
    case 'checking':
      return 'var(--warn)';
    case 'error':
      return 'var(--error)';
  }
}

export function matchesStatus(t: Torrent, f: StatusFilter): boolean {
  switch (f) {
    case 'all':
      return true;
    case 'downloading':
      return t.state === 'downloading' || t.state === 'metadata' || t.state === 'stalled' || (t.state === 'queued' && t.progress < 1);
    case 'seeding':
      return t.state === 'seeding';
    case 'completed':
      return t.progress >= 1 || t.state === 'completed';
    case 'paused':
      return t.state === 'paused' || t.state === 'completed';
    case 'active':
      return t.down_bps > 0 || t.up_bps > 0;
    case 'inactive':
      return t.down_bps === 0 && t.up_bps === 0;
    case 'errored':
      return t.state === 'error' || !!t.error || t.restore_failed;
  }
}

export interface ListFilter {
  status: StatusFilter;
  /** `null` = any; `''` = without a category. */
  category: string | null;
  tag: string | null;
  text: string;
}

export function filterTorrents(list: Torrent[], f: ListFilter): Torrent[] {
  const words = f.text.trim().toLowerCase().split(/\s+/).filter(Boolean);
  return list.filter((t) => {
    if (!matchesStatus(t, f.status)) return false;
    if (f.category !== null && (t.category ?? '') !== f.category) return false;
    if (f.tag !== null && (f.tag === '' ? t.tags.length > 0 : !t.tags.includes(f.tag))) return false;
    if (words.length) {
      const hay = `${t.name} ${t.hash} ${t.category ?? ''} ${t.tags.join(' ')}`.toLowerCase();
      if (!words.every((w) => hay.includes(w))) return false;
    }
    return true;
  });
}

export type SortKey = 'name' | ColId;

const STATE_ORDER: TorrentState[] = ['downloading', 'metadata', 'checking', 'stalled', 'queued', 'seeding', 'completed', 'paused', 'error'];

function key(t: Torrent, k: SortKey): number | string {
  switch (k) {
    case 'queue':
      return t.queue_pos;
    case 'name':
      return t.name.toLowerCase();
    case 'size':
      return t.size;
    case 'progress':
      return t.progress;
    case 'state':
      return STATE_ORDER.indexOf(t.state);
    case 'down':
      return t.down_bps;
    case 'up':
      return t.up_bps;
    case 'eta':
      return t.eta_secs ?? Number.MAX_SAFE_INTEGER;
    case 'ratio':
      return t.ratio;
    case 'peers':
      return t.peers_live;
    case 'added':
      return t.added_at;
    case 'completed':
      return t.completed_at ?? 0;
    case 'category':
      return (t.category ?? '').toLowerCase();
    case 'tags':
      return t.tags.join(' ').toLowerCase();
    case 'downloaded':
      return t.downloaded;
    case 'uploaded':
      return t.uploaded;
    case 'save_path':
      return t.save_path.toLowerCase();
    case 'tracker':
      return t.trackers[0] ? trackerHost(t.trackers[0]) : '';
    case 'seed_time':
      return t.seeding_secs;
    case 'last_activity':
      return t.last_activity;
    case 'activity':
      return t.down_bps + t.up_bps;
  }
}

export function sortTorrents(list: Torrent[], k: SortKey, desc: boolean): Torrent[] {
  const dir = desc ? -1 : 1;
  const coll = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  return [...list].sort((a, b) => {
    const x = key(a, k), y = key(b, k);
    const c = typeof x === 'string' ? coll.compare(x, y as string) : (x as number) - (y as number);
    return c !== 0 ? c * dir : a.name.localeCompare(b.name);
  });
}

export function statusCounts(list: Torrent[]): Record<StatusFilter, number> {
  const out = Object.fromEntries(STATUS_FILTERS.map((s) => [s.id, 0])) as Record<StatusFilter, number>;
  for (const t of list) for (const s of STATUS_FILTERS) if (matchesStatus(t, s.id)) out[s.id]++;
  return out;
}

/** Parses the add dialog's text box: magnet links, URLs and info hashes, one per line. */
export function parseLinks(text: string): { ok: string[]; bad: string[] } {
  const ok: string[] = [], bad: string[] = [];
  for (const raw of text.split(/[\n\r]+/)) {
    const s = raw.trim();
    if (!s) continue;
    if (/^magnet:\?/i.test(s) || /^https?:\/\//i.test(s) || /^[0-9a-f]{40}$/i.test(s)) ok.push(s);
    else if (/^[A-Z2-7]{32}$/i.test(s)) ok.push(`magnet:?xt=urn:btih:${s}`);
    else bad.push(s);
  }
  return { ok: [...new Set(ok)], bad };
}

/** "MSB-first" bitfield bytes → per-bucket fractions for drawing a pieces bar. */
export function pieceBuckets(bits: number[] | undefined, count: number, buckets: number): number[] {
  const out = new Array(buckets).fill(0);
  if (!bits || !count) return out;
  const per = count / buckets;
  for (let b = 0; b < buckets; b++) {
    const from = Math.floor(b * per), to = Math.max(from + 1, Math.floor((b + 1) * per));
    let have = 0;
    for (let i = from; i < to && i < count; i++) if ((bits[i >> 3] >> (7 - (i & 7))) & 1) have++;
    out[b] = have / (to - from);
  }
  return out;
}
