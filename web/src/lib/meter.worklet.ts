// The audio thread's end of lib/audiofx.ts: passes the sound through (with the Live leveller's gain
// when it's on) and measures what leaves. The math is in audiodsp.ts.
import { Leveller, LoudnessMeter } from './audiodsp';

declare const sampleRate: number;
declare function registerProcessor(name: string, ctor: unknown): void;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}

const POST_EVERY_S = 0.1;

class MeterProcessor extends AudioWorkletProcessor {
  private meter = new LoudnessMeter(sampleRate);
  private leveller = new Leveller(sampleRate);
  private live = false;
  private measuring = true;
  private since = 0;
  private stopped = false;
  /** Kept between calls: `process` runs hundreds of times a second and shouldn't allocate. */
  private channels: Float32Array[] = [];

  constructor() {
    super();
    this.port.onmessage = (e: MessageEvent) => {
      const d = e.data as { live?: boolean; measuring?: boolean; reset?: boolean; stop?: boolean };
      if (d.stop) this.stopped = true;
      if (d.live !== undefined) this.live = d.live;
      if (d.measuring !== undefined) this.measuring = d.measuring;
      if (d.reset) this.meter.reset();
    };
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    if (this.stopped) return false;
    const input = inputs[0];
    const output = outputs[0];
    if (!input?.length) return true;
    const frames = input[0].length;
    if (this.live) {
      this.leveller.process(input, output, frames);
    } else {
      for (let ch = 0; ch < input.length; ch++) output[ch]?.set(input[ch]);
    }
    if (this.measuring) {
      this.channels.length = 0;
      for (let ch = 0; ch < input.length && ch < output.length; ch++) this.channels.push(output[ch]);
      this.meter.push(this.channels, frames);
      this.since += frames / sampleRate;
      if (this.since >= POST_EVERY_S) {
        this.since = 0;
        this.port.postMessage({ ...this.meter.reading(), agc: this.live ? this.leveller.db : 0 });
      }
    }
    return true;
  }
}

registerProcessor('corrent-meter', MeterProcessor);
