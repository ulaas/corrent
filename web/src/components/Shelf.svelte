<script lang="ts">
  // A horizontal row of cards with scroll buttons.
  import Card from './Card.svelte';
  import Icon from './Icon.svelte';
  import { link } from '../lib/router.svelte';
  import { shapeFor, type Shape } from '../lib/media';
  import { cardWidth } from '../lib/cards.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Item } from '../lib/types';

  let {
    title,
    items,
    href,
    shape,
    onplay,
  }: { title: string; items: Item[]; href?: string; shape?: Shape; onplay?: (i: Item) => void } = $props();

  let scroller: HTMLDivElement | undefined = $state();
  let atStart = $state(true);
  let atEnd = $state(false);

  const rowShape = $derived(shape ?? (items[0] ? shapeFor(items[0].kind) : 'poster'));

  function update() {
    if (!scroller) return;
    atStart = scroller.scrollLeft < 8;
    atEnd = scroller.scrollLeft + scroller.clientWidth >= scroller.scrollWidth - 8;
  }
  function page(dir: number) {
    scroller?.scrollBy({ left: dir * scroller.clientWidth * 0.85, behavior: 'smooth' });
  }
  $effect(() => {
    void items.length;
    queueMicrotask(update);
  });
</script>

<section class="shelf">
  <header>
    <h2>{title}</h2>
    {#if href}<a {href} use:link class="all">{t('See all')}<Icon name="caret-right" size={14} /></a>{/if}
    <span class="spacer"></span>
    <button class="btn ghost icon sm nav" disabled={atStart} aria-label={t('Scroll left')} onclick={() => page(-1)}><Icon name="caret-left" /></button>
    <button class="btn ghost icon sm nav" disabled={atEnd} aria-label={t('Scroll right')} onclick={() => page(1)}><Icon name="caret-right" /></button>
  </header>
  <div class="scroller stagger" bind:this={scroller} onscroll={update} style:--w="{cardWidth(rowShape)}px">
    {#each items as item, i (item.id)}
      <div class="cell" style:--i={i}><Card {item} shape={rowShape} {onplay} /></div>
    {/each}
  </div>
</section>

<style>
  .shelf {
    margin-bottom: var(--sp-8);
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }
  h2 {
    font-size: var(--fs-title);
  }
  .all {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--text-dim);
  }
  .all:hover {
    color: var(--accent);
  }
  .scroller {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: var(--w);
    gap: var(--sp-4);
    overflow-x: auto;
    overflow-y: visible;
    scroll-snap-type: x mandatory;
    scroll-padding: 0 var(--page-pad);
    padding: 6px var(--page-pad) 12px;
    margin: -6px calc(-1 * var(--page-pad)) 0;
    scrollbar-width: none;
  }
  .scroller::-webkit-scrollbar {
    display: none;
  }
  .cell {
    scroll-snap-align: start;
  }
  @media (hover: none) {
    .nav {
      display: none;
    }
  }
</style>
