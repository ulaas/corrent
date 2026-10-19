// Playback: where media comes from (direct play today; an HLS session from MS9 plugs in through
// `resolveSource`/`attachSource`), progress reporting, and the persistent music player.
import { api, ApiError, imageUrl, itemFileUrl } from './api';
import { image } from './media';
import { prefs } from './prefs.svelte';
import type { Item, ItemDetail, Page } from './types';

// ---- sources ----

export interface SubtitleTrack {
  index: number;
  lang?: string;
  title?: string;
  forced: boolean;
  external: boolean;
  /** WebVTT URL (text subtitles); others must be burned in. */
  url?: string;
}

/** `POST /api/v1/playback/{item}`: how to play an item (corrent_proto::library::PlaybackInfo). */
export interface PlaybackInfo {
  method: 'direct' | 'hls';
  url: string;
  session_id?: string;
  file_id: number;
  duration_ms?: number;
  subtitles: SubtitleTrack[];
  trickplay?: string;
  reasons: string[];
}

export interface PlaybackOptions {
  file_id?: number;
  audio_track?: number;
  burn_subtitle?: number;
  max_bitrate?: number;
  max_height?: number;
  force_transcode?: boolean;
}

export function browserProfile(): 'web' | 'safari' {
  const ua = navigator.userAgent;
  return /Safari\//.test(ua) && !/Chrome\/|Chromium\/|Edg\//.test(ua) ? 'safari' : 'web';
}

/** Asks the server how to play an item (direct file or an HLS transcode session). */
export async function resolveSource(itemId: number, o: PlaybackOptions = {}): Promise<PlaybackInfo> {
  try {
    return await api.post<PlaybackInfo>(`/playback/${itemId}`, { profile: browserProfile(), ...o });
  } catch (e) {
    // Older servers without the playback API: direct play.
    if (e instanceof ApiError && e.status === 404 && e.code === 'not_found' && /endpoint/.test(e.detail))
      return { method: 'direct', url: itemFileUrl(itemId), file_id: 0, subtitles: [], reasons: [] };
    throw e;
  }
}

/** Ends a server transcode session (fire and forget). */
export function stopSession(info: PlaybackInfo | null | undefined) {
  if (info?.session_id) void api.del(`/playback/sessions/${info.session_id}`).catch(() => {});
}

type HlsLike = { destroy(): void; loadSource(u: string): void; attachMedia(el: HTMLMediaElement): void; on(ev: string, f: (e: string, d: { fatal?: boolean; type?: string }) => void): void; recoverMediaError(): void; startLoad(): void };

/** Attaches a source to a media element; the returned function detaches it. `onfatal` is
 *  called when an HLS stream can't continue. */
export async function attachSource(
  el: HTMLMediaElement,
  src: Pick<PlaybackInfo, 'method' | 'url'>,
  onfatal?: (msg: string) => void,
): Promise<() => void> {
  if (src.method === 'hls' && !el.canPlayType('application/vnd.apple.mpegurl')) {
    const { default: Hls } = await import('hls.js/dist/hls.light.mjs');
    if (!Hls.isSupported()) throw new Error('HLS is not supported by this browser');
    const hls = new Hls({ maxBufferLength: 30, backBufferLength: 60, xhrSetup: (x: XMLHttpRequest) => {
        x.withCredentials = true;
      } }) as unknown as HlsLike;
    let recovered = false;
    hls.on(Hls.Events.ERROR, (_e, d) => {
      if (!d.fatal) return;
      if (d.type === Hls.ErrorTypes.MEDIA_ERROR && !recovered) {
        recovered = true;
        hls.recoverMediaError();
      } else if (d.type === Hls.ErrorTypes.NETWORK_ERROR && !recovered) {
        recovered = true;
        hls.startLoad();
      } else onfatal?.(String(d.type));
    });
    hls.loadSource(src.url);
    hls.attachMedia(el);
    return () => {
      hls.destroy();
      el.removeAttribute('src');
      el.load();
    };
  }
  el.src = src.url;
  return () => {
    el.removeAttribute('src');
    el.load();
  };
}

// ---- progress ----

/** Reports the position every 10 s while playing, and on pause and end. */
export function reportProgress(el: HTMLMediaElement, id: () => number | null): () => void {
  let last = -1;
  const send = (body: { position_ms: number; played?: boolean }) => {
    const i = id();
    if (i == null) return;
    void api.put(`/items/${i}/state`, body).catch(() => {});
  };
  const pos = () => Math.round(el.currentTime * 1000);
  const timer = setInterval(() => {
    if (!el.paused && !el.ended && Math.abs(pos() - last) > 2000) {
      last = pos();
      send({ position_ms: last });
    }
  }, 10_000);
  const onPause = () => {
    if (el.ended || el.currentTime < 1) return;
    last = pos();
    send({ position_ms: last });
  };
  const onEnded = () => send({ position_ms: 0, played: true });
  el.addEventListener('pause', onPause);
  el.addEventListener('ended', onEnded);
  return () => {
    clearInterval(timer);
    el.removeEventListener('pause', onPause);
    el.removeEventListener('ended', onEnded);
    if (!el.ended && el.currentTime > 1) send({ position_ms: pos() });
  };
}

/** Where to start: the saved position unless it's played or almost at the end. */
export function resumeAt(item: Item, durationMs?: number): number {
  const p = item.state.position_ms || 0;
  const d = durationMs || item.runtime_ms || 0;
  if (item.state.played || p < 5000 || (d && p > d - 10_000)) return 0;
  return p;
}

// ---- music ----

export interface QueueEntry {
  id: number;
  /** A live stream (internet radio) instead of a library item. */
  stream?: string;
  title: string;
  artist: string;
  album: string;
  art?: string;
  runtime_ms?: number;
}

export function toEntry(t: Item, album?: Item | ItemDetail | null): QueueEntry {
  const art = image(t, 'poster', 'thumb') ?? image(album ?? null, 'poster', 'thumb');
  return {
    id: t.id,
    title: t.title,
    artist: t.context?.[0] ?? album?.context?.[0] ?? '',
    album: t.context?.[1] ?? album?.title ?? '',
    art: art?.sha,
    runtime_ms: t.runtime_ms,
  };
}

/** All playable children of a container, in order (album → tracks, artist → albums → tracks). */
export async function tracksOf(parentId: number): Promise<Item[]> {
  const out: Item[] = [];
  const page = await api.get<Page<Item>>('/items', { parent_id: parentId, sort: 'index', limit: 500 });
  for (const it of page.items) {
    if (it.kind === 'track' || it.kind === 'book' || it.kind === 'episode') out.push(it);
    else if (it.kind === 'album' || it.kind === 'season') out.push(...(await tracksOf(it.id)));
  }
  return out;
}

class Music {
  queue = $state<QueueEntry[]>([]);
  /** Order of play (indices into queue); shuffled when shuffle is on. */
  order = $state<number[]>([]);
  pos = $state(0);
  playing = $state(false);
  loading = $state(false);
  time = $state(0);
  duration = $state(0);
  shuffle = $state(false);
  repeat = $state<'off' | 'all' | 'one'>('off');
  expanded = $state(false);
  error = $state('');
  private el: HTMLAudioElement | null = null;
  private info: PlaybackInfo | null = null;
  private detach: (() => void) | null = null;
  private stopReport: (() => void) | null = null;

  get current(): QueueEntry | null {
    const i = this.order[this.pos];
    return i == null ? null : (this.queue[i] ?? null);
  }

  get active(): boolean {
    return this.queue.length > 0;
  }

  private audio(): HTMLAudioElement {
    if (this.el) return this.el;
    const a = new Audio();
    a.preload = 'auto';
    a.volume = prefs.volume;
    a.addEventListener('play', () => (this.playing = true));
    a.addEventListener('pause', () => (this.playing = false));
    a.addEventListener('timeupdate', () => (this.time = a.currentTime));
    a.addEventListener('durationchange', () => (this.duration = isFinite(a.duration) ? a.duration : 0));
    a.addEventListener('waiting', () => (this.loading = true));
    a.addEventListener('playing', () => (this.loading = false));
    a.addEventListener('canplay', () => (this.loading = false));
    a.addEventListener('error', () => {
      this.loading = false;
      this.error = 'error';
    });
    a.addEventListener('ended', () => {
      if (this.repeat === 'one') {
        a.currentTime = 0;
        void a.play();
      } else this.next(true);
    });
    this.stopReport = reportProgress(a, () => (this.current?.stream ? null : (this.current?.id ?? null)));
    this.el = a;
    return a;
  }

  /** Plays `entries` starting at `start`. */
  play(entries: QueueEntry[], start = 0) {
    if (!entries.length) return;
    this.queue = entries;
    this.makeOrder(start);
    void this.load(true);
  }

  enqueue(entries: QueueEntry[]) {
    if (!this.queue.length) return this.play(entries);
    const base = this.queue.length;
    this.queue = [...this.queue, ...entries];
    this.order = [...this.order, ...entries.map((_, i) => base + i)];
  }

  private makeOrder(start: number) {
    const idx = this.queue.map((_, i) => i);
    if (this.shuffle) {
      const rest = idx.filter((i) => i !== start);
      for (let i = rest.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [rest[i], rest[j]] = [rest[j], rest[i]];
      }
      this.order = [start, ...rest];
      this.pos = 0;
    } else {
      this.order = idx;
      this.pos = start;
    }
  }

  toggleShuffle() {
    const cur = this.order[this.pos] ?? 0;
    this.shuffle = !this.shuffle;
    this.makeOrder(cur);
  }

  cycleRepeat() {
    this.repeat = this.repeat === 'off' ? 'all' : this.repeat === 'all' ? 'one' : 'off';
  }

  private async load(autoplay: boolean) {
    const cur = this.current;
    if (!cur) return;
    const a = this.audio();
    this.error = '';
    this.detach?.();
    this.time = 0;
    this.duration = (cur.runtime_ms ?? 0) / 1000;
    this.loading = true;
    try {
      stopSession(this.info);
      if (cur.stream) {
        this.info = null;
        a.src = cur.stream;
        this.detach = () => a.removeAttribute('src');
      } else {
        this.info = await resolveSource(cur.id);
        this.detach = await attachSource(a, this.info);
      }
      if (autoplay) await a.play();
    } catch (e) {
      if (!(e instanceof DOMException && e.name === 'AbortError')) this.error = String(e);
      this.loading = false;
    }
    this.mediaSession();
  }

  jump(orderPos: number) {
    if (orderPos < 0 || orderPos >= this.order.length) return;
    this.pos = orderPos;
    void this.load(true);
  }

  next(auto = false) {
    if (this.pos + 1 < this.order.length) this.jump(this.pos + 1);
    else if (this.repeat === 'all' || !auto) {
      if (this.repeat === 'all') this.jump(0);
    } else {
      this.playing = false;
    }
  }

  prev() {
    const a = this.el;
    if (a && a.currentTime > 3) a.currentTime = 0;
    else if (this.pos > 0) this.jump(this.pos - 1);
    else if (a) a.currentTime = 0;
  }

  toggle() {
    const a = this.audio();
    if (a.paused) void a.play().catch(() => {});
    else a.pause();
  }

  pause() {
    this.el?.pause();
  }

  seek(secs: number) {
    if (this.el) this.el.currentTime = Math.max(0, Math.min(secs, this.duration || secs));
  }

  setVolume(v: number) {
    prefs.volume = v;
    if (this.el) this.el.volume = v;
  }

  removeAt(orderPos: number) {
    if (orderPos === this.pos) return;
    const qi = this.order[orderPos];
    const order = this.order.filter((_, i) => i !== orderPos).map((i) => (i > qi ? i - 1 : i));
    this.queue = this.queue.filter((_, i) => i !== qi);
    if (orderPos < this.pos) this.pos--;
    this.order = order;
  }

  close() {
    this.el?.pause();
    this.detach?.();
    this.detach = null;
    stopSession(this.info);
    this.info = null;
    this.queue = [];
    this.order = [];
    this.pos = 0;
    this.playing = false;
    this.expanded = false;
    if ('mediaSession' in navigator) navigator.mediaSession.metadata = null;
  }

  private mediaSession() {
    if (!('mediaSession' in navigator)) return;
    const c = this.current;
    if (!c) return;
    const ms = navigator.mediaSession;
    ms.metadata = new MediaMetadata({
      title: c.title,
      artist: c.artist,
      album: c.album,
      artwork: c.art
        ? [
            { src: imageUrl(c.art, 200), sizes: '200x200' },
            { src: imageUrl(c.art, 400), sizes: '400x400' },
          ]
        : [],
    });
    ms.setActionHandler('play', () => this.toggle());
    ms.setActionHandler('pause', () => this.pause());
    ms.setActionHandler('previoustrack', () => this.prev());
    ms.setActionHandler('nexttrack', () => this.next());
    ms.setActionHandler('seekto', (d) => d.seekTime != null && this.seek(d.seekTime));
  }

  /** Stops reporting (sign-out). */
  dispose() {
    this.close();
    this.stopReport?.();
  }
}

export const music = new Music();
