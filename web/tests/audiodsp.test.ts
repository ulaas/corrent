import { describe, expect, it } from 'vitest';
import { kWeighting, Leveller, LoudnessMeter, TRUE_PEAK_PHASES } from '../src/lib/audiodsp';

// Known signals from EBU Tech 3341 (loudness metering) and 3342 (loudness range).
const dbfs = (db: number) => Math.pow(10, db / 20);

function sine(fs: number, seconds: number, db: number, hz = 1000, phase = 0): Float32Array {
  const out = new Float32Array(Math.round(fs * seconds));
  const a = dbfs(db);
  for (let i = 0; i < out.length; i++) out[i] = a * Math.sin((2 * Math.PI * hz * i) / fs + phase);
  return out;
}

/** Sends a signal in the render quanta of a worklet. */
function run(m: LoudnessMeter, channels: Float32Array[]) {
  const n = channels[0].length;
  for (let at = 0; at < n; at += 128) {
    const len = Math.min(128, n - at);
    m.push(channels.map((c) => c.subarray(at, at + len)), len);
  }
}

function stereoSegments(m: LoudnessMeter, segments: [number, number][]) {
  for (const [db, seconds] of segments) {
    const s = sine(m.fs, seconds, db);
    run(m, [s, s]);
  }
}

describe.each([48000, 44100, 96000])('BS.1770 loudness at %d Hz', (fs) => {
  it('Tech 3341 case 1: a stereo 1 kHz sine at -23 dBFS reads -23 LUFS', () => {
    const m = new LoudnessMeter(fs);
    stereoSegments(m, [[-23, 20]]);
    const r = m.reading();
    expect(Math.abs(r.m + 23)).toBeLessThan(0.1);
    expect(Math.abs(r.s + 23)).toBeLessThan(0.1);
    expect(Math.abs(r.i + 23)).toBeLessThan(0.1);
  });

  it('Tech 3341 case 2: at -33 dBFS reads -33 LUFS', () => {
    const m = new LoudnessMeter(fs);
    stereoSegments(m, [[-33, 20]]);
    expect(Math.abs(m.reading().i + 33)).toBeLessThan(0.1);
  });

  it('a 6 dB change reads 6 LU', () => {
    const a = new LoudnessMeter(fs);
    const b = new LoudnessMeter(fs);
    stereoSegments(a, [[-3, 5]]);
    stereoSegments(b, [[-9, 5]]);
    expect(Math.abs(a.reading().m - b.reading().m - 6)).toBeLessThan(0.01);
  });
});

describe('integrated loudness gating', () => {
  const fs = 48000;
  it('Tech 3341 case 3: quiet parts fall below the relative gate', () => {
    const m = new LoudnessMeter(fs);
    stereoSegments(m, [[-36, 10], [-23, 60], [-36, 10]]);
    expect(Math.abs(m.reading().i + 23)).toBeLessThan(0.1);
  });
  it('Tech 3341 case 4: silence-like parts fall below the absolute gate', () => {
    const m = new LoudnessMeter(fs);
    stereoSegments(m, [[-72, 10], [-36, 10], [-23, 60], [-36, 10], [-72, 10]]);
    expect(Math.abs(m.reading().i + 23)).toBeLessThan(0.1);
  });
  it('Tech 3341 case 5: only the gate decides', () => {
    const m = new LoudnessMeter(fs);
    stereoSegments(m, [[-26, 20], [-20, 20.1], [-26, 20]]);
    expect(Math.abs(m.reading().i + 23)).toBeLessThan(0.15);
  });
  it('starts over on reset', () => {
    const m = new LoudnessMeter(fs);
    stereoSegments(m, [[-10, 3]]);
    m.reset();
    expect(m.reading().i).toBe(-Infinity);
  });
  it('weighs 5.1: surrounds +1.5 dB, no LFE', () => {
    const m = new LoudnessMeter(fs);
    const s = sine(fs, 5, -23);
    const zero = new Float32Array(s.length);
    run(m, [zero, zero, s, zero, zero, zero]);
    const centre = m.reading().m;
    const surround = new LoudnessMeter(fs);
    run(surround, [zero, zero, zero, zero, s, zero]);
    expect(Math.abs(surround.reading().m - centre - 1.49)).toBeLessThan(0.02);
    const lfe = new LoudnessMeter(fs);
    run(lfe, [zero, zero, zero, s, zero, zero]);
    expect(lfe.reading().m).toBe(-Infinity);
  });
});

describe('loudness range (Tech 3342)', () => {
  const lra = (segments: [number, number][]) => {
    const m = new LoudnessMeter(48000);
    stereoSegments(m, segments);
    return m.reading().lra;
  };
  it('case 1: -20 then -30 LUFS is 10 LU', () => expect(Math.abs(lra([[-20, 20], [-30, 20]]) - 10)).toBeLessThan(1));
  it('case 2: -20 then -15 LUFS is 5 LU', () => expect(Math.abs(lra([[-20, 20], [-15, 20]]) - 5)).toBeLessThan(1));
  it('case 3: -40 then -20 LUFS is 20 LU', () => expect(Math.abs(lra([[-40, 20], [-20, 20]]) - 20)).toBeLessThan(1));
  it('case 4: -50, -35, -20, -35, -50 is 15 LU', () =>
    expect(Math.abs(lra([[-50, 20], [-35, 20], [-20, 20], [-35, 20], [-50, 20]]) - 15)).toBeLessThan(1));
});

describe('K-weighting', () => {
  it('reproduces the published 48 kHz coefficients', () => {
    const [shelf, high] = kWeighting(48000);
    expect(shelf.b0).toBeCloseTo(1.53512485958697, 9);
    expect(shelf.b1).toBeCloseTo(-2.69169618940638, 9);
    expect(shelf.b2).toBeCloseTo(1.19839281085285, 9);
    expect(shelf.a1).toBeCloseTo(-1.69065929318241, 9);
    expect(shelf.a2).toBeCloseTo(0.73248077421585, 9);
    expect(high.a1).toBeCloseTo(-1.99004745483398, 9);
    expect(high.a2).toBeCloseTo(0.99007225036621, 9);
  });
  it('differs at another rate and stays stable', () => {
    const [shelf] = kWeighting(44100);
    expect(shelf.b0).not.toBeCloseTo(1.53512485958697, 3);
    expect(Math.abs(shelf.a2)).toBeLessThan(1);
  });
});

describe('true peak', () => {
  const fs = 48000;
  it('each phase of the filter has unity gain at DC', () => {
    for (const p of TRUE_PEAK_PHASES) expect(Math.abs(p.reduce((a, b) => a + b, 0) - 1)).toBeLessThan(0.01);
  });
  it('finds the peak between samples: fs/4 at 45 degrees reads 3 dB over the samples', () => {
    const m = new LoudnessMeter(fs);
    const s = sine(fs, 1, -6, fs / 4, Math.PI / 4);
    run(m, [s, s]);
    const samplePeak = 20 * Math.log10(Math.max(...s.map(Math.abs)));
    expect(samplePeak).toBeCloseTo(-9.01, 1);
    // Tech 3341's tolerance is +0.2 / -0.4 dB.
    const tp = m.reading().tp[0];
    expect(tp).toBeGreaterThan(-6.02 - 0.4);
    expect(tp).toBeLessThan(-6.02 + 0.2);
  });
  it.each([6, 8, 12])('a sine at fs/%d reads its true level at any phase', (div) => {
    for (const phase of [0, 0.3, Math.PI / 4, 1.1]) {
      const m = new LoudnessMeter(fs);
      const s = sine(fs, 4, -6, fs / div, phase); // the 3 s hold has let go of the onset's ringing
      run(m, [s, s]);
      const tp = m.reading().tp[0];
      expect(tp).toBeGreaterThan(-6.02 - 0.4);
      expect(tp).toBeLessThan(-6.02 + 0.2);
    }
  });
  it('holds a peak for 3 seconds, then lets go', () => {
    const m = new LoudnessMeter(fs);
    const burst = sine(fs, 0.2, -1);
    const quiet = sine(fs, 2, -30);
    run(m, [burst, burst]);
    run(m, [quiet, quiet]);
    expect(m.reading().tp[0]).toBeGreaterThan(-2);
    const more = sine(fs, 1.5, -30);
    run(m, [more, more]);
    expect(m.reading().tp[0]).toBeLessThan(-25);
  });
});

describe('channels', () => {
  const fs = 48000;
  it('RMS of a sine is 3 dB below its peak', () => {
    const m = new LoudnessMeter(fs);
    const s = sine(fs, 1, -6);
    run(m, [s, s]);
    expect(m.reading().rms[0]).toBeCloseTo(-9.01, 1);
  });
  it('correlation: identical +1, inverted -1, quadrature about 0', () => {
    const corr = (a: Float32Array, b: Float32Array) => {
      const m = new LoudnessMeter(fs);
      run(m, [a, b]);
      return m.reading().corr!;
    };
    const s = sine(fs, 1, -6);
    expect(corr(s, s)).toBeCloseTo(1, 3);
    expect(corr(s, s.map((x) => -x))).toBeCloseTo(-1, 3);
    expect(Math.abs(corr(s, sine(fs, 1, -6, 1000, Math.PI / 2)))).toBeLessThan(0.02);
  });
  it('mono has no correlation', () => {
    const m = new LoudnessMeter(fs);
    run(m, [sine(fs, 1, -6)]);
    expect(m.reading().corr).toBeNull();
  });
});

describe('live leveller', () => {
  const fs = 48000;
  function through(l: Leveller, channels: Float32Array[]): Float32Array[] {
    const n = channels[0].length;
    const out = channels.map(() => new Float32Array(n));
    for (let at = 0; at < n; at += 128) {
      const len = Math.min(128, n - at);
      l.process(channels.map((c) => c.subarray(at, at + len)), out.map((c) => c.subarray(at, at + len)), len);
    }
    return out;
  }
  const measure = (out: Float32Array[]) => {
    const m = new LoudnessMeter(fs);
    run(m, out);
    return m.reading().m;
  };
  it('brings a quiet programme up to -18 LUFS', () => {
    const l = new Leveller(fs);
    const s = sine(fs, 6, -28);
    const out = through(l, [s, s]);
    expect(Math.abs(measure(out) + 18)).toBeLessThan(1);
  });
  it('brings a loud one down', () => {
    const l = new Leveller(fs);
    const s = sine(fs, 6, -9);
    expect(Math.abs(measure(through(l, [s, s])) + 18)).toBeLessThan(1);
  });
  it('does not boost silence or more than its limit', () => {
    const l = new Leveller(fs);
    const s = sine(fs, 3, -60);
    through(l, [s, s]);
    expect(l.db).toBeLessThan(0.01);
    const quiet = new Leveller(fs);
    const t = sine(fs, 8, -45);
    through(quiet, [t, t]);
    expect(quiet.db).toBeLessThanOrEqual(15.01);
    expect(quiet.db).toBeGreaterThan(14);
  });
  it('never leaves above the ceiling', () => {
    const l = new Leveller(fs);
    const quiet = sine(fs, 3, -40);
    through(l, [quiet, quiet]);
    const hit = sine(fs, 0.5, -3);
    const out = through(l, [hit, hit]);
    expect(Math.max(...out[0].map(Math.abs))).toBeLessThanOrEqual(0.981);
  });
});
