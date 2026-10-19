<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import Img from '../components/Img.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { link, go } from '../lib/router.svelte';
  import { runtime } from '../lib/format';
  import { toast } from '../lib/toast.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Collection, Playlist } from '../lib/types';

  let lists = $state<Playlist[] | null>(null);
  let cols = $state<Collection[]>([]);
  let error = $state('');
  let name = $state('');

  async function load() {
    try {
      [lists, cols] = await Promise.all([api.get<Playlist[]>('/playlists'), api.get<Collection[]>('/collections')]);
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  async function create(e: SubmitEvent) {
    e.preventDefault();
    try {
      const p = await api.post<Playlist>('/playlists', { name: name.trim() });
      go(`/playlist/${p.id}`);
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }
</script>

<div class="page">
  <div class="page-head"><h1>{t('Playlists')}</h1></div>
  {#if error}
    <p class="form-error">{error}</p>
  {:else if !lists}
    <div class="empty"><Spinner /></div>
  {:else}
    <form class="new" onsubmit={create}>
      <input class="input" required bind:value={name} placeholder={t('New playlist')} aria-label={t('New playlist')} />
      <button class="btn primary"><Icon name="plus" size={16} />{t('Create')}</button>
    </form>
    {#if !lists.length}
      <div class="empty"><div class="empty-icon"><Icon name="playlist" size={30} /></div><h3>{t('No playlists yet')}</h3><p>{t('Add songs or videos from their pages, or start one here.')}</p></div>
    {/if}
    <div class="tiles">
      {#each lists as p (p.id)}
        <a class="tile" href="/playlist/{p.id}" use:link>
          <div class="art">{#if p.images?.[0]}<Img image={p.images[0]} w={200} />{:else}<Icon name="playlist" size={36} />{/if}</div>
          <strong class="ellipsis">{p.name}</strong>
          <span class="caption muted">{p.count === 1 ? t('1 item') : t('{n} items', { n: p.count })}{p.runtime_ms ? ` · ${runtime(p.runtime_ms)}` : ''}{p.mine ? '' : ` · ${p.owner}`}</span>
        </a>
      {/each}
    </div>
    {#if cols.length}
      <h2 class="section-title">{t('Collections')}</h2>
      <div class="tiles">
        {#each cols as c (c.id)}
          <a class="tile" href="/collection/{c.id}" use:link>
            <div class="art poster">{#if c.images?.[0]}<Img image={c.images[0]} w={200} />{:else}<Icon name="stack" size={36} />{/if}</div>
            <strong class="ellipsis">{c.name}</strong>
            <span class="caption muted">{c.count === 1 ? t('1 item') : t('{n} items', { n: c.count })}</span>
          </a>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .new {
    display: flex;
    gap: 8px;
    max-width: 520px;
    margin-bottom: 20px;
  }
  .new .input {
    flex: 1;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 18px;
    margin-bottom: 28px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: inherit;
    text-decoration: none;
  }
  .art {
    display: grid;
    place-items: center;
    aspect-ratio: 1;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--bg2);
    color: var(--text-mute);
    margin-bottom: 6px;
  }
  .art.poster {
    aspect-ratio: 2 / 3;
  }
  .tile:hover strong {
    color: var(--accent);
  }
</style>
