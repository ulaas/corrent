<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Modal from '../../components/Modal.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import Progress from '../../components/Progress.svelte';
  import PathField from '../../components/PathField.svelte';
  import { api, errorText } from '../../lib/api';
  import { link } from '../../lib/router.svelte';
  import { libs, loadLibraries } from '../../lib/libraries.svelte';
  import { LIBRARY_ICON, LIBRARY_KIND_LABEL } from '../../lib/media';
  import { live, useTorrents } from '../../lib/torrents.svelte';
  import { confirm } from '../../lib/confirm.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { ago, num } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';
  import RefreshMenu from '../../components/RefreshMenu.svelte';
  import { runLines } from '../../lib/metadata';
  import type { Library, LibraryKind, LibrarySettings, MetadataRun, RefreshMode } from '../../lib/types';

  // Each library's last metadata refresh (then kept up to date by `metadata_status` events).
  const asked = new Set<number>();
  $effect(() => {
    for (const l of libs.list) {
      if (asked.has(l.id)) continue;
      asked.add(l.id);
      api.get<MetadataRun>(`/libraries/${l.id}/refresh-metadata`).then((r) => {
        if (!libs.metadata[l.id]) libs.metadata[l.id] = r;
      }, () => {});
    }
  });

  async function refreshMetadata(l: Library, mode: RefreshMode) {
    try {
      libs.metadata[l.id] = await api.post<MetadataRun>(`/libraries/${l.id}/refresh-metadata`, { mode });
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  $effect(() => useTorrents());

  const KINDS: LibraryKind[] = ['movies', 'shows', 'music', 'audiobooks', 'videos', 'photos', 'podcasts'];

  interface Draft {
    id: number | null;
    name: string;
    kind: LibraryKind;
    folders: string[];
    settings: LibrarySettings;
  }
  let edit = $state<Draft | null>(null);
  let busy = $state(false);
  let error = $state('');

  // Every field set: a toggle bound to a missing one throws (Svelte's props_invalid_value).
  // Matches LibrarySettings::defaults on the server.
  const DEFAULTS: Required<LibrarySettings> = {
    import_categories: [],
    import_mode: 'in_place',
    watch: true,
    fetch_metadata: true,
    language: '',
    played_threshold: 90,
    stop_seeding_when_watched: false,
    write_nfo: false,
  };

  function create() {
    error = '';
    edit = { id: null, name: '', kind: 'movies', folders: [''], settings: structuredClone(DEFAULTS) };
  }
  function open(l: Library) {
    error = '';
    // A server from before a setting existed doesn't send it.
    const settings = { ...structuredClone(DEFAULTS), ...(structuredClone($state.snapshot(l.settings)) as LibrarySettings) };
    edit = { id: l.id, name: l.name, kind: l.kind, folders: l.folders.length ? [...l.folders] : [''], settings };
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!edit) return;
    const folders = edit.folders.map((f) => f.trim()).filter(Boolean);
    if (!edit.name.trim()) return void (error = t('Give the library a name.'));
    if (!folders.length) return void (error = t('Add at least one folder.'));
    busy = true;
    try {
      if (edit.id == null) {
        await api.post('/libraries', { name: edit.name.trim(), kind: edit.kind, folders, settings: edit.settings });
        toast(t('Library created. Scanning…'), 'success');
      } else {
        await api.patch(`/libraries/${edit.id}`, { name: edit.name.trim(), folders, settings: edit.settings });
        toast(t('Saved'), 'success');
      }
      edit = null;
      void loadLibraries();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  async function scan(l: Library) {
    try {
      await api.post(`/libraries/${l.id}/scan`);
      void loadLibraries();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  async function cancel(l: Library) {
    try {
      await api.del(`/libraries/${l.id}/scan`);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  async function remove(l: Library) {
    const r = await confirm({ title: t('Delete “{name}”?', { name: l.name }), text: t('The library and its watch history are removed. Your files are not touched.'), ok: t('Delete'), danger: true });
    if (!r.ok) return;
    try {
      await api.del(`/libraries/${l.id}`);
      void loadLibraries();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
</script>

<div class="head"><p class="muted small grow">{t('Libraries organise folders of media. Corrent watches them and rescans every six hours.')}</p>
  <button class="btn primary" onclick={create}><Icon name="plus" />{t('Add library')}</button></div>

{#if !libs.loaded}
  <Spinner />
{:else if !libs.list.length}
  <div class="empty"><div class="empty-icon"><Icon name="books" size={30} /></div><h3>{t('No libraries yet')}</h3></div>
{:else}
  <div class="libs">
    {#each libs.list as l (l.id)}
      {@const sc = libs.scans[l.id]}
      <div class="lib sgroup">
        <div class="lh">
          <span class="li"><Icon name={LIBRARY_ICON[l.kind]} size={22} /></span>
          <div class="grow">
            <h3>{l.name}</h3>
            <p class="caption muted">{t(LIBRARY_KIND_LABEL[l.kind])} · {l.item_count === 1 ? t('1 item') : t('{n} items', { n: num(l.item_count) })} · {l.scanned_at ? t('Scanned {when}', { when: ago(l.scanned_at) }) : t('Not scanned yet')}</p>
          </div>
          {#if sc?.running}
            <button class="btn sm" onclick={() => cancel(l)}><Icon name="stop" size={14} />{t('Stop')}</button>
          {:else}
            <button class="btn sm" onclick={() => scan(l)}><Icon name="arrows-clockwise" size={14} />{t('Scan now')}</button>
          {/if}
          <RefreshMenu busy={!!libs.metadata[l.id]?.running} onpick={(mode) => refreshMetadata(l, mode)} />
          <button class="btn sm icon" aria-label={t('Edit')} title={t('Edit')} onclick={() => open(l)}><Icon name="pencil-simple" size={16} /></button>
          <button class="btn sm icon danger" aria-label={t('Delete')} title={t('Delete')} onclick={() => remove(l)}><Icon name="trash" size={16} /></button>
        </div>
        {#if sc?.running}
          <div class="scan">
            <Progress value={sc.total ? sc.done / sc.total : 0} height={5} label={t('Scan progress')} />
            <span class="caption muted num">{sc.phase} · {sc.done}/{sc.total} · {t('{a} added, {u} updated, {r} removed', { a: sc.added, u: sc.updated, r: sc.removed })}</span>
          </div>
        {:else if sc?.error}
          <p class="form-error scan">{sc.error}</p>
        {/if}
        {#if libs.metadata[l.id]}
          {@const run = libs.metadata[l.id]}
          <div class="scan">
            {#if run.running && run.total}
              <Progress value={run.done / run.total} height={5} label={t('Refresh metadata')} />
            {/if}
            {#each runLines(run) as [tone, text], i (i)}
              {#if tone === 'warn'}
                <a class="caption warn" href="/settings/metadata" use:link>{text}</a>
              {:else}
                <span class="caption num" class:muted={tone === 'muted'} class:form-error={tone === 'error'}>{text}</span>
              {/if}
            {/each}
          </div>
        {/if}
        <ul class="folders">{#each l.folders as f (f)}<li class="mono caption"><Icon name="folder" size={14} />{f}</li>{/each}</ul>
      </div>
    {/each}
  </div>
{/if}

{#if edit}
  <Modal title={edit.id == null ? t('Add library') : t('Edit library')} size="lg" onclose={() => (edit = null)}>
    <form id="lib-form" class="stack" onsubmit={save}>
      {#if edit.id == null}
        <div class="kinds" role="radiogroup" aria-label={t('Kind')}>
          {#each KINDS as k (k)}
            <button type="button" role="radio" aria-checked={edit.kind === k} class="kind" class:on={edit.kind === k} onclick={() => (edit!.kind = k)}>
              <Icon name={LIBRARY_ICON[k]} size={24} /><span>{t(LIBRARY_KIND_LABEL[k])}</span>
            </button>
          {/each}
        </div>
      {/if}
      <label class="field"><span class="label">{t('Name')}</span><input class="input" bind:value={edit.name} placeholder={t(LIBRARY_KIND_LABEL[edit.kind])} /></label>
      <div class="field">
        <span class="label">{t('Folders')}</span>
        {#each edit.folders as _, i (i)}
          <div class="row">
            <div class="grow"><PathField bind:value={edit.folders[i]} label={t('Folder')} /></div>
            {#if edit.folders.length > 1}<button type="button" class="btn ghost icon" aria-label={t('Remove folder')} onclick={() => edit!.folders.splice(i, 1)}><Icon name="x" /></button>{/if}
          </div>
        {/each}
        <button type="button" class="btn sm add" onclick={() => edit!.folders.push('')}><Icon name="plus" size={14} />{t('Add folder')}</button>
      </div>
      <Toggle bind:checked={edit.settings.watch} label={t('Watch the folders for changes')} />
      <Toggle bind:checked={edit.settings.fetch_metadata} label={t('Fetch metadata and artwork online')} hint={t('Uses metadata plugins when they are installed.')} />
      {#if edit.kind === 'movies' || edit.kind === 'shows'}
        <Toggle bind:checked={edit.settings.write_nfo} label={t('Write .nfo files')} hint={t('Saves titles, plots and ids next to the files for Kodi and other players. Existing .nfo files that Corrent didn’t write are left alone.')} />
      {/if}
      <div class="grid2">
        <label class="field"><span class="label">{t('Counts as played after')}</span>
          <span class="row"><input class="input num-input" type="number" min="50" max="100" bind:value={edit.settings.played_threshold} />%</span></label>
        <label class="field"><span class="label">{t('Metadata language')}</span><input class="input" bind:value={edit.settings.language} placeholder="en, tr…" /></label>
      </div>
      {#if Object.keys(live.labels.categories).length}
        <div class="field">
          <span class="label">{t('Import finished downloads of these categories')}</span>
          <div class="row wrap">
            {#each Object.keys(live.labels.categories) as c (c)}
              {@const on = edit.settings.import_categories.includes(c)}
              <button type="button" class="chip" class:on aria-pressed={on} onclick={() => (edit!.settings.import_categories = on ? edit!.settings.import_categories.filter((x) => x !== c) : [...edit!.settings.import_categories, c])}>{c}</button>
            {/each}
          </div>
          <label class="row small"><span>{t('How')}</span>
            <select class="select" style="width: auto" bind:value={edit.settings.import_mode}>
              <option value="in_place">{t('Leave the files where they are')}</option>
              <option value="hardlink">{t('Hard-link into the library')}</option>
              <option value="copy">{t('Copy into the library')}</option>
            </select></label>
        </div>
      {/if}
      {#if edit.settings.import_categories.length}
        <Toggle bind:checked={edit.settings.stop_seeding_when_watched} label={t('Stop seeding once everyone has watched')} hint={t('Pauses a finished torrent of these categories when every account that sees this library has played what it brought.')} />
      {/if}
      {#if error}<p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{error}</p>{/if}
    </form>
    {#snippet footer()}
      <button class="btn" onclick={() => (edit = null)}>{t('Cancel')}</button>
      <button class="btn primary" type="submit" form="lib-form" disabled={busy}>{#if busy}<Spinner size={14} />{/if}{edit?.id == null ? t('Add library') : t('Save')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-bottom: 18px;
  }
  .lib {
    padding-bottom: 12px;
  }
  .lh {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px 8px;
  }
  .li {
    width: 44px;
    height: 44px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  .scan {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 4px 20px 8px;
  }
  .warn {
    color: var(--warn);
  }
  .folders {
    list-style: none;
    margin: 0;
    padding: 0 20px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .folders li {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    word-break: break-all;
  }
  .kinds {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    gap: 10px;
  }
  .kind {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 14px 8px;
    border-radius: var(--r-lg);
    border: 2px solid var(--border);
    font-weight: 600;
    font-size: var(--fs-small);
    color: var(--text-dim);
    transition: all var(--dur-micro);
  }
  .kind:hover {
    color: var(--text);
    border-color: var(--border-hi);
  }
  .kind.on {
    border-color: var(--accent);
    background: var(--accent-softer);
    color: var(--text);
  }
  .kind.on :global(.icon) {
    color: var(--accent);
  }
  .add {
    align-self: flex-start;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  @media (max-width: 640px) {
    .lh {
      flex-wrap: wrap;
    }
  }
</style>
