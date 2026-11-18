import { describe, expect, it } from 'vitest';
import { castDeviceLabel } from '../src/lib/castlabel';

describe('cast device labels', () => {
  const np = 'not paired';

  it('shows a Chromecast by name and model', () => {
    expect(castDeviceLabel({ name: 'Den', model: 'Chromecast Ultra' }, np)).toBe('Den · Chromecast Ultra');
    expect(castDeviceLabel({ name: 'Den', model: '', kind: '' }, np)).toBe('Den');
  });

  it('names the way other devices are reached', () => {
    expect(castDeviceLabel({ name: 'TV', model: 'QN90', kind: 'dlna' }, np)).toBe('TV · QN90 · DLNA');
    expect(castDeviceLabel({ name: 'Apple TV', model: '', kind: 'airplay' }, np)).toBe('Apple TV · AirPlay');
    expect(castDeviceLabel({ name: 'X', model: 'Y', kind: 'other' }, np)).toBe('X · Y');
  });

  it('says when a device still has to be paired', () => {
    expect(castDeviceLabel({ name: 'Apple TV', model: 'AppleTV14,1', kind: 'airplay', needs_pairing: true }, 'eşleştirilmedi')).toBe(
      'Apple TV · AppleTV14,1 · AirPlay · eşleştirilmedi',
    );
    expect(castDeviceLabel({ name: 'Apple TV', model: '', kind: 'airplay', needs_pairing: false }, np)).toBe('Apple TV · AirPlay');
  });
});
