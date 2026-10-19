// Appearance and layout preferences of this browser (localStorage for now; per-user storage on
// the server comes with GET /api/v1/theme, docs/MEDIASERVER.md U1).
import { accents, fixedAccentPresets } from 'virtual:tokens';
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

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
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
    });
  });
}
