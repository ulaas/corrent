<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Modal from '../../components/Modal.svelte';
  import { api, errorText } from '../../lib/api';
  import { confirm } from '../../lib/confirm.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { can } from '../../lib/session.svelte';
  import { ago, date } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';
  import { Perm, type Device, type PairingCode } from '../../lib/types';
  import type { IconName } from '../../lib/icons';

  let devices = $state<Device[] | null>(null);
  let all = $state(false);
  let error = $state('');
  let pairing = $state<PairingCode | null>(null);
  let left = $state(0);

  async function load() {
    try {
      devices = await api.get<Device[]>('/devices', all ? { all: true } : undefined);
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  $effect(() => {
    void all;
    void load();
  });

  async function revoke(d: Device) {
    const r = await confirm({ title: t('Sign out “{name}”?', { name: d.name }), text: t('The device has to be paired again to connect.'), ok: t('Sign out'), danger: true });
    if (!r.ok) return;
    try {
      await api.del(`/devices/${d.id}`);
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function pair() {
    try {
      pairing = await api.post<PairingCode>('/auth/pair/code', {});
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  $effect(() => {
    if (!pairing) return;
    const p = pairing;
    const tick = () => {
      left = Math.max(0, p.expires_at - Math.floor(Date.now() / 1000));
    };
    tick();
    const h = setInterval(tick, 1000);
    return () => clearInterval(h);
  });

  const ICON: Record<string, IconName> = { windows: 'desktop', linux: 'desktop', macos: 'desktop', web: 'globe', tv: 'television-simple', android: 'devices', ios: 'devices' };
  const copy = (s: string) => navigator.clipboard?.writeText(s).then(() => toast(t('Copied'), 'success'));
</script>

<div class="sgroup">
  <h3>{t('Pair a device')}</h3>
  <p class="sdesc">{t('Apps and TVs sign in with a one-time 6-digit code instead of your password.')}</p>
  <div class="srow wide"><div><button class="btn primary" onclick={pair}><Icon name="qr-code" />{t('Create a pairing code')}</button></div></div>
</div>

<div class="sgroup">
  <h3>{t('Signed-in devices')}</h3>
  {#if can(Perm.USERS_MANAGE)}
    <div class="srow toggle-row"><label class="row small"><input type="checkbox" class="check" bind:checked={all} />{t("Show every user's devices")}</label></div>
  {/if}
  {#if error}
    <div class="srow"><p class="form-error">{error}</p></div>
  {:else if !devices}
    <div class="srow"><Spinner /></div>
  {:else if !devices.length}
    <div class="srow"><span class="muted small">{t('No paired devices yet. Browser sessions are not listed here.')}</span></div>
  {:else}
    {#each devices as d (d.id)}
      <div class="srow dev">
        <div class="row">
          <span class="dicon"><Icon name={ICON[d.platform] ?? 'devices'} size={20} /></span>
          <div class="stext">
            <span class="slabel">{d.name}{#if d.current}<span class="badge accent">{t('This device')}</span>{/if}</span>
            <span class="shint">{[d.platform, d.client].filter(Boolean).join(' · ')} · {t('Last seen {when}', { when: ago(d.last_seen) })} · {d.last_ip}</span>
            <span class="shint">{t('Paired {date}', { date: date(d.created_at) })}</span>
          </div>
        </div>
        <div class="sctl"><button class="btn danger sm" onclick={() => revoke(d)}>{t('Sign out')}</button></div>
      </div>
    {/each}
  {/if}
</div>

{#if pairing}
  <Modal title={t('Pairing code')} size="sm" onclose={() => (pairing = null)}>
    <div class="code-wrap">
      <p class="muted small">{t('Enter this code in the Corrent app on the new device.')}</p>
      <div class="code num" aria-label={pairing.code.split('').join(' ')}>
        {#each pairing.code.split('') as ch, i (i)}<span>{ch}</span>{/each}
      </div>
      {#if left > 0}
        <p class="caption muted">{t('Valid for {m}:{s}', { m: Math.floor(left / 60), s: String(left % 60).padStart(2, '0') })}</p>
      {:else}
        <p class="caption form-error">{t('This code has expired.')}</p>
      {/if}
      {#if pairing.qr_svg}
        <img class="qr" alt={t('QR code of the pairing link')} src={`data:image/svg+xml;base64,${btoa(pairing.qr_svg)}`} />
      {/if}
      <div class="link">
        <span class="mono caption ellipsis" title={pairing.link}>{pairing.link}</span>
        <button class="btn sm" onclick={() => copy(pairing!.link)}><Icon name="copy" size={14} />{t('Copy link')}</button>
      </div>
    </div>
    {#snippet footer()}
      <button class="btn" onclick={pair}><Icon name="arrows-clockwise" />{t('New code')}</button>
      <button class="btn primary" onclick={() => ((pairing = null), void load())}>{t('Done')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .qr {
    display: block;
    width: 200px;
    height: 200px;
    margin: 12px auto;
    border-radius: 8px;
    background: #fff;
  }
  .dev {
    grid-template-columns: 1fr auto;
  }
  .dicon {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--bg3);
    color: var(--text-dim);
    flex: none;
  }
  .slabel {
    display: inline-flex;
    gap: 8px;
    align-items: center;
  }
  .code-wrap {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    text-align: center;
  }
  .code {
    display: flex;
    gap: 8px;
  }
  .code span {
    width: 46px;
    height: 58px;
    display: grid;
    place-items: center;
    border-radius: var(--r-md);
    background: var(--bg0);
    border: 1px solid var(--border-hi);
    font-size: 30px;
    font-weight: 600;
    font-family: var(--font-mono);
  }
  .link {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    padding: 8px 8px 8px 12px;
    border-radius: var(--r-md);
    background: var(--bg2);
  }
</style>
