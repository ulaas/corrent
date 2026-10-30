import { describe, expect, it } from 'vitest';
import { folderProblem, isMedia, magnet, menuFor, type TorrentMenuActions } from '../src/lib/torrentmenu';
import type { MenuItem } from '../src/lib/contextmenu.svelte';
import type { Torrent } from '../src/lib/types';

const tor = (o: Partial<Torrent>): Torrent =>
  ({
    hash: 'ab'.repeat(20),
    name: 'Sintel',
    state: 'downloading',
    size: 1,
    done: 0,
    progress: 0,
    uploaded: 0,
    downloaded: 0,
    down_bps: 0,
    up_bps: 0,
    ratio: 0,
    peers_live: 0,
    peers_seen: 0,
    peers_connecting: 0,
    save_path: '/dl',
    content_path: '/dl/Sintel',
    tags: [],
    added_at: 0,
    queue_pos: 0,
    trackers: [],
    has_metadata: true,
    file_count: 1,
    piece_count: 1,
    piece_size: 1,
    last_activity: 0,
    seeding_secs: 0,
    down_limit_kib: 0,
    up_limit_kib: 0,
    pending: false,
    restore_failed: false,
    ...o,
  }) as Torrent;

const calls: string[] = [];
const on: TorrentMenuActions = {
  act: (a, h) => calls.push(`${a}:${h.length}`),
  move: () => calls.push('move'),
  media: async () => [],
  play: () => {},
  rename: () => calls.push('rename'),
  limits: (_, s) => calls.push(`limits:${s}`),
  setCategory: (_, c) => calls.push(`cat:${c}`),
  setTag: (_, g, o) => calls.push(`tag:${g}:${o}`),
  clearTags: () => calls.push('clear'),
  labels: () => calls.push('labels'),
  trackers: () => calls.push('trackers'),
  copy: (s) => calls.push(`copy:${s}`),
  remove: (_, d) => calls.push(`remove:${d}`),
};
const all = { me: 'ada', manageAll: true, manageOwn: true, categories: ['Movies'], tags: ['hd'] };
const labels = (rows: MenuItem[]) => rows.map((r) => ('label' in r ? r.label : r.kind));

describe('torrent context menu', () => {
  it('offers Pause for a running torrent, Resume for a paused one, Retry for a failed one', () => {
    expect(labels(menuFor([tor({})], all, on))).toContain('Pause');
    expect(labels(menuFor([tor({})], all, on))).not.toContain('Resume');
    expect(labels(menuFor([tor({ state: 'paused' })], all, on))).toContain('Resume');
    const failed = labels(menuFor([tor({ state: 'error' })], all, on));
    expect(failed).toContain('Retry');
    expect(failed).toContain('Pause');
  });
  it('has single-torrent entries only for one', () => {
    const one = labels(menuFor([tor({})], all, on));
    const two = labels(menuFor([tor({}), tor({ hash: 'cd'.repeat(20) })], all, on));
    for (const l of ['Rename…', 'Speed limits…', 'Seeding limits…', 'Add trackers…', 'Play']) {
      expect(one).toContain(l);
      expect(two).not.toContain(l);
    }
    expect(two).toContain('Remove with files…');
  });
  it('highlights another folder for disk and permission errors', () => {
    const rows = menuFor([tor({ state: 'error', error: 'No space left on device (os error 28)' })], all, on);
    expect(rows.find((r) => 'label' in r && r.label === 'Choose another folder…')).toMatchObject({ accent: true });
    expect(labels(menuFor([tor({ state: 'error', error: 'tracker said no' })], all, on))).not.toContain('Choose another folder…');
  });
  it('only cancels or copies an add in progress', () => {
    expect(labels(menuFor([tor({ pending: true })], all, on))).toEqual(['Cancel', 'Copy']);
  });
  it('limits people to copying torrents they may not manage', () => {
    const own = { ...all, manageAll: false };
    expect(labels(menuFor([tor({ added_by: 'bob' })], own, on))).toEqual(['Play', 'sep', 'Copy']);
    expect(labels(menuFor([tor({ added_by: 'ada' })], own, on))).toContain('Pause');
    // Only managers of everything create categories and tags.
    const cat = menuFor([tor({ added_by: 'ada' })], own, on).find((r) => 'label' in r && r.label === 'Category');
    expect(cat && cat.kind === 'submenu' && Array.isArray(cat.items) ? labels(cat.items) : []).toEqual(['None', 'Movies']);
  });
  it('toggles a tag on all or none of the selection', () => {
    calls.length = 0;
    const rows = menuFor([tor({ tags: ['hd'] }), tor({ hash: 'cd'.repeat(20) })], all, on);
    const tags = rows.find((r) => 'label' in r && r.label === 'Tags');
    if (!tags || tags.kind !== 'submenu' || !Array.isArray(tags.items)) throw new Error('no tags');
    const hd = tags.items.find((r) => 'label' in r && r.label === 'hd');
    expect(hd).toMatchObject({ checked: false });
    if (hd && (hd.kind === undefined || hd.kind === 'item')) hd.onselect();
    expect(calls).toEqual(['tag:hd:true']);
  });
});

describe('torrent helpers', () => {
  it('builds magnet links like the desktop', () => {
    expect(magnet(tor({ name: 'My File', trackers: ['udp://t.example:80'] }))).toBe(
      `magnet:?xt=urn:btih:${'ab'.repeat(20)}&dn=My%20File&tr=udp%3A%2F%2Ft.example%3A80`,
    );
  });
  it('knows media files and folder problems', () => {
    expect(isMedia('a/Sintel.MKV')).toBe(true);
    expect(isMedia('readme.txt')).toBe(false);
    expect(folderProblem('Access is denied. (os error 5)')).toBe(true);
    expect(folderProblem('being used by another process (os error 32)')).toBe(false);
    expect(folderProblem(undefined)).toBe(false);
  });
});
