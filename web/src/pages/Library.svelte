<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import VirtualGrid from '../components/VirtualGrid.svelte';
  import Progress from '../components/Progress.svelte';
  import { api, errorText } from '../lib/api';
  import { libs, library, loadLibraries } from '../lib/libraries.svelte';
  import { LIBRARY_ICON, libraryShape, topKind, LIBRARY_KIND_LABEL } from '../lib/media';
  import { route, setQuery } from '../lib/router.svelte';
  import { music, toEntry, tracksOf } from '../lib/player.svelte';
  import { can } from '../lib/session.svelte';
  import { toast } from '../lib/toast.svelte';
  import { num } from '../lib/format';
  import { t, tk } from '../lib/i18n.svelte';
  import { Perm, type Item, type ItemKind, type SortBy } from '../lib/types';

  let { params }: { params: Record<string, string> } = $props();

  const id = $derived(+params.id);
  const lib = $derived(library(id));
  const scan = $derived(libs.scans[id]);

  const SORTS: { id: SortBy; label: string }[] = [
    { id: 'title', label: tk('Title') },
    { id: 'added', label: tk('Date added') },
    { id: 'year', label: tk('Year') },
    { id: 'played', label: tk('Last played') },
  ];
  const FILTERS = [
    { id: '', label: tk('All') },
    { id: 'unplayed', label: tk('Unplayed') },
    { id: 'played', label: tk('Played') },
    { id: 'favorites', label: tk('Favorites') },
  ];

  const sort = $derived((route.query.get('sort') as SortBy) || 'title');
  const desc = $derived(route.query.get('desc') === '1');
  const filter = $derived(route.query.get('filter') ?? '');
  const view = $derived(route.query.get('view') ?? '');
  let total = $state(-1);
  let refresh = $state(0);

  const kind = $derived.by((): ItemKind | undefined => {
    if (!lib) return undefined;
    if (lib.kind === 'music' && view === 'artists') return 'artist';
    if (lib.kind === 'music' && view === 'tracks') return 'track';
    return topKind(lib.kind);
  });

  const query = $derived({
    library_id: id,
    kind,
    sort,
    desc: desc || undefined,
    played: filter === 'played' ? true : filter === 'unplayed' ? false : undefined,
    favorite: filter === 'favorites' ? true : undefined,
    r: refresh || undefined,
  });

  // Reload the grid when a scan of this library finishes.
  let wasRunning = false;
  $effect(() => {
    const running = !!scan?.running;
    if (wasRunning && !running) refresh++;
    wasRunning = running;
  });

  async function rescan() {
    try {
      await api.post(`/libraries/${id}/scan`);
      toast(t('Scanning “{name}”…', { name: lib?.name ?? '' }), 'info');
      void loadLibraries();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function playContainer(it: Item) {
    try {
      const tracks = it.kind === 'track' ? [it] : await tracksOf(it.id);
      if (tracks.length) music.play(tracks.map((x) => toEntry(x, it)));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  const onplay = $derived(lib && (lib.kind === 'music' || lib.kind === 'audiobooks') ? playContainer : undefined);
</script>

{#if !lib}
  {#if libs.loaded}
    <div class="page"><div class="empty"><h3>{t('This library no longer exists.')}</h3></div></div>
  {/if}
{:else}
  <div class="page">
    <div class="page-head lib-head">
      <span class="lib-icon"><Icon name={LIBRARY_ICON[lib.kind]} size={24} /></span>
      <div class="grow">
        <h1 class="ellipsis">{lib.name}</h1>
        <p class="muted small">
          {t(LIBRARY_KIND_LABEL[lib.kind])} ·
          {total >= 0 ? (total === 1 ? t('1 item') : t('{n} items', { n: num(total) })) : '…'}
        </p>
      </div>
      {#if can(Perm.LIBRARY_MANAGE)}
        <button class="btn" onclick={rescan} disabled={scan?.running}>
          <Icon name="arrows-clockwise" class={scan?.running ? 'spin' : ''} />{scan?.running ? t('Scanning…') : t('Scan')}
        </button>
      {/if}
    </div>

    {#if scan?.running}
      <div class="scan panel">
        <span class="small"><strong>{t('Scanning…')}</strong> {scan.phase}</span>
        <Progress value={scan.total ? scan.done / scan.total : 0} height={4} label={t('Scan progress')} />
        <span class="caption muted num">{scan.done} / {scan.total}</span>
      </div>
    {/if}

    <div class="toolbar">
      {#if lib.kind === 'music'}
        <div class="seg" role="tablist" aria-label={t('View')}>
          <button role="tab" aria-selected={view === ''} class:on={view === ''} onclick={() => setQuery({ view: null })}>{t('Albums')}</button>
          <button role="tab" aria-selected={view === 'artists'} class:on={view === 'artists'} onclick={() => setQuery({ view: 'artists' })}>{t('Artists')}</button>
        </div>
      {/if}
      <div class="chips" role="group" aria-label={t('Filter')}>
        {#each FILTERS as f (f.id)}
          <button class="chip" class:on={filter === f.id} aria-pressed={filter === f.id} onclick={() => setQuery({ filter: f.id || null })}>{t(f.label)}</button>
        {/each}
      </div>
      <span class="spacer"></span>
      <label class="sort">
        <span class="sr-only">{t('Sort by')}</span>
        <Icon name="sort-ascending" size={16} />
        <select class="select" value={sort} onchange={(e) => setQuery({ sort: e.currentTarget.value === 'title' ? null : e.currentTarget.value })}>
          {#each SORTS as s (s.id)}<option value={s.id}>{t(s.label)}</option>{/each}
        </select>
      </label>
      <button class="btn icon" aria-label={desc ? t('Descending') : t('Ascending')} title={desc ? t('Descending') : t('Ascending')} onclick={() => setQuery({ desc: desc ? null : '1' })}>
        <Icon name={desc ? 'sort-descending' : 'sort-ascending'} />
      </button>
    </div>

    {#if total === 0}
      <div class="empty">
        <div class="empty-icon"><Icon name={LIBRARY_ICON[lib.kind]} size={32} /></div>
        <h3>{filter ? t('Nothing matches this filter.') : t('This library is empty.')}</h3>
        {#if !filter}<p>{t('Files added to its folders appear here after a scan.')}</p>{/if}
      </div>
    {/if}
    <VirtualGrid {query} shape={kind === 'artist' ? 'square' : libraryShape(lib.kind)} bind:total {onplay} />
  </div>
{/if}

<style>
  .lib-head {
    align-items: center;
  }
  .lib-icon {
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    border-radius: 16px;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  .scan {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 14px;
    padding: 12px 16px;
    margin-bottom: 18px;
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
    margin-bottom: 24px;
    position: sticky;
    top: 0;
    z-index: 5;
    padding: 10px 0;
    background: linear-gradient(var(--bg0) 70%, transparent);
  }
  .chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .seg {
    display: inline-flex;
    padding: 3px;
    border-radius: var(--r-pill);
    background: var(--bg2);
    border: 1px solid var(--border);
  }
  .seg button {
    padding: 4px 14px;
    border-radius: var(--r-pill);
    font-size: var(--fs-caption);
    font-weight: 600;
    color: var(--text-dim);
  }
  .seg button.on {
    background: var(--bg3);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .sort {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
  }
  .sort .select {
    width: auto;
  }
</style>
