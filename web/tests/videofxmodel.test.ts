import { describe, expect, it } from 'vitest';
import {
  ADJUST,
  CATALOGUE,
  GROUPS,
  PRESETS,
  applyPreset,
  defaultVideoFx,
  fit,
  hasEffects,
  isOn,
  normalizeLooks,
  normalizeVideoFx,
  presetOnWeb,
  setValue,
  shader,
  toggle,
  value,
  type VideoFx,
} from '../src/lib/videofxmodel';
import { pictureRect, videoLook } from '../src/lib/videofxlook';
import { ADJUST_LABEL, GROUP_LABEL, HINT, LABEL, PRESET_LABEL, paramLabel } from '../src/lib/videofxlabels';

const only = (id: string, params: Record<string, number> = {}): VideoFx => ({ ...defaultVideoFx(), enabled: true, effects: { [id]: { on: true, params } } });
const adjusted = (adjust: Partial<VideoFx['adjust']>): VideoFx => ({ ...defaultVideoFx(), enabled: true, adjust: { ...defaultVideoFx().adjust, ...adjust } });

describe('video effects settings', () => {
  it('the default is no processing at all', () => {
    const fx = defaultVideoFx();
    expect(fx).toEqual({ enabled: false, preset: null, adjust: { brightness: 0, contrast: 0, saturation: 0, gamma: 0, hue: 0 }, deband: false, effects: {} });
    expect(hasEffects(fx)).toBe(false);
    expect(videoLook(fx)).toEqual({ filter: '', defs: '', overlays: [] });
    expect(normalizeVideoFx(undefined)).toEqual(fx);
    expect(normalizeVideoFx('x')).toEqual(fx);
  });

  it('the catalogue is the desktop one, with what the browser can do marked', () => {
    expect(CATALOGUE.length).toBe(26);
    expect(new Set(CATALOGUE.map((s) => s.id)).size).toBe(CATALOGUE.length);
    expect(GROUPS).toEqual(['colour', 'detail', 'style', 'accessibility', 'tools']);
    expect(ADJUST).toEqual(['brightness', 'contrast', 'saturation', 'gamma', 'hue']);
    for (const s of CATALOGUE) {
      expect(GROUPS).toContain(s.group);
      for (const p of s.params) {
        expect(p.min).toBeLessThan(p.max);
        expect(fit(p, p.default)).toBe(p.default);
        if (!s.web) expect(p.web).toBe(false);
      }
    }
    expect(CATALOGUE.filter((s) => s.web).map((s) => s.id)).toEqual([
      'soften', 'levels', 'temperature', 'tone', 'cinema', 'cross_process', 'mono', 'sepia', 'daltonize', 'colour_blind', 'posterize', 'invert', 'crt', 'vignette', 'grain',
    ]);
    // Reading neighbours is the desktop's.
    for (const id of ['denoise', 'sharpen', 'clarity', 'vibrance', 'night_vision', 'pixelate', 'chromatic', 'bloom', 'outline', 'halftone', 'zebra']) expect(shader(id)?.web).toBe(false);
    expect(shader('crt')!.params.map((p) => [p.key, p.web])).toEqual([['curvature', false], ['scanlines', true], ['mask', false]]);
  });

  it('everything has a name', () => {
    for (const s of CATALOGUE) {
      expect(LABEL[s.id], s.id).toBeTruthy();
      expect(HINT[s.id], s.id).toBeTruthy();
      for (const p of s.params.filter((p) => !p.choice)) expect(paramLabel(s.id, p.key), `${s.id}.${p.key}`).toBeTruthy();
    }
    for (const g of GROUPS) expect(GROUP_LABEL[g]).toBeTruthy();
    for (const p of PRESETS) expect(PRESET_LABEL[p], p).toBeTruthy();
    for (const k of ADJUST) expect(ADJUST_LABEL[k]).toBeTruthy();
  });

  it('normalize clamps, keeps to steps and drops what it does not know', () => {
    const evil = 'x; } void main() { discard; }';
    const fx = normalizeVideoFx({
      enabled: 'yes',
      preset: 7,
      deband: true,
      adjust: { brightness: NaN, contrast: 1e9, saturation: -12.6, gamma: '3', hue: -1e9, extra: 1 },
      effects: {
        vignette: { on: true, params: { strength: NaN, radius: 1e30, [evil]: 1 } },
        grain: { on: 'true', params: { amount: 0.123, size: -4 } },
        sepia: null,
        mono: { on: true },
        [evil]: { on: true, params: { amount: 1 } },
        constructor: { on: true },
      },
    });
    expect(fx).toEqual({
      enabled: false,
      preset: null,
      deband: true,
      adjust: { brightness: 0, contrast: 100, saturation: -13, gamma: 0, hue: -100 },
      effects: {
        mono: { on: true, params: {} },
        vignette: { on: true, params: { strength: 0.5, radius: 1 } },
        grain: { on: false, params: { amount: 0.12, size: 1 } },
      },
    });
    expect(normalizeVideoFx(fx)).toEqual(fx);
    expect(normalizeLooks([{ name: 'a', fx: {} }, { name: '' }, 3, null])).toEqual([{ name: 'a', fx: defaultVideoFx() }]);
    expect(normalizeLooks('x')).toEqual([]);
  });

  it('values stay in range and on steps', () => {
    let fx = toggle(defaultVideoFx(), 'grain', true);
    fx = setValue(fx, 'grain', 'amount', 99);
    expect(value(fx, 'grain', 'amount')).toBe(0.3);
    fx = setValue(fx, 'grain', 'amount', NaN);
    expect(value(fx, 'grain', 'amount')).toBe(0.08);
    fx = setValue(fx, 'grain', 'amount', 0.123);
    expect(value(fx, 'grain', 'amount')).toBe(0.12);
    expect(value(fx, 'grain', 'size')).toBe(1.5);
    fx = setValue(fx, 'grain', 'no such value', 1);
    fx = setValue(fx, 'no such effect', 'amount', 1);
    fx = toggle(fx, 'no such effect', true);
    expect(Object.keys(fx.effects)).toEqual(['grain']);
    expect(isOn(fx, 'grain')).toBe(true);
    expect(value(fx, 'no such effect', 'amount')).toBe(0);
  });

  it('every built-in look normalizes to itself and is on', () => {
    expect(PRESETS).toEqual(['vivid', 'cinema', 'warm', 'old_film', 'black_white', 'retro_tv', 'comic', 'night']);
    for (const name of PRESETS) {
      const fx = applyPreset(defaultVideoFx(), name);
      expect(normalizeVideoFx(JSON.parse(JSON.stringify(fx))), name).toEqual(fx);
      expect(fx.enabled && fx.preset === name && hasEffects(fx), name).toBe(true);
    }
    const old = applyPreset(defaultVideoFx(), 'old_film');
    expect(old.effects).toEqual({
      grain: { on: true, params: { amount: 0.14, size: 2 } },
      sepia: { on: true, params: { amount: 0.7 } },
      soften: { on: true, params: { amount: 0.3 } },
      vignette: { on: true, params: { radius: 0.65, strength: 0.6 } },
    });
    expect(applyPreset(defaultVideoFx(), 'night').adjust.brightness).toBe(-8);
    expect(isOn(applyPreset(applyPreset(defaultVideoFx(), 'vivid'), 'old_film'), 'vibrance')).toBe(false);
    expect(applyPreset(old, 'no such look')).toBe(old);
  });

  it('knows how much of a look the browser draws', () => {
    expect(PRESETS.map(presetOnWeb)).toEqual(['none', 'all', 'all', 'all', 'all', 'some', 'some', 'all']);
    // Vivid is sharpening, clarity and vibrance: nothing to draw here.
    expect(videoLook(applyPreset(defaultVideoFx(), 'vivid'))).toEqual({ filter: '', defs: '', overlays: [] });
  });
});

describe('what the browser is asked to do', () => {
  it('nothing when off, at neutral values, or for effects only the desktop has', () => {
    const nothing = { filter: '', defs: '', overlays: [] };
    const cinema = applyPreset(defaultVideoFx(), 'cinema');
    expect(videoLook({ ...cinema, enabled: false })).toEqual(nothing);
    expect(videoLook({ ...defaultVideoFx(), enabled: true, deband: true })).toEqual(nothing);
    expect(videoLook(only('sharpen'))).toEqual(nothing);
    expect(videoLook(only('sepia', { amount: 0 }))).toEqual(nothing);
    expect(videoLook(only('temperature', { warmth: 0 }))).toEqual(nothing);
    expect(videoLook(only('levels', { black: 0, white: 1 }))).toEqual(nothing);
    expect(videoLook(only('grain', { amount: 0 }))).toEqual(nothing);
    expect(videoLook(only('crt', { scanlines: 0 }))).toEqual(nothing);
  });

  it("uses the browser's own functions where they are the same sum", () => {
    expect(videoLook(only('mono'))).toEqual({ filter: 'grayscale(1)', defs: '', overlays: [] });
    expect(videoLook(only('sepia')).filter).toBe('sepia(0.8)');
    expect(videoLook(only('invert')).filter).toBe('invert(1)');
    // mpv's contrast is a gain from black: CSS calls that brightness.
    expect(videoLook(adjusted({ contrast: 40, saturation: -100 }))).toEqual({ filter: 'saturate(0) brightness(1.4)', defs: '', overlays: [] });
    // 1.5 px taps at 1080 lines, half as far apart at 540.
    expect(videoLook(only('soften', { amount: 1 }), { height: 1080 }).filter).toBe('blur(1.06px)');
    expect(videoLook(only('soften', { amount: 1 }), { height: 540 }).filter).toBe('blur(0.53px)');
  });

  it('writes SVG filters for the rest, on the encoded values', () => {
    const b = videoLook(adjusted({ brightness: 30, contrast: 40 }));
    expect(b.filter).toMatch(/^url\(#vfx-0-[0-9a-z]+\)$/);
    expect(b.defs).toContain('color-interpolation-filters="sRGB"');
    expect(b.defs).toContain('<feColorMatrix in="SourceGraphic" type="matrix" values="1.4 0 0 0 0.3 0 1.4 0 0 0.3 0 0 1.4 0 0.3 0 0 0 1 0" result="r0"/>');
    expect(b.overlays).toEqual([]);
    // A half turn of the colour keeps the greys: every row still adds up to 1.
    const hue = videoLook(adjusted({ hue: 100 })).defs;
    const rows = /values="([^"]+)"/.exec(hue)![1].split(' ').map(Number);
    for (const r of [0, 5, 10]) expect(rows[r] + rows[r + 1] + rows[r + 2]).toBeCloseTo(1, 3);
    expect(rows[0]).toBeCloseTo(2 * 0.2126 - 1, 3);
    // +100 is three stops: a power of 1/8.
    expect(videoLook(adjusted({ gamma: 100 })).defs).toContain('<feFuncR type="gamma" amplitude="1" exponent="0.125" offset="0"/>');
    expect(videoLook(only('levels')).defs).toContain('<feFuncG type="linear" slope="1.1111" intercept="-0.0556"/>');
    expect(videoLook(only('posterize', { levels: 3 })).defs).toContain('<feFuncB type="discrete" tableValues="0 0.5 0.5 1"/>');
    // Warmer: more red, less blue, and a grey as bright as before.
    const warm = /values="([^"]+)"/.exec(videoLook(only('temperature')).defs)![1].split(' ').map(Number);
    expect(warm[0]).toBeGreaterThan(1);
    expect(warm[12]).toBeLessThan(1);
    expect(0.2126 * warm[0] + 0.7152 * warm[6] + 0.0722 * warm[12]).toBeCloseTo(1, 3);
  });

  it('simulates colour vision on linear light with the desktop numbers', () => {
    const defs = videoLook(only('colour_blind')).defs;
    expect(defs).toContain('values="0.3673 0.8606 -0.228 0 0 0.2801 0.6725 0.0474 0 0 -0.0118 0.0429 0.9689 0 0 0 0 0 1 0" color-interpolation-filters="linearRGB"');
    expect(defs).not.toContain('feComposite');
    // Less severe: mixed with the picture.
    expect(videoLook(only('colour_blind', { severity: 0.5 })).defs).toContain('k2="0.5" k3="0.5"');
    const fix = videoLook(only('daltonize', { kind: 2 })).defs;
    expect(fix).toContain('1.2555 -0.0767 -0.1788');
    expect(fix.match(/<fe/g)).toHaveLength(4);
  });

  it('keeps the order the effects run in, sharing a filter between neighbours', () => {
    const fx = normalizeVideoFx({ enabled: true, adjust: { gamma: 10 }, effects: { levels: { on: true }, tone: { on: true }, mono: { on: true }, posterize: { on: true }, invert: { on: true } } });
    const look = videoLook(fx);
    expect(look.filter).toMatch(/^url\(#vfx-0-\w+\) grayscale\(1\) url\(#vfx-1-\w+\) invert\(1\)$/);
    expect(look.defs.match(/<filter /g)).toHaveLength(2);
    // Gamma, levels and tone (brightness picture, its factor, the product) in the first.
    expect(look.defs.split('</filter>')[0].match(/result=/g)).toHaveLength(5);
    // A new text has a new id; the same one keeps it.
    expect(videoLook(fx).filter).toBe(look.filter);
    expect(videoLook({ ...fx, adjust: { ...fx.adjust, gamma: 11 } }).filter).not.toBe(look.filter);
  });

  it('only ever writes numbers into the markup', () => {
    let fx = defaultVideoFx();
    for (const s of CATALOGUE) fx = toggle(fx, s.id, true);
    const look = videoLook({ ...fx, enabled: true, adjust: { brightness: 12, contrast: -7, saturation: 30, gamma: -20, hue: 15 } }, { height: 720 });
    expect(look.defs).not.toMatch(/NaN|Infinity|undefined|e-\d/);
    expect(look.filter).not.toMatch(/NaN|Infinity|undefined|e-\d/);
    expect(look.overlays.map((o) => o.kind)).toEqual(['scanlines', 'vignette', 'grain']);
    for (const o of look.overlays) expect(o.style).not.toMatch(/NaN|Infinity|undefined|e-\d|[<>"]/);
  });

  it('draws the vignette, the scanlines and the grain as layers', () => {
    const v = videoLook(only('vignette'));
    expect(v.filter).toBe('');
    expect(v.overlays).toHaveLength(1);
    expect(v.overlays[0].style).toMatch(/^background:radial-gradient\(ellipse farthest-corner at 50% 50%,rgba\(0,0,0,0\) 35%,/);
    // The corners get the shader's darkest value: strength × smoothstep(0.35, 1.05, 1).
    expect(v.overlays[0].style).toMatch(/rgba\(0,0,0,0\.4927\) 100%\)$/);
    const crt = videoLook(only('crt'), { height: 2160 });
    expect(crt.filter).toBe('brightness(1.175)');
    expect(crt.overlays[0].kind).toBe('scanlines');
    expect(crt.overlays[0].style).toContain('rgba(0,0,0,0.35) 50%');
    expect(crt.overlays[0].style).toMatch(/0 0\/100% 6px$/);
    expect(videoLook(only('grain'), { height: 1080 }).overlays).toEqual([{ kind: 'grain', style: 'opacity:0.096;background-size:384px 384px' }]);
  });

  it('finds the picture inside its element', () => {
    expect(pictureRect({ w: 1000, h: 1000 }, { w: 1920, h: 1080 }, true)).toEqual({ x: 0, y: 218.75, w: 1000, h: 562.5 });
    expect(pictureRect({ w: 1000, h: 300 }, { w: 400, h: 300 }, true)).toEqual({ x: 300, y: 0, w: 400, h: 300 });
    expect(pictureRect({ w: 1000, h: 300 }, { w: 400, h: 300 }, false)).toEqual({ x: 0, y: 0, w: 1000, h: 300 });
    expect(pictureRect({ w: 1000, h: 300 }, { w: 0, h: 0 }, true)).toEqual({ x: 0, y: 0, w: 1000, h: 300 });
  });
});
