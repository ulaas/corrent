<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { confirm } from '../../lib/confirm.svelte';
  import { ago } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';
  import type { Library, Podcast } from '../../lib/types';

  let podcasts = $state<Podcast[] | null>(null);
  let libraries = $state<Library[]>([]);
  let error = $state('');
  let url = $state('');
  let library = $state<number | null>(null);
  let keep = $state(5);
  let busy = $state(false);

  async function load() {
    try {
      podcasts = await api.get<Podcast[]>('/podcasts');
      libraries = (await api.get<Library[]>('/libraries')).filter((l) => l.kind === 'podcasts');
      library ??= libraries[0]?.id ?? null;
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();
  $effect(() => {
    const h = setInterval(load, 10000);
    return () => clearInterval(h);
  });

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!url.trim() || library == null) return;
    busy = true;
    try {
      await api.post<Podcast>('/podcasts', { url: url.trim(), library_id: library, keep });
      url = '';
      toast(t('Subscribed: new episodes download in the background.'), 'success');
      void load();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  async function remove(p: Podcast) {
    const r = await confirm({ title: t('Unsubscribe from “{name}”?', { name: p.title }), text: t('Downloaded episodes stay in the library.'), ok: t('Unsubscribe'), danger: true });
    if (!r.ok) return;
    try {
      await api.del(`/podcasts/${p.id}`);
      void load();
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  async function refresh(p: Podcast) {
    try {
      await api.post(`/podcasts/${p.id}/refresh`);
      toast(t('Checking for new episodes…'), 'success');
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }
</script>

{#if error}
  <p class="form-error">{error}</p>
{:else if !podcasts}
  <div class="center"><Spinner /></div>
{:else}
  <div class="sgroup">
    <h3>{t('Podcasts')}</h3>
    <p class="sdesc">{t('The server downloads new episodes of the podcasts you follow into a podcast library, and keeps the newest few.')}</p>
    {#if !libraries.length}
      <div class="srow"><span class="muted small">{t('Add a library of the kind “Podcasts” first (Settings → Libraries).')}</span></div>
    {:else}
      <form class="srow add" onsubmit={add}>
        <input class="input grow" type="url" required bind:value={url} placeholder="https://…/feed.xml" aria-label={t('Feed address')} />
        {#if libraries.length > 1}
          <select class="input" bind:value={library} aria-label={t('Library')}>
            {#each libraries as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
          </select>
        {/if}
        <label class="small muted keep">{t('Keep')} <input class="input num" type="number" min="0" max="1000" bind:value={keep} aria-label={t('Episodes to keep (0 = all)')} /></label>
        <button class="btn primary sm" disabled={busy}>{#if busy}<Spinner size={14} />{:else}<Icon name="plus" size={14} />{/if}{t('Subscribe')}</button>
      </form>
    {/if}
    {#each podcasts as p (p.id)}
      <div class="srow">
        <div class="stext">
          <span class="slabel">{p.title}</span>
          <span class="shint ellipsis">{p.url}</span>
          {#if p.error}<span class="form-error small">{p.error}</span>
          {:else}<span class="shint">{p.episodes === 1 ? t('1 episode') : t('{n} episodes', { n: p.episodes })} · {p.checked_at ? t('checked {when}', { when: ago(p.checked_at) }) : t('not checked yet')}</span>{/if}
        </div>
        <div class="sctl">
          <button class="btn sm" onclick={() => refresh(p)}><Icon name="arrows-clockwise" size={14} />{t('Check now')}</button>
          <button class="btn sm ghost" aria-label={t('Unsubscribe')} title={t('Unsubscribe')} onclick={() => remove(p)}><Icon name="trash" size={14} /></button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 60px 0;
  }
  .add {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
  .add .grow {
    flex: 1 1 260px;
  }
  .keep {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .keep .num {
    width: 72px;
  }
  .sctl {
    display: flex;
    gap: 8px;
  }
</style>
