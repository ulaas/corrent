<script lang="ts">
  // Internet radio: stations the administrator added, played through the server.
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { api, errorText, url } from '../lib/api';
  import { music } from '../lib/player.svelte';
  import { can } from '../lib/session.svelte';
  import { toast } from '../lib/toast.svelte';
  import { t } from '../lib/i18n.svelte';
  import { Perm, type Station } from '../lib/types';

  let stations = $state<Station[] | null>(null);
  let error = $state('');
  let name = $state('');
  let address = $state('');
  let busy = $state(false);
  const admin = $derived(can(Perm.SETTINGS_WRITE));

  async function load() {
    try {
      stations = await api.get<Station[]>('/radio');
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  function play(s: Station, i: number) {
    music.play([{ id: -1 - i, stream: url(`/radio/${s.id}/stream`), title: s.name, artist: t('Radio'), album: '' }]);
  }

  async function add(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    try {
      await api.post('/radio', { name: name.trim(), url: address.trim() });
      name = '';
      address = '';
      void load();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  async function remove(s: Station) {
    try {
      await api.del(`/radio/${s.id}`);
      void load();
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }
</script>

<div class="page-head"><h1>{t('Radio')}</h1></div>
{#if error}
  <p class="form-error">{error}</p>
{:else if !stations}
  <div class="center"><Spinner /></div>
{:else}
  {#if !stations.length}
    <div class="empty"><div class="empty-icon"><Icon name="broadcast" size={30} /></div><h3>{t('No stations yet')}</h3><p>{admin ? t('Add a station by its stream address below.') : t('An administrator can add stations.')}</p></div>
  {/if}
  <ul class="stations">
    {#each stations as s, i (s.id)}
      {@const on = music.current?.stream?.includes(`/radio/${s.id}/`)}
      <li class:on>
        <button class="play" onclick={() => (on ? music.toggle() : play(s, i))} aria-label={t('Play {name}', { name: s.name })}>
          <Icon name={on && music.playing ? 'pause-fill' : 'play-fill'} size={20} />
        </button>
        <span class="grow ellipsis">{s.name}</span>
        {#if admin}<button class="btn ghost icon sm" aria-label={t('Remove')} title={t('Remove')} onclick={() => remove(s)}><Icon name="trash" size={16} /></button>{/if}
      </li>
    {/each}
  </ul>
  {#if admin}
    <form class="add" onsubmit={add}>
      <input class="input" required bind:value={name} placeholder={t('Station name')} aria-label={t('Station name')} />
      <input class="input grow" type="url" required bind:value={address} placeholder="https://…/stream" aria-label={t('Stream address')} />
      <button class="btn primary" disabled={busy}><Icon name="plus" size={16} />{t('Add station')}</button>
    </form>
  {/if}
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 60px 0;
  }
  .stations {
    list-style: none;
    margin: 0 0 20px;
    padding: 0;
    display: grid;
    gap: 6px;
    max-width: 720px;
  }
  .stations li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-radius: var(--r-md);
    background: var(--bg2);
  }
  .stations li.on {
    outline: 1px solid var(--accent);
  }
  .play {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    border: 0;
    background: var(--accent);
    color: var(--on-accent, #fff);
    cursor: pointer;
  }
  .grow {
    flex: 1;
  }
  .add {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    max-width: 720px;
  }
  .add .grow {
    flex: 1 1 260px;
  }
</style>
