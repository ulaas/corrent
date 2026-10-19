<script lang="ts">
  // Browses the server's folders (GET /server/fs) to pick a path.
  import Modal from './Modal.svelte';
  import Icon from './Icon.svelte';
  import Spinner from './Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { bytes } from '../lib/format';
  import { t } from '../lib/i18n.svelte';
  import type { FsEntry, FsListing } from '../lib/types';

  let {
    start = '',
    title = '',
    onpick,
    onclose,
  }: { start?: string; title?: string; onpick: (path: string) => void; onclose: () => void } = $props();

  let listing = $state<FsListing | null>(null);
  let busy = $state(false);
  let error = $state('');
  let typed = $state('');
  let newName = $state('');
  let creating = $state(false);

  async function open(path: string, fallback = true) {
    busy = true;
    error = '';
    try {
      listing = await api.get<FsListing>('/server/fs', { path });
      typed = listing.path;
    } catch (e) {
      if (fallback && path) return open('', false);
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
  // svelte-ignore state_referenced_locally
  void open(start);

  async function mkdir(e: Event) {
    e.preventDefault();
    if (!listing?.path || !newName.trim()) return;
    const sep = listing.path.includes('\\') && !listing.path.includes('/') ? '\\' : '/';
    const path = listing.path.replace(/[\\/]+$/, '') + sep + newName.trim();
    try {
      const made = await api.post<FsEntry>('/server/fs/mkdir', { path });
      creating = false;
      newName = '';
      await open(made.path);
    } catch (err) {
      error = errorText(err);
    }
  }

  const crumbs = $derived.by(() => {
    const p = listing?.path ?? '';
    if (!p) return [];
    const win = /^[A-Za-z]:\\/.test(p);
    const parts = p.split(/[\\/]/).filter(Boolean);
    const out: { name: string; path: string }[] = [];
    let acc = win ? '' : '/';
    if (!win) out.push({ name: '/', path: '/' });
    for (const part of parts) {
      acc = win ? (acc ? `${acc}${part}\\` : `${part}\\`) : `${acc}${part}/`;
      out.push({ name: part, path: win ? acc : acc.replace(/\/$/, '') || '/' });
    }
    return out;
  });
</script>

<Modal title={title || t('Choose a folder')} size="md" {onclose}>
  <form class="path" onsubmit={(e) => { e.preventDefault(); void open(typed); }}>
    <input class="input mono" bind:value={typed} placeholder={t('Type a path')} aria-label={t('Path')} />
    <button class="btn" type="submit">{t('Go')}</button>
  </form>
  {#if crumbs.length}
    <nav class="crumbs" aria-label={t('Path')}>
      <button class="crumb" onclick={() => open('', false)}><Icon name="hard-drives" size={15} /></button>
      {#each crumbs as c (c.path)}<span class="faint">/</span><button class="crumb" onclick={() => open(c.path)}>{c.name}</button>{/each}
    </nav>
  {/if}
  <div class="list" aria-busy={busy}>
    {#if busy && !listing}
      <div class="center"><Spinner /></div>
    {:else if listing}
      {#if listing.parent}
        <button class="entry" onclick={() => open(listing!.parent!)}><Icon name="arrow-up" />..</button>
      {/if}
      {#each listing.entries.filter((e) => e.dir) as e (e.path)}
        <button class="entry" ondblclick={() => onpick(e.path)} onclick={() => open(e.path)}>
          <Icon name="folder" /><span class="ellipsis">{e.name}</span>
        </button>
      {:else}
        <p class="faint center">{t('No folders here.')}</p>
      {/each}
    {/if}
  </div>
  {#if error}<p class="form-error" role="alert">{error}</p>{/if}
  {#if creating && listing?.path}
    <form class="path new" onsubmit={mkdir}>
      <input class="input" bind:value={newName} placeholder={t('New folder name')} aria-label={t('New folder name')} />
      <button class="btn primary" type="submit" disabled={!newName.trim()}>{t('Create')}</button>
      <button class="btn ghost" type="button" onclick={() => (creating = false)}>{t('Cancel')}</button>
    </form>
  {/if}
  {#snippet footer()}
    {#if listing?.free_space != null}<span class="caption muted free">{t('{size} free', { size: bytes(listing.free_space) })}</span>{/if}
    {#if listing?.path && !creating}
      <button class="btn ghost" onclick={() => (creating = true)}><Icon name="folder-plus" />{t('New folder')}</button>
    {/if}
    <button class="btn" onclick={onclose}>{t('Cancel')}</button>
    <button class="btn primary" disabled={!listing?.path} onclick={() => listing?.path && onpick(listing.path)}>{t('Use this folder')}</button>
  {/snippet}
</Modal>

<style>
  .path {
    display: flex;
    gap: 8px;
  }
  .path.new {
    margin-top: 12px;
  }
  .crumbs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
    margin: 12px 0 8px;
    font-size: var(--fs-small);
  }
  .crumb {
    padding: 2px 6px;
    border-radius: var(--r-xs);
    color: var(--text-dim);
    display: inline-flex;
  }
  .crumb:hover {
    background: var(--hover);
    color: var(--text);
  }
  .list {
    height: min(46vh, 380px);
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    padding: 4px;
    background: var(--bg0);
    margin-top: 8px;
  }
  .entry {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border-radius: var(--r-sm);
    text-align: left;
    font-size: var(--fs-small);
  }
  .entry:hover {
    background: var(--hover);
  }
  .entry :global(.icon) {
    color: var(--accent);
  }
  .center {
    display: grid;
    place-items: center;
    padding: 40px 0;
  }
  .free {
    margin-right: auto;
    align-self: center;
  }
</style>
