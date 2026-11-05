<script lang="ts">
  // The player's overlay, two pages that can show together: A/V (I) and the audio meters (Shift+I).
  import { t } from '../lib/i18n.svelte';
  import { prefs } from '../lib/prefs.svelte';
  import type { FxHost } from '../lib/fxhost.svelte';
  import type { HlsStats, PlaybackInfo } from '../lib/player.svelte';

  let {
    video = null,
    info = null,
    fx,
    av = false,
    audio = false,
    hls = null,
  }: {
    video?: HTMLVideoElement | null;
    info?: PlaybackInfo | null;
    fx: FxHost | null;
    av?: boolean;
    audio?: boolean;
    hls?: HlsStats | null;
  } = $props();

  // ---- A/V ----
  let lines = $state<[string, string][]>([]);
  $effect(() => {
    if (!av || !video) return;
    const el = video as HTMLVideoElement & {
      requestVideoFrameCallback?: (f: (now: number, m: { presentedFrames: number; mediaTime: number }) => void) => number;
      cancelVideoFrameCallback?: (h: number) => void;
    };
    // Presented frames and how far the shown frame is from the audio clock.
    let frames = 0;
    let offset = NaN;
    let lastFrames = 0;
    let lastAt = performance.now();
    let fps = NaN;
    let handle = 0;
    const onFrame = (_now: number, m: { presentedFrames: number; mediaTime: number }) => {
      frames = m.presentedFrames;
      offset = (m.mediaTime - el.currentTime) * 1000;
      handle = el.requestVideoFrameCallback!(onFrame);
    };
    if (el.requestVideoFrameCallback) handle = el.requestVideoFrameCallback(onFrame);
    const tick = () => {
      const now = performance.now();
      if (frames) fps = ((frames - lastFrames) * 1000) / Math.max(1, now - lastAt);
      lastFrames = frames;
      lastAt = now;
      const q = el.getVideoPlaybackQuality?.();
      let ahead = 0;
      for (let i = 0; i < el.buffered.length; i++) {
        if (el.buffered.start(i) <= el.currentTime && el.currentTime <= el.buffered.end(i)) ahead = el.buffered.end(i) - el.currentTime;
      }
      const out: [string, string][] = [
        [t('Video'), `${el.videoWidth}×${el.videoHeight}${info?.method ? ` · ${info.method}` : ''}`],
      ];
      if (isFinite(fps)) out.push([t('Presented'), t('{n} fps', { n: fps.toFixed(1) })]);
      if (q) out.push([t('Dropped'), t('{dropped}/{total} frames dropped', { dropped: q.droppedVideoFrames, total: q.totalVideoFrames })]);
      out.push([t('Buffered'), t('{s} s buffered', { s: ahead.toFixed(1) })]);
      if (isFinite(offset)) out.push([t('A/V offset'), `${offset.toFixed(0)} ms`]);
      if (hls) {
        out.push([t('Bitrate'), `${(hls.bps / 1e6).toFixed(2)} Mbit/s`]);
        out.push([t('Segment load'), `${hls.loadMs.toFixed(0)} ms`]);
      }
      const c = fx?.chain;
      if (c) out.push([t('Latency'), `${c.latencyMs.toFixed(0)} ms`]);
      lines = out;
    };
    tick();
    const timer = setInterval(tick, 1000);
    return () => {
      clearInterval(timer);
      if (handle) el.cancelVideoFrameCallback?.(handle);
    };
  });

  // ---- meters ----
  const r = $derived(fx?.reading ?? null);
  const fmt = (v: number | undefined, digits = 1) => (v === undefined || !isFinite(v) ? '–' : v.toFixed(digits));
  /** -60..0 dB as 0..100 %. */
  const pct = (db: number) => (isFinite(db) ? Math.max(0, Math.min(100, ((db + 60) / 60) * 100)) : 0);
  const channelName = (i: number, n: number) => (n === 2 ? ['L', 'R'][i] : n === 1 ? 'M' : (['L', 'R', 'C', 'LFE', 'SL', 'SR'][i] ?? String(i + 1)));

  // ---- spectrum (web only: the browser gives one, libmpv doesn't) ----
  let canvas: HTMLCanvasElement | undefined = $state();
  let peakHz = $state(0);
  $effect(() => {
    const c = fx?.chain;
    if (!audio || !c || !canvas) return;
    const cv = canvas;
    const a = c.analyser;
    const data = new Float32Array(a.frequencyBinCount);
    const g = cv.getContext('2d');
    if (!g) return;
    const dpr = devicePixelRatio || 1;
    cv.width = cv.clientWidth * dpr;
    cv.height = cv.clientHeight * dpr;
    const color = getComputedStyle(cv).getPropertyValue('--accent').trim() || '#4f8cff';
    const BARS = 64;
    const fs = c.ctx.sampleRate;
    const hzOf = (b: number) => 20 * Math.pow(1000, b / BARS);
    let frame = 0;
    const draw = () => {
      frame = requestAnimationFrame(draw);
      a.getFloatFrequencyData(data);
      let best = -Infinity;
      let bestBin = 0;
      for (let i = 1; i < data.length; i++) {
        if (data[i] > best) {
          best = data[i];
          bestBin = i;
        }
      }
      peakHz = best > -90 ? (bestBin * fs) / a.fftSize : 0;
      g.clearRect(0, 0, cv.width, cv.height);
      g.fillStyle = color;
      const w = cv.width / BARS;
      for (let b = 0; b < BARS; b++) {
        const from = Math.max(1, Math.floor((hzOf(b) * a.fftSize) / fs));
        const to = Math.max(from + 1, Math.floor((hzOf(b + 1) * a.fftSize) / fs));
        let m = -Infinity;
        for (let i = from; i < to && i < data.length; i++) if (data[i] > m) m = data[i];
        const h = Math.max(0, Math.min(1, (m + 90) / 90)) * cv.height;
        g.fillRect(b * w, cv.height - h, Math.max(1, w - 1 * dpr), h);
      }
    };
    draw();
    return () => cancelAnimationFrame(frame);
  });
</script>

{#if av || audio}
  <div class="stats num">
    {#if av}
      <dl>
        {#each lines as [k, v] (k)}<div><dt>{k}</dt><dd>{v}</dd></div>{/each}
      </dl>
    {/if}
    {#if audio}
      {#if !prefs.audioFx.enabled}
        <p class="note">{t('Sound processing is off. Turn it on to measure what you hear.')}</p>
        <button class="btn sm" onclick={() => (prefs.audioFx.enabled = true)}>{t('Turn on')}</button>
      {:else if fx?.bypassed}
        <p class="note">{t("This stream can't go through sound processing: it plays as it is.")}</p>
      {:else if !r}
        <p class="note">{fx?.chain?.failed ? t("The meters couldn't start in this browser.") : t('Measuring…')}</p>
      {:else}
        <dl class="lufs" data-m={r.m} data-s={r.s} data-i={r.i} data-lra={r.lra}>
          <div><dt>{t('Momentary')}</dt><dd>{fmt(r.m)} LUFS</dd></div>
          <div><dt>{t('Short-term')}</dt><dd>{fmt(r.s)} LUFS</dd></div>
          <div><dt>{t('Integrated')}</dt><dd>{fmt(r.i)} LUFS</dd></div>
          <div><dt>{t('Range')}</dt><dd>{fmt(r.lra)} LU</dd></div>
          <div><dt>{t('Gain reduction')}</dt><dd>{fmt(-r.reduction)} dB</dd></div>
          {#if r.agc}<div><dt>{t('Leveller')}</dt><dd>{r.agc > 0 ? '+' : ''}{fmt(r.agc)} dB</dd></div>{/if}
        </dl>
        <div class="bars">
          <div class="h"><span>{t('True peak')}</span></div>
          {#each r.tp as v, i (i)}
            <div class="bar" data-tp={v}><span class="c">{channelName(i, r.tp.length)}</span><span class="track"><i style:width="{pct(v)}%"></i><b style:left="{pct(-1)}%"></b></span><span class="v">{fmt(v)} dBTP</span></div>
          {/each}
          <div class="h"><span>RMS</span></div>
          {#each r.rms as v, i (i)}
            <div class="bar"><span class="c">{channelName(i, r.rms.length)}</span><span class="track"><i style:width="{pct(v)}%"></i></span><span class="v">{fmt(v)} dBFS</span></div>
          {/each}
          {#if r.corr !== null}
            <div class="h"><span>{t('Correlation')}</span></div>
            <div class="bar" data-corr={r.corr}><span class="c"></span><span class="track mid"><u style:left="{((r.corr + 1) / 2) * 100}%"></u></span><span class="v">{r.corr.toFixed(2)}</span></div>
          {/if}
        </div>
      {/if}
      {#if fx?.chain}
        <div class="spectrum">
          <div class="h"><span>{t('Spectrum')}</span><span data-peak-hz={peakHz}>{peakHz ? t('Peak {hz} Hz', { hz: Math.round(peakHz) }) : ''}</span></div>
          <canvas bind:this={canvas} aria-hidden="true"></canvas>
          <div class="out">{fx.chain.ctx.sampleRate} Hz · {t('{n} ch', { n: fx.chain.channels })} · {t('Latency')} {fx.chain.latencyMs.toFixed(0)} ms</div>
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .stats {
    width: min(320px, calc(100vw - 32px));
    max-height: calc(100dvh - 180px);
    overflow-y: auto;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    border-radius: var(--r-md);
    background: rgba(0, 0, 0, 0.72);
    color: #fff;
    font-size: 12px;
    line-height: 1.5;
  }
  dl {
    margin: 0;
  }
  dl div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  dt {
    color: rgba(255, 255, 255, 0.7);
  }
  dd {
    margin: 0;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .note {
    margin: 0;
    color: rgba(255, 255, 255, 0.8);
  }
  .bars {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .h {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    color: rgba(255, 255, 255, 0.7);
  }
  .bar {
    display: grid;
    grid-template-columns: 22px 1fr 76px;
    align-items: center;
    gap: 6px;
  }
  .c {
    color: rgba(255, 255, 255, 0.7);
  }
  .v {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .track {
    position: relative;
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.16);
    overflow: hidden;
  }
  .track i {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: 3px;
    background: var(--accent);
  }
  /* The -1 dBTP mark. */
  .track b {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: #fff;
  }
  .track.mid::before {
    content: '';
    position: absolute;
    left: 50%;
    top: 0;
    bottom: 0;
    width: 1px;
    background: rgba(255, 255, 255, 0.5);
  }
  .track u {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 4px;
    margin-left: -2px;
    border-radius: 2px;
    background: var(--accent);
    text-decoration: none;
  }
  canvas {
    display: block;
    width: 100%;
    height: 64px;
    margin-top: 4px;
  }
  .out {
    margin-top: 4px;
    color: rgba(255, 255, 255, 0.7);
  }
</style>
