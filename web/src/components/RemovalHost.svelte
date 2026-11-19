<script lang="ts">
  // "Remove from library…": the files a removal is about, kept (the library then ignores them)
  // or deleted. Deleting is off while a torrent in the list still holds them.
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { removing } from '../lib/removal.svelte';
  import { toast } from '../lib/toast.svelte';
  import { bytes, fileName } from '../lib/format';
  import { t } from '../lib/i18n.svelte';
  import type { RemovalPlan, Removed } from '../lib/types';

  const req = $derived(removing.current);
  let plan = $state<RemovalPlan | null>(null);
  let error = $state('');
  let busy = $state(false);
  let n = 0;

  $effect(() => {
    const r = req;
    if (!r) return;
    untrack(() => void load(r.id));
  });

  async function load(id: number) {
    const my = ++n;
    plan = null;
    error = '';
    try {
      const p = await api.get<RemovalPlan>(`/items/${id}/removal`);
      if (my === n) plan = p;
    } catch (err) {
      if (my === n) error = errorText(err);
    }
  }

  async function remove(deleteFiles: boolean) {
    if (!req) return;
    busy = true;
    try {
      const r = await api.del<Removed>(`/items/${req.id}`, { query: { delete_files: String(deleteFiles) } });
      const failed = r.failed ?? [];
      if (failed.length) {
        toast(
          failed.length === 1
            ? t("1 file couldn't be deleted and stays in the library: {error}", { error: failed[0].error })
            : t("{n} files couldn't be deleted and stay in the library: {error}", { n: failed.length, error: failed[0].error }),
          'error',
        );
      } else {
        toast(deleteFiles ? t('Removed, and its files deleted.') : t('Removed from the library. Its files stay.'), 'success');
      }
      const done = req.done;
      removing.current = null;
      if (!failed.length) done?.();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  const held = $derived(!!plan?.torrents?.length);
</script>

{#if req}
  <Modal title={t('Remove “{title}” from the library?', { title: req.title })} size="md" onclose={() => !busy && (removing.current = null)}>
    <p class="caption muted">{t('Its files can stay where they are, and the library then leaves them out, or be deleted from the disk.')}</p>
    {#if error}
      <p class="form-error">{error}</p>
    {:else if !plan}
      <div class="center"><Spinner /></div>
    {:else}
      <p class="total">{plan.files.length === 1 ? t('1 file, {size}', { size: bytes(plan.size) }) : t('{n} files, {size}', { n: plan.files.length, size: bytes(plan.size) })}</p>
      <ul class="files">
        {#each plan.files as f (f.path)}
          <li class:with={!f.media} title={f.path}><span class="name caption">{fileName(f.path)}</span><span class="caption muted">{bytes(f.size)}</span></li>
        {/each}
      </ul>
      {#if held}
        <p class="caption held">{t('These files belong to a torrent in the list ({names}). To delete them, remove the torrent with its data.', { names: plan.torrents!.join(', ') })}</p>
      {/if}
    {/if}
    {#snippet footer()}
      <button class="btn" disabled={busy} onclick={() => (removing.current = null)}>{t('Cancel')}</button>
      <button class="btn primary" disabled={!plan || busy} onclick={() => remove(false)}>{#if busy}<Spinner size={14} />{/if}{t('Remove, keep the files')}</button>
      <button class="btn danger solid" disabled={!plan || busy || held} onclick={() => remove(true)}>{t('Delete the files')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 24px 0;
  }
  .total {
    margin: 10px 0 6px;
  }
  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 240px;
    overflow-y: auto;
    display: grid;
    gap: 2px;
  }
  .files li {
    display: flex;
    gap: 10px;
    align-items: baseline;
    min-width: 0;
  }
  .files .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .files .with .name {
    color: var(--text-dim);
  }
  .held {
    margin-top: 8px;
    color: var(--warn);
  }
</style>
