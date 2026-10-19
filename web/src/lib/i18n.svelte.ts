// Translations: the English text is the key (`t("Hello {name}", { name })`). Turkish strings
// come from assets/i18n/{tr.toml,web/tr.toml} through the Vite plugin, loaded on demand.
import { interpolate } from './interpolate';

export type Lang = 'en' | 'tr';
export const LANGS: { id: Lang | ''; label: string }[] = [
  { id: '', label: 'Automatic' },
  { id: 'en', label: 'English' },
  { id: 'tr', label: 'Türkçe' },
];

const KEY = 'corrent.lang';

const state = $state<{ lang: Lang; catalog: Record<string, string> }>({ lang: 'en', catalog: {} });

function stored(): Lang | '' {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'en' || v === 'tr' ? v : '';
  } catch {
    return '';
  }
}

function detect(): Lang {
  for (const l of navigator.languages ?? [navigator.language]) {
    if (l.toLowerCase().startsWith('tr')) return 'tr';
    if (l.toLowerCase().startsWith('en')) return 'en';
  }
  return 'en';
}

export function langSetting(): Lang | '' {
  return stored();
}

export function currentLang(): Lang {
  return state.lang;
}

export async function setLang(choice: Lang | ''): Promise<void> {
  try {
    if (choice) localStorage.setItem(KEY, choice);
    else localStorage.removeItem(KEY);
  } catch {
    /* private mode */
  }
  const lang = choice || detect();
  const catalog = lang === 'tr' ? (await import('virtual:i18n/tr')).default : {};
  state.catalog = catalog;
  state.lang = lang;
  document.documentElement.lang = lang;
}

export function initI18n(): Promise<void> {
  return setLang(stored());
}

/** Translates `key` (English) and fills `{placeholders}`. */
export function t(key: string, params?: Record<string, unknown>): string {
  return interpolate(state.catalog[key] ?? key, params);
}

/** Marks a string for translation where it is defined; translate it with `t(x)` where shown. */
export function tk<T extends string>(key: T): T {
  return key;
}

/** BCP 47 locale for Intl formatters. */
export function locale(): string {
  return state.lang === 'tr' ? 'tr-TR' : 'en-GB';
}
