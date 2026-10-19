// Seek-bar thumbnails from a WebVTT sprite index ("sheet-0.jpg#xywh=0,0,160,90" cues).
export interface TrickCue {
  start: number;
  end: number;
  url: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

function secs(ts: string): number {
  const parts = ts.trim().split(':').map(Number);
  return parts.reduce((a, v) => a * 60 + v, 0);
}

export function parseTrickplay(vtt: string, base: string): TrickCue[] {
  const out: TrickCue[] = [];
  const blocks = vtt.replace(/\r/g, '').split(/\n\n+/);
  for (const b of blocks) {
    const lines = b.split('\n').filter(Boolean);
    const ti = lines.findIndex((l) => l.includes('-->'));
    if (ti < 0 || !lines[ti + 1]) continue;
    const [a, z] = lines[ti].split('-->');
    const m = /^(.*)#xywh=(\d+),(\d+),(\d+),(\d+)/.exec(lines[ti + 1].trim());
    if (!m) continue;
    const u = new URL(m[1], new URL(base, 'http://x/'));
    out.push({
      start: secs(a),
      end: secs(z.split(' ').filter(Boolean)[0]),
      url: u.pathname + u.search,
      x: +m[2],
      y: +m[3],
      w: +m[4],
      h: +m[5],
    });
  }
  return out;
}

export function cueAt(cues: TrickCue[], t: number): TrickCue | null {
  let lo = 0, hi = cues.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (cues[mid].end <= t) lo = mid + 1;
    else if (cues[mid].start > t) hi = mid - 1;
    else return cues[mid];
  }
  return null;
}
