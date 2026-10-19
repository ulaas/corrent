import { describe, expect, it } from 'vitest';
import { deflateSync } from 'node:zlib';
import { interpolate } from '../src/lib/interpolate';
import { cueAt, parseTrickplay } from '../src/lib/trickplay';
import { inflate } from '../src/lib/events.svelte';
import { bytes, clock, duration } from '../src/lib/format';
import { keysIn, placeholders } from '../scripts/i18n-keys.mjs';
import { onColor } from '../src/lib/color';

describe('i18n', () => {
  it('fills named placeholders and keeps unknown ones', () => {
    expect(interpolate('Hello {name}, {n} new', { name: 'Ada', n: 3 })).toBe('Hello Ada, 3 new');
    expect(interpolate('{x} stays', {})).toBe('{x} stays');
  });

  it('finds t() and tk() keys in source text', () => {
    const src = `t('One'); t("Two {n}", { n }); tk('Three'); x.t('no'); const at = (a) => 1; at('no'); t('It\\'s')`;
    expect(keysIn(src)).toEqual(['One', 'Two {n}', 'Three', "It's"]);
    expect(placeholders('{b} and {a}')).toEqual(['a', 'b']);
  });
});

describe('formatting', () => {
  it('formats sizes, clocks and durations', () => {
    expect(bytes(0)).toBe('0 B');
    expect(bytes(1536)).toBe('1.50 KiB');
    expect(bytes(5 * 1024 ** 3)).toBe('5.00 GiB');
    expect(clock(3723000)).toBe('1:02:03');
    expect(clock(65000)).toBe('1:05');
    expect(duration(3700)).toBe('1 h 1 min');
    expect(duration(null)).toBe('∞');
  });

  it('picks readable text on accents', () => {
    expect(onColor('#FFD400')).toBe('#0B0D12');
    expect(onColor('#2F6FEB')).toBe('#FFFFFF');
  });
});

describe('events', () => {
  it('inflates zlib frames', async () => {
    const json = JSON.stringify({ t: 'hello', server_id: 'x', user: 'u' });
    const z = deflateSync(Buffer.from(json));
    expect(await inflate(z.buffer.slice(z.byteOffset, z.byteOffset + z.byteLength) as ArrayBuffer)).toBe(json);
  });
});

describe('trickplay', () => {
  it('parses a sprite index and finds cues', () => {
    const vtt = `WEBVTT\n\n00:00:00.000 --> 00:00:10.000\nsheet-0.jpg#xywh=0,0,160,90\n\n00:00:10.000 --> 00:00:20.000\nsheet-0.jpg#xywh=160,0,160,90\n`;
    const cues = parseTrickplay(vtt, '/api/v1/files/3/trickplay/index.vtt');
    expect(cues).toHaveLength(2);
    expect(cues[1]).toMatchObject({ start: 10, end: 20, url: '/api/v1/files/3/trickplay/sheet-0.jpg', x: 160, w: 160 });
    expect(cueAt(cues, 12)?.x).toBe(160);
    expect(cueAt(cues, 25)).toBeNull();
  });
});
