<script lang="ts">
  // The server's ceiling for conversions (`[transcode]` in its server.toml) and the encoder it
  // found; the desktop's page is src/app/media/transcode_settings.rs.
  import { fly } from 'svelte/transition';
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { t, tk } from '../../lib/i18n.svelte';
  import { ms } from '../../lib/prefs.svelte';
  import { QUALITIES, qualityLabel } from '../../lib/quality';
  import { toast } from '../../lib/toast.svelte';
  import type { TranscodeConfig, TranscodeInfo } from '../../lib/types';

  let saved = $state<TranscodeConfig | null>(null);
  let draft = $state<TranscodeConfig | null>(null);
  let encoder = $state<string | undefined>();
  let error = $state('');
  let busy = $state(false);

  /** libx264 presets, lightest first (`X264_PRESETS` on the server). */
  const EFFORTS = [
    { id: 'ultrafast', label: tk('Lightest') },
    { id: 'superfast', label: tk('Lighter') },
    { id: 'veryfast', label: tk('Balanced') },
    { id: 'faster', label: tk('Better') },
    { id: 'fast', label: tk('Better still') },
    { id: 'medium', label: tk('Best picture') },
  ];

  function take(info: TranscodeInfo) {
    const { encoder: e, ...config } = info;
    encoder = e;
    saved = config;
    draft = { ...config };
  }

  api.get<TranscodeInfo>('/transcode/config').then(take, (e) => (error = errorText(e)));

  const dirty = $derived(!!saved && !!draft && JSON.stringify(draft) !== JSON.stringify(saved));
  const whole = (v: unknown, min: number, max: number) => Math.max(min, Math.min(max, Math.round(Number(v) || 0)));
  const mbit = {
    get value() {
      return Math.round((draft?.max_bitrate_kbps ?? 0) / 1000);
    },
    set value(v: number) {
      if (draft) draft.max_bitrate_kbps = whole(v, 0, 100) * 1000;
    },
  };
  const sessions = {
    get value() {
      return draft?.max_sessions ?? 1;
    },
    set value(v: number) {
      if (draft) draft.max_sessions = whole(v, 1, 16);
    },
  };

  async function save() {
    if (!draft) return;
    busy = true;
    try {
      take(await api.put<TranscodeInfo>('/transcode/config', $state.snapshot(draft)));
      toast(t('Settings saved'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }
</script>

{#if error}
  <p class="form-error">{error}</p>
{:else if !draft}
  <div class="center"><Spinner /></div>
{:else}
  <div class="sgroup">
    <h3>{t('Transcoding')}</h3>
    <p class="sdesc">{t('How media is converted when a player can’t play it as it is, or asks for a lower quality. Files that play as they are aren’t touched.')}</p>
    <p class="sdesc">{encoder ? t('Encoder in use: {name}', { name: encoder }) : t('No working video encoder was found: only sound can be converted.')}</p>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Largest converted picture')}</span><span class="shint">{t('Whatever a player asks for, conversions are no taller than this.')}</span></div>
      <div class="sctl chips" role="group" aria-label={t('Largest converted picture')}>
        <button type="button" class="chip" class:on={draft.max_height === 0} aria-pressed={draft.max_height === 0} onclick={() => draft && (draft.max_height = 0)}>{t('No limit')}</button>
        {#each QUALITIES as q (q.id)}
          <button type="button" class="chip" class:on={draft.max_height === q.height} aria-pressed={draft.max_height === q.height} onclick={() => draft && (draft.max_height = q.height)}>{qualityLabel(q.id)}</button>
        {/each}
      </div>
    </div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Highest converted bitrate')}</span><span class="shint">{t('In Mbit/s; for a slow upload link. 0 means no limit.')}</span></div>
      <div class="sctl"><input class="input num-input" type="number" min="0" max="100" bind:value={mbit.value} aria-label={t('Highest converted bitrate')} /></div>
    </div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Encoding effort')}</span><span class="shint">{t('Lighter is easier on the processor; better looks cleaner at the same bitrate. Graphics-card encoders ignore this.')}</span></div>
      <div class="sctl chips" role="group" aria-label={t('Encoding effort')}>
        {#each EFFORTS as e (e.id)}
          <button type="button" class="chip" class:on={draft.x264_preset === e.id} aria-pressed={draft.x264_preset === e.id} onclick={() => draft && (draft.x264_preset = e.id)}>{t(e.label)}</button>
        {/each}
      </div>
    </div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Conversions at once')}</span><span class="shint">{t('More players than this get an error instead of a stuttering picture.')}</span></div>
      <div class="sctl"><input class="input num-input" type="number" min="1" max="16" bind:value={sessions.value} aria-label={t('Conversions at once')} /></div>
    </div>
  </div>

  {#if dirty}
    <div class="savebar" transition:fly={{ y: 20, duration: ms(200) }}>
      <Icon name="info" />
      <span class="grow small">{t('You have unsaved changes.')}</span>
      <button class="btn ghost" onclick={() => saved && (draft = { ...saved })}>{t('Discard')}</button>
      <button class="btn primary" onclick={save} disabled={busy}>{#if busy}<Spinner size={14} />{/if}{t('Save changes')}</button>
    </div>
  {/if}
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 60px 0;
  }
  .chips {
    flex-wrap: wrap;
    gap: 6px;
  }
</style>
