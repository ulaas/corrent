<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { confirm } from '../../lib/confirm.svelte';
  import { t, tk } from '../../lib/i18n.svelte';
  import type { NotifyKind, NotifyTarget, PluginInfo } from '../../lib/types';

  const EVENTS: [string, string][] = [
    ['download_done', tk('A download finished')],
    ['torrent_added', tk('A torrent was added')],
    ['release_grabbed', tk('A wanted release was found')],
    ['device_paired', tk('A device was paired')],
    ['task_failed', tk('A server task failed')],
  ];
  const KINDS: [NotifyKind, string, string][] = [
    ['ntfy', tk('ntfy'), 'https://ntfy.sh/my-topic'],
    ['discord', tk('Discord'), 'https://discord.com/api/webhooks/…'],
    ['webhook', tk('Webhook (JSON)'), 'https://example.org/hook'],
    ['plugin', tk('Notifier plugin'), ''],
  ];

  let targets = $state<NotifyTarget[] | null>(null);
  let notifiers = $state<PluginInfo[]>([]);
  let error = $state('');
  let draft = $state<NotifyTarget | null>(null);
  let busy = $state(false);

  async function load() {
    try {
      targets = await api.get<NotifyTarget[]>('/notifications');
      notifiers = (await api.get<PluginInfo[]>('/plugins').catch(() => [])).filter((p) => p.manifest.kinds.includes('notifier'));
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  function add() {
    draft = { id: 0, name: '', kind: 'ntfy', url: '', plugin: notifiers[0]?.manifest.id ?? '', events: [], enabled: true };
  }

  async function save() {
    if (!draft) return;
    busy = true;
    try {
      const d = $state.snapshot(draft) as NotifyTarget;
      if (d.id) await api.put(`/notifications/${d.id}`, d);
      else await api.post('/notifications', d);
      draft = null;
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  async function test(n: NotifyTarget) {
    try {
      await api.post(`/notifications/${n.id}/test`);
      toast(t('Test notification sent'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function remove(n: NotifyTarget) {
    const r = await confirm({ title: t('Remove “{name}”?', { name: n.name }), ok: t('Remove'), danger: true });
    if (!r.ok) return;
    try {
      await api.del(`/notifications/${n.id}`);
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  function toggleEvent(e: string, on: boolean) {
    if (!draft) return;
    draft.events = on ? [...draft.events, e] : draft.events.filter((x) => x !== e);
  }
  const kindLabel = (k: NotifyKind) => t(KINDS.find((x) => x[0] === k)?.[1] ?? k);
</script>

<div class="sgroup">
  <h3>{t('Notifications')}</h3>
  <p class="sdesc">{t('Get told when downloads finish, wanted releases are found or a new device signs in — on your phone with ntfy, in Discord, or anywhere a webhook goes.')}</p>
  <div class="srow wide"><div><button class="btn primary" onclick={add}><Icon name="plus" />{t('Add a destination')}</button></div></div>
</div>

{#if error}
  <p class="form-error">{error}</p>
{:else if !targets}
  <div class="center"><Spinner /></div>
{:else}
  <div class="sgroup">
    {#if !targets.length}<div class="srow"><span class="muted small">{t('No destinations yet.')}</span></div>{/if}
    {#each targets as n (n.id)}
      <div class="srow target">
        <div class="stext">
          <span class="slabel">{n.name} <span class="badge">{kindLabel(n.kind)}</span>{#if !n.enabled}<span class="badge">{t('Off')}</span>{/if}</span>
          <span class="shint">{n.events.length ? n.events.map((e) => t(EVENTS.find((x) => x[0] === e)?.[1] ?? e)).join(', ') : t('Every event')}</span>
        </div>
        <div class="sctl">
          <button class="btn sm" onclick={() => test(n)}><Icon name="bell" size={14} />{t('Test')}</button>
          <button class="btn sm" onclick={() => (draft = structuredClone($state.snapshot(n) as NotifyTarget))}><Icon name="pencil-simple" size={14} />{t('Edit')}</button>
          <button class="btn sm danger" onclick={() => remove(n)} aria-label={t('Remove')}><Icon name="trash" size={14} /></button>
        </div>
      </div>
    {/each}
  </div>
{/if}

{#if draft}
  <div class="sgroup">
    <h3>{draft.id ? t('Edit destination') : t('New destination')}</h3>
    <div class="srow"><div class="stext"><span class="slabel">{t('Name')}</span></div><div class="sctl"><input class="input" bind:value={draft.name} placeholder={t('My phone')} /></div></div>
    <div class="srow">
      <div class="stext"><span class="slabel">{t('Kind')}</span></div>
      <div class="sctl"><select class="select" bind:value={draft.kind}>{#each KINDS as [k, label] (k)}<option value={k}>{t(label)}</option>{/each}</select></div>
    </div>
    {#if draft.kind === 'plugin'}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Plugin')}</span>{#if !notifiers.length}<span class="shint">{t('No notifier plugin is installed.')}</span>{/if}</div>
        <div class="sctl"><select class="select" bind:value={draft.plugin}>{#each notifiers as p (p.manifest.id)}<option value={p.manifest.id}>{p.manifest.name}</option>{/each}</select></div>
      </div>
    {:else}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Address')}</span></div>
        <div class="sctl"><input class="input mono" bind:value={draft.url} placeholder={KINDS.find((k) => k[0] === draft!.kind)?.[2]} /></div>
      </div>
    {/if}
    <div class="srow col">
      <div class="stext"><span class="slabel">{t('Events')}</span><span class="shint">{t('None ticked: every event.')}</span></div>
      <div class="events">
        {#each EVENTS as [e, label] (e)}
          <label class="row small"><input type="checkbox" class="check" checked={draft.events.includes(e)} onchange={(ev) => toggleEvent(e, (ev.currentTarget as HTMLInputElement).checked)} />{t(label)}</label>
        {/each}
      </div>
    </div>
    <div class="srow toggle-row"><Toggle bind:checked={draft.enabled} label={t('On')} /></div>
    <div class="srow wide">
      <div class="row end">
        <button class="btn ghost" onclick={() => (draft = null)}>{t('Cancel')}</button>
        <button class="btn primary" onclick={save} disabled={busy}>{t('Save')}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 60px 0;
  }
  .target {
    grid-template-columns: 1fr auto;
  }
  .sctl {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .col {
    grid-template-columns: 1fr;
    gap: 6px;
  }
  .events {
    display: grid;
    gap: 6px;
  }
  .end {
    justify-content: flex-end;
    gap: 8px;
  }
</style>
