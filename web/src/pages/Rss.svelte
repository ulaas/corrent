<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import Modal from '../components/Modal.svelte';
  import RssConfig from '../components/rss/RssConfig.svelte';
  import { api, errorText } from '../lib/api';
  import { ago, bytes } from '../lib/format';
  import { toast } from '../lib/toast.svelte';
  import { can } from '../lib/session.svelte';
  import { t } from '../lib/i18n.svelte';
  import { Perm, type Article, type RssView } from '../lib/types';

  let view = $state<RssView | null>(null);
  let error = $state('');
  let feed = $state<string>('');
  let filter = $state('');
  let configuring = $state(false);
  let refreshing = $state(false);

  async function load() {
    try {
      view = await api.get<RssView>('/rss');
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  // Feeds refresh in the background: poll while something is loading (or shortly after a refresh).
  $effect(() => {
    const busy = refreshing || view?.feeds.some((f) => f.loading);
    if (!busy) return;
    const timer = setInterval(async () => {
      await load();
      if (!view?.feeds.some((f) => f.loading)) refreshing = false;
    }, 1500);
    return () => clearInterval(timer);
  });

  async function refresh(url?: string) {
    refreshing = true;
    try {
      await api.post('/rss/refresh', url ? { url } : {});
    } catch (e) {
      toast(errorText(e), 'error');
      refreshing = false;
    }
    setTimeout(() => (refreshing = false), 8000);
  }

  async function download(feedUrl: string, a: Article) {
    try {
      await api.post('/rss/download', { feed: feedUrl, guid: a.guid });
      toast(t('Adding “{name}”…', { name: a.title }), 'success');
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  const articles = $derived.by(() => {
    const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
    const out: { feed: string; feedTitle: string; a: Article; downloaded: boolean }[] = [];
    for (const f of view?.feeds ?? []) {
      if (feed && f.url !== feed) continue;
      for (const x of f.articles) {
        if (words.every((w) => x.article.title.toLowerCase().includes(w)))
          out.push({ feed: f.url, feedTitle: f.title || f.url, a: x.article, downloaded: x.downloaded });
      }
    }
    return out.sort((p, q) => (q.a.published ?? 0) - (p.a.published ?? 0));
  });
  const canAdd = $derived(can(Perm.TORRENTS_ADD));
</script>

<div class="page">
  <div class="page-head">
    <h1>{t('RSS')}</h1>
    <button class="btn" onclick={() => refresh()} disabled={refreshing}><Icon name="arrows-clockwise" class={refreshing ? 'spin' : ''} />{t('Refresh all')}</button>
    {#if can(Perm.SETTINGS_READ)}
      <button class="btn primary" onclick={() => (configuring = true)}><Icon name="sliders-horizontal" />{t('Feeds and rules')}</button>
    {/if}
  </div>

  {#if error}
    <p class="form-error">{error}</p>
  {:else if !view}
    <div class="empty"><Spinner /></div>
  {:else if !view.feeds.length}
    <div class="empty">
      <div class="empty-icon"><Icon name="rss" size={30} /></div>
      <h3>{t('No feeds yet')}</h3>
      <p>{t('Subscribe to RSS feeds of releases and let rules download what you want.')}</p>
      {#if can(Perm.SETTINGS_WRITE)}<button class="btn primary" onclick={() => (configuring = true)}><Icon name="plus" />{t('Add a feed')}</button>{/if}
    </div>
  {:else}
    <div class="layout">
      <nav class="feeds" aria-label={t('Feeds')}>
        <button class="fitem" class:on={feed === ''} onclick={() => (feed = '')}>
          <Icon name="rows" size={17} /><span class="grow">{t('All feeds')}</span>
          <span class="cnt num">{view.feeds.reduce((a, f) => a + f.articles.length, 0)}</span>
        </button>
        {#each view.feeds as f (f.url)}
          <button class="fitem" class:on={feed === f.url} class:disabled={!f.enabled} onclick={() => (feed = f.url)} title={f.error ?? f.url}>
            {#if f.loading}<Spinner size={15} />{:else if f.error}<Icon name="warning-circle" size={17} class="err" />{:else}<Icon name="rss" size={17} />{/if}
            <span class="grow ellipsis">{f.title || f.url}</span>
            <span class="cnt num">{f.articles.length}</span>
          </button>
        {/each}
      </nav>

      <section class="articles">
        <div class="tools">
          <div class="search"><Icon name="magnifying-glass" size={16} /><input bind:value={filter} placeholder={t('Filter articles')} aria-label={t('Filter articles')} /></div>
          {#if feed}
            {@const f = view.feeds.find((x) => x.url === feed)}
            <span class="caption muted">{f?.last_ok ? t('Updated {when}', { when: ago(f.last_ok) }) : ''}</span>
            <button class="btn sm" onclick={() => refresh(feed)}><Icon name="arrows-clockwise" size={14} />{t('Refresh')}</button>
            {#if f?.error}<span class="badge err" title={f.error}>{f.error}</span>{/if}
          {/if}
        </div>
        {#if !articles.length}
          <p class="muted empty-small">{t('No articles.')}</p>
        {:else}
          <ul class="alist">
            {#each articles as x (x.feed + x.a.guid)}
              <li class:done={x.downloaded}>
                <div class="grow main">
                  <span class="title">{x.a.title}</span>
                  <span class="caption muted meta">
                    {#if !feed}<span>{x.feedTitle}</span>{/if}
                    {#if x.a.published}<span>{ago(x.a.published)}</span>{/if}
                    {#if x.a.size}<span class="num">{bytes(x.a.size)}</span>{/if}
                    {#if x.a.page}<a class="text-link" href={x.a.page} target="_blank" rel="noreferrer noopener">{t('Details')}</a>{/if}
                  </span>
                </div>
                {#if x.downloaded}
                  <span class="badge ok"><Icon name="check" size={12} />{t('Downloaded')}</span>
                {:else if canAdd && x.a.torrent}
                  <button class="btn sm" onclick={() => download(x.feed, x.a)}><Icon name="download-simple" size={14} />{t('Download')}</button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    </div>
  {/if}
</div>

{#if configuring}
  <Modal title={t('Feeds and rules')} size="xl" onclose={() => ((configuring = false), void load())}>
    <RssConfig onsaved={() => void refresh()} />
  </Modal>
{/if}

<style>
  .layout {
    display: grid;
    grid-template-columns: 260px 1fr;
    gap: 24px;
    align-items: start;
  }
  .feeds {
    display: flex;
    flex-direction: column;
    gap: 2px;
    position: sticky;
    top: 0;
  }
  .fitem {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: var(--r-md);
    font-weight: 600;
    font-size: var(--fs-small);
    color: var(--text-dim);
    text-align: left;
  }
  .fitem:hover {
    background: var(--hover);
    color: var(--text);
  }
  .fitem.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .fitem.disabled {
    opacity: 0.55;
  }
  .fitem :global(.err) {
    color: var(--error);
  }
  .cnt {
    font-size: 11px;
    color: var(--text-mute);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
    flex-wrap: wrap;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--control-h);
    padding: 0 12px;
    border-radius: var(--r-md);
    border: 1px solid var(--border);
    background: var(--bg0);
    color: var(--text-dim);
    width: min(320px, 100%);
  }
  .search input {
    flex: 1;
    border: 0;
    background: transparent;
    outline: none;
    color: var(--text);
    min-width: 0;
  }
  .alist {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--bg1);
    overflow: hidden;
  }
  .alist li {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 16px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .alist li:last-child {
    border-bottom: 0;
  }
  .alist li:hover {
    background: var(--hover);
  }
  .alist li.done .title {
    color: var(--text-dim);
  }
  .main {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .title {
    font-weight: 600;
    word-break: break-word;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .empty-small {
    padding: 40px 0;
    text-align: center;
  }
  @media (max-width: 800px) {
    .layout {
      grid-template-columns: 1fr;
    }
    .feeds {
      position: static;
      flex-direction: row;
      overflow-x: auto;
    }
    .fitem {
      flex: none;
    }
  }
</style>
