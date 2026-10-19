<script lang="ts">
  // Seek bar: drag or click, arrow keys, chapter marks, buffered ranges, a time tooltip and
  // optional trickplay thumbnails.
  import { clock } from '../lib/format';
  import type { Chapter } from '../lib/types';

  export interface Thumb {
    url: string;
    x: number;
    y: number;
    w: number;
    h: number;
  }

  let {
    value,
    max,
    onseek,
    label,
    chapters = [],
    buffered = [],
    thumb,
    big = false,
  }: {
    value: number;
    max: number;
    onseek: (secs: number) => void;
    label: string;
    chapters?: Chapter[];
    buffered?: [number, number][];
    thumb?: (secs: number) => Thumb | null;
    big?: boolean;
  } = $props();

  let el: HTMLDivElement | undefined = $state();
  let hover = $state<number | null>(null);
  let drag = $state<number | null>(null);

  const shown = $derived(drag ?? value);
  const pct = (v: number) => (max > 0 ? Math.max(0, Math.min(100, (v / max) * 100)) : 0);

  function at(e: PointerEvent): number {
    const r = el!.getBoundingClientRect();
    return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * max;
  }

  function down(e: PointerEvent) {
    if (!max) return;
    el!.setPointerCapture(e.pointerId);
    drag = at(e);
  }
  function move(e: PointerEvent) {
    if (!max) return;
    hover = at(e);
    if (drag != null) drag = hover;
  }
  function up() {
    if (drag != null) onseek(drag);
    drag = null;
  }
  function key(e: KeyboardEvent) {
    const step = e.shiftKey ? 30 : 5;
    if (e.key === 'ArrowRight') onseek(Math.min(max, value + step));
    else if (e.key === 'ArrowLeft') onseek(Math.max(0, value - step));
    else if (e.key === 'Home') onseek(0);
    else if (e.key === 'End') onseek(max);
    else return;
    e.preventDefault();
  }

  const chapterAt = (secs: number) => {
    let c: Chapter | undefined;
    for (const ch of chapters) if (ch.start_ms / 1000 <= secs) c = ch;
    return c?.title;
  };
  const tip = $derived(hover ?? drag);
  const tipThumb = $derived(tip != null && thumb ? thumb(tip) : null);
</script>

<div
  bind:this={el}
  class="seek"
  class:big
  class:dragging={drag != null}
  role="slider"
  tabindex="0"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={Math.round(max)}
  aria-valuenow={Math.round(shown)}
  aria-valuetext={`${clock(shown * 1000)} / ${clock(max * 1000)}`}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={() => (drag = null)}
  onpointerleave={() => (hover = null)}
  onkeydown={key}>
  <div class="track">
    {#each buffered as [a, b], i (i)}
      <div class="buf" style:left="{pct(a)}%" style:width="{pct(b) - pct(a)}%"></div>
    {/each}
    <div class="fill" style:width="{pct(shown)}%"></div>
    {#each chapters as c, i (i)}
      {#if c.start_ms > 0}<div class="mark" style:left="{pct(c.start_ms / 1000)}%"></div>{/if}
    {/each}
  </div>
  <div class="knob" style:left="{pct(shown)}%"></div>
  {#if tip != null && max > 0}
    <div class="tip" style:left="clamp(40px, {pct(tip)}%, calc(100% - 40px))">
      {#if tipThumb}
        <span
          class="thumb"
          style:width="{tipThumb.w}px"
          style:height="{tipThumb.h}px"
          style:background-image="url({tipThumb.url})"
          style:background-position="-{tipThumb.x}px -{tipThumb.y}px"></span>
      {/if}
      {#if chapterAt(tip)}<span class="ch">{chapterAt(tip)}</span>{/if}
      <span class="num">{clock(tip * 1000)}</span>
    </div>
  {/if}
</div>

<style>
  .seek {
    position: relative;
    height: 18px;
    display: flex;
    align-items: center;
    cursor: pointer;
    touch-action: none;
    border-radius: 4px;
  }
  .track {
    position: relative;
    width: 100%;
    height: 3px;
    background: color-mix(in srgb, var(--text) 16%, transparent);
    transition: height var(--dur-micro) var(--ease-standard);
    overflow: hidden;
  }
  .big .track {
    height: 4px;
    border-radius: 4px;
  }
  .seek:hover .track,
  .dragging .track {
    height: 6px;
  }
  .buf {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in srgb, var(--text) 22%, transparent);
  }
  .fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--accent);
  }
  .mark {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 3px;
    margin-left: -1px;
    background: var(--bg0);
  }
  .knob {
    position: absolute;
    top: 50%;
    width: 14px;
    height: 14px;
    margin: -7px 0 0 -7px;
    border-radius: 50%;
    background: var(--accent);
    box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 25%, transparent);
    transform: scale(0);
    transition: transform var(--dur-micro) var(--ease-emphasized);
  }
  .seek:hover .knob,
  .dragging .knob,
  .seek:focus-visible .knob {
    transform: scale(1);
  }
  .tip {
    position: absolute;
    bottom: calc(100% + 8px);
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    border-radius: var(--r-sm);
    background: rgba(10, 12, 16, 0.9);
    color: #fff;
    font-size: 12px;
    pointer-events: none;
    white-space: nowrap;
    z-index: 5;
  }
  .thumb {
    display: block;
    border-radius: 4px;
    background-repeat: no-repeat;
  }
  .ch {
    font-weight: 600;
  }
</style>
