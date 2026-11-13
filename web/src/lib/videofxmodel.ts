// The video effects settings, the same shape as the desktop's (crates/corrent-core/src/video_fx.rs).
// Kept in `prefs.videoFx`, so they follow the user. What a browser does with them is in
// `videofxlook.ts`, which only the player loads.
//
// On the desktop the effects are shaders in mpv's renderer. Here the picture is a `<video>` the
// browser draws, and only what CSS can do to that element is offered: effects that read
// neighbouring pixels stay on the desktop (`web: false`).
//
// The catalogue (ids, groups, values and their ranges, the looks) is `videofx.catalogue.json`, which
// a test of the Rust side (`the_web_catalogue_matches`) compares with its own.
import data from './videofx.catalogue.json';

export type Group = 'colour' | 'detail' | 'style' | 'accessibility' | 'tools';

export interface Param {
  key: string;
  min: number;
  max: number;
  default: number;
  /** What a slider moves by. */
  step: number;
  /** A choice between a few things (0, 1, 2…), not an amount. */
  choice: boolean;
  /** The browser can do this part of the effect. */
  web: boolean;
}

export interface Shader {
  id: string;
  group: Group;
  params: Param[];
  /** The browser can do it (perhaps without some of its values). */
  web: boolean;
}

export interface Effect {
  on: boolean;
  params: Record<string, number>;
}

/** The picture controls, each -100..100 with 0 for "as it is". */
export interface Adjust {
  brightness: number;
  contrast: number;
  saturation: number;
  gamma: number;
  hue: number;
}

export interface VideoFx {
  /** The master switch: off shows the picture as it is, and keeps everything below. */
  enabled: boolean;
  /** The look last picked (a built-in or a saved look's name), until something is changed by hand. */
  preset: string | null;
  adjust: Adjust;
  /** mpv's debanding: kept so the settings have one shape, never drawn here. */
  deband: boolean;
  /** By `Shader.id`. An effect that isn't here is off, at its defaults. */
  effects: Record<string, Effect>;
}

export interface VideoPreset {
  name: string;
  fx: VideoFx;
}

/** What the browser draws: `true`, or the values of the effect that it can (the others are ignored). */
const WEB: Record<string, true | string[]> = {
  soften: true,
  levels: true,
  temperature: true,
  tone: true,
  cinema: true,
  cross_process: true,
  mono: true,
  sepia: true,
  daltonize: true,
  colour_blind: true,
  posterize: true,
  invert: true,
  // Scanlines only: no curved glass, no shadow mask.
  crt: ['scanlines'],
  vignette: true,
  grain: true,
};

export const ADJUST = data.adjust as (keyof Adjust)[];
export const GROUPS = data.groups as Group[];

/** The built-in effects, in the order they run. */
export const CATALOGUE: Shader[] = data.effects.map((s) => {
  const web = WEB[s.id];
  return { id: s.id, group: s.group as Group, web: !!web, params: s.params.map((p) => ({ ...p, web: web === true || (!!web && web.includes(p.key)) })) };
});

const BY_ID = new Map(CATALOGUE.map((s) => [s.id, s]));
export const shader = (id: string): Shader | undefined => BY_ID.get(id);
const param = (id: string, key: string): Param | undefined => shader(id)?.params.find((p) => p.key === key);

/** The built-in looks, by the name kept in `VideoFx.preset`. */
export const PRESETS: string[] = data.presets.map((p) => p.name);

/** `v` as a value takes it: in range, on a step, and the default for nonsense. */
export function fit(p: Param, v: unknown): number {
  if (typeof v !== 'number' || !isFinite(v)) return p.default;
  const c = Math.max(p.min, Math.min(p.max, v));
  const fitted = p.min + Math.round((c - p.min) / p.step) * p.step;
  return Math.round(Math.max(p.min, Math.min(p.max, fitted)) * 1e4) / 1e4;
}

export const defaultAdjust = (): Adjust => ({ brightness: 0, contrast: 0, saturation: 0, gamma: 0, hue: 0 });

/** No processing at all. */
export function defaultVideoFx(): VideoFx {
  return { enabled: false, preset: null, adjust: defaultAdjust(), deband: false, effects: {} };
}

/** Whatever came from the server or an older version, as valid settings: effects and values this
 *  version doesn't know are dropped, numbers are brought into range. */
export function normalizeVideoFx(raw: unknown): VideoFx {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Partial<VideoFx>;
  const a = (r.adjust && typeof r.adjust === 'object' ? r.adjust : {}) as Partial<Adjust>;
  const adjust = defaultAdjust();
  for (const k of ADJUST) {
    const v = a[k];
    adjust[k] = typeof v === 'number' && isFinite(v) ? Math.round(Math.max(-100, Math.min(100, v))) + 0 : 0;
  }
  const given = (r.effects && typeof r.effects === 'object' ? r.effects : {}) as Record<string, Partial<Effect> | null>;
  const effects: Record<string, Effect> = {};
  for (const s of CATALOGUE) {
    const e = Object.hasOwn(given, s.id) ? given[s.id] : null;
    if (!e || typeof e !== 'object') continue;
    const values = (e.params && typeof e.params === 'object' ? e.params : {}) as Record<string, unknown>;
    const params: Record<string, number> = {};
    for (const p of s.params) if (Object.hasOwn(values, p.key)) params[p.key] = fit(p, values[p.key]);
    effects[s.id] = { on: e.on === true, params };
  }
  return { enabled: r.enabled === true, preset: typeof r.preset === 'string' ? r.preset : null, adjust, deband: r.deband === true, effects };
}

export function normalizeLooks(raw: unknown): VideoPreset[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter((p) => p && typeof p.name === 'string' && p.name).map((p) => ({ name: p.name as string, fx: normalizeVideoFx(p.fx) }));
}

export function isOn(fx: VideoFx, id: string): boolean {
  return fx.effects[id]?.on === true;
}

/** Turns a built-in effect on or off; its values stay. */
export function toggle(fx: VideoFx, id: string, on: boolean): VideoFx {
  if (!shader(id)) return fx;
  return { ...fx, effects: { ...fx.effects, [id]: { on, params: { ...fx.effects[id]?.params } } } };
}

/** A value of an effect: in range, the default when it isn't set. */
export function value(fx: VideoFx, id: string, key: string): number {
  const p = param(id, key);
  if (!p) return 0;
  const set = fx.effects[id]?.params;
  return set && Object.hasOwn(set, key) ? fit(p, set[key]) : p.default;
}

export function setValue(fx: VideoFx, id: string, key: string, v: number): VideoFx {
  const p = param(id, key);
  if (!p) return fx;
  const e = fx.effects[id];
  return { ...fx, effects: { ...fx.effects, [id]: { on: e?.on === true, params: { ...e?.params, [key]: fit(p, v) } } } };
}

/** Starts from a built-in look; its values can then be changed. An unknown name changes nothing. */
export function applyPreset(fx: VideoFx, name: string): VideoFx {
  const look = data.presets.find((p) => p.name === name);
  if (!look) return fx;
  let out = defaultVideoFx();
  for (const [id, values] of Object.entries(look.effects)) {
    out = toggle(out, id, true);
    for (const [key, v] of Object.entries(values as Record<string, number>)) out = setValue(out, id, key, v);
  }
  out.adjust = { ...out.adjust, ...look.adjust };
  return { ...out, deband: look.deband, enabled: true, preset: name };
}

/** How much of a built-in look the browser can draw. */
export function presetOnWeb(name: string): 'all' | 'some' | 'none' {
  const look = data.presets.find((p) => p.name === name);
  if (!look) return 'none';
  const ids = Object.keys(look.effects);
  const drawn = ids.filter((id) => shader(id)?.web).length;
  const adjusted = Object.keys(look.adjust).length > 0;
  if (drawn === ids.length && !look.deband && ids.every((id) => shader(id)!.params.every((p) => p.web))) return 'all';
  return drawn > 0 || adjusted ? 'some' : 'none';
}

/** Something is set that would change the picture where every effect can be drawn (the desktop's
 *  `has_effects`); `videoLook` says what changes it here. */
export function hasEffects(fx: VideoFx): boolean {
  return fx.enabled && (fx.deband || ADJUST.some((k) => fx.adjust[k] !== 0) || CATALOGUE.some((s) => isOn(fx, s.id)));
}
