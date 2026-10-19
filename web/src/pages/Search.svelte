<script lang="ts" module>
  import type { SearchResponse } from '../lib/types';
  const last = $state<{ q: string; res: SearchResponse | null }>({ q: '', res: null });
</script>

<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import AddTorrent from '../components/torrents/AddTorrent.svelte';
  import { api, errorText } from '../lib/api';
  import { link } from '../lib/router.svelte';
  import { bytes, ago, num } from '../lib/format';
  import { toast } from '../lib/toast.svelte';
  import { useTorrents } from '../lib/torrents.svelte';
  import { isAdmin } from '../lib/session.svelte';
  import { t, tk } from '../lib/i18n.svelte';
  import type { SearchResult } from '../lib/types';

  $effect(() => useTorrents());

  let q = $state(last.q);
  let busy = $state(false);
  let error = $state('');
  let sort = $state<'seeders' | 'size' | 'published' | 'title'>('seeders');
  let desc = $state(true);
  let filter = $state('');
  let adding = $state<string | null>(null);
  let added = $state<Set<string>>(new Set());

  async function search(e?: Event) {
    e?.preventDefault();
    const text = q.trim();
    if (!text) return;
    busy = true;
    error = '';
    try {
      last.res = await api.get<SearchResponse>('/search', { q: text });
      last.q = text;
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  const SORTS = [
    { id: 'seeders', label: tk('Seeders') },
    { id: 'size', label: tk('Size') },
    { id: 'published', label: tk('Date') },
    { id: 'title', label: tk('Name') },
  ] as const;

  const rows = $derived.by(() => {
    const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
    const list = (last.res?.results ?? []).filter((r) => words.every((w) => r.title.toLowerCase().includes(w)));
    const k = (r: SearchResult) => (sort === 'title' ? r.title.toLowerCase() : (r[sort] ?? -1));
    return [...list].sort((a, b) => {
      const x = k(a), y = k(b);
      const c = typeof x === 'string' ? x.localeCompare(y as string) : (x as number) - (y as number);
      return desc ? -c : c;
    });
  });

  async function quickAdd(r: SearchResult) {
    if (!r.link) return;
    try {
      await api.post('/torrents', { urls: [r.link] });
      added = new Set([...added, r.link]);
      toast(t('Adding “{name}”…', { name: r.title }), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
</script>

<div class="page">
  <div class="page-head"><h1>{t('Search torrents')}</h1></div>
  <form class="bar" role="search" onsubmit={search}>
    <div class="box">
      <Icon name="magnifying-glass" />
      <input bind:value={q} placeholder={t('Search your indexers…')} aria-label={t('Search torrents')} />
    </div>
    <button class="btn primary lg" disabled={busy || !q.trim()}>{#if busy}<Spinner size={16} />{/if}{t('Search')}</button>
  </form>

  {#if error}
    <p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{error}</p>
  {/if}

  {#if last.res}
    {#each last.res.errors as [name, why] (name + why)}
      <p class="provider-err small"><Icon name="warning" size={14} />{t('{provider}: {error}', { provider: name || t('Provider'), error: why })}</p>
    {/each}
    {#if !last.res.results.length && !last.res.errors.length}
      <div class="empty">
        <div class="empty-icon"><Icon name="magnifying-glass" size={30} /></div>
        <h3>{t('No results')}</h3>
        <p>{t('No provider found anything. If none is set up yet, add a Torznab provider (Jackett or Prowlarr) in the settings.')}</p>
        {#if isAdmin()}<a class="btn" href="/settings/search" use:link><Icon name="gear-six" />{t('Search providers')}</a>{/if}
      </div>
    {:else if last.res.results.length}
      <div class="tools">
        <span class="muted small">{rows.length === 1 ? t('1 result') : t('{n} results', { n: num(rows.length) })}</span>
        <span class="spacer"></span>
        <input class="input filter" bind:value={filter} placeholder={t('Filter results')} aria-label={t('Filter results')} />
        <select class="select sortsel" bind:value={sort} aria-label={t('Sort by')}>
          {#each SORTS as s (s.id)}<option value={s.id}>{t(s.label)}</option>{/each}
        </select>
        <button class="btn icon" aria-label={desc ? t('Descending') : t('Ascending')} onclick={() => (desc = !desc)}><Icon name={desc ? 'sort-descending' : 'sort-ascending'} /></button>
      </div>
      <ul class="results stagger">
        {#each rows as r, i (r.provider + r.title + (r.link ?? i))}
          <li style:--i={i}>
            <div class="grow main">
              <span class="title">{r.title}</span>
              <span class="meta caption">
                <span class="badge">{r.provider}</span>
                {#if r.category}<span>{r.category}</span>{/if}
                {#if r.size}<span class="num">{bytes(r.size)}</span>{/if}
                {#if r.published}<span>{ago(r.published)}</span>{/if}
                {#if r.page}<a class="text-link" href={r.page} target="_blank" rel="noreferrer noopener">{t('Details')}<Icon name="arrow-square-out" size={12} /></a>{/if}
              </span>
            </div>
            <div class="sp num" title={t('Seeders / peers')}>
              <span class="seed" class:hot={(r.seeders ?? 0) >= 10}>{r.seeders ?? '–'}</span>
              <span class="faint">/ {r.peers ?? '–'}</span>
            </div>
            <div class="acts">
              {#if r.link && added.has(r.link)}
                <span class="badge ok"><Icon name="check" size={12} />{t('Added')}</span>
              {:else}
                <button class="btn sm" disabled={!r.link} onclick={() => (adding = r.link ?? null)}>{t('Add…')}</button>
                <button class="btn sm primary icon" disabled={!r.link} aria-label={t('Add now')} title={t('Add now')} onclick={() => quickAdd(r)}><Icon name="download-simple" size={16} /></button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {:else if !busy}
    <div class="empty">
      <div class="empty-icon"><Icon name="globe" size={30} /></div>
      <h3>{t('Find something to download')}</h3>
      <p>{t('Searches the Torznab providers set up on the server.')}</p>
    </div>
  {/if}
</div>

{#if adding}<AddTorrent initialText={adding} onclose={() => (adding = null)} />{/if}

<style>
  .bar {
    display: flex;
    gap: 10px;
    max-width: 820px;
    margin-bottom: 20px;
  }
  .box {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 16px;
    height: 46px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--bg1);
    color: var(--text-dim);
  }
  .box:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .box input {
    flex: 1;
    border: 0;
    background: transparent;
    outline: none;
    font-size: 16px;
    color: var(--text);
  }
  .provider-err {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--warn);
    margin-bottom: 8px;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0 12px;
    flex-wrap: wrap;
  }
  .filter {
    width: 220px;
  }
  .sortsel {
    width: auto;
  }
  .results {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--bg1);
  }
  .results li {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 16px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .results li:last-child {
    border-bottom: 0;
  }
  .results li:hover {
    background: var(--hover);
  }
  .main {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .title {
    font-weight: 600;
    word-break: break-word;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 12px;
    color: var(--text-dim);
  }
  .meta a {
    display: inline-flex;
    gap: 3px;
    align-items: center;
  }
  .sp {
    width: 90px;
    text-align: right;
    font-size: var(--fs-small);
  }
  .seed {
    font-weight: 600;
    color: var(--text-dim);
  }
  .seed.hot {
    color: var(--success);
  }
  .acts {
    display: flex;
    gap: 6px;
    flex: none;
  }
  @media (max-width: 640px) {
    .results li {
      flex-wrap: wrap;
    }
    .sp {
      width: auto;
      text-align: left;
    }
    .acts {
      margin-left: auto;
    }
  }
</style>
