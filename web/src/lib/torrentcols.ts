// The torrent list's columns (as on the desktop, src/app/mod.rs `Col`): which exist, how wide
// they are, what they show, and which fit the list's width. The name column (with its
// progress bar) is always first.
import { tk } from './i18n.svelte';
import { bytes, date, duration, num, percent, rate } from './format';
import type { Torrent } from './types';

export type ColId =
  | 'queue'
  | 'size'
  | 'progress'
  | 'state'
  | 'peers'
  | 'down'
  | 'up'
  | 'eta'
  | 'ratio'
  | 'category'
  | 'tags'
  | 'added'
  | 'completed'
  | 'downloaded'
  | 'uploaded'
  | 'save_path'
  | 'tracker'
  | 'seed_time'
  | 'last_activity'
  | 'activity';

export interface ColDef {
  id: ColId;
  /** Translation key of the header. */
  label: string;
  width: number;
  right?: boolean;
}

export const COLUMNS: ColDef[] = [
  { id: 'queue', label: '#', width: 40, right: true },
  { id: 'size', label: tk('Size'), width: 84, right: true },
  { id: 'progress', label: tk('Progress'), width: 70, right: true },
  { id: 'state', label: tk('Status'), width: 110 },
  { id: 'peers', label: tk('Peers'), width: 70, right: true },
  { id: 'down', label: tk('Down'), width: 88, right: true },
  { id: 'up', label: tk('Up'), width: 88, right: true },
  { id: 'eta', label: tk('ETA'), width: 76, right: true },
  { id: 'ratio', label: tk('Ratio'), width: 56, right: true },
  { id: 'category', label: tk('Category'), width: 110 },
  { id: 'tags', label: tk('Tags'), width: 110 },
  { id: 'added', label: tk('Added'), width: 130 },
  { id: 'completed', label: tk('Completed'), width: 130 },
  { id: 'downloaded', label: tk('Downloaded'), width: 90, right: true },
  { id: 'uploaded', label: tk('Uploaded'), width: 90, right: true },
  { id: 'save_path', label: tk('Save path'), width: 220 },
  { id: 'tracker', label: tk('Tracker'), width: 160 },
  { id: 'seed_time', label: tk('Seeding time'), width: 100, right: true },
  { id: 'last_activity', label: tk('Last activity'), width: 130 },
  { id: 'activity', label: tk('Activity'), width: 100, right: true },
];

const BY_ID = new Map(COLUMNS.map((c) => [c.id, c]));
export const col = (id: ColId): ColDef => BY_ID.get(id)!;

/** As the list looked before columns could be chosen (the desktop's defaults, Progress being
 * part of the name column here). */
export const DEFAULT_COLUMNS: ColId[] = ['size', 'state', 'down', 'up', 'eta', 'ratio', 'peers'];

/** Stored columns that still exist, without repeats. */
export function known(ids: readonly string[]): ColId[] {
  return [...new Set(ids)].filter((x): x is ColId => BY_ID.has(x as ColId));
}

// Fixed parts of a row: check box, name column's minimum, gaps and padding.
const CHECK = 40;
const NAME_MIN = 240;
const GAP = 8;
const PAD = 24;

/** The chosen columns that fit `width`, dropped from the end (the first chosen matter most). */
export function fit(ids: ColId[], width: number): ColId[] {
  let used = CHECK + NAME_MIN + PAD + GAP;
  const out: ColId[] = [];
  for (const id of ids) {
    used += col(id).width + GAP;
    if (used > width) break;
    out.push(id);
  }
  return out;
}

/** The CSS grid of the header and rows. */
export function template(ids: ColId[]): string {
  return [`${CHECK}px`, `minmax(${NAME_MIN}px, 1fr)`, ...ids.map((id) => `${col(id).width}px`)].join(' ');
}

/** The host of a tracker URL ("udp://tracker.example:80/announce" → "tracker.example"). */
export function trackerHost(url: string): string {
  const m = /^[a-z][a-z0-9+.-]*:\/\/([^/:?#]+)/i.exec(url);
  return m ? m[1] : url;
}

/** What a plain column shows (state, rates and peers are drawn by the page). */
export function cell(x: Torrent, id: ColId): string {
  switch (id) {
    case 'queue':
      return x.pending || x.queue_pos > 1e9 ? '–' : String(x.queue_pos + 1);
    case 'size':
      return bytes(x.size);
    case 'progress':
      return percent(x.progress);
    case 'eta':
      return x.eta_secs != null && x.progress < 1 ? duration(x.eta_secs) : '–';
    case 'ratio':
      return num(x.ratio, 2);
    case 'category':
      return x.category ?? '';
    case 'tags':
      return x.tags.join(', ');
    case 'added':
      return date(x.added_at);
    case 'completed':
      return x.completed_at ? date(x.completed_at) : '–';
    case 'downloaded':
      return bytes(x.downloaded);
    case 'uploaded':
      return bytes(x.uploaded);
    case 'save_path':
      return x.save_path;
    case 'tracker':
      return x.trackers[0] ? trackerHost(x.trackers[0]) : '';
    case 'seed_time':
      return x.seeding_secs ? duration(x.seeding_secs) : '–';
    case 'last_activity':
      return x.last_activity ? date(x.last_activity) : '–';
    case 'activity':
      return x.down_bps || x.up_bps ? rate(x.down_bps + x.up_bps) : '–';
    case 'down':
      return rate(x.down_bps);
    case 'up':
      return rate(x.up_bps);
    case 'state':
    case 'peers':
      return '';
  }
}
