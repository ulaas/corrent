<script lang="ts">
  let {
    value,
    color = 'var(--accent)',
    height = 6,
    label = '',
    glow = false,
  }: { value: number; color?: string; height?: number; label?: string; glow?: boolean } = $props();
  const pct = $derived(Math.max(0, Math.min(1, value || 0)) * 100);
</script>

<div
  class="bar"
  class:glow
  style:--h="{height}px"
  style:--c={color}
  role="progressbar"
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={Math.round(pct)}
  aria-label={label || undefined}>
  <div class="fill" style:transform="scaleX({pct / 100})"></div>
</div>

<style>
  .bar {
    position: relative;
    height: var(--h);
    border-radius: var(--h);
    background: color-mix(in srgb, var(--text) 9%, transparent);
    overflow: hidden;
    width: 100%;
  }
  .fill {
    position: absolute;
    inset: 0;
    transform-origin: left center;
    background: var(--c);
    border-radius: inherit;
    transition: transform calc(900ms * var(--motion)) var(--ease-standard);
  }
  .glow .fill {
    box-shadow: 0 0 12px color-mix(in srgb, var(--c) 60%, transparent);
  }
</style>
