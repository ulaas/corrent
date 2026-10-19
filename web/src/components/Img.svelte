<script lang="ts">
  // Artwork with a blurhash placeholder that fades into the real image.
  import { decode } from 'blurhash';
  import { imageUrl } from '../lib/api';
  import type { ImageRef } from '../lib/types';

  let {
    image,
    w = 400,
    alt = '',
    fit = 'cover',
    eager = false,
  }: { image?: ImageRef | null; w?: 200 | 400 | 800 | 0; alt?: string; fit?: 'cover' | 'contain'; eager?: boolean } =
    $props();

  let loaded = $state(false);
  let failed = $state(false);
  let canvas: HTMLCanvasElement | undefined = $state();

  $effect(() => {
    // Reset when the image changes.
    void image?.sha;
    loaded = false;
    failed = false;
  });

  $effect(() => {
    const hash = image?.blurhash;
    if (!canvas || !hash) return;
    try {
      const px = decode(hash, 32, 32);
      const ctx = canvas.getContext('2d');
      if (!ctx) return;
      const data = ctx.createImageData(32, 32);
      data.data.set(px);
      ctx.putImageData(data, 0, 0);
    } catch {
      /* bad hash */
    }
  });
</script>

<div class="img" class:loaded>
  {#if image?.blurhash}
    <canvas bind:this={canvas} width="32" height="32" aria-hidden="true"></canvas>
  {/if}
  {#if image && !failed}
    <img
      src={imageUrl(image.sha, w || undefined)}
      {alt}
      loading={eager ? 'eager' : 'lazy'}
      decoding="async"
      style:object-fit={fit}
      onload={() => (loaded = true)}
      onerror={() => (failed = true)} />
  {/if}
</div>

<style>
  .img {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }
  canvas,
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }
  canvas {
    transform: scale(1.1);
    filter: saturate(1.2);
  }
  img {
    opacity: 0;
    transition: opacity calc(420ms * var(--motion)) var(--ease-standard);
  }
  .loaded img {
    opacity: 1;
  }
</style>
