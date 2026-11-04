// The math behind the audio meters, free of any Web Audio type so it can be tested with plain
// signals (tests/audiodsp.test.ts). meter.worklet.ts runs it on the audio thread.
//
// Loudness follows ITU-R BS.1770-4 and EBU Tech 3341/3342: K-weighting computed for the actual sample
// rate, momentary (400 ms) and short-term (3 s) windows, integrated loudness with the -70 LUFS and
// -10 LU gates over overlapping 400 ms blocks, loudness range with -70 and -20 LU gates between the
// 10th and 95th percentile, and true peak by 4x oversampling.

export interface Biquad {
  b0: number;
  b1: number;
  b2: number;
  a1: number;
  a2: number;
}

/** The two K-weighting stages for a sample rate (the analog prototypes behind BS.1770's 48 kHz
 *  coefficients, through the bilinear transform; at 48 kHz this reproduces the published values). */
export function kWeighting(fs: number): [Biquad, Biquad] {
  let f0 = 1681.974450955533;
  const G = 3.999843853973347;
  let Q = 0.7071752369554196;
  let K = Math.tan((Math.PI * f0) / fs);
  const Vh = Math.pow(10, G / 20);
  const Vb = Math.pow(Vh, 0.4996667741545416);
  let a0 = 1 + K / Q + K * K;
  const shelf: Biquad = {
    b0: (Vh + (Vb * K) / Q + K * K) / a0,
    b1: (2 * (K * K - Vh)) / a0,
    b2: (Vh - (Vb * K) / Q + K * K) / a0,
    a1: (2 * (K * K - 1)) / a0,
    a2: (1 - K / Q + K * K) / a0,
  };
  f0 = 38.13547087602444;
  Q = 0.5003270373238773;
  K = Math.tan((Math.PI * f0) / fs);
  a0 = 1 + K / Q + K * K;
  const high: Biquad = { b0: 1, b1: -2, b2: 1, a1: (2 * (K * K - 1)) / a0, a2: (1 - K / Q + K * K) / a0 };
  return [shelf, high];
}

/** The 4x interpolating filter (BS.1770 asks for at least 4x and gives an example): a Hann-windowed
 *  sinc, 12 taps per phase, each phase scaled to unity gain at DC. Phase 0 is the sample itself. */
export const TRUE_PEAK_PHASES: readonly (readonly number[])[] = [0, 1, 2, 3].map((p) => {
  const taps = Array.from({ length: 12 }, (_, t) => {
    const n = t * 4 + p;
    const x = (n - 24) / 4;
    const sinc = x === 0 ? 1 : Math.sin(Math.PI * x) / (Math.PI * x);
    return sinc * (0.5 - 0.5 * Math.cos((2 * Math.PI * n) / 48));
  });
  const sum = taps.reduce((a, b) => a + b, 0);
  return taps.map((v) => v / sum);
});
const TP_TAPS = 12;

const toLufs = (energy: number) => (energy > 0 ? -0.691 + 10 * Math.log10(energy) : -Infinity);
const toDb = (linear: number) => (linear > 0 ? 20 * Math.log10(linear) : -Infinity);

/** BS.1770 channel weights in Web Audio's 5.1 order (L R C LFE SL SR); the LFE doesn't count. */
function channelWeight(ch: number, channels: number): number {
  if (channels !== 6) return 1;
  return ch === 3 ? 0 : ch >= 4 ? 1.41 : 1;
}

const ABS_GATE = -70;
const BIN_LU = 0.1;
const BINS = Math.ceil(90 / BIN_LU);

/** Loudness values above the absolute gate, in bins: bounded memory however long the film, and
 *  each bin keeps its summed energy, so means stay exact. */
class LoudnessHistogram {
  counts = new Float64Array(BINS);
  energy = new Float64Array(BINS);
  n = 0;
  sum = 0;

  add(e: number) {
    const l = toLufs(e);
    if (l <= ABS_GATE) return;
    const bin = Math.min(BINS - 1, Math.floor((l - ABS_GATE) / BIN_LU));
    this.counts[bin]++;
    this.energy[bin] += e;
    this.n++;
    this.sum += e;
  }

  /** The loudness `lu` below the mean of everything above the absolute gate. */
  relativeGate(lu: number): number {
    return toLufs(this.sum / this.n) + lu;
  }

  /** Integrated loudness: the mean of the blocks above the relative gate. */
  gatedMean(lu: number): number {
    if (!this.n) return -Infinity;
    const gate = this.relativeGate(lu);
    let e = 0;
    let c = 0;
    for (let i = 0; i < BINS; i++) {
      if (!this.counts[i] || toLufs(this.energy[i] / this.counts[i]) <= gate) continue;
      e += this.energy[i];
      c += this.counts[i];
    }
    return c ? toLufs(e / c) : -Infinity;
  }

  /** The spread between the 10th and the 95th percentile above the relative gate. */
  range(lu: number): number {
    if (!this.n) return 0;
    const gate = this.relativeGate(lu);
    let total = 0;
    let firstBin = 0;
    for (let i = BINS - 1; i >= 0; i--) {
      if (!this.counts[i] || toLufs(this.energy[i] / this.counts[i]) <= gate) continue;
      total += this.counts[i];
      firstBin = i;
    }
    if (total < 2) return 0;
    const lowAt = 0.1 * total;
    const highAt = 0.95 * total;
    let seen = 0;
    let low = NaN;
    let high = NaN;
    for (let i = firstBin; i < BINS; i++) {
      seen += this.counts[i];
      if (isNaN(low) && seen >= lowAt) low = i;
      if (isNaN(high) && seen >= highAt) high = i;
    }
    return (high - low) * BIN_LU;
  }
}

export interface MeterReading {
  /** LUFS; -Infinity until a window has filled or in silence. */
  m: number;
  s: number;
  i: number;
  /** LU. */
  lra: number;
  /** dBTP per channel, held for 3 s. */
  tp: number[];
  /** dBFS per channel, over 300 ms. */
  rms: number[];
  /** -1..1 between the first two channels; null unless there are at least two. */
  corr: number | null;
}

const SUBS_SHORT = 30; // 3 s of 100 ms
const SUBS_MOMENTARY = 4;
const SUBS_RMS = 3;

/** Measures what goes through it, per block of any length. */
export class LoudnessMeter {
  private channels = 0;
  private subLen: number;
  private coef: [Biquad, Biquad];
  /** Per channel: [shelf z1, z2, high z1, z2]. */
  private state: Float64Array[] = [];
  private tpHist: Float64Array[] = [];
  private tpAt = 0;
  private filled = 0;
  private kSum: number[] = [];
  private rawSum: number[] = [];
  private subPeak: number[] = [];
  private crossSum = 0;
  // Rings of finished 100 ms pieces.
  private subs = 0;
  private energyRing = new Float64Array(SUBS_SHORT);
  private rawRing: Float64Array[] = [];
  private peakRing: Float64Array[] = [];
  private lrRing = new Float64Array(SUBS_MOMENTARY);
  private llRing = new Float64Array(SUBS_MOMENTARY);
  private rrRing = new Float64Array(SUBS_MOMENTARY);
  private blocks = new LoudnessHistogram();
  private shorts = new LoudnessHistogram();

  constructor(readonly fs: number) {
    this.subLen = Math.round(fs * 0.1);
    this.coef = kWeighting(fs);
  }

  /** Starts over (a new source). */
  reset() {
    this.setChannels(this.channels);
  }

  private setChannels(n: number) {
    this.channels = n;
    this.state = Array.from({ length: n }, () => new Float64Array(4));
    this.tpHist = Array.from({ length: n }, () => new Float64Array(TP_TAPS));
    this.tpAt = 0;
    this.filled = 0;
    this.kSum = new Array(n).fill(0);
    this.rawSum = new Array(n).fill(0);
    this.subPeak = new Array(n).fill(0);
    this.crossSum = 0;
    this.subs = 0;
    this.energyRing.fill(0);
    this.rawRing = Array.from({ length: n }, () => new Float64Array(SUBS_RMS));
    this.peakRing = Array.from({ length: n }, () => new Float64Array(SUBS_SHORT));
    this.lrRing.fill(0);
    this.llRing.fill(0);
    this.rrRing.fill(0);
    this.blocks = new LoudnessHistogram();
    this.shorts = new LoudnessHistogram();
  }

  /** Takes `frames` samples of each channel. */
  push(input: ArrayLike<number>[], frames: number) {
    if (input.length !== this.channels) this.setChannels(input.length);
    let at = 0;
    while (at < frames) {
      const n = Math.min(frames - at, this.subLen - this.filled);
      for (let ch = 0; ch < this.channels; ch++) this.channel(ch, input[ch], at, n);
      if (this.channels >= 2) {
        let lr = 0;
        const l = input[0];
        const r = input[1];
        for (let i = at; i < at + n; i++) lr += l[i] * r[i];
        this.crossSum += lr;
      }
      this.tpAt = (this.tpAt + n) % TP_TAPS;
      this.filled += n;
      at += n;
      if (this.filled === this.subLen) this.endSub();
    }
  }

  private channel(ch: number, x: ArrayLike<number>, from: number, n: number) {
    const [a, b] = this.coef;
    const st = this.state[ch];
    let z1 = st[0], z2 = st[1], z3 = st[2], z4 = st[3];
    let k = 0;
    let raw = 0;
    let peak = this.subPeak[ch];
    const hist = this.tpHist[ch];
    let pos = this.tpAt;
    for (let i = from; i < from + n; i++) {
      const s = x[i];
      // Transposed direct form II, both stages.
      const y1 = a.b0 * s + z1;
      z1 = a.b1 * s - a.a1 * y1 + z2;
      z2 = a.b2 * s - a.a2 * y1;
      const y2 = b.b0 * y1 + z3;
      z3 = b.b1 * y1 - b.a1 * y2 + z4;
      z4 = b.b2 * y1 - b.a2 * y2;
      k += y2 * y2;
      raw += s * s;
      // True peak: the four interpolated values around this sample.
      hist[pos] = s;
      for (let p = 0; p < 4; p++) {
        const taps = TRUE_PEAK_PHASES[p];
        let v = 0;
        for (let t = 0; t < TP_TAPS; t++) v += taps[t] * hist[(pos - t + TP_TAPS) % TP_TAPS];
        const abs = v < 0 ? -v : v;
        if (abs > peak) peak = abs;
      }
      pos = pos + 1 === TP_TAPS ? 0 : pos + 1;
    }
    st[0] = z1;
    st[1] = z2;
    st[2] = z3;
    st[3] = z4;
    this.kSum[ch] += k;
    this.rawSum[ch] += raw;
    this.subPeak[ch] = peak;
  }

  private endSub() {
    let e = 0;
    const at = this.subs;
    for (let ch = 0; ch < this.channels; ch++) {
      e += channelWeight(ch, this.channels) * (this.kSum[ch] / this.subLen);
      this.rawRing[ch][at % SUBS_RMS] = this.rawSum[ch] / this.subLen;
      this.peakRing[ch][at % SUBS_SHORT] = this.subPeak[ch];
      this.kSum[ch] = 0;
      this.rawSum[ch] = 0;
      this.subPeak[ch] = 0;
    }
    this.energyRing[at % SUBS_SHORT] = e;
    if (this.channels >= 2) {
      this.lrRing[at % SUBS_MOMENTARY] = this.crossSum / this.subLen;
      this.llRing[at % SUBS_MOMENTARY] = this.rawRing[0][at % SUBS_RMS];
      this.rrRing[at % SUBS_MOMENTARY] = this.rawRing[1][at % SUBS_RMS];
    }
    this.crossSum = 0;
    this.filled = 0;
    this.subs++;
    // A 400 ms block every 100 ms (75 % overlap), a 3 s window every 100 ms.
    if (this.subs >= SUBS_MOMENTARY) this.blocks.add(this.windowEnergy(SUBS_MOMENTARY));
    if (this.subs >= SUBS_SHORT) this.shorts.add(this.windowEnergy(SUBS_SHORT));
  }

  private windowEnergy(subs: number): number {
    let e = 0;
    for (let i = 1; i <= subs; i++) e += this.energyRing[(this.subs - i) % SUBS_SHORT];
    return e / subs;
  }

  reading(): MeterReading {
    const n = this.channels;
    const tp: number[] = [];
    const rms: number[] = [];
    for (let ch = 0; ch < n; ch++) {
      tp.push(toDb(Math.max(...this.peakRing[ch], this.subPeak[ch])));
      let r = 0;
      for (const x of this.rawRing[ch]) r += x;
      rms.push(toDb(Math.sqrt(r / SUBS_RMS)));
    }
    let corr: number | null = null;
    if (n >= 2) {
      let lr = 0, ll = 0, rr = 0;
      for (let i = 0; i < SUBS_MOMENTARY; i++) {
        lr += this.lrRing[i];
        ll += this.llRing[i];
        rr += this.rrRing[i];
      }
      corr = ll > 0 && rr > 0 ? lr / Math.sqrt(ll * rr) : 0;
    }
    return {
      m: this.subs >= SUBS_MOMENTARY ? toLufs(this.windowEnergy(SUBS_MOMENTARY)) : -Infinity,
      s: this.subs >= SUBS_SHORT ? toLufs(this.windowEnergy(SUBS_SHORT)) : -Infinity,
      i: this.blocks.gatedMean(-10),
      lra: this.shorts.range(-20),
      tp,
      rms,
      corr,
    };
  }
}

/** Live loudness: a slow gain toward -18 LUFS, smoothed over about half a second (the web's
 *  counterpart of the desktop's `dynaudnorm`). Silence doesn't pump it up, and a block never leaves
 *  above 0.98 whatever the gain. */
export class Leveller {
  static readonly TARGET = -18;
  static readonly MAX_BOOST_DB = 15;
  static readonly MAX_CUT_DB = 12;
  /** Below this nothing is measured as programme. */
  static readonly SILENCE = -50;

  private coef: [Biquad, Biquad];
  private state: Float64Array[] = [];
  private level = 0;
  private gainDb = 0;
  private gain = 1;

  constructor(readonly fs: number) {
    this.coef = kWeighting(fs);
  }

  /** The gain now, in dB. */
  get db(): number {
    return toDb(this.gain);
  }

  /** Copies input to output with the gain applied. */
  process(input: ArrayLike<number>[], output: Float32Array[], frames: number) {
    const dt = frames / this.fs;
    while (this.state.length < input.length) this.state.push(new Float64Array(4));
    const [a, b] = this.coef;
    let e = 0;
    let peak = 0;
    for (let ch = 0; ch < input.length; ch++) {
      const x = input[ch];
      const st = this.state[ch];
      let z1 = st[0], z2 = st[1], z3 = st[2], z4 = st[3];
      let sum = 0;
      for (let i = 0; i < frames; i++) {
        const s = x[i];
        const abs = s < 0 ? -s : s;
        if (abs > peak) peak = abs;
        const y1 = a.b0 * s + z1;
        z1 = a.b1 * s - a.a1 * y1 + z2;
        z2 = a.b2 * s - a.a2 * y1;
        const y2 = b.b0 * y1 + z3;
        z3 = b.b1 * y1 - b.a1 * y2 + z4;
        z4 = b.b2 * y1 - b.a2 * y2;
        sum += y2 * y2;
      }
      st[0] = z1;
      st[1] = z2;
      st[2] = z3;
      st[3] = z4;
      e += input.length === 6 ? channelWeight(ch, 6) * (sum / frames) : sum / frames;
    }
    // About a momentary window of level, half a second of gain.
    this.level += (e - this.level) * (1 - Math.exp(-dt / 0.2));
    const l = toLufs(this.level);
    if (l > Leveller.SILENCE) {
      const want = Math.max(-Leveller.MAX_CUT_DB, Math.min(Leveller.MAX_BOOST_DB, Leveller.TARGET - l));
      this.gainDb += (want - this.gainDb) * (1 - Math.exp(-dt / 0.5));
    }
    let next = Math.pow(10, this.gainDb / 20);
    let from = this.gain;
    if (peak > 0) {
      const ceiling = 0.98 / peak;
      if (next > ceiling) next = ceiling;
      if (from > ceiling) from = ceiling;
    }
    const step = (next - from) / frames;
    for (let ch = 0; ch < input.length; ch++) {
      const x = input[ch];
      const y = output[ch];
      if (!y) continue;
      let g = from;
      for (let i = 0; i < frames; i++) {
        g += step;
        y[i] = x[i] * g;
      }
    }
    this.gain = next;
  }
}
