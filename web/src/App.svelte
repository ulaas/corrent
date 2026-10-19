<script lang="ts">
  import type { Component } from 'svelte';
  import { fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import Sidebar from './components/Sidebar.svelte';
  import Topbar from './components/Topbar.svelte';
  import Toasts from './components/Toasts.svelte';
  import ConfirmHost from './components/ConfirmHost.svelte';
  import MiniPlayer from './components/MiniPlayer.svelte';
  import BottomNav from './components/BottomNav.svelte';
  import Spinner from './components/Spinner.svelte';
  import Auth from './pages/Auth.svelte';
  import Home from './pages/Home.svelte';
  import { boot, session } from './lib/session.svelte';
  import { define, match, route } from './lib/router.svelte';
  import { ms, prefs } from './lib/prefs.svelte';
  import { music } from './lib/player.svelte';
  import { startLibraries } from './lib/libraries.svelte';
  import { startSessionFeed } from './lib/torrents.svelte';
  import { link } from './lib/events.svelte';
  import { t } from './lib/i18n.svelte';

  define('home', '/');
  define('library', '/library/:id');
  define('item', '/item/:id');
  define('play', '/play/:id');
  define('find', '/find');
  define('torrents', '/torrents');
  define('search', '/search');
  define('rss', '/rss');
  define('settings', '/settings');
  define('settings', '/settings/:section');

  // Pages other than Home load on demand (keeps the first load small).
  const pages: Record<string, () => Promise<{ default: Component<any> }>> = {
    library: () => import('./pages/Library.svelte'),
    item: () => import('./pages/ItemPage.svelte'),
    play: () => import('./pages/Player.svelte'),
    find: () => import('./pages/Find.svelte'),
    torrents: () => import('./pages/Torrents.svelte'),
    search: () => import('./pages/Search.svelte'),
    rss: () => import('./pages/Rss.svelte'),
    settings: () => import('./pages/Settings.svelte'),
    notfound: () => import('./pages/NotFound.svelte'),
  };

  const m = $derived(match(route.path));
  let drawer = $state(false);
  let scroller: HTMLElement | undefined = $state();

  boot().then(() => {
    if (session.user) {
      startLibraries();
      startSessionFeed();
    }
  });

  $effect(() => {
    if (session.user) {
      startLibraries();
      startSessionFeed();
    } else {
      music.close();
    }
  });

  // New page: close the drawer and scroll to the top (history navigation keeps the position
  // the browser restores for the window; our scroller is the main element).
  $effect(() => {
    void route.path;
    drawer = false;
    scroller?.scrollTo({ top: 0 });
  });

  const fullscreen = $derived(m.name === 'play');

  function onkeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const typing = target.closest('input, textarea, select, [contenteditable="true"]');
    if (e.key === '/' && !typing && !e.ctrlKey && !e.metaKey) {
      const box = document.getElementById('global-search') as HTMLInputElement | null;
      if (box) {
        e.preventDefault();
        box.focus();
        box.select();
      }
    }
  }
</script>

<svelte:window {onkeydown} />
<svelte:head>
  <title>{session.info?.name ? `Corrent · ${session.info.name}` : 'Corrent'}</title>
</svelte:head>

<a class="skip" href="#main">{t('Skip to content')}</a>

{#if !session.ready}
  <div class="boot"><Spinner size={28} /></div>
{:else if !session.user}
  <Auth />
{:else}
  <div
    class="shell"
    class:collapsed={prefs.sidebarCollapsed}
    class:fullscreen
    class:has-player={music.active && !fullscreen}>
    {#if !fullscreen}
      <Sidebar bind:open={drawer} current={m.name} params={m.params} />
    {/if}
    <div class="main-col">
      {#if !fullscreen}
        <Topbar onmenu={() => (drawer = !drawer)} />
        {#if !link.connected && link.everConnected}
          <div class="offline" role="status">
            <Spinner size={14} />
            {t('Reconnecting to the server…')}
          </div>
        {/if}
      {/if}
      <main id="main" bind:this={scroller} tabindex="-1">
        {#key route.path}
          <div class="route" in:fly={{ y: 10, duration: ms(260), easing: cubicOut }}>
            {#if m.name === 'home'}
              <Home />
            {:else}
              {#await pages[m.name]()}
                <div class="page-loading"><Spinner /></div>
              {:then mod}
                <mod.default params={m.params} />
              {/await}
            {/if}
          </div>
        {/key}
      </main>
    </div>
    {#if !fullscreen}
      <MiniPlayer />
      <BottomNav current={m.name} onmore={() => (drawer = true)} />
    {/if}
  </div>
{/if}

<Toasts />
<ConfirmHost />

<style>
  .skip {
    position: fixed;
    top: -60px;
    left: 12px;
    z-index: 300;
    padding: 8px 14px;
    border-radius: var(--r-md);
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
  }
  .skip:focus {
    top: 12px;
  }
  .boot {
    min-height: 100dvh;
    display: grid;
    place-items: center;
  }
  .shell {
    display: grid;
    grid-template-columns: var(--sidebar-w) 1fr;
    height: 100dvh;
    overflow: hidden;
  }
  .shell.collapsed {
    --sidebar-w: 76px;
  }
  .shell.fullscreen {
    grid-template-columns: 1fr;
  }
  .shell.has-player {
    --player-h: 84px;
  }
  .main-col {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    position: relative;
  }
  main {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    min-height: 0;
    position: relative;
    scroll-behavior: auto;
  }
  main:focus {
    outline: none;
  }
  .route {
    min-height: 100%;
  }
  .page-loading {
    display: grid;
    place-items: center;
    padding: 120px 0;
  }
  .offline {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 6px;
    font-size: var(--fs-caption);
    background: color-mix(in srgb, var(--warn) 16%, var(--bg1));
    color: var(--warn);
    border-bottom: 1px solid var(--border);
  }
  @media (max-width: 900px) {
    .shell,
    .shell.collapsed {
      grid-template-columns: 1fr;
    }
  }
  @media (max-width: 760px) {
    .shell {
      --bottom-nav: 64px;
    }
    .shell.has-player {
      --player-h: 136px;
    }
    main {
      padding-bottom: var(--bottom-nav);
    }
  }
</style>
