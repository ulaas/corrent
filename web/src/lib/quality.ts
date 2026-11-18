// Playback quality choices, the same as the server's and the desktop app's (`QUALITIES` in
// crates/corrent-proto/src/library.rs): a file within a quality's limits plays as it is, a larger
// one is converted down to them by the server.
import { t } from './i18n.svelte';

export interface Quality {
  /** '' is the original: no limits. */
  id: string;
  /** Picture height in lines. */
  height: number;
  /** Total bitrate in bits/s. */
  bitrate: number;
}

export const QUALITIES: Quality[] = [
  { id: '2160', height: 2160, bitrate: 20_000_000 },
  { id: '1080', height: 1080, bitrate: 8_000_000 },
  { id: '720', height: 720, bitrate: 4_000_000 },
  { id: '480', height: 480, bitrate: 1_500_000 },
];

/** The ids on offer, the original first. */
export const QUALITY_IDS: string[] = ['', ...QUALITIES.map((q) => q.id)];

/** A stored choice, or the original when it isn't one of ours. */
export function normalizeQuality(id: unknown): string {
  return typeof id === 'string' && QUALITIES.some((q) => q.id === id) ? id : '';
}

/** What `POST /playback/{item}` is asked for; nothing for the original (and anything unknown). */
export function qualityLimits(id: string): { max_height: number; max_bitrate: number } | {} {
  const q = QUALITIES.find((x) => x.id === id);
  return q ? { max_height: q.height, max_bitrate: q.bitrate } : {};
}

export function qualityLabel(id: string): string {
  return id === '' ? t('Original') : id === '2160' ? '4K' : `${id}p`;
}
