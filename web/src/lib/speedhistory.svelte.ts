// Speed history for the torrent details' Speed tab (as the desktop's): sampled once a second
// while the torrent list is in use — the session's, and that of each torrent that moved data.
import { live } from './torrents.svelte';

/** Samples kept (about five minutes). */
export const KEEP = 300;

export type Sample = [down: number, up: number];

export const speeds = $state<{ all: Sample[]; per: Record<string, Sample[]> }>({ all: [], per: {} });

function push(list: Sample[], s: Sample): Sample[] {
  const out = list.length >= KEEP ? list.slice(list.length - KEEP + 1) : list.slice();
  out.push(s);
  return out;
}

/** One sample now (exported for tests). */
export function sample() {
  if (live.stats) speeds.all = push(speeds.all, [live.stats.down_bps, live.stats.up_bps]);
  const per: Record<string, Sample[]> = {};
  for (const x of live.list) {
    const had = speeds.per[x.hash];
    // Torrents that never moved data have no history (thousands of idle ones cost nothing).
    if (!had && !x.down_bps && !x.up_bps) continue;
    per[x.hash] = push(had ?? [], [x.down_bps, x.up_bps]);
  }
  speeds.per = per;
}

let users = 0;
let timer: ReturnType<typeof setInterval> | null = null;

/** Samples while someone uses it (call the returned function when done). */
export function useSpeedHistory(): () => void {
  if (users++ === 0) timer = setInterval(sample, 1000);
  return () => {
    if (--users === 0 && timer) {
      clearInterval(timer);
      timer = null;
    }
  };
}

/** SVG polyline points of `values` in a `w`×`h` box (0 at the bottom, `max` at the top). */
export function points(values: number[], max: number, w: number, h: number): string {
  if (values.length < 2) return '';
  const step = w / (KEEP - 1);
  const x0 = w - (values.length - 1) * step;
  return values.map((v, i) => `${(x0 + i * step).toFixed(1)},${(h - (max > 0 ? (v / max) * h : 0)).toFixed(1)}`).join(' ');
}
