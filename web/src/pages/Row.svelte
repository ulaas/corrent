<script lang="ts">
  // A home row in full ("See all"): Favorites, Continue watching and Next up as real queries that
  // page through everything; any other row, or a server without those filters, shows the home
  // page's copy of the row.
  import { untrack } from 'svelte';
  import Icon from '../components/Icon.svelte';
  import ItemGrid from '../components/ItemGrid.svelte';
  import VirtualGrid from '../components/VirtualGrid.svelte';
  import { api, errorText } from '../lib/api';
  import { rowFilter } from '../lib/homerows';
  import { homeRowTitle, shapeFor, type Shape } from '../lib/media';
  import { libs } from '../lib/libraries.svelte';
  import { back } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { HomeRow, Item, Page } from '../lib/types';

  let { params }: { params: Record<string, string> } = $props();
  let row = $state<HomeRow | null>(null);
  let error = $state('');
  let grid = $state<{ query: Record<string, string | number | boolean>; shape: Shape } | null>(null);
  let total = $state(-1);

  $effect(() => {
    const id = params.id;
    const filter = rowFilter(id);
    grid = null;
    row = null;
    error = '';
    const fromHome = () =>
      api.get<HomeRow[]>('/home').then(
        (rows) => {
          if (params.id === id) row = rows.find((r) => r.id === id) ?? { id, title: '', items: [] };
        },
        (e) => (error = errorText(e)),
      );
    if (!filter) return void fromHome();
    // The first item tells the cards' shape, and whether this server knows the filter (one that
    // doesn't would answer with the whole library).
    api.get<Page<Item>>('/items', { ...filter.query, limit: 1 }).then(
      (p) => {
        if (params.id !== id) return;
        if (p.items[0] && !filter.fits(p.items[0])) return void fromHome();
        grid = { query: filter.query, shape: shapeFor(p.items[0]?.kind ?? 'movie') };
      },
      () => void fromHome(),
    );
  });

  // Rows that come from /home follow library changes.
  $effect(() => {
    void libs.changed;
    if (!untrack(() => row)) return;
    const id = params.id;
    api.get<HomeRow[]>('/home').then(
      (rows) => {
        if (params.id === id) row = rows.find((r) => r.id === id) ?? row;
      },
      () => {},
    );
  });

  const title = $derived(homeRowTitle(params.id, row?.title ?? ''));
</script>

{#if grid}
  <div class="page">
    <button class="btn ghost back" onclick={() => back()}><Icon name="arrow-left" />{t('Back')}</button>
    <div class="page-head"><h1>{title}</h1></div>
    {#if total === 0}
      <div class="empty"><div class="empty-icon"><Icon name="rows" size={30} /></div><h3>{t('Nothing here yet')}</h3></div>
    {/if}
    <VirtualGrid query={grid.query} shape={grid.shape} bind:total />
  </div>
{:else}
  <ItemGrid {title} icon="rows" items={row?.items ?? null} {error} />
{/if}

<style>
  .back {
    margin-bottom: 8px;
  }
</style>
