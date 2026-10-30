<script lang="ts">
  // Settings → Metadata (as the app's, src/app/media/providers.rs): the server's metadata and
  // subtitle providers — on or off, their API keys, and a test of each.
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText } from '../../lib/api';
  import { identifying, testText } from '../../lib/metadata';
  import { can } from '../../lib/session.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { t } from '../../lib/i18n.svelte';
  import { Perm, type PluginInfo, type ProviderTest } from '../../lib/types';

  /** Where to get a key, for the providers that need one. */
  const KEY_PAGE: Record<string, string> = {
    tmdb: 'https://www.themoviedb.org/settings/api',
    omdb: 'https://www.omdbapi.com/apikey.aspx',
    tvdb: 'https://thetvdb.com/api-information',
    fanart: 'https://fanart.tv/get-an-api-key/',
    opensubtitles: 'https://www.opensubtitles.com/en/consumers',
  };

  let list = $state<PluginInfo[] | null>(null);
  let error = $state('');
  // Typed but not saved yet: plugin → setting → value. Secrets come back masked, so they are
  // sent only when typed anew.
  let edits = $state<Record<string, Record<string, string>>>({});
  let busy = $state('');
  let tests = $state<Record<string, ProviderTest | { failed: string } | 'running'>>({});
  const manage = $derived(can(Perm.PLUGINS_MANAGE));

  async function load() {
    try {
      list = await api.get<PluginInfo[]>('/plugins');
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  const providers = $derived(
    (list ?? []).filter(
      (p) => !p.manifest.id.startsWith('example') && (p.manifest.kinds.includes('metadata') || p.manifest.kinds.includes('subtitles')),
    ),
  );
  const names = $derived(identifying(list ?? []));

  function replace(p: PluginInfo) {
    if (list) list = list.map((x) => (x.manifest.id === p.manifest.id ? p : x));
  }

  async function setEnabled(p: PluginInfo, enabled: boolean) {
    busy = p.manifest.id;
    try {
      replace(await api.put<PluginInfo>(`/plugins/${p.manifest.id}/enabled`, { enabled }));
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = '';
    }
  }

  const typed = (p: PluginInfo) =>
    Object.fromEntries(Object.entries(edits[p.manifest.id] ?? {}).map(([k, v]) => [k, v.trim()]).filter(([, v]) => v));

  async function save(p: PluginInfo) {
    const id = p.manifest.id;
    busy = id;
    try {
      replace(await api.put<PluginInfo>(`/plugins/${id}/settings`, typed(p)));
      edits[id] = {};
      delete tests[id];
      toast(t('Saved. Titles without details are looked up again.'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = '';
    }
  }

  async function test(p: PluginInfo) {
    const id = p.manifest.id;
    tests[id] = 'running';
    try {
      tests[id] = await api.post<ProviderTest>(`/plugins/${id}/test`, {});
    } catch (e) {
      tests[id] = { failed: errorText(e) };
    }
  }

  function badge(p: PluginInfo): [string, string] {
    if (!p.enabled) return [t('Off'), ''];
    if (!p.missing.length) return [t('Ready'), 'ok'];
    return [t('Needs an API key'), 'warn'];
  }
</script>

<div class="sgroup">
  <p class="sdesc">{t('Providers fill in titles, plots, cast, posters and ratings. Most need a free API key from their website; without one they are skipped.')}</p>
  {#if list}
    {#if names.length}
      <p class="sdesc">{t('Identifying titles: {providers}', { providers: names.join(', ') })}</p>
    {:else}
      <p class="sdesc warn"><Icon name="warning" size={14} />{t('No provider can identify titles yet. Add a free TMDB key for movies and shows.')}</p>
    {/if}
  {/if}
</div>

{#if error}
  <p class="form-error">{error}</p>
{:else if !list}
  <div class="center"><Spinner /></div>
{:else}
  {#each providers as p (p.manifest.id)}
    {@const id = p.manifest.id}
    {@const [label, tone] = badge(p)}
    {@const result = tests[id]}
    <div class="sgroup provider">
      <div class="head">
        <strong class="grow">{p.manifest.name}</strong>
        <span class="badge {tone}">{label}</span>
        <Toggle checked={p.enabled} disabled={!manage || busy === id} label={t('Enabled')} onchange={(v: boolean) => setEnabled(p, v)} />
      </div>
      {#if p.manifest.description}<p class="shint">{p.manifest.description}</p>{/if}
      {#if p.manifest.supplements}
        <p class="shint">{t("Adds to titles another provider identified (TMDB or TVDB); it can't identify titles on its own.")}</p>
      {/if}
      {#if manage}
        {#each p.manifest.settings as s (s.key)}
          <label class="field">
            <span class="slabel">{s.label}</span>
            <input
              class="input"
              type={s.secret ? 'password' : 'text'}
              autocomplete="off"
              placeholder={p.settings[s.key] || s.default || ''}
              value={edits[id]?.[s.key] ?? ''}
              oninput={(e) => (edits[id] = { ...(edits[id] ?? {}), [s.key]: e.currentTarget.value })} />
            {#if s.help}<span class="shint">{s.help}</span>{/if}
          </label>
        {/each}
        <div class="row">
          {#if Object.keys(typed(p)).length}
            <button class="btn primary sm" disabled={busy === id} onclick={() => save(p)}>{t('Save')}</button>
          {/if}
          {#if p.enabled && !p.missing.length}
            <button class="btn sm" disabled={result === 'running'} title={t('Looks up a well-known title with the saved settings.')} onclick={() => test(p)}>
              {#if result === 'running'}<Spinner size={12} />{:else}<Icon name="play-circle" size={14} />{/if}{t('Test')}
            </button>
          {/if}
          {#if KEY_PAGE[id]}
            <a class="btn ghost sm" href={KEY_PAGE[id]} target="_blank" rel="noreferrer noopener"><Icon name="arrow-square-out" size={14} />{t('Get a key')}</a>
          {/if}
        </div>
      {/if}
      {#if result && result !== 'running'}
        {@const [ok, text] = testText(result)}
        <p class="shint" class:ok class:warn={!ok}>{text}</p>
      {/if}
    </div>
  {/each}
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 40px 0;
  }
  .provider {
    display: grid;
    gap: 8px;
    padding: 14px 16px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .field {
    display: grid;
    gap: 4px;
    max-width: 460px;
  }
  .row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .warn {
    color: var(--warn);
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .ok {
    color: var(--success);
  }
</style>
