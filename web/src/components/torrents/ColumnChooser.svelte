<script lang="ts">
  // Which torrent list columns are shown, and in what order (as the desktop's column chooser,
  // opened by right-clicking the list's header). Saved in the user's preferences.
  import Modal from '../Modal.svelte';
  import Icon from '../Icon.svelte';
  import { prefs } from '../../lib/prefs.svelte';
  import { COLUMNS, DEFAULT_COLUMNS, col, known, type ColId } from '../../lib/torrentcols';
  import { t } from '../../lib/i18n.svelte';

  let { onclose }: { onclose: () => void } = $props();

  const shown = $derived(known(prefs.torrentColumns));
  const hidden = $derived(COLUMNS.filter((c) => !shown.includes(c.id)).map((c) => c.id));

  function set(list: ColId[]) {
    prefs.torrentColumns = list;
  }
  function move(i: number, by: -1 | 1) {
    const list = [...shown];
    [list[i], list[i + by]] = [list[i + by], list[i]];
    set(list);
  }
</script>

<Modal title={t('Columns')} size="sm" {onclose}>
  <ul class="cols">
    <li class="row"><input type="checkbox" class="check" checked disabled /><span class="grow">{t('Name')}</span></li>
    {#each shown as id, i (id)}
      <li class="row">
        <input type="checkbox" class="check" checked aria-label={t(col(id).label)} onchange={() => set(shown.filter((x) => x !== id))} />
        <span class="grow">{t(col(id).label)}</span>
        <button class="btn ghost icon sm" disabled={i === 0} aria-label={t('Move up')} title={t('Move up')} onclick={() => move(i, -1)}><Icon name="caret-up" size={14} /></button>
        <button class="btn ghost icon sm" disabled={i === shown.length - 1} aria-label={t('Move down')} title={t('Move down')} onclick={() => move(i, 1)}><Icon name="caret-down" size={14} /></button>
      </li>
    {/each}
    {#each hidden as id (id)}
      <li class="row">
        <input type="checkbox" class="check" aria-label={t(col(id).label)} onchange={() => set([...shown, id])} />
        <span class="grow muted">{t(col(id).label)}</span>
      </li>
    {/each}
  </ul>
  <p class="caption muted">{t('Columns that don’t fit the window are left out from the end.')}</p>
  {#snippet footer()}
    <button class="btn" onclick={() => set([...DEFAULT_COLUMNS])}>{t('Reset to defaults')}</button>
    <button class="btn primary" onclick={onclose}>{t('Close')}</button>
  {/snippet}
</Modal>

<style>
  .cols {
    list-style: none;
    margin: 0 0 10px;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 34px;
    font-size: var(--fs-small);
  }
</style>
