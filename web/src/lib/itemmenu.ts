// The context menu of a media card or row: the desktop item page's buttons
// (src/app/media/mod.rs), reachable from grids and lists.
import { api, errorText } from './api';
import { identify } from './identify.svelte';
import { go } from './router.svelte';
import { isPlayable, itemHref, playHref } from './media';
import { outcomeText } from './metadata';
import { toggleFavorite, togglePlayed } from './itemstate';
import { toast } from './toast.svelte';
import { t } from './i18n.svelte';
import type { MenuItem } from './contextmenu.svelte';
import type { Item, ItemDetail, Playlist, RefreshMode, RefreshOutcome } from './types';

export interface ItemMenuOptions {
  /** May manage the library (Identify…, Refresh metadata, Find subtitles). */
  manage: boolean;
  /** Plays it in place (albums, shows, tracks in a list) instead of opening the player. */
  onplay?: (item: Item) => void;
  /** Extra rows at the end (a playlist's Move up/down, Remove). */
  extra?: MenuItem[];
}

async function addToPlaylist(p: Playlist, id: number) {
  try {
    const r = await api.post<{ added: number }>(`/playlists/${p.id}/items`, { items: [id] });
    toast(r.added === 1 ? t('Added 1 item to “{name}”', { name: p.name }) : t('Added {n} items to “{name}”', { n: r.added, name: p.name }), 'success');
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

async function refresh(id: number, mode: RefreshMode) {
  try {
    const [kind, text] = outcomeText(await api.post<RefreshOutcome>(`/items/${id}/refresh-metadata`, { mode }));
    toast(text, kind);
  } catch (e) {
    toast(errorText(e), 'error');
  }
}

/** IMDb and TMDB pages (cards don't carry provider ids: asked for when the submenu opens). */
async function links(item: Item): Promise<MenuItem[]> {
  const d = await api.get<ItemDetail>(`/items/${item.id}`);
  const ids = d.provider_ids ?? {};
  const out: MenuItem[] = [];
  const page = (url: string) => () => void open(url, '_blank', 'noopener');
  if (ids.imdb) out.push({ label: 'IMDb', icon: 'arrow-square-out', onselect: page(`https://www.imdb.com/title/${encodeURIComponent(ids.imdb)}/`) });
  if (ids.tmdb) out.push({ label: 'TMDB', icon: 'arrow-square-out', onselect: page(`https://www.themoviedb.org/${item.kind === 'show' ? 'tv' : 'movie'}/${encodeURIComponent(ids.tmdb)}`) });
  return out;
}

export function itemMenu(item: Item, o: ItemMenuOptions): MenuItem[] {
  const k = item.kind;
  const rows: MenuItem[] = [];
  const playable = isPlayable(k) && k !== 'track';
  if (o.onplay) {
    rows.push({ label: t('Play'), icon: 'play', onselect: () => o.onplay!(item) });
  } else if (playable) {
    rows.push({ label: t('Play'), icon: 'play', onselect: () => go(playHref(item.id)) });
    if (item.state.position_ms > 0 && !item.state.played) {
      rows.push({ label: t('From the start'), icon: 'arrow-counter-clockwise', onselect: () => go(playHref(item.id, false)) });
    }
  }
  if (isPlayable(k) || k === 'album' || k === 'season') {
    rows.push({
      kind: 'submenu',
      label: t('Add to playlist'),
      icon: 'playlist',
      items: async () =>
        (await api.get<Playlist[]>('/playlists'))
          .filter((p) => p.mine)
          .map((p): MenuItem => ({ label: p.name, onselect: () => void addToPlaylist(p, item.id) })),
    });
  }
  rows.push({ kind: 'sep' });
  rows.push({
    label: item.state.played ? t('Mark as unplayed') : t('Mark as played'),
    icon: item.state.played ? 'circle' : 'check-circle',
    onselect: () => void togglePlayed(item),
  });
  rows.push({
    label: item.state.favorite ? t('Remove from favorites') : t('Add to favorites'),
    icon: item.state.favorite ? 'heart-fill' : 'heart',
    onselect: () => void toggleFavorite(item),
  });
  if (o.manage) {
    rows.push({ kind: 'sep' });
    if (k === 'movie' || k === 'episode' || k === 'video') {
      rows.push({ label: t('Find subtitles'), icon: 'closed-captioning', onselect: () => go(`${itemHref(item)}?subtitles=1`) });
    }
    if (k === 'movie' || k === 'show' || k === 'video') {
      rows.push({ label: t('Identify…'), icon: 'magnifying-glass', onselect: () => identify({ id: item.id, title: item.title, year: item.year }) });
    }
    if (k === 'movie' || k === 'show' || k === 'video' || k === 'album' || k === 'artist') {
      rows.push({
        kind: 'submenu',
        label: t('Refresh metadata'),
        icon: 'arrows-clockwise',
        items: [
          { label: t('Fill in missing details'), onselect: () => void refresh(item.id, 'missing') },
          { label: t('Replace all details'), onselect: () => void refresh(item.id, 'all') },
        ],
      });
    }
  }
  if (k === 'movie' || k === 'show') {
    rows.push({ kind: 'submenu', label: 'IMDb / TMDB', icon: 'arrow-square-out', items: () => links(item) });
  }
  rows.push({ kind: 'sep' });
  rows.push({
    label: t('Copy link'),
    icon: 'link',
    onselect: () => void navigator.clipboard?.writeText(location.origin + itemHref(item)).then(() => toast(t('Copied'), 'success')),
  });
  if (o.extra?.length) rows.push({ kind: 'sep' }, ...o.extra);
  return rows;
}
