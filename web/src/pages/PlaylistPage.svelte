<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import Toggle from '../components/Toggle.svelte';
  import { api, errorText } from '../lib/api';
  import { link, go } from '../lib/router.svelte';
  import { clock, runtime } from '../lib/format';
  import { confirm } from '../lib/confirm.svelte';
  import { toast } from '../lib/toast.svelte';
  import { playList } from '../lib/lists';
  import { t } from '../lib/i18n.svelte';
  import type { Collection, Item, Playlist } from '../lib/types';

  let { params, collection = false }: { params: Record<string, string>; collection?: boolean } = $props();
  const id = $derived(+params.id);
  let info = $state<Playlist | Collection | null>(null);
  let items = $state<Item[]>([]);
  let error = $state('');
  const mine = $derived(!collection && (info as Playlist | null)?.mine === true);

  async function load() {
    try {
      if (collection) {
        const all = await api.get<Collection[]>('/collections');
        info = all.find((c) => c.id === id) ?? null;
        items = await api.get<Item[]>(`/collections/${id}/items`);
      } else {
        info = await api.get<Playlist>(`/playlists/${id}`);
        items = await api.get<Item[]>(`/playlists/${id}/items`);
      }
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  $effect(() => {
    void id;
    void load();
  });

  async function edit(body: Record<string, unknown>) {
    try {
      info = await api.patch<Playlist>(`/playlists/${id}`, body);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function removeAt(i: number) {
    await api.del(`/playlists/${id}/items/${i}`).catch((e) => toast(errorText(e), 'error'));
    void load();
  }

  async function move(from: number, to: number) {
    if (to < 0 || to >= items.length) return;
    await api.post(`/playlists/${id}/move`, { from, to }).catch((e) => toast(errorText(e), 'error'));
    void load();
  }

  async function remove() {
    const r = await confirm({ title: t('Delete “{name}”?', { name: info?.name ?? '' }), ok: t('Delete'), danger: true });
    if (!r.ok) return;
    await api.del(`/playlists/${id}`).catch((e) => toast(errorText(e), 'error'));
    go('/playlists');
  }

  const total = $derived(items.reduce((s, i) => s + (i.runtime_ms ?? 0), 0));
</script>

<div class="page">
  {#if error}
    <p class="form-error">{error}</p>
  {:else if !info}
    <div class="empty"><Spinner /></div>
  {:else}
    <div class="page-head">
      {#if mine}
        <input class="title-input" value={info.name} aria-label={t('Name')} onchange={(e) => edit({ name: e.currentTarget.value })} />
      {:else}
        <h1>{info.name}</h1>
      {/if}
      <span class="muted">{items.length === 1 ? t('1 item') : t('{n} items', { n: items.length })}{total ? ` · ${runtime(total)}` : ''}</span>
    </div>
    <div class="actions">
      <button class="btn primary" disabled={!items.length} onclick={() => playList(items)}><Icon name="play-fill" />{t('Play')}</button>
      <button class="btn" disabled={!items.length} onclick={() => playList(items, 0, true)}><Icon name="shuffle" />{t('Shuffle')}</button>
      {#if mine}
        <Toggle checked={(info as Playlist).public} label={t('Others can see it')} onchange={(v: boolean) => edit({ public: v })} />
        <button class="btn ghost" onclick={remove}><Icon name="trash" />{t('Delete')}</button>
      {/if}
    </div>
    <ol class="rows">
      {#each items as it, i (it.id + '-' + i)}
        <li>
          <button class="row-btn" onclick={() => playList(items, i)}>
            <span class="n num">{i + 1}</span>
            <span class="grow"><span class="ellipsis t">{it.title}</span>{#if it.context?.length}<span class="ellipsis caption muted">{it.context.join(' · ')}</span>{/if}</span>
            <span class="caption muted num">{clock(it.runtime_ms)}</span>
          </button>
          <a class="btn ghost icon sm" href="/item/{it.id}" use:link aria-label={t('Open')} title={t('Open')}><Icon name="arrow-square-out" size={14} /></a>
          {#if mine}
            <button class="btn ghost icon sm" aria-label={t('Move up')} title={t('Move up')} onclick={() => move(i, i - 1)}><Icon name="arrow-up" size={14} /></button>
            <button class="btn ghost icon sm" aria-label={t('Move down')} title={t('Move down')} onclick={() => move(i, i + 1)}><Icon name="arrow-down" size={14} /></button>
            <button class="btn ghost icon sm" aria-label={t('Remove')} title={t('Remove')} onclick={() => removeAt(i)}><Icon name="x" size={14} /></button>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .title-input {
    font: inherit;
    font-size: var(--fs-h1, 2rem);
    font-weight: 700;
    background: none;
    border: 0;
    border-bottom: 1px dashed var(--border);
    color: var(--text);
    min-width: 0;
    width: 100%;
    max-width: 640px;
  }
  .actions {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    margin: 8px 0 20px;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 900px;
  }
  .rows li {
    display: flex;
    align-items: center;
    gap: 4px;
    border-radius: var(--r-md);
  }
  .rows li:hover {
    background: var(--bg2);
  }
  .row-btn {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 14px;
    min-width: 0;
    padding: 8px 10px;
    background: none;
    border: 0;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .n {
    width: 28px;
    color: var(--text-mute);
  }
  .grow {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
</style>
