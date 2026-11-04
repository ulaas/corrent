<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import Sidebar from './components/Sidebar.svelte';
  import Topbar from './components/Topbar.svelte';
  import Toasts from './components/Toasts.svelte';
  import ConfirmHost from './components/ConfirmHost.svelte';
  import ContextMenu from './components/ContextMenu.svelte';
  import IdentifyHost from './components/IdentifyHost.svelte';
  import Shortcuts from './components/Shortcuts.svelte';
  import MiniPlayer from './components/MiniPlayer.svelte';
  import BottomNav from './components/BottomNav.svelte';
  import Spinner from './components/Spinner.svelte';
  import Icon from './components/Icon.svelte';
  import Auth from './pages/Auth.svelte';
  import Home from './pages/Home.svelte';
  import { boot, session } from './lib/session.svelte';
  import { define, match, rememberScroll, route, scrollOf } from './lib/router.svelte';
  import { ms, prefs } from './lib/prefs.svelte';
  import { music } from './lib/player.svelte';
  import { startLibraries } from './lib/libraries.svelte';
  import { loadExtensions } from './lib/extensions.svelte';
  import { startSessionFeed } from './lib/torrents.svelte';
  import { link } from './lib/events.svelte';
  import { t } from './lib/i18n.svelte';
  import { lazy, lazyFailed, type Loader } from './lib/lazy.svelte';

  define('home', '/');
  define('library', '/library/:id');
  define('item', '/item/:id');
  define('play', '/play/:id');
  define('find', '/find');
  define('torrents', '/torrents');
  define('search', '/search');
  define('rss', '/rss');
  define('radio', '/radio');
  define('playlists', '/playlists');
  define('playlist', '/playlist/:id');
  define('collection', '/collection/:id');
  define('person', '/person/:id');
  define('row', '/row/:id');
  define('agent', '/agent');
  define('settings', '/settings');
  define('settings', '/settings/:section');

  // Pages other than Home load on demand (keeps the first load small).
  const pages: Record<string, Loader> = {
    library: () => import('./pages/Library.svelte'),
    item: () => import('./pages/ItemPage.svelte'),
    play: () => import('./pages/Player.svelte'),
    find: () => import('./pages/Find.svelte'),
    torrents: () => import('./pages/Torrents.svelte'),
    search: () => import('./pages/Search.svelte'),
    rss: () => import('./pages/Rss.svelte'),
    radio: () => import('./pages/Radio.svelte'),
    playlists: () => import('./pages/Playlists.svelte'),
    playlist: () => import('./pages/PlaylistPage.svelte'),
    collection: () => import('./pages/CollectionPage.svelte'),
    person: () => import('./pages/Person.svelte'),
    row: () => import('./pages/Row.svelte'),
    agent: () => import('./pages/Agent.svelte'),
    settings: () => import('./pages/Settings.svelte'),
    notfound: () => import('./pages/NotFound.svelte'),
  };

  const m = $derived(match(route.path));
  // What makes a page a new page: Settings stays mounted while its sections change.
  const routeKey = $derived(m.name === 'settings' ? 'settings' : route.path);
  const Page = $derived(m.name === 'home' ? undefined : lazy(m.name, pages[m.name]));
  let drawer = $state(false);
  let scroller: HTMLElement | undefined = $state();

  void boot();

  $effect(() => {
    if (session.user) {
      startLibraries();
      startSessionFeed();
      void loadExtensions();
    } else {
      music.close();
    }
  });

  // New page: close the drawer and scroll to the top. Back and Forward go to where the entry was
  // left (browsers only do that for the window; our scroller is the main element).
  let lastPath = route.path;
  let restoring = false;
  let endRestore: (() => void) | null = null;

  $effect(() => {
    const path = route.path;
    const key = route.key;
    const restored = route.restored;
    untrack(() => {
      drawer = false;
      const changed = path !== lastPath;
      lastPath = path;
      endRestore?.();
      if (restored) restoreScroll(key);
      else if (changed) scroller?.scrollTo({ top: 0 });
    });
  });

  /** The page may not be tall enough yet (its data is on the way): keeps trying for a few seconds,
   *  unless the person starts scrolling themselves. */
  function restoreScroll(key: number) {
    const el = scroller;
    const target = scrollOf(key);
    if (!el) return;
    if (!target) return el.scrollTo({ top: 0 });
    restoring = true;
    const until = performance.now() + 3000;
    let frame = 0;
    const stop = () => {
      cancelAnimationFrame(frame);
      restoring = false;
      endRestore = null;
      for (const k of ['wheel', 'touchstart', 'pointerdown', 'keydown']) el.removeEventListener(k, stop);
    };
    endRestore = stop;
    for (const k of ['wheel', 'touchstart', 'pointerdown', 'keydown']) el.addEventListener(k, stop, { passive: true });
    const attempt = () => {
      el.scrollTop = target;
      if (Math.abs(el.scrollTop - target) < 2 || performance.now() > until) return stop();
      frame = requestAnimationFrame(attempt);
    };
    void tick().then(attempt);
  }

  function onMainScroll() {
    if (!restoring && scroller) rememberScroll(route.key, scroller.scrollTop);
  }

  const fullscreen = $derived(m.name === 'play');
  let help = $state(false);

  function onkeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const typing = target.closest('input, textarea, select, [contenteditable="true"]');
    if (e.key === '?' && !typing && !e.ctrlKey && !e.metaKey && session.user) {
      e.preventDefault();
      // A dialog can't show over an element in full screen.
      if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
      help = !help;
      return;
    }
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
      <main id="main" bind:this={scroller} tabindex="-1" onscroll={onMainScroll}>
        {#key routeKey}
          <div class="route" in:fly={{ y: 10, duration: ms(260), easing: cubicOut }}>
            {#if m.name === 'home'}
              <Home />
            {:else}
              {#if Page}
                <Page params={m.params} />
              {:else if lazyFailed(m.name)}
                <div class="page"><div class="empty"><div class="empty-icon"><Icon name="warning-circle" size={30} /></div><h3>{t("Couldn't load this page.")}</h3>
                  <button class="btn" onclick={() => location.reload()}><Icon name="arrow-clockwise" />{t('Try again')}</button></div></div>
              {:else}
                <div class="page-loading"><Spinner /></div>
              {/if}
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
<ContextMenu />
<IdentifyHost />
{#if help}<Shortcuts onclose={() => (help = false)} />{/if}

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
