import { describe, expect, it } from 'vitest';
import { normalizeQuality, QUALITIES, QUALITY_IDS, qualityLabel, qualityLimits } from '../src/lib/quality';

describe('playback quality', () => {
  it('has the limits of the server’s presets', () => {
    expect(qualityLimits('2160')).toEqual({ max_height: 2160, max_bitrate: 20_000_000 });
    expect(qualityLimits('1080')).toEqual({ max_height: 1080, max_bitrate: 8_000_000 });
    expect(qualityLimits('720')).toEqual({ max_height: 720, max_bitrate: 4_000_000 });
    expect(qualityLimits('480')).toEqual({ max_height: 480, max_bitrate: 1_500_000 });
  });

  it('sends no limits for the original or an unknown id', () => {
    expect(qualityLimits('')).toEqual({});
    expect(qualityLimits('360')).toEqual({});
  });

  it('names the choices', () => {
    expect(QUALITY_IDS).toEqual(['', '2160', '1080', '720', '480']);
    expect(QUALITY_IDS.map(qualityLabel)).toEqual(['Original', '4K', '1080p', '720p', '480p']);
  });

  it('keeps only known stored choices', () => {
    for (const q of QUALITIES) expect(normalizeQuality(q.id)).toBe(q.id);
    expect(normalizeQuality('360')).toBe('');
    expect(normalizeQuality(720)).toBe('');
    expect(normalizeQuality(undefined)).toBe('');
  });
});
