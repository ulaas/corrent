// Where a context menu goes: at the pointer, flipped and clamped so it stays on screen.

export interface Box {
  left: number;
  top: number;
}

/** Margin kept from the window's edges. */
export const EDGE = 8;

/** A menu of `w`×`h` opened at (x, y) in a `vw`×`vh` window: right and down from the pointer,
 * flipped left/up when it doesn't fit, clamped when it fits neither way. */
export function place(x: number, y: number, w: number, h: number, vw: number, vh: number): Box {
  let left = x + w <= vw - EDGE ? x : x - w;
  let top = y + h <= vh - EDGE ? y : y - h;
  left = Math.max(EDGE, Math.min(left, vw - EDGE - w));
  top = Math.max(EDGE, Math.min(top, vh - EDGE - h));
  return { left, top };
}

/** A submenu next to its row (`row`: the row's box): to the right, or to the left when there is
 * no room, its top aligned with the row's. */
export function placeSub(
  row: { left: number; right: number; top: number },
  w: number,
  h: number,
  vw: number,
  vh: number,
): Box {
  const OVERLAP = 4;
  let left = row.right - OVERLAP;
  if (left + w > vw - EDGE) left = row.left - w + OVERLAP;
  left = Math.max(EDGE, Math.min(left, vw - EDGE - w));
  const top = Math.max(EDGE, Math.min(row.top - 6, vh - EDGE - h));
  return { left, top };
}

/** The next enabled row from `i` going `dir` (wrapping), or -1 when none is. */
export function step(enabled: boolean[], i: number, dir: 1 | -1): number {
  const n = enabled.length;
  for (let k = 1; k <= n; k++) {
    const j = (((i + dir * k) % n) + n) % n;
    if (enabled[j]) return j;
  }
  return -1;
}

/** The next enabled row after `i` whose label starts with `ch` (typeahead), or -1. */
export function jump(labels: (string | null)[], i: number, ch: string): number {
  const c = ch.toLocaleLowerCase();
  const n = labels.length;
  for (let k = 1; k <= n; k++) {
    const j = (i + k) % n;
    if (labels[j]?.toLocaleLowerCase().startsWith(c)) return j;
  }
  return -1;
}
