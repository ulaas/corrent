// Input logic of the video player that needs no element: telling single from double taps, and
// moving a D-pad's focus over the controls.

/** Two taps on the same half within this time are a double tap (and keep counting: tap-tap-tap
 *  skips twice). */
export const DOUBLE_TAP_MS = 300;

export class TapTracker {
  private at = -Infinity;
  private side = 0;

  /** A tap at `now` (ms) on a half of the picture (-1 left, 1 right). */
  tap(now: number, side: -1 | 1): 'single' | 'double' {
    const double = side === this.side && now - this.at < DOUBLE_TAP_MS;
    this.at = now;
    this.side = side;
    return double ? 'double' : 'single';
  }
}

/** Where an arrow key goes from column `x` of row `y`, for rows of the given lengths (an empty row
 *  is passed over); null when there is nothing further that way. */
export function moveInRows(rows: number[], y: number, x: number, key: 'up' | 'down' | 'left' | 'right'): [number, number] | null {
  if (key === 'left' || key === 'right') {
    const next = x + (key === 'left' ? -1 : 1);
    return next >= 0 && next < (rows[y] ?? 0) ? [y, next] : null;
  }
  const dir = key === 'up' ? -1 : 1;
  for (let r = y + dir; r >= 0 && r < rows.length; r += dir) {
    if (rows[r] > 0) return [r, Math.min(x, rows[r] - 1)];
  }
  return null;
}

/** What a remote's or keyboard's media and back keys mean (`e.key`, lower case; TV browsers send
 *  their back button as a key code). */
export function mediaAction(key: string, keyCode: number): 'toggle' | 'play' | 'pause' | 'stop' | 'forward' | 'rewind' | 'next' | 'back' | null {
  switch (key) {
    case 'mediaplaypause':
      return 'toggle';
    case 'mediaplay':
      return 'play';
    case 'mediapause':
      return 'pause';
    case 'mediastop':
      return 'stop';
    case 'mediafastforward':
      return 'forward';
    case 'mediarewind':
      return 'rewind';
    case 'mediatracknext':
      return 'next';
  }
  // webOS, Tizen.
  return keyCode === 461 || keyCode === 10009 ? 'back' : null;
}
