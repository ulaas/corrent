<script lang="ts">
  // Full-screen photo viewer with previous/next among the photo's siblings.
  import { fade } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import { api, imageUrl, itemFileUrl } from '../lib/api';
  import { back, replaceAddress } from '../lib/router.svelte';
  import { photoSiblings } from '../lib/gridcache';
  import { image } from '../lib/media';
  import { toggleFavorite } from '../lib/itemstate';
  import { dayDate } from '../lib/format';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Item, ItemDetail, Page } from '../lib/types';

  let { item }: { item: ItemDetail } = $props();

  let siblings = $state<Item[]>([]);
  // Stepping stays on this page: no remount, no fetch of the item and its siblings per swipe.
  let shownId = $state(0);
  const cur = $derived<Item>(siblings.find((s) => s.id === shownId) ?? item);
  let loaded = $state(false);
  let zoomed = $state(false);
  let src = $state('');
  let chrome = $state(true);
  let hideTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    shownId = item.id;
    const q = item.parent_id ? { parent_id: item.parent_id } : { library_id: item.library_id, kind: 'photo' };
    const key = JSON.stringify(q);
    const known = photoSiblings.get(key);
    if (known?.some((s) => s.id === item.id)) {
      siblings = known;
      return;
    }
    api
      .get<Page<Item>>('/items', { ...q, sort: 'title', limit: 2000 })
      .then((p) => {
        siblings = p.items.filter((i) => i.kind === 'photo');
        photoSiblings.set(key, siblings);
      })
      .catch(() => {});
  });

  /** The photo's 800 px artwork if it has one, which loads far quicker than the file. */
  function preview(x: Item): string {
    const art = image(x, 'thumb', 'backdrop', 'poster');
    return art ? imageUrl(art.sha, 800) : itemFileUrl(x.id);
  }
  /** Screens this sharp show the difference without zooming. */
  const sharp = () => innerWidth * (devicePixelRatio || 1) > 1200;

  // The preview first; the original over it when it's wanted.
  $effect(() => {
    const x = cur;
    const original = itemFileUrl(x.id);
    src = zoomed ? original : preview(x);
    if (zoomed || !sharp() || src === original) return;
    const full = new Image();
    let live = true;
    full.onload = () => {
      if (live) src = original;
    };
    full.src = original;
    return () => {
      live = false;
    };
  });

  $effect(() => {
    void cur.id;
    loaded = false;
  });

  const idx = $derived(siblings.findIndex((s) => s.id === cur.id));
  const prev = $derived(idx > 0 ? siblings[idx - 1] : null);
  const next = $derived(idx >= 0 && idx < siblings.length - 1 ? siblings[idx + 1] : null);

  // The neighbours are ready before the swipe.
  $effect(() => {
    for (const x of [prev, next]) if (x) new Image().src = preview(x);
  });

  function nav(to: Item | null) {
    if (!to) return;
    zoomed = false;
    shownId = to.id;
    replaceAddress(`/item/${to.id}`);
  }

  function poke() {
    chrome = true;
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => (chrome = false), 2500);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowLeft') nav(prev);
    else if (e.key === 'ArrowRight') nav(next);
    else if (e.key === 'Escape') {
      if (zoomed) zoomed = false;
      else back(`/library/${item.library_id}`);
    } else if (e.key === 'z') zoomed = !zoomed;
    else return;
    e.preventDefault();
  }

  let touchX = 0;
</script>

<svelte:window {onkeydown} />

<div
  class="viewer"
  class:chrome
  class:zoomed
  role="presentation"
  onpointermove={poke}
  ontouchstart={(e) => (touchX = e.touches[0].clientX)}
  ontouchend={(e) => {
    const dx = e.changedTouches[0].clientX - touchX;
    if (!zoomed && Math.abs(dx) > 60) nav(dx > 0 ? prev : next);
  }}>
  {#key cur.id}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <img
      {src}
      alt={cur.title}
      class:loaded
      onload={() => (loaded = true)}
      onerror={() => {
        // No such bucket for this artwork: the file itself.
        if (src !== itemFileUrl(cur.id)) src = itemFileUrl(cur.id);
      }}
      ondblclick={() => (zoomed = !zoomed)}
      in:fade={{ duration: ms(260) }} />
  {/key}
  <header>
    <button class="btn ghost icon" aria-label={t('Close')} onclick={() => back(`/library/${item.library_id}`)}><Icon name="x" size={22} /></button>
    <div class="grow">
      <strong class="ellipsis">{cur.title}</strong>
      <span class="caption">{dayDate(cur.added_at)}{siblings.length ? ` · ${idx + 1} / ${siblings.length}` : ''}</span>
    </div>
    <button class="btn ghost icon" aria-pressed={zoomed} aria-label={zoomed ? t('Fit to window') : t('Actual size')} title={zoomed ? t('Fit to window') : t('Actual size')} onclick={() => (zoomed = !zoomed)}>
      <Icon name={zoomed ? 'corners-in' : 'corners-out'} size={22} />
    </button>
    <button class="btn ghost icon" class:fav={cur.state.favorite} aria-pressed={cur.state.favorite} aria-label={cur.state.favorite ? t('Remove from favorites') : t('Add to favorites')} onclick={() => toggleFavorite(cur)}>
      <Icon name={cur.state.favorite ? 'heart-fill' : 'heart'} size={22} />
    </button>
  </header>
  {#if prev}<button class="nav left" aria-label={t('Previous')} onclick={() => nav(prev)}><Icon name="caret-left" size={28} /></button>{/if}
  {#if next}<button class="nav right" aria-label={t('Next')} onclick={() => nav(next)}><Icon name="caret-right" size={28} /></button>{/if}
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 90;
    background: #050608;
    display: grid;
    place-items: center;
    color: #fff;
  }
  img {
    max-width: 100vw;
    max-height: 100dvh;
    object-fit: contain;
    opacity: 0;
    transition: opacity var(--dur-page);
  }
  img.loaded {
    opacity: 1;
  }
  .zoomed {
    overflow: auto;
    place-items: start;
  }
  .zoomed img {
    max-width: none;
    max-height: none;
  }
  .zoomed header {
    position: fixed;
  }
  header {
    position: absolute;
    inset: 0 0 auto 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 18px;
    background: linear-gradient(rgba(0, 0, 0, 0.65), transparent);
    opacity: 0;
    transition: opacity var(--dur-page);
  }
  header .btn {
    color: #fff;
  }
  header div {
    display: flex;
    flex-direction: column;
  }
  .caption {
    opacity: 0.7;
  }
  .fav {
    color: #ff5c8a !important;
  }
  .nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 56px;
    height: 56px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.45);
    color: #fff;
    opacity: 0;
    transition: opacity var(--dur-page), background var(--dur-micro);
  }
  .nav:hover {
    background: rgba(0, 0, 0, 0.7);
  }
  .left {
    left: 20px;
  }
  .right {
    right: 20px;
  }
  .chrome header,
  .chrome .nav,
  .viewer:focus-within header {
    opacity: 1;
  }
</style>
