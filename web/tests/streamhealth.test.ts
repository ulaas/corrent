import { describe, expect, it } from 'vitest';
import { bandwidth, health, type HealthInput, HISTORY, pushSample, sparkPoints } from '../src/lib/streamhealth';

const base: HealthInput = { produced: 10, idle_secs: 0, encoding: true, speed: 2, ahead_secs: 3 };
const of = (x: Partial<HealthInput>) => health({ ...base, ...x });

describe('stream health', () => {
  it('is smooth with enough ahead of the player, however slow the encoder', () => {
    expect(of({ ahead_secs: 8, speed: 0.5 })).toBe('smooth');
    expect(of({ ahead_secs: 40, encoding: false, speed: 0 })).toBe('smooth');
    expect(of({ ahead_secs: 8, speed: 0 })).toBe('smooth');
  });

  it('is smooth while the encoder is clearly faster than real time', () => {
    expect(of({ speed: 1.1, ahead_secs: 0 })).toBe('smooth');
    expect(of({ speed: 3.2, ahead_secs: 2 })).toBe('smooth');
  });

  it('is barely keeping up around real time with little ahead', () => {
    expect(of({ speed: 0.95 })).toBe('barely');
    expect(of({ speed: 1.0 })).toBe('barely');
    expect(of({ speed: 1.09, ahead_secs: 7.9 })).toBe('barely');
  });

  it('is too slow below real time with little ahead', () => {
    expect(of({ speed: 0.94 })).toBe('slow');
    expect(of({ speed: 0.1, ahead_secs: 0 })).toBe('slow');
  });

  it('is starting before anything is made, or while the encoder has no speed yet', () => {
    expect(of({ produced: 0, speed: 0, ahead_secs: 0 })).toBe('starting');
    expect(of({ produced: 0, speed: 5, ahead_secs: 30 })).toBe('starting');
    expect(of({ speed: 0, ahead_secs: 2 })).toBe('starting');
  });

  it('waits for the player when the encoder rests and nothing was fetched for a while', () => {
    expect(of({ encoding: false, speed: 0, idle_secs: 11, ahead_secs: 60 })).toBe('waiting');
    expect(of({ encoding: false, speed: 0, idle_secs: 11, ahead_secs: 0 })).toBe('waiting');
    expect(of({ encoding: false, speed: 0, idle_secs: 10, ahead_secs: 60 })).toBe('smooth');
    // Still encoding: the player's silence alone says nothing.
    expect(of({ encoding: true, speed: 2, idle_secs: 30 })).toBe('smooth');
  });

  it('is smooth when the encoder is done near the end of the file', () => {
    expect(of({ encoding: false, speed: 0, idle_secs: 2, ahead_secs: 3 })).toBe('smooth');
  });
});

describe('bandwidth between readings', () => {
  it('is the bits sent per second', () => {
    expect(bandwidth({ bytes: 1000, at: 0 }, { bytes: 1_001_000, at: 2000 })).toBe(4_000_000);
  });

  it('is nothing without an earlier reading, time, or a growing counter', () => {
    expect(bandwidth(undefined, { bytes: 5000, at: 2000 })).toBe(0);
    expect(bandwidth({ bytes: 1000, at: 2000 }, { bytes: 9000, at: 2000 })).toBe(0);
    expect(bandwidth({ bytes: 9000, at: 0 }, { bytes: 1000, at: 2000 })).toBe(0);
  });
});

describe('sparkline history', () => {
  it('keeps the newest samples', () => {
    let list: number[] = [];
    for (let i = 0; i < HISTORY + 5; i++) list = pushSample(list, i);
    expect(list.length).toBe(HISTORY);
    expect(list[0]).toBe(5);
    expect(list[list.length - 1]).toBe(HISTORY + 4);
    expect(pushSample([1, 2, 3], 4, 3)).toEqual([2, 3, 4]);
  });

  it('draws the newest sample at the right edge', () => {
    expect(sparkPoints([1], 10, 100, 20)).toBe('');
    expect(sparkPoints([0, 5, 10], 10, 100, 20, 3)).toBe('0.0,20.0 50.0,10.0 100.0,0.0');
    expect(sparkPoints([5, 10], 10, 100, 20, 3)).toBe('50.0,10.0 100.0,0.0');
    // Above the scale or without one: clamped, flat.
    expect(sparkPoints([20, 0], 10, 100, 20, 2)).toBe('0.0,0.0 100.0,20.0');
    expect(sparkPoints([3, 4], 0, 100, 20, 2)).toBe('0.0,20.0 100.0,20.0');
  });
});
