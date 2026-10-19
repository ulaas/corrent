<script lang="ts">
  // A windowed grid for very large libraries: only the rows near the viewport exist, and
  // items are fetched page by page (GET /items?offset=&limit=) as they come into view.
  import { untrack } from 'svelte';
  import Card from './Card.svelte';
  import { api } from '../lib/api';
  import { aspect, type Shape } from '../lib/media';
  import { cardWidth } from '../lib/cards.svelte';
  import type { Item, Page } from '../lib/types';

  type Query = Record<string, string | number | boolean | undefined>;

  let {
    query,
    shape,
    total = $bindable(-1),
    onplay,
  }: { query: Query; shape: Shape; total?: number; onplay?: (i: Item) => void } = $props();

  const PAGE = 120;
  const GAP = 20;
  const TEXT_H = 50;

  let host: HTMLDivElement | undefined = $state();
  let width = $state(0);
  let scrollTop = $state(0);
  let viewport = $state(800);
  let offsetTop = $state(0);
  let pages = $state<Record<number, Item[]>>({});
  const loading = new Set<number>();
  let generation = 0;

  const minW = $derived(cardWidth(shape));
  const cols = $derived(Math.max(1, Math.floor((width + GAP) / (minW + GAP))));
  const cellW = $derived(cols > 0 ? (width - GAP * (cols - 1)) / cols : minW);
  const rowH = $derived(cellW * aspect(shape) + TEXT_H + GAP);
  const rows = $derived(total > 0 ? Math.ceil(total / cols) : 0);
  const first = $derived(Math.max(0, Math.floor((scrollTop - offsetTop) / rowH) - 2));
  const last = $derived(Math.min(rows - 1, Math.ceil((scrollTop - offsetTop + viewport) / rowH) + 2));

  function load(page: number) {
    untrack(() => loadPage(page));
  }

  function loadPage(page: number) {
    if (pages[page] || loading.has(page)) return;
    loading.add(page);
    const gen = generation;
    api
      .get<Page<Item>>('/items', { ...query, offset: page * PAGE, limit: PAGE })
      .then((p) => {
        if (gen !== generation) return;
        pages[page] = p.items;
        total = p.total;
      })
      .catch(() => {})
      .finally(() => loading.delete(page));
  }

  // New query: start over.
  $effect(() => {
    void JSON.stringify(query);
    untrack(() => {
      generation++;
      pages = {};
      loading.clear();
      total = -1;
      load(0);
    });
  });

  // Fetch the pages the visible rows need.
  $effect(() => {
    if (total <= 0) return;
    const a = first * cols, b = Math.min(total - 1, (last + 1) * cols - 1);
    for (let p = Math.floor(a / PAGE); p <= Math.floor(b / PAGE); p++) load(p);
  });

  function itemAt(i: number): Item | undefined {
    return pages[Math.floor(i / PAGE)]?.[i % PAGE];
  }

  $effect(() => {
    if (!host) return;
    const scroller = (host.closest('main') as HTMLElement | null) ?? document.documentElement;
    const measure = () => {
      width = host!.clientWidth;
      viewport = scroller.clientHeight;
      offsetTop = host!.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
      scrollTop = scroller.scrollTop;
    };
    const onScroll = () => (scrollTop = scroller.scrollTop);
    const ro = new ResizeObserver(measure);
    ro.observe(host);
    ro.observe(scroller);
    scroller.addEventListener('scroll', onScroll, { passive: true });
    measure();
    return () => {
      ro.disconnect();
      scroller.removeEventListener('scroll', onScroll);
    };
  });

  const visibleRows = $derived(rows > 0 && last >= first ? Array.from({ length: last - first + 1 }, (_, i) => first + i) : []);
</script>

<div class="vgrid" bind:this={host} style:height="{Math.max(0, rows * rowH - GAP)}px">
  {#each visibleRows as r (r)}
    <div class="vrow" style:transform="translateY({r * rowH}px)" style:grid-template-columns="repeat({cols}, 1fr)" style:gap="{GAP}px">
      {#each Array(cols) as _, c (c)}
        {@const i = r * cols + c}
        {#if i < total}
          {@const it = itemAt(i)}
          {#if it}
            <div class="cell"><Card item={it} {shape} {onplay} /></div>
          {:else}
            <div class="cell">
              <div class="skeleton ph" style:aspect-ratio={1 / aspect(shape)}></div>
              <div class="skeleton t"></div>
            </div>
          {/if}
        {/if}
      {/each}
    </div>
  {/each}
</div>

<style>
  .vgrid {
    position: relative;
    width: 100%;
    contain: layout;
  }
  .vrow {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    display: grid;
    will-change: transform;
  }
  .cell {
    min-width: 0;
    animation: fadein var(--dur-page) var(--ease-standard) both;
  }
  :global(:root[data-motion='off']) .cell {
    animation: none;
  }
  @keyframes fadein {
    from {
      opacity: 0;
    }
  }
  .ph {
    width: 100%;
  }
  .t {
    height: 12px;
    width: 70%;
    margin-top: 12px;
  }
</style>
