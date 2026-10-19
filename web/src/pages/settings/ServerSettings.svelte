<script lang="ts" module>
  import type { ServerSettings } from '../../lib/types';
  // Shared by every server section, so switching sections keeps unsaved edits.
  const store = $state<{ saved: ServerSettings | null; draft: ServerSettings | null; error: string }>({
    saved: null,
    draft: null,
    error: '',
  });
</script>

<script lang="ts">
  import { fly } from 'svelte/transition';
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import PathField from '../../components/PathField.svelte';
  import { api, errorText } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { can } from '../../lib/session.svelte';
  import { ms } from '../../lib/prefs.svelte';
  import { t, tk } from '../../lib/i18n.svelte';
  import { Perm } from '../../lib/types';

  let { section }: { section: string } = $props();

  const MASK = '••••••••';
  let busy = $state(false);
  const writable = $derived(can(Perm.SETTINGS_WRITE));

  async function load() {
    try {
      const s = await api.get<ServerSettings>('/server/settings');
      store.saved = s;
      store.draft = structuredClone(s);
      store.error = '';
    } catch (e) {
      store.error = errorText(e);
    }
  }
  if (!store.draft) void load();

  const dirty = $derived(!!store.draft && JSON.stringify(store.draft) !== JSON.stringify(store.saved));

  async function save() {
    if (!store.draft) return;
    busy = true;
    try {
      const s = await api.put<ServerSettings>('/server/settings', store.draft);
      store.saved = s;
      store.draft = structuredClone(s);
      toast(t('Settings saved'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  function discard() {
    if (store.saved) store.draft = structuredClone($state.snapshot(store.saved) as ServerSettings);
  }

  const DAYS = [tk('Mon'), tk('Tue'), tk('Wed'), tk('Thu'), tk('Fri'), tk('Sat'), tk('Sun')];
  const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  const mins = (v: string) => {
    const [h, m] = v.split(':').map(Number);
    return (h || 0) * 60 + (m || 0);
  };
  const s = $derived(store.draft);
</script>

{#if store.error}
  <p class="form-error">{store.error}</p>
{:else if !s}
  <div class="center"><Spinner /></div>
{:else}
  <fieldset class="fs" disabled={!writable}>
    {#if !writable}<p class="ro small muted"><Icon name="lock" size={14} />{t('You can see these settings but not change them.')}</p>{/if}

    {#if section === 'downloads'}
      <div class="sgroup">
        <h3>{t('Where downloads go')}</h3>
        <div class="srow wide">
          <div class="stext"><span class="slabel">{t('Default download folder')}</span><span class="shint">{t('New torrents are saved here unless a category says otherwise.')}</span></div>
          <PathField bind:value={s.downloads.save_path} label={t('Default download folder')} />
        </div>
        <div class="srow toggle-row"><Toggle bind:checked={s.downloads.create_subfolder} label={t('Create a subfolder for multi-file torrents')} /></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.downloads.start_paused} label={t('Add torrents paused')} hint={t('New torrents wait until you start them.')} /></div>
      </div>
    {:else if section === 'connection'}
      <div class="sgroup">
        <h3>{t('Listening')}</h3>
        <div class="srow"><div class="stext"><span class="slabel">{t('Listening port')}</span><span class="shint">{t('Peers connect to this port.')}</span></div>
          <div class="sctl"><input class="input num-input" type="number" min="1" max="65535" bind:value={s.connection.listen_port} disabled={s.connection.random_port} /></div></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.connection.random_port} label={t('Use a random port on every start')} /></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.connection.upnp} label={t('Forward the port with UPnP')} /></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Protocols')}</span></div>
          <div class="sctl"><select class="select" bind:value={s.connection.listen_mode}>
            <option value="Tcp">TCP</option><option value="Utp">µTP</option><option value="TcpAndUtp">TCP + µTP</option>
          </select></div></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.connection.ipv4_only} label={t('IPv4 only')} /></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Listen address')}</span><span class="shint">{t('A specific local IP to accept connections on. Leave empty for all.')}</span></div>
          <div class="sctl"><input class="input mono" bind:value={s.connection.listen_address} placeholder="0.0.0.0" /></div></div>
      </div>
      <div class="sgroup">
        <h3>{t('Connections')}</h3>
        <div class="srow"><div class="stext"><span class="slabel">{t('Maximum peers overall')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.connection.max_peers_global} /></div></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Maximum peers per torrent')}</span><span class="shint">{t('0 = unlimited')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.connection.max_peers_per_torrent} /></div></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Connection timeout (seconds)')}</span></div><div class="sctl"><input class="input num-input" type="number" min="1" bind:value={s.connection.connect_timeout_secs} /></div></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('IP blocklist URL')}</span><span class="shint">{t('A P2P or DAT list of addresses to refuse.')}</span></div><div class="sctl"><input class="input mono" bind:value={s.connection.blocklist_url} placeholder="https://…" /></div></div>
      </div>
      <div class="sgroup">
        <h3>{t('Proxy')}</h3>
        <div class="srow toggle-row"><Toggle bind:checked={s.connection.proxy_enabled} label={t('Connect through a SOCKS5 proxy')} /></div>
        {#if s.connection.proxy_enabled}
          <div class="srow"><div class="stext"><span class="slabel">{t('Host and port')}</span></div><div class="sctl"><input class="input" bind:value={s.connection.proxy_host} placeholder="proxy.example" /><input class="input num-input" type="number" bind:value={s.connection.proxy_port} /></div></div>
          <div class="srow"><div class="stext"><span class="slabel">{t('User name')}</span></div><div class="sctl"><input class="input" bind:value={s.connection.proxy_username} autocomplete="off" /></div></div>
          <div class="srow"><div class="stext"><span class="slabel">{t('Password')}</span>{#if s.connection.proxy_password === MASK}<span class="shint">{t('Stored. Type a new one to replace it.')}</span>{/if}</div>
            <div class="sctl"><input class="input" type="password" bind:value={s.connection.proxy_password} autocomplete="new-password" /></div></div>
        {/if}
      </div>
    {:else if section === 'speed'}
      <div class="sgroup">
        <h3>{t('Speed limits')}</h3>
        <p class="sdesc">{t('In KiB/s; 0 = unlimited.')}</p>
        <div class="srow"><div class="stext"><span class="slabel">{t('Download')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.speed.download_limit} /><span class="caption muted">KiB/s</span></div></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Upload')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.speed.upload_limit} /><span class="caption muted">KiB/s</span></div></div>
      </div>
      <div class="sgroup">
        <h3>{t('Alternative speed limits')}</h3>
        <p class="sdesc">{t('Slower limits you can switch on by hand or on a schedule.')}</p>
        <div class="srow"><div class="stext"><span class="slabel">{t('Download')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.speed.alt_download_limit} /><span class="caption muted">KiB/s</span></div></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Upload')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.speed.alt_upload_limit} /><span class="caption muted">KiB/s</span></div></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.speed.alt_enabled} label={t('Use the alternative limits now')} /></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.speed.schedule.enabled} label={t('Switch on automatically on a schedule')} /></div>
        {#if s.speed.schedule.enabled}
          <div class="srow"><div class="stext"><span class="slabel">{t('From – to')}</span></div>
            <div class="sctl">
              <input class="input" type="time" value={hhmm(s.speed.schedule.from_minutes)} onchange={(e) => (s.speed.schedule.from_minutes = mins(e.currentTarget.value))} aria-label={t('From')} />
              <span>–</span>
              <input class="input" type="time" value={hhmm(s.speed.schedule.to_minutes)} onchange={(e) => (s.speed.schedule.to_minutes = mins(e.currentTarget.value))} aria-label={t('To')} />
            </div></div>
          <div class="srow"><div class="stext"><span class="slabel">{t('Days')}</span></div>
            <div class="sctl days" role="group" aria-label={t('Days')}>
              {#each DAYS as d, i (d)}
                {@const on = (s.speed.schedule.days & (1 << i)) !== 0}
                <button type="button" class="chip" class:on aria-pressed={on} onclick={() => (s.speed.schedule.days ^= 1 << i)}>{t(d)}</button>
              {/each}
            </div></div>
        {/if}
      </div>
    {:else if section === 'queue'}
      <div class="sgroup">
        <h3>{t('Queueing')}</h3>
        <div class="srow toggle-row"><Toggle bind:checked={s.queue.enabled} label={t('Limit how many torrents run at once')} /></div>
        {#if s.queue.enabled}
          <div class="srow"><div class="stext"><span class="slabel">{t('Active downloads')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.queue.max_active_downloads} /></div></div>
          <div class="srow"><div class="stext"><span class="slabel">{t('Active uploads')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.queue.max_active_uploads} /></div></div>
          <div class="srow"><div class="stext"><span class="slabel">{t('Active torrents in total')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.queue.max_active_total} /></div></div>
          <div class="srow toggle-row"><Toggle bind:checked={s.queue.ignore_slow} label={t("Don't count slow torrents")} hint={t('Torrents below these speeds leave room for others.')} /></div>
          {#if s.queue.ignore_slow}
            <div class="srow"><div class="stext"><span class="slabel">{t('Slow download below')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.queue.slow_down_kib} /><span class="caption muted">KiB/s</span></div></div>
            <div class="srow"><div class="stext"><span class="slabel">{t('Slow upload below')}</span></div><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.queue.slow_up_kib} /><span class="caption muted">KiB/s</span></div></div>
          {/if}
        {/if}
      </div>
    {:else if section === 'seeding'}
      <div class="sgroup">
        <h3>{t('Stop seeding')}</h3>
        <div class="srow"><Toggle bind:checked={s.seeding.ratio_enabled} label={t('When the ratio reaches')} /><div class="sctl"><input class="input num-input" type="number" step="0.1" min="0" bind:value={s.seeding.ratio} disabled={!s.seeding.ratio_enabled} /></div></div>
        <div class="srow"><Toggle bind:checked={s.seeding.time_enabled} label={t('After seeding for (minutes)')} /><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.seeding.time_minutes} disabled={!s.seeding.time_enabled} /></div></div>
        <div class="srow"><Toggle bind:checked={s.seeding.inactive_enabled} label={t('After being inactive for (minutes)')} /><div class="sctl"><input class="input num-input" type="number" min="0" bind:value={s.seeding.inactive_minutes} disabled={!s.seeding.inactive_enabled} /></div></div>
        <div class="srow"><div class="stext"><span class="slabel">{t('Then')}</span></div>
          <div class="sctl"><select class="select" bind:value={s.seeding.action}>
            <option value="Pause">{t('Pause the torrent')}</option>
            <option value="Remove">{t('Remove the torrent')}</option>
            <option value="RemoveWithData">{t('Remove the torrent and its files')}</option>
          </select></div></div>
      </div>
    {:else if section === 'bittorrent'}
      <div class="sgroup">
        <h3>{t('Finding peers')}</h3>
        <div class="srow toggle-row"><Toggle bind:checked={s.bittorrent.dht} label={t('DHT')} hint={t('Find peers without trackers.')} /></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.bittorrent.lsd} label={t('Local peer discovery')} hint={t('Find peers on your own network.')} /></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.bittorrent.trackers_enabled} label={t('Use trackers')} /></div>
        <div class="srow toggle-row"><Toggle bind:checked={s.bittorrent.fastresume} label={t('Fast resume')} hint={t('Skip re-checking files after a restart.')} /></div>
        <div class="srow wide">
          <div class="stext"><span class="slabel">{t('Extra trackers for every torrent')}</span><span class="shint">{t('One per line.')}</span></div>
          <textarea class="input mono" rows="4" value={s.bittorrent.default_trackers.join('\n')} onchange={(e) => (s.bittorrent.default_trackers = e.currentTarget.value.split(/\s+/).filter(Boolean))}></textarea>
        </div>
      </div>
    {:else if section === 'search'}
      <div class="sgroup">
        <h3>{t('Torznab providers')}</h3>
        <p class="sdesc">{t('Jackett, Prowlarr or any Torznab indexer. The API key is stored on the server.')}</p>
        {#each s.search.providers as p, i (i)}
          <div class="srow provider">
            <Toggle bind:checked={p.enabled} />
            <input class="input" bind:value={p.name} placeholder={t('Name')} aria-label={t('Name')} />
            <input class="input mono" bind:value={p.url} placeholder="http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab" aria-label={t('Address')} />
            <input class="input" type="password" bind:value={p.api_key} placeholder={t('API key')} aria-label={t('API key')} autocomplete="off" />
            <button type="button" class="btn ghost icon" aria-label={t('Remove provider')} onclick={() => s.search.providers.splice(i, 1)}><Icon name="trash" /></button>
          </div>
        {:else}
          <div class="srow"><span class="muted small">{t('No providers yet.')}</span></div>
        {/each}
        <div class="srow wide"><button type="button" class="btn sm" onclick={() => s.search.providers.push({ name: '', url: '', api_key: '', enabled: true })}><Icon name="plus" size={14} />{t('Add provider')}</button></div>
      </div>
    {/if}
  </fieldset>

  {#if dirty && writable}
    <div class="savebar" transition:fly={{ y: 20, duration: ms(200) }}>
      <Icon name="info" />
      <span class="grow small">{t('You have unsaved changes.')}</span>
      <button class="btn ghost" onclick={discard}>{t('Discard')}</button>
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
  .fs {
    border: 0;
    margin: 0;
    padding: 0;
    min-width: 0;
  }
  .ro {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 14px;
  }
  .days {
    flex-wrap: wrap;
    gap: 4px;
  }
  .provider {
    grid-template-columns: auto 1fr 2fr 1fr auto;
  }
  @media (max-width: 760px) {
    .provider {
      grid-template-columns: 1fr;
    }
  }
</style>
