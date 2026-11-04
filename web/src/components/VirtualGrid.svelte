<script lang="ts">
  // A windowed grid for very large libraries: only the rows near the viewport exist, and
  // items are fetched page by page (GET /items?offset=&limit=) as they come into view.
  import { untrack } from 'svelte';
  import Card from './Card.svelte';
  import Icon from './Icon.svelte';
  import { api, errorText } from '../lib/api';
  import { cachedGrid, rememberGrid } from '../lib/gridcache';
  import { onRelink } from '../lib/events.svelte';
  import { t } from '../lib/i18n.svelte';
  import { aspect, type Shape } from '../lib/media';
  import { cardWidth } from '../lib/cards.svelte';
  import type { Item, Page } from '../lib/types';

  type Query = Record<string, string | number | boolean | undefined>;

  let {
    query,
    shape,
    total = $bindable(-1),
    error = $bindable(''),
    onplay,
  }: { query: Query; shape: Shape; total?: number; error?: string; onplay?: (i: Item) => void } = $props();

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
  /** Pages shown from the cache, not fetched yet in this grid's life: they refresh when seen. */
  const stale = new Set<number>();
  let generation = 0;
  let activeKey = '';

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

  function loadPage(page: number, force = false) {
    if ((pages[page] && !force && !stale.has(page)) || loading.has(page)) return;
    loading.add(page);
    const gen = generation;
    api
      .get<Page<Item>>('/items', { ...query, offset: page * PAGE, limit: PAGE })
      .then((p) => {
        if (gen !== generation) return;
        pages[page] = p.items;
        total = p.total;
        error = '';
        stale.delete(page);
        rememberGrid(activeKey, { pages, total });
      })
      .catch((e) => {
        if (gen !== generation) return;
        // What the cache shows stays on screen; only a page with nothing to show is an error.
        if (!pages[page]) error = errorText(e);
        stale.delete(page);
      })
      .finally(() => loading.delete(page));
  }

  /** Try again (the button, or the link is back): what is on screen stays until the answer. */
  function reload() {
    error = '';
    const keep = visiblePages();
    if (!keep.length) return load(0);
    for (const k of Object.keys(pages)) if (!keep.includes(+k)) delete pages[+k];
    for (const p of keep) loadPage(p, true);
  }
  onRelink(reload);

  function visiblePages(): number[] {
    if (total <= 0) return [];
    const a = first * cols, b = Math.min(total - 1, (last + 1) * cols - 1);
    const out: number[] = [];
    for (let p = Math.floor(a / PAGE); p <= Math.floor(b / PAGE); p++) out.push(p);
    return out;
  }

  // New query: start over, or from what is known of it (Back to a grid shows it at once).
  $effect(() => {
    const key = JSON.stringify(query);
    untrack(() => {
      if (key === activeKey) return;
      activeKey = key;
      generation++;
      loading.clear();
      stale.clear();
      error = '';
      const known = cachedGrid(key);
      if (known && known.total > 0) {
        pages = known.pages;
        total = known.total;
        for (const p of Object.keys(known.pages)) stale.add(+p);
      } else {
        pages = {};
        total = -1;
        load(0);
      }
    });
  });

  // Fetch the pages the visible rows need.
  $effect(() => {
    if (error) return;
    for (const p of visiblePages()) load(p);
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

{#if error}
  <div class="empty">
    <div class="empty-icon"><Icon name="warning-circle" size={30} /></div>
    <h3>{t("Couldn't load this list.")}</h3>
    <p>{error}</p>
    <button class="btn" onclick={reload}><Icon name="arrow-clockwise" />{t('Try again')}</button>
  </div>
{/if}
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
