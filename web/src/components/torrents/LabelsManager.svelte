<script lang="ts">
  // Categories (with folder and speed limits) and the tag list.
  import Modal from '../Modal.svelte';
  import Icon from '../Icon.svelte';
  import PathField from '../PathField.svelte';
  import { api, errorText } from '../../lib/api';
  import { live } from '../../lib/torrents.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { t } from '../../lib/i18n.svelte';

  let { onclose }: { onclose: () => void } = $props();

  let cats = $state(
    Object.entries(live.labels.categories).map(([name, c]) => ({ name, save_path: c.save_path ?? '', down: c.down_limit_kib, up: c.up_limit_kib })),
  );
  let tags = $state([...live.labels.tags]);
  let newTag = $state('');
  let error = $state('');
  let busy = $state(false);

  async function save() {
    error = '';
    const names = cats.map((c) => c.name.trim()).filter(Boolean);
    if (new Set(names).size !== names.length) {
      error = t('Two categories have the same name.');
      return;
    }
    busy = true;
    try {
      const map: Record<string, unknown> = {};
      for (const c of cats) {
        if (!c.name.trim()) continue;
        map[c.name.trim()] = { save_path: c.save_path.trim() || undefined, down_limit_kib: Math.max(0, +c.down || 0), up_limit_kib: Math.max(0, +c.up || 0) };
      }
      await api.put('/torrents/categories', map);
      await api.put('/torrents/tags', tags);
      toast(t('Saved'), 'success');
      onclose();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={t('Categories and tags')} size="lg" {onclose}>
  <h3 class="sub">{t('Categories')}</h3>
  <div class="cats">
    {#each cats as c, i (i)}
      <div class="cat panel">
        <label class="field"><span class="label">{t('Name')}</span><input class="input" bind:value={c.name} /></label>
        <label class="field wide"><span class="label">{t('Folder')}</span><PathField bind:value={c.save_path} placeholder={t('Default download folder')} label={t('Folder')} /></label>
        <label class="field"><span class="label">{t('Down KiB/s')}</span><input class="input num-input" type="number" min="0" bind:value={c.down} /></label>
        <label class="field"><span class="label">{t('Up KiB/s')}</span><input class="input num-input" type="number" min="0" bind:value={c.up} /></label>
        <button class="btn ghost icon del" aria-label={t('Delete category')} title={t('Delete category')} onclick={() => cats.splice(i, 1)}><Icon name="trash" /></button>
      </div>
    {:else}
      <p class="muted small">{t('No categories yet.')}</p>
    {/each}
  </div>
  <button class="btn sm" onclick={() => cats.push({ name: '', save_path: '', down: 0, up: 0 })}><Icon name="plus" size={14} />{t('Add category')}</button>

  <h3 class="sub">{t('Tags')}</h3>
  <div class="row wrap">
    {#each tags as tag (tag)}
      <span class="chip on">{tag}<button aria-label={t('Remove tag {tag}', { tag })} onclick={() => (tags = tags.filter((x) => x !== tag))}><Icon name="x" size={12} /></button></span>
    {/each}
    <form class="row" onsubmit={(e) => { e.preventDefault(); const v = newTag.trim(); if (v && !tags.includes(v)) tags = [...tags, v]; newTag = ''; }}>
      <input class="input" style="width: 160px" bind:value={newTag} placeholder={t('New tag')} aria-label={t('New tag')} />
      <button class="btn sm" disabled={!newTag.trim()}>{t('Add')}</button>
    </form>
  </div>
  {#if error}<p class="form-error" role="alert">{error}</p>{/if}
  {#snippet footer()}
    <button class="btn" onclick={onclose}>{t('Cancel')}</button>
    <button class="btn primary" onclick={save} disabled={busy}>{t('Save')}</button>
  {/snippet}
</Modal>

<style>
  .sub {
    margin: 4px 0 10px;
  }
  .sub:not(:first-child) {
    margin-top: 24px;
  }
  .cats {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 10px;
  }
  .cat {
    display: grid;
    grid-template-columns: 1fr 2fr 100px 100px auto;
    gap: 10px;
    align-items: end;
    padding: 12px;
  }
  .del {
    color: var(--error);
  }
  .chip button {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
  }
  @media (max-width: 760px) {
    .cat {
      grid-template-columns: 1fr 1fr;
    }
    .wide {
      grid-column: 1 / -1;
    }
  }
</style>
