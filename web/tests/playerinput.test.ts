import { describe, expect, it } from 'vitest';
import { DOUBLE_TAP_MS, TapTracker, mediaAction, moveInRows } from '../src/lib/playerinput';

describe('taps', () => {
  it('counts two quick taps on one half as a double tap, and keeps counting', () => {
    const t = new TapTracker();
    expect(t.tap(1000, 1)).toBe('single');
    expect(t.tap(1000 + DOUBLE_TAP_MS - 1, 1)).toBe('double');
    expect(t.tap(1000 + 2 * DOUBLE_TAP_MS - 2, 1)).toBe('double');
  });
  it('takes the other half or a slow tap as a new single tap', () => {
    const t = new TapTracker();
    t.tap(0, 1);
    expect(t.tap(50, -1)).toBe('single');
    expect(t.tap(50 + DOUBLE_TAP_MS, -1)).toBe('single');
  });
});

describe('D-pad focus', () => {
  const rows = [1, 1, 6];
  it('walks along a row and stops at its ends', () => {
    expect(moveInRows(rows, 2, 3, 'left')).toEqual([2, 2]);
    expect(moveInRows(rows, 2, 0, 'left')).toBeNull();
    expect(moveInRows(rows, 2, 5, 'right')).toBeNull();
  });
  it('moves between rows, keeping the column where it can', () => {
    expect(moveInRows(rows, 2, 4, 'up')).toEqual([1, 0]);
    expect(moveInRows(rows, 1, 0, 'down')).toEqual([2, 0]);
    expect(moveInRows(rows, 0, 0, 'up')).toBeNull();
    expect(moveInRows(rows, 2, 0, 'down')).toBeNull();
  });
  it('passes over empty rows', () => {
    expect(moveInRows([1, 0, 3], 2, 2, 'up')).toEqual([0, 0]);
  });
});

describe('media keys', () => {
  it('maps the keys and the TV back codes', () => {
    expect(mediaAction('mediaplaypause', 0)).toBe('toggle');
    expect(mediaAction('mediatracknext', 0)).toBe('next');
    expect(mediaAction('', 461)).toBe('back');
    expect(mediaAction('', 10009)).toBe('back');
    expect(mediaAction('a', 65)).toBeNull();
  });
});
