<script lang="ts">
  import { fly } from 'svelte/transition';
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
    import { castConfig, castErrorText, castsActive, saveCastConfig, stopCastOn } from '../../lib/cast.svelte';
  import { t } from '../../lib/i18n.svelte';
  import { ms } from '../../lib/prefs.svelte';
  import { toast } from '../../lib/toast.svelte';
  import type { CastConfig, CastStatus } from '../../lib/types';
  import { everyVisible } from '../../lib/visibility.svelte';

  let saved = $state<CastConfig | null>(null);
  let draft = $state<CastConfig | null>(null);
  let devices = $state('');
  let error = $state('');
  let busy = $state(false);
  let active = $state<CastStatus[]>([]);

  const heights = [
    { value: 720, label: '720p' },
    { value: 1080, label: '1080p' },
    { value: 2160, label: '4K' },
    { value: 0, label: t('No limit') },
  ];

  function take(c: CastConfig) {
    saved = c;
    draft = structuredClone(c);
    devices = c.devices.join('\n');
  }

  void castConfig().then(take, (e) => (error = castErrorText(e)));

  $effect(() =>
    everyVisible(async () => {
      try {
        active = await castsActive();
      } catch {
        /* the list stays as it was */
      }
    }, 3000),
  );

  const pending = $derived(draft && { ...draft, devices: devices.split('\n').map((l) => l.trim()).filter(Boolean) });
  const dirty = $derived(!!saved && !!pending && JSON.stringify(pending) !== JSON.stringify(saved));
  const mbit = {
    get value() {
      return Math.round((draft?.max_bitrate_kbps ?? 0) / 1000);
    },
    set value(v: number) {
      if (draft) draft.max_bitrate_kbps = Math.max(0, Math.round(v)) * 1000;
    },
  };

  async function save() {
    if (!pending) return;
    busy = true;
    try {
      take(await saveCastConfig(pending));
      toast(t('Settings saved'), 'success');
    } catch (e) {
      toast(castErrorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  async function stop(s: CastStatus) {
    try {
      await stopCastOn(s.device.id);
      active = active.filter((a) => a.device.id !== s.device.id);
    } catch (e) {
      toast(castErrorText(e), 'error');
    }
  }
</script>

{#if error}
  <p class="form-error">{error}</p>
{:else if !draft}
  <div class="center"><Spinner /></div>
{:else}
  {#if active.length}
    <div class="sgroup">
      <h3>{t('Casting now')}</h3>
      {#each active as s (s.device.id)}
        <div class="srow">
          <div class="stext"><span class="slabel">{s.device.name}</span><span class="shint">{s.title}</span></div>
          <div class="sctl"><button class="btn sm" onclick={() => stop(s)}><Icon name="stop" size={14} />{t('Stop casting')}</button></div>
        </div>
      {/each}
    </div>
  {/if}

  <div class="sgroup">
    <h3>{t('Casting')}</h3>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.enabled} label={t('Allow casting to Chromecasts')} hint={t('While something is casting, the device fetches it from this computer over plain HTTP on the local network. Only signed links are served, and the port closes soon after casting ends.')} />
    </div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Largest picture sent as it is')}</span><span class="shint">{t('Taller videos are converted first. Older Chromecasts stop at 1080p.')}</span></div>
      <div class="sctl">
        <select class="input" bind:value={draft.max_height} aria-label={t('Largest picture sent as it is')}>
          {#each heights as h (h.value)}<option value={h.value}>{h.label}</option>{/each}
        </select>
      </div>
    </div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Highest bitrate sent as it is')}</span><span class="shint">{t('In Mbit/s; for a slow Wi-Fi. 0 means no limit.')}</span></div>
      <div class="sctl"><input class="input num-input" type="number" min="0" max="200" bind:value={mbit.value} aria-label={t('Highest bitrate sent as it is')} /></div>
    </div>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.hevc} label={t('The devices play H.265')} hint={t('Chromecast with Google TV and most Cast-enabled TVs do; such files then needn’t be converted.')} />
    </div>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.copy_streams} label={t('Copy streams when only the file type is in the way')} hint={t('MKV files the device can play aren’t converted, which is much lighter on this computer. If a device fails on the copy, it gets a converted stream instead.')} />
    </div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Port for the devices')}</span><span class="shint">{t('0 picks one. Set one to open it in a firewall.')}</span></div>
      <div class="sctl"><input class="input num-input" type="number" min="0" max="65535" bind:value={draft.port} aria-label={t('Port for the devices')} /></div>
    </div>
    <div class="srow col">
      <div class="stext"><span class="slabel">{t('Devices to add by hand')}</span><span class="shint">{t('One a line, for networks where the search finds nothing: 192.168.1.20 or Bedroom=192.168.1.20')}</span></div>
      <textarea class="input mono" rows="3" bind:value={devices} aria-label={t('Devices to add by hand')}></textarea>
    </div>
  </div>

  {#if dirty}
    <div class="savebar" transition:fly={{ y: 20, duration: ms(200) }}>
      <Icon name="info" />
      <span class="grow small">{t('You have unsaved changes.')}</span>
      <button class="btn ghost" onclick={() => saved && take(saved)}>{t('Discard')}</button>
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
  .col {
    grid-template-columns: 1fr;
    gap: 6px;
  }
  textarea {
    width: 100%;
    resize: vertical;
  }
</style>
