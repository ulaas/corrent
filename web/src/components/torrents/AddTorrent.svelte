<script lang="ts">
  import Modal from '../Modal.svelte';
  import Icon from '../Icon.svelte';
  import Spinner from '../Spinner.svelte';
  import PathField from '../PathField.svelte';
  import TagInput from './TagInput.svelte';
  import { api, errorText } from '../../lib/api';
  import { parseLinks } from '../../lib/torrentlist';
  import { live } from '../../lib/torrents.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { bytes } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';

  let { initialText = '', initialFiles = [], onclose }: { initialText?: string; initialFiles?: File[]; onclose: () => void } = $props();

  // svelte-ignore state_referenced_locally
  let text = $state(initialText);
  // svelte-ignore state_referenced_locally
  let files = $state<File[]>([...initialFiles]);
  let savePath = $state('');
  let category = $state('');
  let tags = $state<string[]>([]);
  let paused = $state(false);
  let subfolder = $state(true);
  let top = $state(false);
  let busy = $state(false);
  let error = $state('');
  let dragging = $state(false);
  let input: HTMLInputElement | undefined = $state();

  const parsed = $derived(parseLinks(text));
  const count = $derived(parsed.ok.length + files.length);
  const catPath = $derived(category ? live.labels.categories[category]?.save_path : undefined);

  function addFiles(list: FileList | File[] | null | undefined) {
    if (!list) return;
    const more = [...list].filter((f) => f.name.toLowerCase().endsWith('.torrent'));
    const bad = [...list].length - more.length;
    files = [...files, ...more.filter((f) => !files.some((g) => g.name === f.name && g.size === f.size))];
    if (bad) error = t('Only .torrent files can be uploaded.');
  }

  async function submit(e: Event) {
    e.preventDefault();
    error = '';
    if (parsed.bad.length) {
      error = t('Not a magnet link, URL or info hash: {text}', { text: parsed.bad[0] });
      return;
    }
    const options = {
      urls: parsed.ok,
      save_path: savePath.trim() || undefined,
      category: category || undefined,
      tags,
      paused,
      create_subfolder: subfolder,
      add_to_top: top,
    };
    busy = true;
    try {
      let added = 0;
      if (files.length) {
        const fd = new FormData();
        fd.append('options', JSON.stringify(options));
        for (const f of files) fd.append('torrents', f, f.name);
        added = (await api.post<{ count: number }>('/torrents', fd)).count;
      } else {
        added = (await api.post<{ count: number }>('/torrents', options)).count;
      }
      toast(added === 1 ? t('Adding 1 torrent…') : t('Adding {n} torrents…', { n: added }), 'success');
      onclose();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={t('Add torrents')} size="lg" {onclose}>
  <form id="add-form" class="stack" onsubmit={submit}>
    <label class="field">
      <span class="label">{t('Magnet links, URLs or info hashes')}</span>
      <textarea class="input mono links" bind:value={text} placeholder="magnet:?xt=urn:btih:…" rows="4" spellcheck="false" data-autofocus></textarea>
      <span class="hint">{t('One per line.')}{#if parsed.ok.length} · {parsed.ok.length === 1 ? t('1 link') : t('{n} links', { n: parsed.ok.length })}{/if}</span>
    </label>

    <div
      class="drop"
      class:dragging
      role="button"
      tabindex="0"
      ondragover={(e) => { e.preventDefault(); dragging = true; }}
      ondragleave={() => (dragging = false)}
      ondrop={(e) => { e.preventDefault(); dragging = false; addFiles(e.dataTransfer?.files); }}
      onclick={() => input?.click()}
      onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), input?.click())}>
      <Icon name="upload-simple" size={24} />
      <span><strong>{t('Choose .torrent files')}</strong> {t('or drop them here')}</span>
      <input bind:this={input} type="file" accept=".torrent,application/x-bittorrent" multiple hidden onchange={(e) => addFiles(e.currentTarget.files)} />
    </div>
    {#if files.length}
      <ul class="files">
        {#each files as f, i (f.name + f.size)}
          <li><Icon name="file" size={16} /><span class="grow ellipsis">{f.name}</span><span class="caption muted">{bytes(f.size)}</span>
            <button type="button" class="btn ghost icon sm" aria-label={t('Remove')} onclick={() => (files = files.filter((_, j) => j !== i))}><Icon name="x" size={14} /></button></li>
        {/each}
      </ul>
    {/if}

    <div class="grid2">
      <label class="field">
        <span class="label">{t('Save to')}</span>
        <PathField bind:value={savePath} placeholder={catPath ?? t('Default download folder')} label={t('Save to')} />
      </label>
      <label class="field">
        <span class="label">{t('Category')}</span>
        <select class="select" bind:value={category}>
          <option value="">{t('None')}</option>
          {#each Object.keys(live.labels.categories) as c (c)}<option value={c}>{c}</option>{/each}
        </select>
      </label>
    </div>
    <div class="field">
      <span class="label">{t('Tags')}</span>
      <TagInput bind:tags known={live.labels.tags} />
    </div>
    <div class="checks">
      <label class="row"><input type="checkbox" class="check" bind:checked={paused} />{t('Add paused')}</label>
      <label class="row"><input type="checkbox" class="check" bind:checked={subfolder} />{t('Create subfolder')}</label>
      <label class="row"><input type="checkbox" class="check" bind:checked={top} />{t('Add to top of queue')}</label>
    </div>
    {#if error}<p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{error}</p>{/if}
  </form>
  {#snippet footer()}
    <button class="btn" onclick={onclose}>{t('Cancel')}</button>
    <button class="btn primary" type="submit" form="add-form" disabled={busy || count === 0}>
      {#if busy}<Spinner size={14} />{/if}
      {count > 1 ? t('Add {n} torrents', { n: count }) : t('Add torrent')}
    </button>
  {/snippet}
</Modal>

<style>
  .links {
    min-height: 92px;
    font-size: 12.5px;
  }
  .drop {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 18px;
    border: 1.5px dashed var(--border-hi);
    border-radius: var(--r-lg);
    color: var(--text-dim);
    cursor: pointer;
    transition: all var(--dur-micro);
  }
  .drop:hover,
  .drop.dragging {
    border-color: var(--accent);
    background: var(--accent-softer);
    color: var(--text);
  }
  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .files li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 10px;
    border-radius: var(--r-sm);
    background: var(--bg2);
    font-size: var(--fs-small);
  }
  .grid2 {
    display: grid;
    grid-template-columns: 2fr 1fr;
    gap: 14px;
  }
  .checks {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 24px;
    font-size: var(--fs-small);
  }
  .checks label {
    cursor: pointer;
  }
  @media (max-width: 640px) {
    .grid2 {
      grid-template-columns: 1fr;
    }
  }
</style>
