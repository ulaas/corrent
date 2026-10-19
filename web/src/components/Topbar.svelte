<script lang="ts">
  import { fly } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import Menu from './Menu.svelte';
  import Img from './Img.svelte';
  import { api } from '../lib/api';
  import { go, route } from '../lib/router.svelte';
  import { logout, session } from '../lib/session.svelte';
  import { prefs, scheme, ms } from '../lib/prefs.svelte';
  import { cardImage, itemHref, KIND_LABEL, shapeFor, subtitle } from '../lib/media';
  import { t } from '../lib/i18n.svelte';
  import type { Item, SearchResults } from '../lib/types';

  let { onmenu }: { onmenu: () => void } = $props();

  let q = $state(route.path === '/find' ? (route.query.get('q') ?? '') : '');
  let results = $state<Item[]>([]);
  let open = $state(false);
  let active = $state(-1);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let ctl: AbortController | null = null;

  function oninput() {
    if (timer) clearTimeout(timer);
    active = -1;
    const text = q.trim();
    if (!text) {
      results = [];
      open = false;
      return;
    }
    timer = setTimeout(async () => {
      ctl?.abort();
      ctl = new AbortController();
      try {
        const r = await api.get<SearchResults>('/library/search', { q: text }, ctl.signal);
        results = r.items.slice(0, 8);
        open = true;
      } catch {
        /* aborted or offline */
      }
    }, 180);
  }

  function submit(e: Event) {
    e.preventDefault();
    if (active >= 0 && results[active]) {
      choose(results[active]);
      return;
    }
    const text = q.trim();
    if (!text) return;
    open = false;
    go(`/find?q=${encodeURIComponent(text)}`);
  }

  function choose(it: Item) {
    open = false;
    go(itemHref(it));
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      if (open) open = false;
      else (e.target as HTMLInputElement).blur();
    } else if (e.key === 'ArrowDown' && results.length) {
      e.preventDefault();
      open = true;
      active = (active + 1) % results.length;
    } else if (e.key === 'ArrowUp' && results.length) {
      e.preventDefault();
      active = (active - 1 + results.length) % results.length;
    }
  }

  const initials = $derived(
    (session.user?.display_name || session.user?.name || '?')
      .split(/\s+/)
      .map((w) => w[0])
      .join('')
      .slice(0, 2)
      .toUpperCase(),
  );
</script>

<header class="topbar">
  <button class="btn ghost icon menu-btn" aria-label={t('Menu')} onclick={onmenu}><Icon name="list" size={22} /></button>

  <form class="search" role="search" onsubmit={submit}>
    <Icon name="magnifying-glass" size={18} />
    <input
      id="global-search"
      type="search"
      bind:value={q}
      {oninput}
      {onkeydown}
      onfocus={() => results.length && (open = true)}
      onblur={() => setTimeout(() => (open = false), 150)}
      placeholder={t('Search your libraries')}
      aria-label={t('Search your libraries')}
      autocomplete="off"
      role="combobox"
      aria-expanded={open}
      aria-controls="search-suggest"
      aria-activedescendant={active >= 0 ? `sugg-${active}` : undefined} />
    <kbd class="hint" aria-hidden="true">/</kbd>
    {#if open && results.length}
      <ul id="search-suggest" class="suggest" role="listbox" transition:fly={{ y: -6, duration: ms(150) }}>
        {#each results as it, i (it.id)}
          {@const shape = shapeFor(it.kind)}
          <li
            id="sugg-{i}"
            role="option"
            aria-selected={i === active}
            class:active={i === active}
            onpointerdown={(e) => {
              e.preventDefault();
              choose(it);
            }}>
            <span class="thumb {shape}"><Img image={cardImage(it, shape)} w={200} /></span>
            <span class="grow">
              <span class="ellipsis title">{it.title}</span>
              <span class="ellipsis caption muted">{t(KIND_LABEL[it.kind])}{subtitle(it) ? ` · ${subtitle(it)}` : ''}</span>
            </span>
          </li>
        {/each}
        <li class="all" role="option" aria-selected="false" onpointerdown={(e) => { e.preventDefault(); open = false; go(`/find?q=${encodeURIComponent(q.trim())}`); }}>
          {t('See all results')}
        </li>
      </ul>
    {/if}
  </form>

  <div class="spacer"></div>

  <button
    class="btn ghost icon"
    aria-label={scheme() === 'dark' ? t('Switch to light theme') : t('Switch to dark theme')}
    title={scheme() === 'dark' ? t('Switch to light theme') : t('Switch to dark theme')}
    onclick={() => (prefs.theme = scheme() === 'dark' ? 'light' : 'dark')}>
    <Icon name={scheme() === 'dark' ? 'sun' : 'moon'} size={20} />
  </button>

  <Menu label={t('Account')} triggerClass="avatar-btn" width={240}>
    {#snippet trigger()}<span class="avatar">{initials}</span>{/snippet}
    {#snippet children(close)}
      <div class="who">
        <strong class="ellipsis">{session.user?.display_name || session.user?.name}</strong>
        <span class="caption muted ellipsis">{session.user?.name} · {session.info?.name}</span>
      </div>
      <div class="sep"></div>
      <button class="mi" role="menuitem" onclick={() => { close(); go('/settings/account'); }}><Icon name="user" />{t('Account')}</button>
      <button class="mi" role="menuitem" onclick={() => { close(); go('/settings/appearance'); }}><Icon name="palette" />{t('Appearance')}</button>
      <button class="mi" role="menuitem" onclick={() => { close(); go('/settings/devices'); }}><Icon name="devices" />{t('Devices')}</button>
      <div class="sep"></div>
      <button class="mi danger" role="menuitem" onclick={() => { close(); void logout(); }}><Icon name="sign-out" />{t('Sign out')}</button>
    {/snippet}
  </Menu>
</header>

<style>
  .topbar {
    height: var(--topbar-h);
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 var(--page-pad);
    border-bottom: 1px solid var(--border);
    background: var(--glass);
    backdrop-filter: blur(16px) saturate(1.4);
    position: relative;
    z-index: 30;
  }
  .menu-btn {
    display: none;
    margin-left: -8px;
  }
  .search {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    width: min(520px, 100%);
    height: 40px;
    padding: 0 10px 0 14px;
    border-radius: var(--r-pill);
    background: var(--bg0);
    border: 1px solid var(--border);
    color: var(--text-dim);
    transition:
      border-color var(--dur-micro),
      box-shadow var(--dur-micro),
      width var(--dur-page) var(--ease-standard);
  }
  .search:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--text);
  }
  .search input::-webkit-search-cancel-button {
    display: none;
  }
  .search input::placeholder {
    color: var(--text-mute);
  }
  .search:focus-within .hint {
    display: none;
  }
  .suggest {
    position: absolute;
    top: calc(100% + 8px);
    left: 0;
    right: 0;
    margin: 0;
    padding: 6px;
    list-style: none;
    background: var(--bg2);
    border: 1px solid var(--border-hi);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-3);
    z-index: 60;
  }
  .suggest li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 8px;
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .suggest li:hover,
  .suggest li.active {
    background: var(--hover);
  }
  .suggest .title {
    display: block;
    color: var(--text);
    font-weight: 600;
  }
  .suggest .caption {
    display: block;
  }
  .thumb {
    position: relative;
    flex: none;
    width: 36px;
    height: 54px;
    border-radius: var(--r-xs);
    overflow: hidden;
    background: var(--bg3);
  }
  .thumb.square {
    height: 36px;
  }
  .thumb.landscape {
    width: 56px;
    height: 32px;
  }
  .all {
    justify-content: center;
    color: var(--accent);
    font-weight: 600;
    font-size: var(--fs-small);
  }
  :global(.avatar-btn) {
    border-radius: 50%;
  }
  .avatar {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 40%, var(--queued)));
    color: var(--on-accent);
    font-weight: 600;
    font-size: 13px;
    transition: transform var(--dur-micro) var(--ease-emphasized);
  }
  :global(.avatar-btn:hover) .avatar {
    transform: scale(1.06);
  }
  .who {
    display: flex;
    flex-direction: column;
    padding: 8px 10px 4px;
    min-width: 0;
  }
  @media (max-width: 900px) {
    .menu-btn {
      display: inline-flex;
    }
  }
  @media (max-width: 760px) {
    .hint {
      display: none;
    }
  }
</style>
