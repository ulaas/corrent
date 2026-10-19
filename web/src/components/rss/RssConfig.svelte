<script lang="ts">
  // Feeds and download rules (GET/PUT /rss/config).
  import Icon from '../Icon.svelte';
  import Spinner from '../Spinner.svelte';
  import Toggle from '../Toggle.svelte';
  import PathField from '../PathField.svelte';
  import { api, errorText } from '../../lib/api';
  import { live, useTorrents } from '../../lib/torrents.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { can } from '../../lib/session.svelte';
  import { t } from '../../lib/i18n.svelte';
  import { Perm, type RssRule, type RssSettings } from '../../lib/types';

  let { onsaved }: { onsaved?: () => void } = $props();

  $effect(() => useTorrents());

  let cfg = $state<RssSettings | null>(null);
  let error = $state('');
  let busy = $state(false);
  let rule = $state(-1);
  let newFeed = $state('');
  const writable = $derived(can(Perm.SETTINGS_WRITE));

  api.get<RssSettings>('/rss/config').then((c) => {
    cfg = c;
    rule = c.rules.length ? 0 : -1;
  }).catch((e) => (error = errorText(e)));

  function addFeed(e: Event) {
    e.preventDefault();
    const url = newFeed.trim();
    if (!cfg || !url) return;
    if (!/^https?:\/\//i.test(url)) {
      error = t('A feed address starts with http:// or https://.');
      return;
    }
    if (!cfg.feeds.some((f) => f.url === url)) cfg.feeds.push({ url, name: '', enabled: true });
    newFeed = '';
    error = '';
  }

  function addRule() {
    if (!cfg) return;
    const r: RssRule = {
      name: t('New rule'),
      enabled: true,
      feeds: [],
      must_contain: '',
      must_not_contain: '',
      use_regex: false,
      episode_filter: '',
      smart_episodes: true,
      category: null,
      save_path: null,
      start_paused: false,
    };
    cfg.rules.push(r);
    rule = cfg.rules.length - 1;
  }

  async function save() {
    if (!cfg) return;
    busy = true;
    error = '';
    try {
      cfg = await api.put<RssSettings>('/rss/config', cfg);
      toast(t('Saved'), 'success');
      onsaved?.();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }

  const r = $derived(cfg && rule >= 0 ? cfg.rules[rule] : null);
</script>

{#if !cfg}
  {#if error}<p class="form-error">{error}</p>{:else}<div class="center"><Spinner /></div>{/if}
{:else}
  <div class="rss-config">
    <section>
      <h3>{t('Feeds')}</h3>
      <ul class="feeds">
        {#each cfg.feeds as f, i (f.url)}
          <li class="panel">
            <input type="checkbox" class="check" bind:checked={f.enabled} aria-label={t('Enabled')} disabled={!writable} />
            <div class="grow fcol">
              <input class="input name" bind:value={f.name} placeholder={t('Name (optional)')} aria-label={t('Name')} disabled={!writable} />
              <span class="mono caption muted ellipsis" title={f.url}>{f.url}</span>
            </div>
            {#if writable}<button class="btn ghost icon sm" aria-label={t('Remove feed')} title={t('Remove feed')} onclick={() => cfg!.feeds.splice(i, 1)}><Icon name="trash" size={16} /></button>{/if}
          </li>
        {:else}
          <li class="muted small">{t('No feeds yet.')}</li>
        {/each}
      </ul>
      {#if writable}
        <form class="row" onsubmit={addFeed}>
          <input class="input" bind:value={newFeed} placeholder="https://example.org/feed.xml" aria-label={t('Feed address')} />
          <button class="btn" disabled={!newFeed.trim()}><Icon name="plus" />{t('Add feed')}</button>
        </form>
      {/if}
      <div class="opts">
        <Toggle bind:checked={cfg.auto_download} label={t('Download matching articles automatically')} disabled={!writable} />
        <label class="row small">{t('Refresh every')}<input class="input num-input" type="number" min="1" bind:value={cfg.refresh_minutes} disabled={!writable} />{t('minutes')}</label>
        <label class="row small">{t('Keep at most')}<input class="input num-input" type="number" min="10" bind:value={cfg.max_articles} disabled={!writable} />{t('articles per feed')}</label>
      </div>
    </section>

    <section>
      <h3>{t('Download rules')}</h3>
      <div class="rules">
        <ul class="rule-list">
          {#each cfg.rules as rr, i (i)}
            <li><button class:on={rule === i} onclick={() => (rule = i)}><span class="dot" class:off={!rr.enabled}></span><span class="ellipsis">{rr.name || t('Unnamed rule')}</span></button></li>
          {/each}
          {#if writable}<li><button class="add" onclick={addRule}><Icon name="plus" size={14} />{t('Add rule')}</button></li>{/if}
        </ul>
        {#if r}
          <div class="rule-form stack">
            <div class="row">
              <label class="field grow"><span class="label">{t('Name')}</span><input class="input" bind:value={r.name} disabled={!writable} /></label>
              <Toggle bind:checked={r.enabled} label={t('Enabled')} disabled={!writable} />
            </div>
            <label class="field"><span class="label">{t('Must contain')}</span><input class="input mono" bind:value={r.must_contain} disabled={!writable} /><span class="hint">{t('Words separated by spaces must all appear; | separates alternatives; * and ? are wildcards.')}</span></label>
            <label class="field"><span class="label">{t('Must not contain')}</span><input class="input mono" bind:value={r.must_not_contain} disabled={!writable} /></label>
            <div class="row wrap">
              <label class="row small"><input type="checkbox" class="check" bind:checked={r.use_regex} disabled={!writable} />{t('Regular expressions')}</label>
              <label class="row small"><input type="checkbox" class="check" bind:checked={r.smart_episodes} disabled={!writable} />{t('Each episode only once')}</label>
              <label class="row small"><input type="checkbox" class="check" bind:checked={r.start_paused} disabled={!writable} />{t('Add paused')}</label>
            </div>
            <label class="field"><span class="label">{t('Episode filter')}</span><input class="input mono" bind:value={r.episode_filter} placeholder="1x1-5;8;10-" disabled={!writable} /></label>
            <div class="grid2">
              <label class="field"><span class="label">{t('Category')}</span>
                <select class="select" value={r.category ?? ''} onchange={(e) => (r.category = e.currentTarget.value || null)} disabled={!writable}>
                  <option value="">{t('None')}</option>
                  {#each Object.keys(live.labels.categories) as c (c)}<option value={c}>{c}</option>{/each}
                </select>
              </label>
              <div class="field"><span class="label">{t('Save to')}</span>
                <PathField bind:value={() => r.save_path ?? '', (v) => (r.save_path = v.trim() || null)} placeholder={t('Default download folder')} label={t('Save to')} />
              </div>
            </div>
            <fieldset class="field feeds-for">
              <legend class="label">{t('Applies to')}</legend>
              <label class="row small"><input type="checkbox" class="check" checked={!r.feeds.length} onchange={() => (r.feeds = [])} disabled={!writable} />{t('All feeds')}</label>
              {#each cfg.feeds as f (f.url)}
                <label class="row small"><input type="checkbox" class="check" checked={r.feeds.includes(f.url)} disabled={!writable}
                    onchange={(e) => (r.feeds = e.currentTarget.checked ? [...r.feeds, f.url] : r.feeds.filter((u) => u !== f.url))} /><span class="ellipsis">{f.name || f.url}</span></label>
              {/each}
            </fieldset>
            {#if writable}
              <button class="btn danger sm del" onclick={() => { cfg!.rules.splice(rule, 1); rule = Math.min(rule, cfg!.rules.length - 1); }}><Icon name="trash" size={14} />{t('Delete rule')}</button>
            {/if}
          </div>
        {:else}
          <p class="muted small">{t('Rules add new articles that match automatically.')}</p>
        {/if}
      </div>
    </section>

    {#if error}<p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{error}</p>{/if}
    {#if writable}
      <div class="save"><button class="btn primary" onclick={save} disabled={busy}>{#if busy}<Spinner size={14} />{/if}{t('Save')}</button></div>
    {/if}
  </div>
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 40px 0;
  }
  .rss-config {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  h3 {
    margin-bottom: 12px;
  }
  .feeds {
    list-style: none;
    padding: 0;
    margin: 0 0 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .feeds li.panel {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
  }
  .fcol {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .name {
    height: 30px;
  }
  .opts {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 16px;
    max-width: 520px;
  }
  .opts .num-input {
    width: 90px;
  }
  .rules {
    display: grid;
    grid-template-columns: 220px 1fr;
    gap: 20px;
  }
  .rule-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .rule-list button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    border-radius: var(--r-md);
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--text-dim);
    text-align: left;
  }
  .rule-list button:hover {
    background: var(--hover);
  }
  .rule-list button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .rule-list .add {
    color: var(--accent);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--success);
    flex: none;
  }
  .dot.off {
    background: var(--paused);
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 2fr;
    gap: 12px;
  }
  .feeds-for {
    border: 0;
    margin: 0;
    padding: 0;
    gap: 6px;
  }
  .del {
    align-self: flex-start;
  }
  .save {
    display: flex;
    justify-content: flex-end;
  }
  @media (max-width: 760px) {
    .rules,
    .grid2 {
      grid-template-columns: 1fr;
    }
  }
</style>
