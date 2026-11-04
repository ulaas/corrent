// The sound processing settings, the same shape as the desktop's (crates/corrent-core/src/audio_fx.rs).
// Kept in `prefs.audioFx`, so they follow the user. Nothing here touches Web Audio: the default costs
// nothing, not even an import of the graph.

export interface Band {
  hz: number;
  q: number;
  db: number;
}
export interface Night {
  threshold_db: number;
  ratio: number;
  makeup_db: number;
}
export interface Crossfeed {
  strength: number;
  range: number;
}
export type Loudness = 'off' | 'live';

export interface AudioFx {
  enabled: boolean;
  /** "night" | "dialogue" | "crossfeed" | a user preset's name, until its values change. */
  preset: string | null;
  eq: Band[];
  night: Night | null;
  dialogue: boolean;
  crossfeed: Crossfeed | null;
  loudness: Loudness;
  meters: boolean;
}

export interface UserPreset {
  name: string;
  fx: AudioFx;
}

/** ISO centres of the ten bands. */
export const EQ_HZ = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
export const DIALOGUE_BOOST_DB = 3;
export const DIALOGUE_CUT_DB = -2;
export const DIALOGUE_BOOST_HZ = 2500;
export const DIALOGUE_CUT_HZ = 250;

export const flatEq = (): Band[] => EQ_HZ.map((hz) => ({ hz, q: 1, db: 0 }));
export const defaultNight = (): Night => ({ threshold_db: -24, ratio: 4, makeup_db: 6 });
export const defaultCrossfeed = (): Crossfeed => ({ strength: 0.5, range: 0.5 });

export function defaultFx(): AudioFx {
  return { enabled: false, preset: null, eq: flatEq(), night: null, dialogue: false, crossfeed: null, loudness: 'off', meters: false };
}

const num = (v: unknown, lo: number, hi: number, fallback: number) => (typeof v === 'number' && isFinite(v) ? Math.max(lo, Math.min(hi, v)) : fallback);

/** Whatever came from the server or an older version, as valid settings. */
export function normalizeFx(raw: unknown): AudioFx {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Partial<AudioFx>;
  const eq = Array.isArray(r.eq) && r.eq.length ? r.eq : flatEq();
  return {
    enabled: r.enabled === true,
    preset: typeof r.preset === 'string' ? r.preset : null,
    eq: eq.slice(0, 16).map((b, i) => ({ hz: num(b?.hz, 10, 22000, EQ_HZ[i] ?? 1000), q: num(b?.q, 0.1, 10, 1), db: num(b?.db, -24, 24, 0) })),
    night: r.night
      ? { threshold_db: num(r.night.threshold_db, -60, 0, -24), ratio: num(r.night.ratio, 1, 20, 4), makeup_db: num(r.night.makeup_db, 0, 24, 6) }
      : null,
    dialogue: r.dialogue === true,
    crossfeed: r.crossfeed ? { strength: num(r.crossfeed.strength, 0, 1, 0.5), range: num(r.crossfeed.range, 0, 1, 0.5) } : null,
    loudness: r.loudness === 'live' ? 'live' : 'off',
    meters: r.meters === true,
  };
}

export function normalizePresets(raw: unknown): UserPreset[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter((p) => p && typeof p.name === 'string' && p.name).map((p) => ({ name: p.name as string, fx: normalizeFx(p.fx) }));
}

/** No effects (Flat). Loudness and the meters are separate choices and stay. */
export function flatten(fx: AudioFx): AudioFx {
  return { ...fx, enabled: false, preset: null, eq: flatEq(), night: null, dialogue: false, crossfeed: null };
}

export const PRESETS = ['night', 'dialogue', 'crossfeed'] as const;

/** Starts from a built-in preset; its values can then be changed. */
export function applyPreset(fx: AudioFx, name: (typeof PRESETS)[number]): AudioFx {
  const out = flatten(fx);
  if (name === 'night') out.night = defaultNight();
  else if (name === 'dialogue') out.dialogue = true;
  else out.crossfeed = defaultCrossfeed();
  return { ...out, enabled: true, preset: name };
}

export function hasEq(fx: AudioFx): boolean {
  return fx.eq.some((b) => b.db !== 0);
}

/** The gain (dB, never positive) in front of the equalizer, so a boost can't clip a loud master. */
export function headroomDb(fx: AudioFx): number {
  const band = fx.eq.reduce((m, b) => Math.max(m, b.db), 0);
  return -(band + (fx.dialogue ? DIALOGUE_BOOST_DB : 0));
}

/** The gain `DynamicsCompressorNode` adds on its own (Web Audio: (1 / full-range gain)^0.6), which
 *  the graph takes back so `makeup_db` means the same as on the desktop. Above the knee the curve
 *  tends to threshold + (level - threshold) / ratio. */
export function compressorAutoMakeupDb(thresholdDb: number, ratio: number): number {
  const fullRange = thresholdDb - thresholdDb / ratio;
  return -0.6 * fullRange;
}

/** Why a source can't go through Web Audio: a cross-origin stream comes out silent, and Safari's
 *  native HLS is unreliable there. */
export function mustBypass(src: { external?: boolean; method?: string }, safari: boolean): boolean {
  return !!src.external || (safari && (src.method === 'hls' || src.method === 'remux'));
}
