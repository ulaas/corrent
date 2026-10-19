<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { date } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';
  import type { AuditEntry } from '../../lib/types';

  let entries = $state<AuditEntry[]>([]);
  let loaded = $state(false);
  let more = $state(true);
  let error = $state('');
  let filter = $state('');
  let busy = $state(false);

  async function load(before?: number) {
    busy = true;
    try {
      const page = await api.get<AuditEntry[]>('/server/audit', { limit: 200, before });
      entries = before ? [...entries, ...page] : page;
      more = page.length === 200;
    } catch (e) {
      error = errorText(e);
    } finally {
      loaded = true;
      busy = false;
    }
  }
  void load();

  const shown = $derived.by(() => {
    const w = filter.toLowerCase().trim();
    return w ? entries.filter((e) => `${e.user ?? ''} ${e.ip} ${e.action} ${e.target} ${e.details ?? ''}`.toLowerCase().includes(w)) : entries;
  });
</script>

<div class="tools">
  <input class="input" bind:value={filter} placeholder={t('Filter')} aria-label={t('Filter')} />
  <button class="btn" onclick={() => load()}><Icon name="arrows-clockwise" />{t('Refresh')}</button>
</div>
{#if error}
  <p class="form-error">{error}</p>
{:else if !loaded}
  <Spinner />
{:else}
  <div class="tbl panel">
    <table class="table">
      <thead><tr><th>{t('Time')}</th><th>{t('User')}</th><th>{t('Action')}</th><th>{t('Target')}</th><th>{t('Address')}</th><th>{t('Result')}</th></tr></thead>
      <tbody>
        {#each shown as e (e.id)}
          <tr>
            <td class="nowrap">{date(e.ts)}</td>
            <td>{e.user ?? '–'}</td>
            <td><code>{e.action}</code></td>
            <td class="target" title={e.details}>{e.target}{#if e.details}<span class="faint"> · {e.details}</span>{/if}</td>
            <td class="mono">{e.ip}</td>
            <td>{#if e.ok}<span class="badge ok">{t('OK')}</span>{:else}<span class="badge err">{t('Failed')}</span>{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if more}
    <button class="btn more" disabled={busy} onclick={() => load(entries[entries.length - 1]?.id)}>{t('Load older entries')}</button>
  {/if}
{/if}

<style>
  .tools {
    display: flex;
    gap: 8px;
    margin-bottom: 14px;
    max-width: 480px;
  }
  .tbl {
    overflow: auto;
  }
  .nowrap {
    white-space: nowrap;
  }
  .target {
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .more {
    margin-top: 14px;
  }
</style>
