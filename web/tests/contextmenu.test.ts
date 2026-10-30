import { describe, expect, it } from 'vitest';
import { EDGE, jump, place, placeSub, step } from '../src/lib/menuplace';
import { tidy, type MenuItem } from '../src/lib/contextmenu.svelte';

describe('context menu placement', () => {
  it('opens right and down from the pointer when it fits', () => {
    expect(place(100, 100, 200, 300, 1000, 800)).toEqual({ left: 100, top: 100 });
  });
  it('flips left and up near the edges', () => {
    expect(place(950, 700, 200, 300, 1000, 800)).toEqual({ left: 750, top: 400 });
  });
  it('clamps when it fits neither way', () => {
    expect(place(100, 300, 200, 780, 1000, 800)).toEqual({ left: 100, top: EDGE });
    expect(place(10, 10, 2000, 100, 1000, 800).left).toBe(EDGE);
  });
  it('puts submenus beside their row, on the left when the right is full', () => {
    expect(placeSub({ left: 100, right: 300, top: 200 }, 200, 100, 1000, 800)).toEqual({ left: 296, top: 194 });
    expect(placeSub({ left: 700, right: 900, top: 200 }, 200, 100, 1000, 800)).toEqual({ left: 504, top: 194 });
    expect(placeSub({ left: 100, right: 300, top: 750 }, 200, 100, 1000, 800).top).toBe(800 - EDGE - 100);
  });
});

describe('context menu keys', () => {
  it('skips disabled rows and wraps', () => {
    const on = [true, false, true, false];
    expect(step(on, 0, 1)).toBe(2);
    expect(step(on, 2, 1)).toBe(0);
    expect(step(on, 0, -1)).toBe(2);
    expect(step(on, -1, 1)).toBe(0);
    expect(step([false, false], 0, 1)).toBe(-1);
  });
  it('jumps to the next row starting with a letter', () => {
    const labels = ['Pause', null, 'Play', 'Remove'];
    expect(jump(labels, 0, 'p')).toBe(2);
    expect(jump(labels, 2, 'P')).toBe(0);
    expect(jump(labels, 0, 'z')).toBe(-1);
  });
});

describe('menu rows', () => {
  const item = (label: string): MenuItem => ({ label, onselect: () => {} });
  it('drops stray separators and empty headers', () => {
    const rows = tidy([{ kind: 'sep' }, item('A'), { kind: 'sep' }, { kind: 'sep' }, { kind: 'header', label: 'H' }, { kind: 'sep' }, item('B'), { kind: 'sep' }]);
    expect(rows.map((r) => ('label' in r ? r.label : r.kind))).toEqual(['A', 'sep', 'B']);
    expect(tidy([item('A'), { kind: 'header', label: 'H' }])).toHaveLength(1);
    expect(tidy([{ kind: 'header', label: 'H' }, item('A')])).toHaveLength(2);
  });
});
