<script lang="ts">
  import { fly } from 'svelte/transition';
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { ms } from '../../lib/prefs.svelte';
  import { t } from '../../lib/i18n.svelte';
  import type { RemoteAccess, RemoteConfig } from '../../lib/types';

  let saved = $state<RemoteAccess | null>(null);
  let draft = $state<RemoteConfig | null>(null);
  let lists = $state({ allowed: '', proxies: '', hosts: '' });
  let error = $state('');
  let busy = $state(false);

  const toText = (l: string[]) => l.join('\n');
  const toList = (s: string) => s.split(/[\n,]/).map((x) => x.trim()).filter(Boolean);

  function take(r: RemoteAccess) {
    saved = r;
    draft = structuredClone(r.config);
    lists = { allowed: toText(r.config.allowed_networks), proxies: toText(r.config.trusted_proxies), hosts: toText(r.config.hostnames) };
  }

  async function load() {
    try {
      take(await api.get<RemoteAccess>('/server/remote'));
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  // The status (Connect address, mapped port) settles a few seconds after a change.
  $effect(() => {
    const h = setInterval(async () => {
      try {
        const r = await api.get<RemoteAccess>('/server/remote');
        if (saved) saved.status = r.status;
      } catch {
        /* shown on the next save */
      }
    }, 5000);
    return () => clearInterval(h);
  });

  const pending = $derived.by(() => {
    if (!draft) return null;
    return { ...draft, allowed_networks: toList(lists.allowed), trusted_proxies: toList(lists.proxies), hostnames: toList(lists.hosts) };
  });
  const dirty = $derived(!!saved && !!pending && JSON.stringify(pending) !== JSON.stringify(saved.config));

  async function save() {
    if (!pending) return;
    busy = true;
    try {
      take(await api.put<RemoteAccess>('/server/remote', pending));
      toast(t('Settings saved'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  const copy = (s: string) => navigator.clipboard?.writeText(s).then(() => toast(t('Copied'), 'success'));
  const st = $derived(saved?.status);
</script>

{#if error}
  <p class="form-error">{error}</p>
{:else if !draft || !st}
  <div class="center"><Spinner /></div>
{:else}
  <div class="sgroup">
    <h3>{t('On this network')}</h3>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.discovery} label={t('Announce the server on the local network')} hint={t('The Corrent app finds it in “Connect to a server” without typing an address.')} />
    </div>
    {#if st.discovery_error}<div class="srow"><p class="form-error small">{st.discovery_error}</p></div>{/if}
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.dlna} label={t('Share libraries with TVs and players (DLNA)')} hint={t('Smart TVs, receivers, VLC and Kodi on this network find the server and play its files. DLNA has no sign-in: anyone on this network can browse what the chosen account may see.')} />
    </div>
    {#if draft.dlna}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Account to share')}</span><span class="shint">{t('Empty: the first administrator. A limited account keeps some libraries private.')}</span></div>
        <div class="sctl"><input class="input" bind:value={draft.dlna_user} placeholder={t('User name')} aria-label={t('Account to share')} /></div>
      </div>
      {#if st.dlna_port}<div class="srow col"><span class="small muted">{t('DLNA is on (port {port}).', { port: st.dlna_port })}</span></div>{/if}
    {/if}
    {#if st.dlna_error}<div class="srow"><p class="form-error small">{st.dlna_error}</p></div>{/if}
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.jellyfin_api} label={t('Let Jellyfin apps connect')} hint={t('Jellyfin Web, Android TV, Swiftfin, Findroid, Infuse and the Kodi add-on sign in with a Corrent account and play the libraries. On this network they use plain HTTP on the port below; browsers that trust this server’s certificate can also use its address with /jellyfin.')} />
    </div>
    {#if draft.jellyfin_api}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Port for Jellyfin apps')}</span><span class="shint">{t('Type this computer’s IP address and the port in the app (a name works once it’s listed under Host names). 0 turns the plain-HTTP port off.')}</span></div>
        <div class="sctl"><input class="input num-input" type="number" min="0" max="65535" bind:value={draft.jellyfin_port} aria-label={t('Port for Jellyfin apps')} /></div>
      </div>
      <div class="srow toggle-row">
        <Toggle bind:checked={draft.jellyfin_discovery} label={t('Let Jellyfin apps find this server')} hint={t('Answers the apps’ search on this network (UDP 7359).')} />
      </div>
      {#if st.jellyfin_port}<div class="srow col"><span class="small muted">{t('Jellyfin apps: http://<this computer>:{port}', { port: st.jellyfin_port })}</span></div>{/if}
    {/if}
    {#if st.jellyfin_error}<div class="srow"><p class="form-error small">{st.jellyfin_error}</p></div>{/if}
  </div>

  <div class="sgroup">
    <h3>{t('Corrent Connect')}</h3>
    <p class="sdesc">{t('Reach this server from anywhere, even behind a router or a mobile network, without opening ports. Connections are end-to-end encrypted; when a direct path is impossible they pass through a relay that can’t read them.')}</p>
    <div class="srow toggle-row"><Toggle bind:checked={draft.connect} label={t('Turn on Corrent Connect')} /></div>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.connect_relay} label={t('Use public relays')} hint={t('Needed outside this network. While on, the server’s addresses are published to the relay operator’s lookup service.')} />
    </div>
    {#if draft.connect_relay}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Your own relay')}</span><span class="shint">{t('Optional: the address of a relay you run (iroh-relay) to use instead of the public ones.')}</span></div>
        <div class="sctl"><input class="input" type="url" bind:value={draft.connect_relay_url} placeholder="https://relay.example.org" aria-label={t('Your own relay')} /></div>
      </div>
    {/if}
    {#if st.connect_id}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Connect address')}</span><span class="shint mono ellipsis">{st.connect_id}</span></div>
        <div class="sctl"><button class="btn sm" onclick={() => copy(st.connect_id!)}><Icon name="copy" size={14} />{t('Copy')}</button></div>
      </div>
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Relay')}</span><span class="shint">{st.connect_relay ?? t('Not connected to a relay yet')}</span></div>
      </div>
      <div class="srow col"><span class="small muted">{t('New pairing codes (Settings → Devices) include this address, so paired apps use it when the server’s address can’t be reached.')}</span></div>
    {/if}
    {#if st.connect_error}<div class="srow"><p class="form-error small">{st.connect_error}</p></div>{/if}
  </div>

  <div class="sgroup">
    <h3>{t('Direct access')}</h3>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.remote_access} label={t('Accept clients outside the local network')} hint={t('Browsers and apps that reach this server over the internet. They still need an account.')} />
    </div>
    <div class="srow toggle-row">
      <Toggle bind:checked={draft.port_mapping} label={t('Ask the router to forward the port')} hint={t('UPnP, NAT-PMP or PCP, for port {port}.', { port: st.https_port })} />
    </div>
    {#if st.mapped_address}
      <div class="srow"><div class="stext"><span class="slabel">{t('Public address')}</span><span class="shint mono">{st.mapped_address}</span></div></div>
    {/if}
    {#if st.port_mapping_error}<div class="srow"><p class="form-error small">{st.port_mapping_error}</p></div>{/if}
  </div>

  <div class="sgroup">
    <h3>{t('Advanced')}</h3>
    <div class="srow col">
      <div class="stext"><span class="slabel">{t('Networks treated as local')}</span><span class="shint">{t('One per line, e.g. 203.0.113.0/24.')}</span></div>
      <textarea class="input mono" rows="3" bind:value={lists.allowed}></textarea>
    </div>
    <div class="srow col">
      <div class="stext"><span class="slabel">{t('Trusted reverse proxies')}</span><span class="shint">{t('Their X-Forwarded-For header is believed. One network per line.')}</span></div>
      <textarea class="input mono" rows="2" bind:value={lists.proxies}></textarea>
    </div>
    <div class="srow col">
      <div class="stext"><span class="slabel">{t('Extra host names')}</span><span class="shint">{t('Names browsers may use for this server, e.g. media.example.org.')}</span></div>
      <textarea class="input mono" rows="2" bind:value={lists.hosts}></textarea>
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
