<script lang="ts">
  // "Refresh metadata": fill in what is missing, or replace everything providers gave.
  import Icon from './Icon.svelte';
  import Menu from './Menu.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { RefreshMode } from '../lib/types';

  let { onpick, big = false, busy = false }: { onpick: (mode: RefreshMode) => void; big?: boolean; busy?: boolean } = $props();
</script>

<Menu label={t('Refresh metadata')} triggerClass={big ? 'btn lg' : 'btn sm'} width={300}>
  {#snippet trigger()}<Icon name="arrows-clockwise" size={big ? 20 : 14} />{busy ? t('Looking up metadata…') : t('Refresh metadata')}{/snippet}
  {#snippet children(close)}
    <button class="mi two" role="menuitem" disabled={busy} onclick={() => { close(); onpick('missing'); }}>
      <span>{t('Fill in missing details')}</span>
      <span class="caption muted">{t('Looks up what has no details yet, including what no provider matched before.')}</span>
    </button>
    <button class="mi two" role="menuitem" disabled={busy} onclick={() => { close(); onpick('all'); }}>
      <span>{t('Replace all details')}</span>
      <span class="caption muted">{t('Fetches everything from the providers again. Fields you edited and locked stay.')}</span>
    </button>
  {/snippet}
</Menu>

<style>
  .mi.two {
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    white-space: normal;
    text-align: start;
  }
</style>
