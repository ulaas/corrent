// Helpers for library items: artwork choice, card shapes, links and labels.
import { t, tk } from './i18n.svelte';
import type { ImageRef, Item, ItemKind, LibraryKind } from './types';
import type { IconName } from './icons';

export type Shape = 'poster' | 'square' | 'landscape';

export function shapeFor(kind: ItemKind): Shape {
  switch (kind) {
    case 'album':
    case 'artist':
    case 'track':
      return 'square';
    case 'episode':
    case 'video':
    case 'photo':
    case 'folder':
      return 'landscape';
    default:
      return 'poster';
  }
}

/** Aspect ratio (height / width) of a card's artwork. */
export function aspect(shape: Shape): number {
  return shape === 'poster' ? 1.5 : shape === 'square' ? 1 : 9 / 16;
}

export function libraryShape(kind: LibraryKind): Shape {
  return kind === 'music' ? 'square' : kind === 'videos' || kind === 'photos' ? 'landscape' : 'poster';
}

/** The kind of top-level item a library's grid lists. */
export function topKind(kind: LibraryKind): ItemKind | undefined {
  switch (kind) {
    case 'movies':
      return 'movie';
    case 'shows':
      return 'show';
    case 'music':
      return 'album';
    case 'audiobooks':
      return 'book';
    case 'videos':
      return 'video';
    case 'photos':
      return 'photo';
  }
}

export function image(item: Pick<Item, 'images'> | null | undefined, ...kinds: string[]): ImageRef | undefined {
  const imgs = item?.images ?? [];
  for (const k of kinds) {
    const f = imgs.find((i) => i.kind === k);
    if (f) return f;
  }
  return undefined;
}

export function cardImage(item: Item, shape: Shape): ImageRef | undefined {
  if (shape === 'landscape') return image(item, 'thumb', 'backdrop', 'poster') ?? item.images?.[0];
  return image(item, 'poster', 'thumb', 'backdrop') ?? item.images?.[0];
}

export function backdrop(item: Pick<Item, 'images'> | null | undefined): ImageRef | undefined {
  return image(item, 'backdrop', 'thumb');
}

export function isPlayable(kind: ItemKind): boolean {
  return kind === 'movie' || kind === 'episode' || kind === 'track' || kind === 'book' || kind === 'video';
}

export function isAudio(kind: ItemKind): boolean {
  return kind === 'track' || kind === 'book';
}

export function itemHref(item: Pick<Item, 'id' | 'kind'>): string {
  return `/item/${item.id}`;
}

export function playHref(id: number, resume = true): string {
  return `/play/${id}${resume ? '' : '?from=0'}`;
}

/** "S1 · E3" for episodes, "Track 2" etc. */
export function subtitle(item: Item): string {
  const ctx = item.context ?? [];
  switch (item.kind) {
    case 'episode':
      return [ctx[0], item.index != null ? t('Episode {n}', { n: item.index }) : ''].filter(Boolean).join(' · ');
    case 'season':
      return ctx[0] ?? '';
    case 'track':
    case 'album':
      return ctx.join(' · ') || (item.year ? String(item.year) : '');
    default:
      return item.year ? String(item.year) : '';
  }
}

/** Fraction watched (for the progress line on cards). */
export function watched(item: Item): number {
  if (!item.runtime_ms || !item.state.position_ms) return 0;
  return Math.min(1, item.state.position_ms / item.runtime_ms);
}

export const LIBRARY_ICON: Record<LibraryKind, IconName> = {
  movies: 'film-strip',
  shows: 'television-simple',
  music: 'music-notes',
  audiobooks: 'book-open',
  videos: 'video-camera',
  photos: 'image',
};

export const LIBRARY_KIND_LABEL: Record<LibraryKind, string> = {
  movies: tk('Movies'),
  shows: tk('TV shows'),
  music: tk('Music'),
  audiobooks: tk('Audiobooks'),
  videos: tk('Videos'),
  photos: tk('Photos'),
};

export const KIND_LABEL: Record<ItemKind, string> = {
  movie: tk('Movie'),
  show: tk('Show'),
  season: tk('Season'),
  episode: tk('Episode'),
  artist: tk('Artist'),
  album: tk('Album'),
  track: tk('Track'),
  book: tk('Book'),
  video: tk('Video'),
  photo: tk('Photo'),
  folder: tk('Folder'),
};

/** Translated home row titles by id (the server's title is the fallback). */
export function homeRowTitle(id: string, fallback: string, libraryName?: string): string {
  if (id === 'continue') return t('Continue watching');
  if (id === 'next_up') return t('Next up');
  if (id === 'favorites') return t('Favorites');
  if (id.startsWith('recent:') && libraryName) return t('Recently added in {library}', { library: libraryName });
  return fallback;
}

export type SkipKind = 'intro' | 'recap' | 'credits';

/** What a chapter is, by its title (same rules as the server's `chapter_kind`). */
export function chapterKind(title: string | null | undefined): SkipKind | null {
  if (!title) return null;
  const t = title.trim().toLowerCase();
  const words = t.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  const has = (w: string) => words.includes(w);
  if (has('recap') || t.startsWith('previously')) return 'recap';
  if (has('intro') || has('opening') || has('op') || t === 'title sequence' || t === 'theme song') return 'intro';
  if (has('credits') || has('outro') || has('ending') || has('ed') || (has('end') && has('titles'))) return 'credits';
  return null;
}

/** The skippable chapter at `pos` seconds and where it ends. */
export function skipAt(chapters: { start_ms: number; title?: string | null }[], pos: number, duration: number): { kind: SkipKind; end: number } | null {
  let i = -1;
  for (let k = 0; k < chapters.length; k++) if (chapters[k].start_ms / 1000 <= pos) i = k;
  if (i < 0) return null;
  const kind = chapterKind(chapters[i].title);
  if (!kind) return null;
  const end = i + 1 < chapters.length ? chapters[i + 1].start_ms / 1000 : duration;
  return end - pos > 2 ? { kind, end } : null;
}
