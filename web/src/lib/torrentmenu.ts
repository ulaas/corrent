// The torrent list's context menu, as on the desktop (src/app/actions.rs), built from the
// selection: pure, so it can be tested.
import type { MenuItem } from './contextmenu.svelte';
import type { FileEntry, Torrent } from './types';
import { t } from './i18n.svelte';

/** What the menu's rows do (the page carries them out). */
export interface TorrentMenuActions {
  act: (action: string, hashes: string[]) => void;
  move: (hashes: string[]) => void;
  /** Media files of a torrent, largest first (Play ▸). */
  media: (hash: string) => Promise<FileEntry[]>;
  play: (hash: string, file: FileEntry) => void;
  rename: (t: Torrent) => void;
  addToLibrary: (t: Torrent) => void;
  limits: (t: Torrent, section: 'speed' | 'seed') => void;
  setCategory: (sel: Torrent[], category: string | null) => void;
  setTag: (sel: Torrent[], tag: string, on: boolean) => void;
  clearTags: (sel: Torrent[]) => void;
  /** "New category…" / "New tag…" (the labels dialog). */
  labels: () => void;
  trackers: (t: Torrent) => void;
  copy: (text: string) => void;
  remove: (hashes: string[], withData: boolean) => void;
}

export interface TorrentMenuContext {
  /** The signed-in account (owners may manage their own torrents). */
  me: string | null;
  manageAll: boolean;
  manageOwn: boolean;
  /** May import into libraries (LIBRARY_MANAGE). */
  library: boolean;
  categories: string[];
  tags: string[];
}

/** An error the fix for which is usually another folder (desktop: `classify_io`). */
export function folderProblem(error: string | undefined): boolean {
  if (!error) return false;
  const m = error.toLowerCase();
  const full = ['os error 28', 'os error 112', 'os error 39', 'no space left', 'not enough space', 'disk is full'];
  const locked = ['os error 32', 'os error 33', 'being used by another process'];
  const denied = ['os error 5)', 'os error 13', 'access is denied', 'permission denied', 'read-only file system', 'os error 30'];
  const missing = ['os error 2)', 'os error 3)', 'no such file', 'cannot find the path'];
  if (full.some((s) => m.includes(s))) return true;
  if (locked.some((s) => m.includes(s))) return false;
  return denied.some((s) => m.includes(s)) || missing.some((s) => m.includes(s));
}

const MEDIA = new Set(['mp4', 'm4v', 'mkv', 'webm', 'avi', 'mov', 'ts', 'mp3', 'flac', 'ogg', 'opus', 'm4a', 'aac', 'wav']);

/** A video or audio file by its name (the engine's stream server serves these). */
export function isMedia(name: string): boolean {
  const i = name.lastIndexOf('.');
  return i > 0 && MEDIA.has(name.slice(i + 1).toLowerCase());
}

function enc(s: string): string {
  return encodeURIComponent(s).replace(/[!'()*]/g, (c) => '%' + c.charCodeAt(0).toString(16).toUpperCase());
}

export function magnet(x: Torrent): string {
  return `magnet:?xt=urn:btih:${x.hash}&dn=${enc(x.name)}` + x.trackers.map((tr) => `&tr=${enc(tr)}`).join('');
}

/** Not transferring by choice or because it is done (desktop: `is_paused_like`). */
const pausedLike = (x: Torrent) => x.state === 'paused' || x.state === 'queued' || x.state === 'completed';

export function menuFor(sel: Torrent[], ctx: TorrentMenuContext, on: TorrentMenuActions): MenuItem[] {
  if (!sel.length) return [];
  const hashes = sel.map((x) => x.hash);
  const one = sel.length === 1 ? sel[0] : null;
  const manage = ctx.manageAll || (ctx.manageOwn && !!ctx.me && sel.every((x) => x.added_by === ctx.me));
  const copy: MenuItem = {
    kind: 'submenu',
    label: t('Copy'),
    icon: 'copy',
    items: [
      { label: t('Name'), onselect: () => on.copy(sel.map((x) => x.name).join('\n')) },
      { label: t('Info hash'), onselect: () => on.copy(hashes.join('\n')) },
      { label: t('Magnet link'), onselect: () => on.copy(sel.map(magnet).join('\n')) },
      { label: t('Save path'), onselect: () => on.copy(sel.map((x) => x.content_path).join('\n')) },
    ],
  };
  // Still being added: it can only be stopped.
  if (sel.every((x) => x.pending)) {
    return [...(manage ? [{ label: t('Cancel'), icon: 'x', onselect: () => on.act('cancel', hashes) } as MenuItem] : []), copy];
  }
  const rows: MenuItem[] = [];
  const play: MenuItem[] = [];
  if (one?.has_metadata && one.file_count > 0) {
    play.push({
      kind: 'submenu',
      label: t('Play'),
      icon: 'play-circle',
      items: async () => {
        const files = await on.media(one.hash);
        if (!files.length) return [];
        return [
          { kind: 'header', label: t('Streams while downloading') },
          ...files.slice(0, 12).map((f): MenuItem => ({ label: f.path[f.path.length - 1] ?? '', icon: 'play', onselect: () => on.play(one.hash, f) })),
        ];
      },
    });
  }
  if (!manage) return [...play, ...(play.length ? [{ kind: 'sep' } as MenuItem] : []), copy];

  const retry = sel.every((x) => x.restore_failed || x.state === 'error');
  if (sel.some((x) => pausedLike(x) || x.state === 'error')) {
    rows.push({ label: retry ? t('Retry') : t('Resume'), icon: retry ? 'arrow-clockwise' : 'play', onselect: () => on.act('resume', hashes) });
  }
  if (sel.some((x) => !pausedLike(x))) rows.push({ label: t('Pause'), icon: 'pause', onselect: () => on.act('pause', hashes) });
  rows.push({ label: t('Force start'), icon: 'lightning', onselect: () => on.act('force_start', hashes) });
  if (one && !one.restore_failed && folderProblem(one.error)) {
    rows.push({ kind: 'sep' }, { label: t('Choose another folder…'), icon: 'folder-simple-dashed', accent: true, onselect: () => on.move(hashes) });
  }
  rows.push({ kind: 'sep' }, ...play);
  rows.push({ label: t('Move to…'), icon: 'folder-simple-dashed', onselect: () => on.move(hashes) });
  if (one && ctx.library && one.has_metadata && !one.restore_failed) {
    rows.push({ label: t('Add to library…'), icon: 'film-slate', onselect: () => on.addToLibrary(one) });
  }
  if (one) rows.push({ label: t('Rename…'), icon: 'pencil-simple', shortcut: 'F2', onselect: () => on.rename(one) });
  rows.push({
    kind: 'submenu',
    label: t('Category'),
    icon: 'folder-simple',
    items: [
      ...(ctx.manageAll ? [{ label: t('New category…'), onselect: on.labels } as MenuItem, { kind: 'sep' } as MenuItem] : []),
      { label: t('None'), radio: true, checked: sel.every((x) => !x.category), onselect: () => on.setCategory(sel, null) },
      ...ctx.categories.map((c): MenuItem => ({ label: c, radio: true, checked: sel.every((x) => x.category === c), onselect: () => on.setCategory(sel, c) })),
    ],
  });
  rows.push({
    kind: 'submenu',
    label: t('Tags'),
    icon: 'tag',
    items: [
      ...(ctx.manageAll ? [{ label: t('New tag…'), onselect: on.labels } as MenuItem, { kind: 'sep' } as MenuItem] : []),
      ...ctx.tags.map((g): MenuItem => {
        const all = sel.every((x) => x.tags.includes(g));
        return { label: g, checked: all, onselect: () => on.setTag(sel, g, !all) };
      }),
      ...(sel.some((x) => x.tags.length) ? [{ kind: 'sep' } as MenuItem, { label: t('Remove all tags'), onselect: () => on.clearTags(sel) } as MenuItem] : []),
    ],
  });
  rows.push({
    kind: 'submenu',
    label: t('Queue'),
    icon: 'list-numbers',
    items: [
      { label: t('Move to top'), icon: 'arrow-line-up', onselect: () => on.act('queue_top', hashes) },
      { label: t('Move up'), icon: 'arrow-up', onselect: () => on.act('queue_up', hashes) },
      { label: t('Move down'), icon: 'arrow-down', onselect: () => on.act('queue_down', hashes) },
      { label: t('Move to bottom'), icon: 'arrow-line-down', onselect: () => on.act('queue_bottom', hashes) },
    ],
  });
  if (one) {
    rows.push({ label: t('Speed limits…'), icon: 'gauge', onselect: () => on.limits(one, 'speed') });
    rows.push({ label: t('Seeding limits…'), icon: 'scales', onselect: () => on.limits(one, 'seed') });
  }
  rows.push({ kind: 'sep' }, { label: t('Force recheck'), icon: 'arrows-clockwise', onselect: () => on.act('recheck', hashes) });
  if (one) rows.push({ label: t('Add trackers…'), icon: 'broadcast', onselect: () => on.trackers(one) });
  rows.push(copy, { kind: 'sep' });
  rows.push({ label: t('Remove…'), icon: 'trash', onselect: () => on.remove(hashes, false) });
  rows.push({ label: t('Remove with files…'), icon: 'trash-simple', danger: true, onselect: () => on.remove(hashes, true) });
  return rows;
}
