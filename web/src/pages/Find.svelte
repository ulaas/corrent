<script lang="ts">
  import Card from '../components/Card.svelte';
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { route, setQuery } from '../lib/router.svelte';
  import { shapeFor, KIND_LABEL } from '../lib/media';
  import { cardWidth } from '../lib/cards.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Item, ItemKind, SearchResults } from '../lib/types';

  const q = $derived(route.query.get('q') ?? '');
  let text = $state(route.query.get('q') ?? '');
  let res = $state<SearchResults | null>(null);
  let busy = $state(false);
  let error = $state('');

  $effect(() => {
    const query = q.trim();
    if (!query) {
      res = null;
      return;
    }
    const ctl = new AbortController();
    busy = true;
    error = '';
    api
      .get<SearchResults>('/library/search', { q: query }, ctl.signal)
      .then((r) => (res = r))
      .catch((e) => {
        if (!ctl.signal.aborted) error = errorText(e);
      })
      .finally(() => (busy = false));
    return () => ctl.abort();
  });

  const groups = $derived.by(() => {
    const m = new Map<ItemKind, Item[]>();
    for (const it of res?.items ?? []) {
      if (!m.has(it.kind)) m.set(it.kind, []);
      m.get(it.kind)!.push(it);
    }
    return [...m.entries()];
  });
</script>

<div class="page">
  <div class="page-head"><h1>{q ? t('Results for “{q}”', { q }) : t('Find')}</h1></div>
  <form class="find" role="search" onsubmit={(e) => { e.preventDefault(); setQuery({ q: text.trim() || null }); }}>
    <Icon name="magnifying-glass" />
    <input class="input" bind:value={text} placeholder={t('Movies, shows, music, people…')} aria-label={t('Search your libraries')} />
    <button class="btn primary">{t('Search')}</button>
  </form>

  {#if busy && !res}
    <div class="empty"><Spinner /></div>
  {:else if error}
    <p class="form-error">{error}</p>
  {:else if res && !res.items.length && !res.people.length}
    <div class="empty"><div class="empty-icon"><Icon name="magnifying-glass" size={30} /></div><h3>{t('No matches')}</h3><p>{t('Try a different spelling or fewer words.')}</p></div>
  {:else if res}
    {#each groups as [kind, items] (kind)}
      <h2 class="section-title">{t(KIND_LABEL[kind])} <span class="badge">{items.length}</span></h2>
      <div class="grid" style:--w="{cardWidth(shapeFor(kind))}px">
        {#each items as it (it.id)}<Card item={it} shape={shapeFor(kind)} />{/each}
      </div>
    {/each}
    {#if res.people.length}
      <h2 class="section-title">{t('People')}</h2>
      <div class="row wrap">{#each res.people as p (p.person_id + p.role)}<span class="chip">{p.name}<span class="faint">{p.role}</span></span>{/each}</div>
    {/if}
  {/if}
</div>

<style>
  .find {
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 640px;
    margin-bottom: 18px;
    color: var(--text-dim);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--w), 1fr));
    gap: 20px;
  }
</style>
