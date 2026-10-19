<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { confirm } from '../../lib/confirm.svelte';
  import { can } from '../../lib/session.svelte';
  import { t, tk } from '../../lib/i18n.svelte';
  import { Perm, type PluginInfo } from '../../lib/types';

  const MASK = '••••••••';
  let plugins = $state<PluginInfo[] | null>(null);
  let error = $state('');
  let edits = $state<Record<string, Record<string, string>>>({});
  let open = $state<string | null>(null);
  let busy = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
  const manage = $derived(can(Perm.PLUGINS_MANAGE));

  const KIND: Record<string, string> = {
    metadata: tk('Metadata'),
    indexer: tk('Indexer'),
    notifier: tk('Notifications'),
    subtitles: tk('Subtitles'),
    parser: tk('Names'),
    task: tk('Task'),
    events: tk('Events'),
  };

  interface CatalogItem {
    id: string;
    name: string;
    version: string;
    description: string;
    installed: string | null;
  }
  let catalog = $state<{ configured: boolean; entries: CatalogItem[] } | null>(null);
  let catalogError = $state('');
  let installing = $state('');

  async function loadCatalog() {
    if (!manage) return;
    try {
      catalog = await api.get('/plugins/catalog');
      catalogError = '';
    } catch (e) {
      catalogError = errorText(e);
    }
  }
  void loadCatalog();

  async function installFromCatalog(id: string) {
    installing = id;
    try {
      await api.post('/plugins/catalog/install', { id });
      toast(t('Plugin installed'), 'success');
      void load();
      void loadCatalog();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      installing = '';
    }
  }

  async function load() {
    try {
      plugins = await api.get<PluginInfo[]>('/plugins');
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  function replace(p: PluginInfo) {
    if (plugins) plugins = plugins.map((x) => (x.manifest.id === p.manifest.id ? p : x));
  }

  async function setEnabled(p: PluginInfo, enabled: boolean) {
    try {
      replace(await api.put<PluginInfo>(`/plugins/${p.manifest.id}/enabled`, { enabled }));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  function edit(p: PluginInfo) {
    open = open === p.manifest.id ? null : p.manifest.id;
    edits[p.manifest.id] = { ...p.settings };
  }

  async function saveSettings(p: PluginInfo) {
    busy = true;
    try {
      replace(await api.put<PluginInfo>(`/plugins/${p.manifest.id}/settings`, edits[p.manifest.id]));
      open = null;
      toast(t('Settings saved'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  async function remove(p: PluginInfo) {
    const r = await confirm({ title: t('Remove “{name}”?', { name: p.manifest.name }), text: t('Its settings are deleted too.'), ok: t('Remove'), danger: true });
    if (!r.ok) return;
    try {
      await api.del(`/plugins/${p.manifest.id}`);
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  /** A plugin is a folder with plugin.toml and plugin.wasm: pick both files. */
  async function install(files: FileList | null) {
    if (!files) return;
    const manifest = [...files].find((f) => f.name.endsWith('.toml'));
    const wasm = [...files].find((f) => f.name.endsWith('.wasm'));
    if (!manifest || !wasm) {
      toast(t('Choose the plugin’s .toml and .wasm files together.'), 'error');
      return;
    }
    const form = new FormData();
    form.append('manifest', manifest);
    form.append('wasm', wasm);
    busy = true;
    try {
      await api.post('/plugins', form);
      toast(t('Plugin installed'), 'success');
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
      if (fileInput) fileInput.value = '';
    }
  }
</script>

<div class="sgroup">
  <h3>{t('Plugins')}</h3>
  <p class="sdesc">{t('Plugins find metadata and artwork, search indexers and send notifications. They run sandboxed and may only reach the hosts they list.')}</p>
  {#if manage}
    <div class="srow wide">
      <div>
        <button class="btn" onclick={() => fileInput?.click()} disabled={busy}><Icon name="upload-simple" />{t('Install a plugin…')}</button>
        <input bind:this={fileInput} type="file" accept=".toml,.wasm" multiple hidden onchange={(e) => install((e.currentTarget as HTMLInputElement).files)} />
      </div>
    </div>
  {/if}
</div>

{#if error}
  <p class="form-error">{error}</p>
{:else if !plugins}
  <div class="center"><Spinner /></div>
{:else}
  <div class="sgroup">
    {#each plugins as p (p.manifest.id)}
      <div class="srow plugin">
        <div class="stext">
          <span class="slabel">
            {p.manifest.name}
            <span class="badge">{p.manifest.version}</span>
            {#each p.manifest.kinds as k (k)}<span class="badge accent">{t(KIND[k] ?? k)}</span>{/each}
            {#if p.builtin}<span class="badge">{t('Built in')}</span>{/if}
          </span>
          {#if p.manifest.description}<span class="shint">{p.manifest.description}</span>{/if}
          {#if p.manifest.permissions.hosts.length}
            <span class="shint mono small">{t('Reaches: {hosts}', { hosts: p.manifest.permissions.hosts.join(', ') })}</span>
          {/if}
          {#if p.missing.length}
            <span class="shint warn"><Icon name="warning" size={14} />{t('Needs settings: {keys}', { keys: p.missing.join(', ') })}</span>
          {/if}
          {#if p.failures > 0}
            <span class="shint warn">{t('Failed {n} times recently', { n: p.failures })}</span>
          {/if}
        </div>
        <div class="sctl">
          {#if p.manifest.settings.length && manage}
            <button class="btn sm" onclick={() => edit(p)}><Icon name="sliders-horizontal" size={14} />{t('Settings')}</button>
          {/if}
          {#if manage && !p.builtin}
            <button class="btn sm danger" onclick={() => remove(p)} aria-label={t('Remove')}><Icon name="trash" size={14} /></button>
          {/if}
          <Toggle checked={p.enabled} disabled={!manage} label={t('Enabled')} onchange={(v: boolean) => setEnabled(p, v)} />
        </div>
      </div>
      {#if open === p.manifest.id && edits[p.manifest.id]}
        <div class="srow settings">
          {#each p.manifest.settings as s (s.key)}
            <label class="field">
              <span class="slabel">{s.label}{#if s.required} *{/if}</span>
              <input
                class="input"
                type={s.secret ? 'password' : 'text'}
                autocomplete="off"
                placeholder={s.secret && p.settings[s.key] === MASK ? t('Saved (unchanged)') : (s.default ?? '')}
                bind:value={edits[p.manifest.id][s.key]}
              />
              {#if s.help}<span class="shint">{s.help}</span>{/if}
            </label>
          {/each}
          <div class="row end">
            <button class="btn ghost" onclick={() => (open = null)}>{t('Cancel')}</button>
            <button class="btn primary" onclick={() => saveSettings(p)} disabled={busy}>{t('Save')}</button>
          </div>
        </div>
      {/if}
    {/each}
  </div>
{/if}

{#if manage && catalog?.configured}
  <div class="sgroup">
    <h3>{t('Catalog')}</h3>
    <p class="sdesc">{t('Plugins from the catalog this server trusts. Each download is checked against its signature.')}</p>
    {#if catalogError}<div class="srow"><p class="form-error">{catalogError}</p></div>{/if}
    {#each catalog.entries as e (e.id)}
      <div class="srow plugin">
        <div class="stext">
          <span class="slabel">{e.name} <span class="badge">{e.version}</span>{#if e.installed}<span class="badge accent">{t('Installed {v}', { v: e.installed })}</span>{/if}</span>
          {#if e.description}<span class="shint">{e.description}</span>{/if}
        </div>
        <div class="sctl">
          {#if e.installed !== e.version}
            <button class="btn sm" disabled={!!installing} onclick={() => installFromCatalog(e.id)}>
              {#if installing === e.id}<Spinner size={12} />{:else}<Icon name="download-simple" size={14} />{/if}
              {e.installed ? t('Update') : t('Install')}
            </button>
          {/if}
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
  .plugin {
    grid-template-columns: 1fr auto;
  }
  .sctl {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .settings {
    grid-template-columns: 1fr;
    gap: 10px;
  }
  .field {
    display: grid;
    gap: 4px;
  }
  .warn {
    color: var(--warn);
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .end {
    justify-content: flex-end;
    gap: 8px;
  }
</style>
