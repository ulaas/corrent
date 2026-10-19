<script lang="ts">
  // Running transcodes (GET /playback/sessions; admins see everyone's).
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { link } from '../../lib/router.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { t } from '../../lib/i18n.svelte';

  interface PlaybackSession {
    id: string;
    user: string;
    title: string;
    item_id?: number;
    encoder: string;
    video: boolean;
    segments: number;
    produced: number;
    idle_secs: number;
  }

  let list = $state<PlaybackSession[] | null>(null);
  let error = $state('');

  async function load() {
    try {
      list = await api.get<PlaybackSession[]>('/playback/sessions');
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  $effect(() => {
    void load();
    const h = setInterval(load, 3000);
    return () => clearInterval(h);
  });

  async function stop(s: PlaybackSession) {
    try {
      await api.del(`/playback/sessions/${s.id}`);
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
</script>

<p class="muted small lead">{t('Media the server is transcoding right now. Direct play does not show up here.')}</p>
{#if error}
  <p class="form-error">{error}</p>
{:else if !list}
  <Spinner />
{:else if !list.length}
  <div class="empty"><div class="empty-icon"><Icon name="broadcast" size={30} /></div><h3>{t('Nothing is being transcoded')}</h3></div>
{:else}
  <div class="sgroup">
    {#each list as s (s.id)}
      <div class="srow srow2">
        <div class="row">
          <span class="ic"><Icon name={s.video ? 'film-strip' : 'music-note'} size={20} /></span>
          <div class="stext">
            {#if s.item_id}<a class="slabel text-link" href="/item/{s.item_id}" use:link>{s.title}</a>{:else}<span class="slabel">{s.title}</span>{/if}
            <span class="shint">{s.user} · {s.encoder} · {t('{n} of {total} segments', { n: s.produced, total: s.segments })}{s.idle_secs > 5 ? ` · ${t('idle {n} s', { n: s.idle_secs })}` : ''}</span>
            <div class="bar"><div style:width="{s.segments ? (s.produced / s.segments) * 100 : 0}%"></div></div>
          </div>
        </div>
        <div class="sctl"><button class="btn sm danger" onclick={() => stop(s)}><Icon name="stop" size={14} />{t('Stop')}</button></div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .lead {
    margin-bottom: 16px;
  }
  .srow2 {
    grid-template-columns: 1fr auto;
  }
  .ic {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  .bar {
    margin-top: 6px;
    height: 4px;
    width: min(320px, 100%);
    border-radius: 2px;
    background: var(--bg3);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
    transition: width var(--dur-page);
  }
</style>
