<script lang="ts">
  import { fade } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import { link } from '../lib/router.svelte';
  import { libs } from '../lib/libraries.svelte';
  import { LIBRARY_ICON } from '../lib/media';
  import { live } from '../lib/torrents.svelte';
  import { prefs, ms } from '../lib/prefs.svelte';
  import { can } from '../lib/session.svelte';
  import { rate } from '../lib/format';
  import { t } from '../lib/i18n.svelte';
  import { Perm } from '../lib/types';

  let {
    open = $bindable(false),
    current,
    params,
  }: { open?: boolean; current: string; params: Record<string, string> } = $props();

  const stats = $derived(live.stats);
  const canAdd = $derived(can(Perm.TORRENTS_ADD));
</script>

{#if open}
  <div class="scrim" role="presentation" transition:fade={{ duration: ms(160) }} onclick={() => (open = false)}></div>
{/if}

<nav class="sidebar" class:open aria-label={t('Main')}>
  <div class="brand">
    <img src="/icons/corrent-64.png" alt="" width="30" height="30" />
    <span class="name">Corrent</span>
    <button
      class="btn ghost icon sm collapse"
      aria-label={prefs.sidebarCollapsed ? t('Expand sidebar') : t('Collapse sidebar')}
      title={prefs.sidebarCollapsed ? t('Expand sidebar') : t('Collapse sidebar')}
      onclick={() => (prefs.sidebarCollapsed = !prefs.sidebarCollapsed)}>
      <Icon name={prefs.sidebarCollapsed ? 'caret-right' : 'caret-left'} size={16} />
    </button>
  </div>

  <div class="scroll">
    <a href="/" use:link class="item" class:active={current === 'home'} aria-current={current === 'home' ? 'page' : undefined}>
      <Icon name="house" size={20} /><span class="label">{t('Home')}</span>
    </a>

    {#if libs.list.length}
      <div class="group">{t('Libraries')}</div>
      {#each libs.list as l (l.id)}
        {@const active = current === 'library' && params.id === String(l.id)}
        <a href="/library/{l.id}" use:link class="item" class:active aria-current={active ? 'page' : undefined} title={l.name}>
          <Icon name={LIBRARY_ICON[l.kind]} size={20} />
          <span class="label">{l.name}</span>
          {#if libs.scans[l.id]?.running}<span class="dot pulse" title={t('Scanning…')}></span>{/if}
        </a>
      {/each}
    {/if}

    {#if canAdd || can(Perm.TORRENTS_MANAGE_OWN)}
      <div class="group">{t('Downloads')}</div>
      <a href="/torrents" use:link class="item" class:active={current === 'torrents'} aria-current={current === 'torrents' ? 'page' : undefined}>
        <Icon name="download-simple" size={20} />
        <span class="label">{t('Torrents')}</span>
        {#if stats && stats.down_bps > 0}
          <span class="rate num">{rate(stats.down_bps)}</span>
        {/if}
      </a>
      {#if canAdd}
        <a href="/search" use:link class="item" class:active={current === 'search'} aria-current={current === 'search' ? 'page' : undefined}>
          <Icon name="magnifying-glass" size={20} /><span class="label">{t('Search')}</span>
        </a>
      {/if}
      <a href="/rss" use:link class="item" class:active={current === 'rss'} aria-current={current === 'rss' ? 'page' : undefined}>
        <Icon name="rss" size={20} /><span class="label">{t('RSS')}</span>
      </a>
    {/if}
  </div>

  <div class="foot">
    <a href="/settings" use:link class="item" class:active={current === 'settings'} aria-current={current === 'settings' ? 'page' : undefined}>
      <Icon name="gear-six" size={20} /><span class="label">{t('Settings')}</span>
    </a>
  </div>
</nav>

<style>
  .sidebar {
    width: var(--sidebar-w);
    height: 100dvh;
    display: flex;
    flex-direction: column;
    background: var(--bg1);
    border-right: 1px solid var(--border);
    transition: width var(--dur-page) var(--ease-standard);
    z-index: 40;
    overflow: hidden;
  }
  .brand {
    height: var(--topbar-h);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px 0 22px;
    flex: none;
  }
  .brand img {
    border-radius: 8px;
  }
  .name {
    flex: 1;
    font-weight: 600;
    font-size: 18px;
    letter-spacing: -0.02em;
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 6px 12px;
  }
  .group {
    margin: 18px 12px 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-mute);
    white-space: nowrap;
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 14px;
    height: calc(42px * var(--density));
    padding: 0 12px;
    margin: 2px 0;
    border-radius: var(--r-md);
    color: var(--text-dim);
    font-weight: 600;
    font-size: var(--fs-small);
    white-space: nowrap;
    transition:
      background var(--dur-micro),
      color var(--dur-micro);
  }
  .item:hover {
    background: var(--hover);
    color: var(--text);
  }
  .item.active {
    background: var(--accent-soft);
    color: var(--text);
  }
  .item.active::before {
    content: '';
    position: absolute;
    left: -12px;
    top: 10px;
    bottom: 10px;
    width: 3px;
    border-radius: 0 3px 3px 0;
    background: var(--accent);
  }
  .item.active :global(.icon) {
    color: var(--accent);
  }
  .label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rate {
    font-size: 11px;
    color: var(--accent);
    font-weight: 600;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }
  .pulse {
    animation: pulse 1.4s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .foot {
    padding: 8px 12px 14px;
    border-top: 1px solid var(--border);
  }
  .scrim {
    display: none;
  }

  /* Collapsed (desktop) */
  :global(.collapsed) .sidebar .name,
  :global(.collapsed) .sidebar .label,
  :global(.collapsed) .sidebar .rate,
  :global(.collapsed) .sidebar .group {
    display: none;
  }
  :global(.collapsed) .sidebar .brand {
    flex-direction: column;
    justify-content: center;
    padding: 8px 0;
    height: auto;
    min-height: var(--topbar-h);
  }
  :global(.collapsed) .sidebar .item {
    justify-content: center;
    padding: 0;
  }
  :global(.collapsed) .sidebar .dot {
    position: absolute;
    top: 8px;
    right: 10px;
  }

  @media (max-width: 900px) {
    .sidebar {
      position: fixed;
      inset: 0 auto 0 0;
      width: min(300px, 86vw);
      transform: translateX(-102%);
      transition: transform var(--dur-page) var(--ease-standard);
      box-shadow: var(--shadow-4);
    }
    .sidebar.open {
      transform: none;
    }
    .collapse {
      display: none;
    }
    .scrim {
      display: block;
      position: fixed;
      inset: 0;
      background: var(--scrim);
      z-index: 39;
    }
    :global(.collapsed) .sidebar .name,
    :global(.collapsed) .sidebar .label,
    :global(.collapsed) .sidebar .group {
      display: initial;
    }
    :global(.collapsed) .sidebar .group {
      display: block;
    }
    :global(.collapsed) .sidebar .item {
      justify-content: flex-start;
      padding: 0 12px;
    }
    :global(.collapsed) .sidebar .brand {
      flex-direction: row;
      justify-content: flex-start;
      padding: 0 12px 0 22px;
    }
  }
</style>
