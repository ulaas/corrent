<script lang="ts">
  import { session } from '../../lib/session.svelte';
  import { link } from '../../lib/events.svelte';
  import { live } from '../../lib/torrents.svelte';
  import { duration, bytes } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';

  const info = $derived(session.info);
  const stats = $derived(live.stats);
  const copy = (s: string) => navigator.clipboard?.writeText(s);
</script>

<div class="hero">
  <img src="/icons/corrent-128.png" alt="" width="72" height="72" />
  <div>
    <h3>Corrent</h3>
    <p class="muted small">{t('Media and downloads from your own server.')}</p>
  </div>
</div>

<div class="sgroup">
  <div class="srow"><span class="slabel">{t('Server name')}</span><span class="sctl">{info?.name}</span></div>
  <div class="srow"><span class="slabel">{t('Version')}</span><span class="sctl num">{info?.version} · API {info?.api_version}</span></div>
  <div class="srow"><span class="slabel">{t('Server ID')}</span><span class="sctl mono">{info?.id}</span></div>
  {#if info?.tls_fingerprint}
    <div class="srow wide"><div class="stext"><span class="slabel">{t('Certificate fingerprint')}</span><span class="shint">{t('Compare this with what your apps show when pairing.')}</span></div>
      <button class="fp mono" onclick={() => copy(info!.tls_fingerprint!)} title={t('Copy')}>{info.tls_fingerprint.match(/.{1,4}/g)?.join(' ')}</button></div>
  {/if}
  <div class="srow"><span class="slabel">{t('Live connection')}</span><span class="sctl">{#if link.connected}<span class="badge ok">{t('Connected')}</span>{:else}<span class="badge warn">{t('Reconnecting…')}</span>{/if}</span></div>
  {#if stats}
    <div class="srow"><span class="slabel">{t('Uptime')}</span><span class="sctl">{duration(stats.uptime_secs)}</span></div>
    <div class="srow"><span class="slabel">{t('Downloaded / uploaded (all time)')}</span><span class="sctl num">{bytes(stats.all_time_downloaded)} / {bytes(stats.all_time_uploaded)}</span></div>
    {#if stats.listen_port}<div class="srow"><span class="slabel">{t('Listening port')}</span><span class="sctl num">{stats.listen_port}</span></div>{/if}
  {/if}
</div>

<p class="caption faint">{t('Fonts: Inter and JetBrains Mono (SIL Open Font License). Icons: Phosphor (MIT). Video streaming: hls.js (Apache 2.0).')}</p>

<style>
  .hero {
    display: flex;
    align-items: center;
    gap: 18px;
    margin-bottom: 20px;
  }
  .hero img {
    border-radius: 20px;
    box-shadow: var(--shadow-3);
  }
  .fp {
    text-align: left;
    font-size: 12px;
    word-break: break-all;
    color: var(--text-dim);
  }
  .fp:hover {
    color: var(--text);
  }
</style>
