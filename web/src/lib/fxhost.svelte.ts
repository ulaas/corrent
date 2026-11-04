// One media element's link to sound processing and to the volume. With processing off (the default)
// it only sets the element's volume: no AudioContext, nothing of the graph loaded. The graph is built
// on the first play once processing is on, and stays with the element after that.
import { perceptual, prefs } from './prefs.svelte';
import type { FxChain, Reading } from './audiofx';

export class FxHost {
  chain = $state.raw<FxChain | null>(null);
  reading = $state.raw<Reading | null>(null);
  /** This source can't go through Web Audio, so processing is off for it. */
  bypassed = $state(false);
  private building = false;
  private disposed = false;
  private stopEffects: () => void;
  private waiting: (() => void) | null = null;

  constructor(
    readonly el: HTMLMediaElement,
    muted: () => boolean,
    private bypass: () => boolean,
  ) {
    el.addEventListener('play', this.onplay);
    el.addEventListener('loadstart', this.onsource);
    this.stopEffects = $effect.root(() => {
      $effect(() => {
        const level = perceptual(prefs.volume);
        const mute = muted();
        const c = this.chain;
        if (c) {
          // Element volume and the final gain would multiply.
          el.volume = 1;
          el.muted = false;
          c.setLevel(mute ? 0 : level);
        } else {
          el.volume = level;
          el.muted = mute;
        }
      });
      $effect(() => {
        const fx = $state.snapshot(prefs.audioFx);
        const c = this.chain;
        if (c) c.update(fx);
        else if (fx.enabled && !el.paused) void this.ensure();
      });
    });
  }

  private onplay = () => void this.ensure();
  private onsource = () => {
    this.reading = null;
    this.chain?.resetMeters();
  };

  private async ensure() {
    if (this.chain || this.building || this.disposed || !prefs.audioFx.enabled) return;
    if (this.bypass()) {
      this.bypassed = true;
      return;
    }
    this.bypassed = false;
    this.building = true;
    try {
      const { createChain } = await import('./audiofx');
      const chain = await createChain(this.el, (r) => (this.reading = r));
      if (this.disposed) return chain?.dispose();
      if (chain) {
        chain.update($state.snapshot(prefs.audioFx));
        this.chain = chain;
      } else this.onNextGesture();
    } catch (e) {
      console.warn('sound processing', e);
    } finally {
      this.building = false;
    }
  }

  /** The browser wants a gesture before audio starts: the element plays on as it is until then. */
  private onNextGesture() {
    if (this.waiting) return;
    const go = () => {
      this.clearWaiting();
      void this.ensure();
    };
    addEventListener('pointerdown', go, { once: true });
    addEventListener('keydown', go, { once: true });
    this.waiting = () => {
      removeEventListener('pointerdown', go);
      removeEventListener('keydown', go);
    };
  }

  private clearWaiting() {
    this.waiting?.();
    this.waiting = null;
  }

  dispose() {
    this.disposed = true;
    this.clearWaiting();
    this.el.removeEventListener('play', this.onplay);
    this.el.removeEventListener('loadstart', this.onsource);
    this.stopEffects();
    this.chain?.dispose();
    this.chain = null;
  }
}
