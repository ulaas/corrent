// Number, size, speed, time and date formatting.
import { locale, t } from './i18n.svelte';

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];

export function bytes(n: number | null | undefined): string {
  if (n == null || !isFinite(n)) return '–';
  let v = Math.max(0, n);
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  const digits = i === 0 ? 0 : v < 10 ? 2 : v < 100 ? 1 : 0;
  return `${v.toLocaleString(locale(), { minimumFractionDigits: digits, maximumFractionDigits: digits })} ${UNITS[i]}`;
}

export function rate(bps: number | null | undefined): string {
  if (!bps) return '–';
  return t('{size}/s', { size: bytes(bps) });
}

/** "1 h 5 min", "3 min 20 s", "2 d 4 h". */
export function duration(secs: number | null | undefined): string {
  if (secs == null || !isFinite(secs) || secs < 0) return '∞';
  const s = Math.round(secs);
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  if (d > 0) return t('{d} d {h} h', { d, h });
  if (h > 0) return t('{h} h {m} min', { h, m });
  if (m > 0) return t('{m} min {s} s', { m, s: r });
  return t('{s} s', { s: r });
}

/** Media clock: 1:02:03 / 4:05. */
export function clock(ms: number | null | undefined): string {
  if (ms == null || !isFinite(ms) || ms < 0) return '0:00';
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  const pad = (x: number) => String(x).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(r)}` : `${m}:${pad(r)}`;
}

/** Runtime in words: "1 h 52 min", "45 min". */
export function runtime(ms: number | null | undefined): string {
  if (!ms) return '';
  const min = Math.round(ms / 60000);
  const h = Math.floor(min / 60), m = min % 60;
  if (h > 0) return m ? t('{h} h {m} min', { h, m }) : t('{h} h', { h });
  return t('{m} min', { m });
}

export function percent(p: number, digits = 1): string {
  const v = Math.max(0, Math.min(1, p || 0)) * 100;
  return `${v.toLocaleString(locale(), { minimumFractionDigits: v >= 100 ? 0 : digits, maximumFractionDigits: v >= 100 ? 0 : digits })}%`;
}

export function num(n: number, digits = 0): string {
  return n.toLocaleString(locale(), { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function date(unixSecs: number | null | undefined): string {
  if (!unixSecs) return '–';
  return new Date(unixSecs * 1000).toLocaleString(locale(), { dateStyle: 'medium', timeStyle: 'short' });
}

export function dayDate(unixSecs: number | null | undefined): string {
  if (!unixSecs) return '–';
  return new Date(unixSecs * 1000).toLocaleDateString(locale(), { dateStyle: 'medium' });
}

export function time(unixSecsOrMs: number): string {
  const ms = unixSecsOrMs > 1e12 ? unixSecsOrMs : unixSecsOrMs * 1000;
  return new Date(ms).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

/** "3 minutes ago", "in 2 hours". */
export function ago(unixSecs: number | null | undefined): string {
  if (!unixSecs) return '–';
  const diff = unixSecs - Date.now() / 1000;
  const rtf = new Intl.RelativeTimeFormat(locale(), { numeric: 'auto' });
  const abs = Math.abs(diff);
  const steps: [number, Intl.RelativeTimeFormatUnit][] = [
    [60, 'second'],
    [3600, 'minute'],
    [86400, 'hour'],
    [86400 * 30, 'day'],
    [86400 * 365, 'month'],
    [Infinity, 'year'],
  ];
  let unit: Intl.RelativeTimeFormatUnit = 'second';
  let div = 1;
  for (const [limit, u] of steps) {
    unit = u;
    if (abs < limit) break;
    div = limit;
  }
  return rtf.format(Math.round(diff / div), unit);
}
