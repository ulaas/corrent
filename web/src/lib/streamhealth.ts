// Streaming performance (Settings → Streaming performance): whether a conversion keeps up with
// its player, the bandwidth between two readings of `GET /playback/stats`, and the short
// histories behind the page's sparklines. Pure: no translations, no state.

/** What the verdict reads of a `PlaybackSession`. */
export interface HealthInput {
  /** Segments made so far. */
  produced: number;
  /** Seconds since the player last fetched anything. */
  idle_secs: number;
  /** The encoder is running (not resting ahead of the player). */
  encoding: boolean;
  /** Seconds of media made per second, lately (0 while the encoder rests). */
  speed: number;
  /** Seconds of media ready beyond the player's position. */
  ahead_secs: number;
}

export type Health = 'smooth' | 'barely' | 'slow' | 'starting' | 'waiting';

/** Media ready beyond the player from which a slow encoder doesn't matter, in seconds. */
export const AHEAD_OK = 8;
/** An encoder at least this fast against real time builds up a buffer. */
export const SPEED_OK = 1.1;
/** Below this the player catches up with the encoder sooner or later. */
export const SPEED_MIN = 0.95;
/** A resting encoder whose player fetched nothing for longer than this waits for it, in seconds. */
export const IDLE_WAIT = 10;

export function health(s: HealthInput): Health {
  if (!s.produced) return 'starting';
  // A paused or gone player: whatever is buffered, nobody is watching it being made.
  if (!s.encoding && s.idle_secs > IDLE_WAIT) return 'waiting';
  if (s.ahead_secs >= AHEAD_OK) return 'smooth';
  if (s.encoding) {
    if (s.speed >= SPEED_OK) return 'smooth';
    if (s.speed >= SPEED_MIN) return 'barely';
    return s.speed > 0 ? 'slow' : 'starting';
  }
  // Resting with little ahead: the end of the file, all of it made.
  return 'smooth';
}

/** One reading of a session's `sent_bytes`. */
export interface Reading {
  bytes: number;
  /** When it was read, in milliseconds. */
  at: number;
}

/** Bits per second between two readings; 0 without an earlier one, or when the counter went
 *  back (a new session under the same id) or no time passed. */
export function bandwidth(prev: Reading | undefined, now: Reading): number {
  if (!prev) return 0;
  const secs = (now.at - prev.at) / 1000;
  if (secs <= 0 || now.bytes < prev.bytes) return 0;
  return ((now.bytes - prev.bytes) * 8) / secs;
}

/** Samples kept for the sparklines (two minutes at a reading every two seconds). */
export const HISTORY = 60;

/** `list` with `v` at its end, at most `keep` long. */
export function pushSample(list: number[], v: number, keep = HISTORY): number[] {
  const out = list.length >= keep ? list.slice(list.length - keep + 1) : list.slice();
  out.push(v);
  return out;
}

/** SVG polyline points of `values` in a `w`×`h` box, the newest at the right edge (0 at the
 *  bottom, `max` at the top); a full history spans the width. */
export function sparkPoints(values: number[], max: number, w: number, h: number, keep = HISTORY): string {
  if (values.length < 2) return '';
  const step = w / (keep - 1);
  const x0 = w - (values.length - 1) * step;
  return values
    .map((v, i) => `${(x0 + i * step).toFixed(1)},${(h - (max > 0 ? Math.min(1, Math.max(0, v / max)) * h : 0)).toFixed(1)}`)
    .join(' ');
}
