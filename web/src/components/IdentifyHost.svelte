<script lang="ts">
  // "Identify…" (as the desktop's, src/app/media/identify.rs): search the metadata providers by
  // title and year, then pick the right result.
  import { untrack } from 'svelte';
  import Modal from './Modal.svelte';
  import Spinner from './Spinner.svelte';
  import Icon from './Icon.svelte';
  import { api, errorText } from '../lib/api';
  import { identifying } from '../lib/identify.svelte';
  import { toast } from '../lib/toast.svelte';
  import { t } from '../lib/i18n.svelte';

  interface Candidate {
    provider: string;
    id: string;
    title: string;
    year?: number;
    overview?: string;
  }

  const req = $derived(identifying.current);
  let title = $state('');
  let year = $state('');
  let list = $state<Candidate[] | null>(null);
  let error = $state('');
  let applying = $state('');
  let n = 0;

  // A new request fills the fields and searches once (typing must not start over).
  $effect(() => {
    const r = req;
    if (!r) return;
    untrack(() => {
      title = r.title;
      year = r.year ? String(r.year) : '';
      void search();
    });
  });

  async function search(e?: Event) {
    e?.preventDefault();
    if (!req) return;
    const my = ++n;
    list = null;
    error = '';
    const q: Record<string, string> = { title: title.trim() };
    if (/^\d{4}$/.test(year.trim())) q.year = year.trim();
    try {
      const found = await api.get<Candidate[]>(`/items/${req.id}/identify`, q);
      if (my === n) list = found;
    } catch (err) {
      if (my === n) error = errorText(err);
    }
  }

  async function apply(c: Candidate) {
    if (!req) return;
    applying = c.provider + c.id;
    try {
      await api.post(`/items/${req.id}/identify`, { provider: c.provider, id: c.id });
      toast(t('Details updated.'), 'success');
      const done = req.done;
      identifying.current = null;
      done?.();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      applying = '';
    }
  }

  const short = (o: string) => (o.length > 220 ? o.slice(0, 220).trimEnd() + '…' : o);
</script>

{#if req}
  <Modal title={t('Identify')} size="md" onclose={() => (identifying.current = null)}>
    <p class="caption muted">{t('Search the metadata providers, then pick the right one.')}</p>
    <form class="row search" onsubmit={search}>
      <label class="field grow"><span class="label">{t('Title')}</span><input class="input" bind:value={title} /></label>
      <label class="field year"><span class="label">{t('Year')}</span><input class="input" inputmode="numeric" bind:value={year} /></label>
      <button class="btn" type="submit"><Icon name="magnifying-glass" size={16} />{t('Search')}</button>
    </form>
    <div class="results">
      {#if error}
        <p class="form-error">{error}</p>
      {:else if !list}
        <div class="center"><Spinner /></div>
      {:else if !list.length}
        <p class="muted">{t('Nothing found. Try another title or year; providers that need an API key (TMDB, OMDb) need it in Settings → Metadata.')}</p>
      {:else}
        {#each list as c (c.provider + c.id)}
          <div class="cand panel">
            <div class="row head">
              <strong class="grow">{c.title}{c.year ? ` (${c.year})` : ''}</strong>
              <span class="caption muted">{c.provider}</span>
              <button class="btn sm" disabled={!!applying} onclick={() => apply(c)}>
                {#if applying === c.provider + c.id}<Spinner size={12} />{/if}{t('This one')}
              </button>
            </div>
            {#if c.overview?.trim()}<p class="caption muted">{short(c.overview)}</p>{/if}
          </div>
        {/each}
      {/if}
    </div>
    {#snippet footer()}
      <button class="btn primary" onclick={() => (identifying.current = null)}>{t('Close')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .search {
    align-items: flex-end;
    gap: 10px;
    margin: 8px 0 12px;
  }
  .year {
    width: 90px;
  }
  .results {
    display: grid;
    gap: 6px;
  }
  .center {
    display: grid;
    place-items: center;
    padding: 24px 0;
  }
  .cand {
    padding: 10px 12px;
    display: grid;
    gap: 4px;
  }
  .head {
    gap: 10px;
    align-items: center;
  }
</style>
