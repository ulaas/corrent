<script lang="ts">
  // Filter (and, with `sorts`, sort) toggle buttons for a list; the choice is kept in the page's
  // query string (lib/filters.ts). Genres and decades come from GET /facets and take rows of
  // their own, shown on demand.
  import Icon from './Icon.svelte';
  import { api } from '../lib/api';
  import { route, setQuery } from '../lib/router.svelte';
  import { isFiltered, nextSort, readFilter, writeFilter, type ItemFilter, type Watch } from '../lib/filters';
  import { t, tk } from '../lib/i18n.svelte';
  import type { Facets, ItemKind, SortBy } from '../lib/types';

  let {
    library,
    kind,
    sorts,
  }: { library?: number; kind?: ItemKind; sorts?: { id: SortBy; label: string }[] } = $props();

  const WATCH: { id: Watch | undefined; label: string }[] = [
    { id: undefined, label: tk('All') },
    { id: 'unplayed', label: tk('Unplayed') },
    { id: 'in_progress', label: tk('In progress') },
    { id: 'played', label: tk('Played') },
  ];

  const filter = $derived(readFilter(route.query));
  const sort = $derived((route.query.get('sort') as SortBy) || 'title');
  const desc = $derived(route.query.get('desc') === '1');
  let facets = $state<Facets>({ genres: [], decades: [] });
  let more = $state(false);
  const narrowed = $derived(!!(filter.genre || filter.decade));
  const hasMore = $derived(facets.genres.length > 0 || facets.decades.length > 0);

  let asked = 0;
  $effect(() => {
    const q = { library_id: library, kind };
    const n = ++asked;
    facets = { genres: [], decades: [] };
    api.get<Facets>('/facets', q).then(
      // A slower answer for the page before this one is not this page's.
      (f) => n === asked && (facets = f),
      // An older server has no filters to offer.
      () => {},
    );
  });

  function set(patch: Partial<ItemFilter>) {
    setQuery(writeFilter({ ...filter, ...patch }));
  }
  function toggleMore() {
    if (more || narrowed) {
      more = false;
      set({ genre: undefined, decade: undefined });
    } else more = true;
  }
  function clear() {
    more = false;
    setQuery(writeFilter({}));
  }
  function pickSort(id: SortBy) {
    const next = nextSort({ sort, desc }, id);
    setQuery({ sort: next.sort === 'title' ? null : next.sort, desc: next.desc ? '1' : null });
  }
</script>

<div class="bar">
  <div class="line">
    <div class="chips" role="group" aria-label={t('Filter')}>
      {#each WATCH as w (w.label)}
        <button class="chip" class:on={filter.watch === w.id} aria-pressed={filter.watch === w.id} onclick={() => set({ watch: w.id })}>{t(w.label)}</button>
      {/each}
    </div>
    <div class="chips">
      <button class="chip" class:on={filter.favorite} aria-pressed={!!filter.favorite} onclick={() => set({ favorite: !filter.favorite })}>
        <Icon name={filter.favorite ? 'heart-fill' : 'heart'} size={14} />{t('Favorites')}
      </button>
      {#if hasMore || narrowed}
        <button class="chip" class:on={more || narrowed} aria-pressed={more || narrowed} aria-expanded={more || narrowed} onclick={toggleMore}>
          <Icon name="funnel" size={14} />{t('Genre and year')}
        </button>
      {/if}
      {#if isFiltered(filter)}
        <button class="chip" onclick={clear}><Icon name="x" size={14} />{t('Clear filters')}</button>
      {/if}
    </div>
    {#if sorts}
      <span class="spacer"></span>
      <div class="chips" role="group" aria-label={t('Sort by')}>
        <span class="lead">{t('Sort')}</span>
        {#each sorts as s (s.id)}
          <button
            class="chip"
            class:on={sort === s.id}
            aria-pressed={sort === s.id}
            title={sort === s.id ? t('Reverse order') : undefined}
            onclick={() => pickSort(s.id)}>
            {t(s.label)}
            {#if sort === s.id}<Icon name={desc ? 'arrow-down' : 'arrow-up'} size={13} />{/if}
          </button>
        {/each}
      </div>
    {/if}
  </div>
  {#if more || narrowed}
    {#if facets.genres.length}
      <div class="chips" role="group" aria-label={t('Genre')}>
        <span class="lead">{t('Genre')}</span>
        {#each facets.genres as g (g)}
          <button class="chip" class:on={filter.genre === g} aria-pressed={filter.genre === g} onclick={() => set({ genre: filter.genre === g ? undefined : g })}>{g}</button>
        {/each}
      </div>
    {/if}
    {#if facets.decades.length}
      <div class="chips" role="group" aria-label={t('Year')}>
        <span class="lead">{t('Year')}</span>
        {#each facets.decades as d (d)}
          <button class="chip num" class:on={filter.decade === d} aria-pressed={filter.decade === d} onclick={() => set({ decade: filter.decade === d ? undefined : d })}>{d}–{d + 9}</button>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .bar {
    display: grid;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 14px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .lead {
    color: var(--text-dim);
    font-size: var(--fs-caption);
    margin-right: 2px;
  }
</style>
