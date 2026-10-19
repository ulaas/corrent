<script lang="ts">
  import Modal from '../Modal.svelte';
  import Icon from '../Icon.svelte';
  import TagInput from './TagInput.svelte';
  import { api, errorText } from '../../lib/api';
  import { live } from '../../lib/torrents.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { t } from '../../lib/i18n.svelte';
  import type { Torrent } from '../../lib/types';

  let { torrent, onclose }: { torrent: Torrent; onclose: () => void } = $props();

  // svelte-ignore state_referenced_locally
  const orig = torrent;
  let name = $state(orig.name);
  let category = $state(orig.category ?? '');
  let tags = $state<string[]>([...orig.tags]);
  let down = $state(orig.down_limit_kib);
  let up = $state(orig.up_limit_kib);
  let seedMode = $state<'keep' | 'global' | 'custom'>('keep');
  let ratio = $state(2);
  let minutes = $state(1440);
  let error = $state('');
  let busy = $state(false);

  async function save(e: Event) {
    e.preventDefault();
    const body: Record<string, unknown> = {};
    if (name.trim() !== orig.name) body.name = name.trim();
    if (category !== (orig.category ?? '')) body.category = category;
    if (tags.join('\n') !== orig.tags.join('\n')) body.tags = tags;
    if (down !== orig.down_limit_kib) body.down_limit_kib = Math.max(0, Math.round(down) || 0);
    if (up !== orig.up_limit_kib) body.up_limit_kib = Math.max(0, Math.round(up) || 0);
    if (seedMode === 'global') body.seed = { ratio: null, minutes: null };
    if (seedMode === 'custom') body.seed = { ratio: ratio > 0 ? ratio : null, minutes: minutes > 0 ? Math.round(minutes) : null };
    if (!Object.keys(body).length) return onclose();
    busy = true;
    try {
      await api.patch(`/torrents/${orig.hash}`, body);
      toast(t('Saved'), 'success');
      onclose();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={t('Edit torrent')} size="md" {onclose}>
  <form id="edit-form" class="stack" onsubmit={save}>
    <label class="field">
      <span class="label">{t('Name')}</span>
      <input class="input" bind:value={name} />
      <span class="hint">{t('Leave empty to use the name from the torrent.')}</span>
    </label>
    <label class="field">
      <span class="label">{t('Category')}</span>
      <select class="select" bind:value={category}>
        <option value="">{t('None')}</option>
        {#each Object.keys(live.labels.categories) as c (c)}<option value={c}>{c}</option>{/each}
      </select>
    </label>
    <div class="field"><span class="label">{t('Tags')}</span><TagInput bind:tags known={live.labels.tags} /></div>
    <div class="grid2">
      <label class="field"><span class="label">{t('Download limit (KiB/s)')}</span><input class="input num-input" type="number" min="0" bind:value={down} /><span class="hint">{t('0 = unlimited')}</span></label>
      <label class="field"><span class="label">{t('Upload limit (KiB/s)')}</span><input class="input num-input" type="number" min="0" bind:value={up} /><span class="hint">{t('0 = unlimited')}</span></label>
    </div>
    <fieldset class="field seed">
      <legend class="label">{t('Seeding limits')}</legend>
      <label class="row"><input type="radio" bind:group={seedMode} value="keep" />{t('Leave as they are')}</label>
      <label class="row"><input type="radio" bind:group={seedMode} value="global" />{t('Use the global settings')}</label>
      <label class="row"><input type="radio" bind:group={seedMode} value="custom" />{t('Stop at')}</label>
      {#if seedMode === 'custom'}
        <div class="grid2 indent">
          <label class="field"><span class="label">{t('Ratio')}</span><input class="input num-input" type="number" step="0.1" min="0" bind:value={ratio} /></label>
          <label class="field"><span class="label">{t('Seeding time (minutes)')}</span><input class="input num-input" type="number" min="0" bind:value={minutes} /></label>
        </div>
      {/if}
    </fieldset>
    {#if error}<p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{error}</p>{/if}
  </form>
  {#snippet footer()}
    <button class="btn" onclick={onclose}>{t('Cancel')}</button>
    <button class="btn primary" type="submit" form="edit-form" disabled={busy}>{t('Save')}</button>
  {/snippet}
</Modal>

<style>
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .seed {
    border: 0;
    padding: 0;
    margin: 0;
    gap: 8px;
    font-size: var(--fs-small);
  }
  .seed label.row {
    cursor: pointer;
  }
  .indent {
    padding-left: 24px;
  }
  input[type='radio'] {
    accent-color: var(--accent);
  }
</style>
