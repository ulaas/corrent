// Appearance and layout preferences: kept per user on the server (/me/prefs, shared by their
// devices) and cached in localStorage; volume and the sidebar stay per device. A user with no
// saved preferences starts from the server's theme (/theme).
import { accents, fixedAccentPresets } from 'virtual:tokens';
import { api } from './api';
import { onColor } from './color';

export type ThemeMode = 'system' | 'dark' | 'light';
export type Density = 'compact' | 'comfortable' | 'spacious';
export type CardSize = 'small' | 'medium' | 'large';

export interface Prefs {
  theme: ThemeMode;
  preset: string;
  accent: string;
  customAccent: string;
  density: Density;
  radius: number;
  motion: number;
  fontScale: number;
  cardSize: CardSize;
  /** Home row ids in the order shown ("continue", "next_up", "recent:<id>", "favorites"). */
  homeOrder: string[];
  homeHidden: string[];
  sidebarCollapsed: boolean;
  volume: number;
}

const DEFAULTS: Prefs = {
  theme: 'system',
  preset: 'corrent',
  accent: 'blue',
  customAccent: '#4f8cff',
  density: 'comfortable',
  radius: 1,
  motion: 1,
  fontScale: 1,
  cardSize: 'medium',
  homeOrder: [],
  homeHidden: [],
  sidebarCollapsed: false,
  volume: 1,
};

const KEY = 'corrent.prefs';

let hadLocal = false;
/** Saving to the server starts once the user's stored preferences were read. */
let synced = false;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

/** Per-device settings never go to the server. */
function shared(p: Prefs): Partial<Prefs> {
  const { volume: _v, sidebarCollapsed: _s, ...rest } = p;
  return rest;
}

/** After signing in: the user's preferences from the server (or the server's theme). */
export async function syncPrefs(): Promise<void> {
  synced = false;
  try {
    const remote = await api.get<Partial<Prefs>>('/me/prefs');
    if (remote && Object.keys(remote).length) {
      Object.assign(prefs, { ...DEFAULTS, ...remote, volume: prefs.volume, sidebarCollapsed: prefs.sidebarCollapsed });
    } else if (!hadLocal) {
      const t = await api.get<{ preset: string; accent: string; custom_accent: string }>('/theme');
      Object.assign(prefs, { preset: t.preset, accent: t.accent, customAccent: t.custom_accent });
    }
  } catch {
    /* offline or older server: keep this browser's */
  }
  synced = true;
}

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    hadLocal = !!raw;
    if (raw) return { ...DEFAULTS, ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return { ...DEFAULTS };
}

export const prefs = $state<Prefs>(load());

export const ACCENTS = Object.keys(accents);

const darkQuery = typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: dark)') : null;
const reducedQuery = typeof matchMedia === 'function' ? matchMedia('(prefers-reduced-motion: reduce)') : null;
const env = $state({ systemDark: darkQuery?.matches ?? true, reduced: reducedQuery?.matches ?? false });
darkQuery?.addEventListener('change', (e) => (env.systemDark = e.matches));
reducedQuery?.addEventListener('change', (e) => (env.reduced = e.matches));

export function scheme(): 'dark' | 'light' {
  return prefs.theme === 'system' ? (env.systemDark ? 'dark' : 'light') : prefs.theme;
}

/** Multiplier for animation durations: 0 when motion is off or reduced. */
export function motion(): number {
  return env.reduced ? 0 : prefs.motion;
}

/** A duration in ms scaled by the motion setting. */
export function ms(base: number): number {
  return Math.round(base * motion());
}

export function resetPrefs() {
  Object.assign(prefs, { ...DEFAULTS, homeOrder: [], homeHidden: [] });
}

/** Keeps <html> attributes and CSS variables in sync with the preferences. */
export function applyPrefs() {
  $effect.root(() => {
    $effect(() => {
      const root = document.documentElement;
      const s = scheme();
      root.dataset.scheme = s;
      root.dataset.preset = prefs.preset;
      root.dataset.accent = prefs.accent;
      root.dataset.density = prefs.density;
      const fixed = fixedAccentPresets.includes(prefs.preset);
      if (prefs.accent === 'custom' && !fixed) {
        root.style.setProperty('--accent', prefs.customAccent);
        root.style.setProperty('--on-accent', onColor(prefs.customAccent));
      } else {
        root.style.removeProperty('--accent');
        root.style.removeProperty('--on-accent');
      }
      root.style.setProperty('--radius-scale', String(prefs.radius));
      root.style.setProperty('--density', String({ compact: 0.8, comfortable: 1, spacious: 1.2 }[prefs.density]));
      root.style.setProperty('--motion-user', String(prefs.motion));
      root.style.setProperty('--font-scale', String(prefs.fontScale));
      root.dataset.motion = motion() === 0 ? 'off' : 'on';
      const meta = document.querySelector('meta[name="theme-color"]');
      meta?.setAttribute('content', getComputedStyle(root).getPropertyValue('--bg1').trim() || '#161920');
    });
    $effect(() => {
      const snapshot = JSON.stringify(prefs);
      try {
        localStorage.setItem(KEY, snapshot);
      } catch {
        /* private mode */
      }
      if (synced) {
        const body = shared(JSON.parse(snapshot) as Prefs);
        clearTimeout(saveTimer);
        saveTimer = setTimeout(() => void api.put('/me/prefs', body).catch(() => {}), 800);
      }
    });
  });
}
