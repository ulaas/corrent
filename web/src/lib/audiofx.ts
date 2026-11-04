// The Web Audio graph behind sound processing, loaded only once someone turned it on:
//
//   element -> headroom -> EQ bands, dialogue -> compressor, makeup, limiter (night)
//           -> crossfeed -> analyser -> meter worklet (meters, Live leveller) -> volume -> speakers
//
// Stages that do nothing aren't connected. `createMediaElementSource` takes the element's sound for
// good, so turning everything off leaves source -> volume, never an unrouted element.
import { compressorAutoMakeupDb, DIALOGUE_BOOST_DB, DIALOGUE_BOOST_HZ, DIALOGUE_CUT_DB, DIALOGUE_CUT_HZ, hasEq, headroomDb, type AudioFx } from './audiofxmodel';
import type { MeterReading } from './audiodsp';
import workletUrl from './meter.worklet.ts?worker&url';

export interface Reading extends MeterReading {
  /** dB taken away right now by the compressor and the limiter, and given by the leveller (agc). */
  reduction: number;
  agc: number;
}

const gainOf = (db: number) => Math.pow(10, db / 20);
const LIMIT_THRESHOLD_DB = -1;
const LIMIT_RATIO = 20;
const SMOOTH_S = 0.02;

let shared: AudioContext | null = null;
let workletReady: Promise<void> | null = null;

/** A graph on `el`, or null when the browser won't start audio yet (no gesture so far): the element
 *  keeps playing by itself and nothing has been taken from it. */
export async function createChain(el: HTMLMediaElement, onReading: (r: Reading) => void): Promise<FxChain | null> {
  shared ??= new AudioContext();
  const ctx = shared;
  if (ctx.state !== 'running') {
    await Promise.race([ctx.resume().catch(() => {}), new Promise((f) => setTimeout(f, 400))]);
  }
  return ctx.state === 'running' ? new FxChain(el, ctx, onReading) : null;
}

type Stage = [AudioNode, AudioNode];

export class FxChain {
  readonly analyser: AnalyserNode;
  private source: MediaElementAudioSourceNode;
  private headroom: GainNode;
  private bands: BiquadFilterNode[] = [];
  private dialogueCut: BiquadFilterNode;
  private dialogueBoost: BiquadFilterNode;
  private comp: DynamicsCompressorNode;
  private compGain: GainNode;
  private limit: DynamicsCompressorNode;
  private limitGain: GainNode;
  private xfIn: GainNode;
  private xfOut: ChannelMergerNode;
  private shelf: BiquadFilterNode;
  private volume: GainNode;
  private worklet: AudioWorkletNode | null = null;
  private live = false;
  private wired: Stage[] = [];
  private wiredKey = '';
  private fx: AudioFx | null = null;
  /** The worklet module couldn't load (it says why in the console); the rest works without it. */
  failed = false;

  constructor(
    el: HTMLMediaElement,
    readonly ctx: AudioContext,
    private onReading: (r: Reading) => void,
  ) {
    this.source = ctx.createMediaElementSource(el);
    this.headroom = ctx.createGain();
    this.volume = ctx.createGain();
    this.dialogueCut = this.peaking(DIALOGUE_CUT_HZ, 1.2, DIALOGUE_CUT_DB);
    this.dialogueBoost = this.peaking(DIALOGUE_BOOST_HZ, 1, DIALOGUE_BOOST_DB);
    this.comp = ctx.createDynamicsCompressor();
    this.comp.knee.value = 6;
    this.comp.attack.value = 0.02;
    this.comp.release.value = 0.4;
    this.compGain = ctx.createGain();
    // The nearest Web Audio has to a limiter at -1 dB; it adds its own makeup too, so that is taken back.
    this.limit = ctx.createDynamicsCompressor();
    this.limit.threshold.value = LIMIT_THRESHOLD_DB;
    this.limit.knee.value = 0;
    this.limit.ratio.value = LIMIT_RATIO;
    this.limit.attack.value = 0.001;
    this.limit.release.value = 0.05;
    this.limitGain = ctx.createGain();
    this.limitGain.gain.value = gainOf(-compressorAutoMakeupDb(LIMIT_THRESHOLD_DB, LIMIT_RATIO));
    this.analyser = ctx.createAnalyser();
    this.analyser.fftSize = 4096;
    this.analyser.smoothingTimeConstant = 0.6;

    // FFmpeg's `crossfeed`: mid and side, with a low shelf on the side, so the bass narrows.
    this.xfIn = ctx.createGain();
    this.xfIn.channelCount = 2;
    this.xfIn.channelCountMode = 'explicit';
    this.xfIn.channelInterpretation = 'speakers';
    this.xfIn.gain.value = 0.9; // its level_in
    const split = ctx.createChannelSplitter(2);
    this.xfOut = ctx.createChannelMerger(2);
    this.shelf = ctx.createBiquadFilter();
    this.shelf.type = 'lowshelf';
    const part = (from: 0 | 1, gain: number, to: AudioNode) => {
      const g = ctx.createGain();
      g.gain.value = gain;
      split.connect(g, from);
      g.connect(to);
    };
    const mid = ctx.createGain();
    part(0, 0.5, mid);
    part(1, 0.5, mid);
    part(0, 0.5, this.shelf);
    part(1, -0.5, this.shelf);
    const invert = ctx.createGain();
    invert.gain.value = -1;
    this.xfIn.connect(split);
    mid.connect(this.xfOut, 0, 0);
    mid.connect(this.xfOut, 0, 1);
    this.shelf.connect(this.xfOut, 0, 0);
    this.shelf.connect(invert);
    invert.connect(this.xfOut, 0, 1);
    this.source.connect(this.volume);
    this.volume.connect(ctx.destination);
    this.wired = [[this.source, this.source], [this.volume, this.volume]];
    this.wiredKey = 'pass';
  }

  private peaking(hz: number, q: number, db: number): BiquadFilterNode {
    const b = this.ctx.createBiquadFilter();
    b.type = 'peaking';
    b.frequency.value = hz;
    b.Q.value = q;
    b.gain.value = db;
    return b;
  }

  private set(p: AudioParam, v: number) {
    p.setTargetAtTime(v, this.ctx.currentTime, SMOOTH_S);
  }

  /** The final gain (the element's own volume stays at 1 while this exists). */
  setLevel(v: number) {
    this.set(this.volume.gain, v);
  }

  /** Takes new settings without a click: values glide, stages only move when one comes or goes. */
  update(fx: AudioFx) {
    this.fx = fx;
    if (this.bands.length !== fx.eq.length) {
      this.wiredKey = '';
      this.unwire();
      this.bands = fx.eq.map((b) => this.peaking(b.hz, b.q, b.db));
    }
    fx.eq.forEach((b, i) => {
      const n = this.bands[i];
      this.set(n.frequency, Math.min(b.hz, this.ctx.sampleRate / 2));
      this.set(n.Q, b.q);
      this.set(n.gain, b.db);
    });
    this.set(this.headroom.gain, gainOf(headroomDb(fx)));
    if (fx.night) {
      const { threshold_db: t, ratio: r, makeup_db: m } = fx.night;
      this.set(this.comp.threshold, t);
      this.set(this.comp.ratio, r);
      this.set(this.compGain.gain, gainOf(m - compressorAutoMakeupDb(t, r)));
    }
    if (fx.crossfeed) {
      this.shelf.frequency.value = Math.max(20, (1 - fx.crossfeed.range) * 2100);
      this.set(this.shelf.gain, fx.crossfeed.strength * -30);
    }
    this.live = fx.enabled && fx.loudness === 'live';
    const metered = fx.enabled && (fx.meters || this.live);
    this.worklet?.port.postMessage({ live: this.live, measuring: fx.enabled && fx.meters });
    if (metered && !this.worklet && !this.failed) void this.loadWorklet();
    this.wire(fx, metered && !!this.worklet);
  }

  private async loadWorklet() {
    try {
      workletReady ??= this.ctx.audioWorklet.addModule(workletUrl);
      await workletReady;
      if (this.worklet) return;
      this.worklet = new AudioWorkletNode(this.ctx, 'corrent-meter');
      this.worklet.port.onmessage = (e) => {
        const r = e.data as MeterReading & { agc: number };
        this.onReading({ ...r, reduction: this.comp.reduction + this.limit.reduction });
      };
      this.wiredKey = '';
      if (this.fx) this.update(this.fx);
    } catch (e) {
      console.warn('audio meters', e);
      workletReady = null;
      this.failed = true;
    }
  }

  private unwire() {
    for (const [, out] of this.wired) out.disconnect();
    this.wired = [];
  }

  private wire(fx: AudioFx, metered: boolean) {
    const eq = fx.enabled && (hasEq(fx) || fx.dialogue);
    const night = fx.enabled && !!fx.night;
    const cross = fx.enabled && !!fx.crossfeed;
    const key = JSON.stringify([eq, fx.dialogue, night, cross, metered, this.bands.length]);
    if (key === this.wiredKey) return;
    this.wiredKey = key;
    this.unwire();
    const one = (n: AudioNode): Stage => [n, n];
    const chain: Stage[] = [one(this.source)];
    if (eq) {
      chain.push(one(this.headroom));
      for (const b of this.bands) chain.push(one(b));
      if (fx.dialogue) chain.push(one(this.dialogueCut), one(this.dialogueBoost));
    }
    if (night) chain.push(one(this.comp), one(this.compGain), one(this.limit), one(this.limitGain));
    // Its inner wiring stays: only what leaves the merger gets reconnected.
    if (cross) chain.push([this.xfIn, this.xfOut]);
    if (metered && this.worklet) chain.push(one(this.analyser), one(this.worklet));
    chain.push(one(this.volume));
    for (let i = 0; i + 1 < chain.length; i++) chain[i][1].connect(chain[i + 1][0]);
    this.volume.connect(this.ctx.destination);
    this.wired = chain;
  }

  /** A new source: integrated loudness and the range start over. */
  resetMeters() {
    this.worklet?.port.postMessage({ reset: true });
  }

  /** What Web Audio adds in front of the speakers, in ms (the browser doesn't compensate lip sync). */
  get latencyMs(): number {
    const c = this.ctx as AudioContext & { outputLatency?: number };
    return ((c.baseLatency ?? 0) + (c.outputLatency ?? 0)) * 1000;
  }

  get channels(): number {
    return this.ctx.destination.channelCount;
  }

  dispose() {
    this.unwire();
    this.volume.disconnect();
    if (this.worklet) this.worklet.port.onmessage = null;
    this.worklet?.disconnect();
  }
}
