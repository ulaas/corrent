<script lang="ts">
  // "Add to playlist": your playlists, or a new one.
  import { fly } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import { api, errorText } from '../lib/api';
  import { toast } from '../lib/toast.svelte';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Playlist } from '../lib/types';

  let { items, big = false }: { items: number[]; big?: boolean } = $props();
  let open = $state(false);
  let lists = $state<Playlist[]>([]);
  let name = $state('');

  async function toggle() {
    open = !open;
    if (open) lists = (await api.get<Playlist[]>('/playlists').catch(() => [])).filter((p) => p.mine);
  }

  async function addTo(p: Playlist) {
    open = false;
    try {
      const r = await api.post<{ added: number }>(`/playlists/${p.id}/items`, { items });
      toast(r.added === 1 ? t('Added 1 item to “{name}”', { name: p.name }) : t('Added {n} items to “{name}”', { n: r.added, name: p.name }), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function create(e: SubmitEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    open = false;
    try {
      const p = await api.post<Playlist>('/playlists', { name: name.trim(), items });
      toast(t('Created “{name}”', { name: p.name }), 'success');
      name = '';
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }
</script>

<div class="pop">
  <button class="btn" class:lg={big} aria-haspopup="menu" aria-expanded={open} onclick={toggle}><Icon name="playlist" />{t('Add to playlist')}</button>
  {#if open}
    <div class="menu" role="menu" transition:fly={{ y: 8, duration: ms(140) }}>
      {#each lists as p (p.id)}
        <button class="mi" role="menuitem" onclick={() => addTo(p)}><span class="grow ellipsis">{p.name}</span><span class="caption muted">{p.count}</span></button>
      {/each}
      <form class="new" onsubmit={create}>
        <input class="input" bind:value={name} placeholder={t('New playlist')} aria-label={t('New playlist')} />
        <button class="btn sm primary" aria-label={t('Create')}><Icon name="plus" size={14} /></button>
      </form>
    </div>
  {/if}
</div>

<style>
  .pop {
    position: relative;
    display: inline-block;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 40;
    min-width: 260px;
    max-height: 360px;
    overflow-y: auto;
    padding: 6px;
    background: var(--bg2);
    border: 1px solid var(--border-hi);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-4);
  }
  .mi {
    display: flex;
    width: 100%;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    background: none;
    border: 0;
    border-radius: var(--r-md);
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .mi:hover {
    background: var(--bg3);
  }
  .new {
    display: flex;
    gap: 6px;
    padding: 6px 4px 2px;
  }
  .new .input {
    flex: 1;
  }
</style>
