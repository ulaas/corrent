import { describe, expect, it } from 'vitest';
import { applyPreset, compressorAutoMakeupDb, defaultFx, flatEq, flatten, hasEq, headroomDb, mustBypass, normalizeFx, normalizePresets } from '../src/lib/audiofxmodel';

describe('sound settings', () => {
  it('default is off, flat and without stages', () => {
    const fx = defaultFx();
    expect(fx.enabled).toBe(false);
    expect(fx.eq.map((b) => b.hz)).toEqual([31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]);
    expect(hasEq(fx)).toBe(false);
    expect(headroomDb(fx)).toBeCloseTo(0);
  });
  it('headroom makes room for the largest boost, dialogue on top', () => {
    const fx = defaultFx();
    fx.eq[2].db = 6;
    fx.eq[5].db = -9;
    expect(headroomDb(fx)).toBe(-6);
    expect(headroomDb({ ...fx, dialogue: true })).toBe(-9);
  });
  it('presets start from flat and keep loudness and meters', () => {
    const before = { ...defaultFx(), meters: true, loudness: 'live' as const };
    before.eq[0].db = 5;
    const night = applyPreset(before, 'night');
    expect(night).toMatchObject({ enabled: true, preset: 'night', meters: true, loudness: 'live', dialogue: false, crossfeed: null });
    expect(night.night).toEqual({ threshold_db: -24, ratio: 4, makeup_db: 6 });
    expect(hasEq(night)).toBe(false);
    expect(applyPreset(before, 'dialogue').dialogue).toBe(true);
    expect(applyPreset(before, 'crossfeed').crossfeed).toEqual({ strength: 0.5, range: 0.5 });
    expect(flatten(night)).toMatchObject({ enabled: false, preset: null, night: null, meters: true, loudness: 'live' });
  });
  it('takes anything from the server without failing', () => {
    expect(normalizeFx(undefined)).toEqual(defaultFx());
    expect(normalizeFx({ enabled: 'yes', eq: [{ hz: 1e9, q: -1, db: 99 }], loudness: 'target', night: { ratio: 'x' } })).toMatchObject({
      enabled: false,
      loudness: 'off',
      eq: [{ hz: 22000, q: 0.1, db: 24 }],
      night: { threshold_db: -24, ratio: 4, makeup_db: 6 },
    });
    expect(normalizeFx({ eq: [] }).eq).toEqual(flatEq());
    expect(normalizePresets([{ name: 'a', fx: {} }, { name: '' }, 3])).toHaveLength(1);
    expect(normalizePresets('x')).toEqual([]);
  });
  it("knows the compressor's own makeup", () => {
    expect(compressorAutoMakeupDb(-24, 4)).toBeCloseTo(10.8);
    expect(compressorAutoMakeupDb(0, 4)).toBeCloseTo(0);
  });
  it('sends cross-origin streams and Safari HLS around the graph', () => {
    expect(mustBypass({ external: true }, false)).toBe(true);
    expect(mustBypass({ method: 'direct' }, false)).toBe(false);
    expect(mustBypass({ method: 'hls' }, false)).toBe(false);
    expect(mustBypass({ method: 'hls' }, true)).toBe(true);
    expect(mustBypass({ method: 'remux' }, true)).toBe(true);
    expect(mustBypass({ method: 'direct' }, true)).toBe(false);
  });
});
