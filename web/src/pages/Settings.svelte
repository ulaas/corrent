<script lang="ts">
  import type { Component } from 'svelte';
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { link, go } from '../lib/router.svelte';
  import { can } from '../lib/session.svelte';
  import { t, tk } from '../lib/i18n.svelte';
  import { Perm } from '../lib/types';
  import type { IconName } from '../lib/icons';
  import { extensions } from '../lib/extensions.svelte';

  let { params }: { params: Record<string, string> } = $props();

  interface Section {
    id: string;
    label: string;
    icon: IconName;
    group: string;
    perm?: number;
    load: () => Promise<{ default: Component<any> }>;
  }

  const server = () => import('./settings/ServerSettings.svelte');
  const SECTIONS: Section[] = [
    { id: 'account', label: tk('Account'), icon: 'user-circle', group: tk('You'), load: () => import('./settings/Account.svelte') },
    { id: 'appearance', label: tk('Appearance'), icon: 'palette', group: tk('You'), load: () => import('./settings/Appearance.svelte') },
    { id: 'devices', label: tk('Devices'), icon: 'devices', group: tk('You'), load: () => import('./settings/Devices.svelte') },
    { id: 'downloads', label: tk('Downloads'), icon: 'download-simple', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'connection', label: tk('Connection'), icon: 'globe', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'speed', label: tk('Speed'), icon: 'gauge', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'queue', label: tk('Queue'), icon: 'list-numbers', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'seeding', label: tk('Seeding'), icon: 'arrow-circle-up', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'bittorrent', label: tk('BitTorrent'), icon: 'magnet', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'search', label: tk('Search providers'), icon: 'magnifying-glass', group: tk('Server'), perm: Perm.SETTINGS_READ, load: server },
    { id: 'rss', label: tk('RSS feeds and rules'), icon: 'rss', group: tk('Server'), perm: Perm.SETTINGS_READ, load: () => import('./settings/RssSettings.svelte') },
    { id: 'remote', label: tk('Remote access'), icon: 'globe', group: tk('Administration'), perm: Perm.SETTINGS_WRITE, load: () => import('./settings/Remote.svelte') },
    { id: 'plugins', label: tk('Plugins'), icon: 'puzzle-piece', group: tk('Administration'), perm: Perm.SETTINGS_READ, load: () => import('./settings/Plugins.svelte') },
    { id: 'tasks', label: tk('Tasks and backups'), icon: 'clock', group: tk('Administration'), perm: Perm.SETTINGS_READ, load: () => import('./settings/Tasks.svelte') },
    { id: 'podcasts', label: tk('Podcasts'), icon: 'microphone-stage', group: tk('Server'), perm: Perm.SETTINGS_WRITE, load: () => import('./settings/Podcasts.svelte') },
    { id: 'notifications', label: tk('Notifications'), icon: 'bell', group: tk('Administration'), perm: Perm.SETTINGS_WRITE, load: () => import('./settings/Notifications.svelte') },
    { id: 'libraries', label: tk('Libraries'), icon: 'books', group: tk('Administration'), perm: Perm.LIBRARY_MANAGE, load: () => import('./settings/Libraries.svelte') },
    { id: 'users', label: tk('Users'), icon: 'users', group: tk('Administration'), perm: Perm.USERS_MANAGE, load: () => import('./settings/Users.svelte') },
    { id: 'sessions', label: tk('Now playing'), icon: 'broadcast', group: tk('Administration'), perm: Perm.PLAYBACK_TRANSCODE, load: () => import('./settings/Sessions.svelte') },
    { id: 'audit', label: tk('Audit log'), icon: 'shield-check', group: tk('Administration'), perm: Perm.USERS_MANAGE, load: () => import('./settings/Audit.svelte') },
    { id: 'log', label: tk('Server log'), icon: 'terminal-window', group: tk('Administration'), perm: Perm.SETTINGS_WRITE, load: () => import('./settings/Log.svelte') },
    { id: 'about', label: tk('About'), icon: 'info', group: tk('Administration'), load: () => import('./settings/About.svelte') },
  ];

  const extensionPages = $derived<Section[]>(
    extensions.pages.map((p) => ({ id: `ext-${p.id}`, label: p.title, icon: 'puzzle-piece', group: tk('Extensions'), load: () => import('./settings/ExtensionPage.svelte') })),
  );
  const visible = $derived([...SECTIONS, ...extensionPages].filter((s) => !s.perm || can(s.perm)));
  const current = $derived(visible.find((s) => s.id === params.section) ?? (params.section ? undefined : visible[0]));
  const groups = $derived([...new Set(visible.map((s) => s.group))]);
  const phoneIndex = $derived(!params.section);
</script>

<div class="settings" class:index={phoneIndex}>
  <nav class="snav" aria-label={t('Settings')}>
    <h1>{t('Settings')}</h1>
    {#each groups as g (g)}
      <div class="g">{t(g)}</div>
      {#each visible.filter((s) => s.group === g) as s (s.id)}
        <a href="/settings/{s.id}" use:link class:on={current?.id === s.id} aria-current={current?.id === s.id ? 'page' : undefined}>
          <Icon name={s.icon} size={18} /><span class="grow">{t(s.label)}</span><Icon name="caret-right" size={14} class="chev" />
        </a>
      {/each}
    {/each}
  </nav>
  <section class="content">
    {#if current}
      <button class="btn ghost back-phone" onclick={() => go('/settings')}><Icon name="arrow-left" />{t('Settings')}</button>
      <h2 class="stitle">{t(current.label)}</h2>
      {#key current.id}
        {#await current.load()}
          <div class="center"><Spinner /></div>
        {:then mod}
          <mod.default section={current.id} />
        {/await}
      {/key}
    {:else}
      <div class="empty"><h3>{t("You can't open this section.")}</h3></div>
    {/if}
  </section>
</div>

<style>
  .settings {
    display: grid;
    grid-template-columns: 260px 1fr;
    gap: 32px;
    max-width: 1280px;
    margin: 0 auto;
    padding: var(--sp-6) var(--page-pad) calc(var(--sp-12) + var(--player-h));
  }
  .snav {
    position: sticky;
    top: 24px;
    align-self: start;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .snav h1 {
    margin-bottom: 14px;
  }
  .g {
    margin: 16px 10px 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-mute);
  }
  .snav a {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    border-radius: var(--r-md);
    font-weight: 600;
    font-size: var(--fs-small);
    color: var(--text-dim);
    transition: background var(--dur-micro), color var(--dur-micro);
  }
  .snav a:hover {
    background: var(--hover);
    color: var(--text);
  }
  .snav a.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .snav a.on :global(.icon:first-child) {
    color: var(--accent);
  }
  .snav :global(.chev) {
    display: none;
  }
  .content {
    min-width: 0;
  }
  .stitle {
    font-size: var(--fs-display);
    margin: 6px 0 22px;
  }
  .back-phone {
    display: none;
  }
  .center {
    display: grid;
    place-items: center;
    padding: 60px 0;
  }
  @media (max-width: 860px) {
    .settings {
      grid-template-columns: 1fr;
    }
    .settings.index .content,
    .settings:not(.index) .snav {
      display: none;
    }
    .snav {
      position: static;
    }
    .settings.index .snav a.on {
      background: none;
      color: var(--text-dim);
    }
    .snav a {
      padding: 12px 10px;
    }
    .snav :global(.chev) {
      display: block;
      color: var(--text-mute);
    }
    .back-phone {
      display: inline-flex;
      margin-left: -12px;
    }
  }
</style>
