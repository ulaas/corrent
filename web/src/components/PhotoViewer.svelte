<script lang="ts">
  // Full-screen photo viewer with previous/next among the photo's siblings.
  import { fade } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import { api, itemFileUrl } from '../lib/api';
  import { back, go } from '../lib/router.svelte';
  import { toggleFavorite } from '../lib/itemstate';
  import { dayDate } from '../lib/format';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Item, ItemDetail, Page } from '../lib/types';

  let { item }: { item: ItemDetail } = $props();

  let siblings = $state<Item[]>([]);
  let loaded = $state(false);
  let chrome = $state(true);
  let hideTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const q = item.parent_id ? { parent_id: item.parent_id } : { library_id: item.library_id, kind: 'photo' };
    api
      .get<Page<Item>>('/items', { ...q, sort: 'title', limit: 2000 })
      .then((p) => (siblings = p.items.filter((i) => i.kind === 'photo')))
      .catch(() => {});
  });

  $effect(() => {
    void item.id;
    loaded = false;
  });

  const idx = $derived(siblings.findIndex((s) => s.id === item.id));
  const prev = $derived(idx > 0 ? siblings[idx - 1] : null);
  const next = $derived(idx >= 0 && idx < siblings.length - 1 ? siblings[idx + 1] : null);

  function nav(to: Item | null) {
    if (to) go(`/item/${to.id}`, { replace: true });
  }

  function poke() {
    chrome = true;
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => (chrome = false), 2500);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowLeft') nav(prev);
    else if (e.key === 'ArrowRight') nav(next);
    else if (e.key === 'Escape') back(`/library/${item.library_id}`);
    else return;
    e.preventDefault();
  }

  let touchX = 0;
</script>

<svelte:window {onkeydown} />

<div
  class="viewer"
  class:chrome
  role="presentation"
  onpointermove={poke}
  ontouchstart={(e) => (touchX = e.touches[0].clientX)}
  ontouchend={(e) => {
    const dx = e.changedTouches[0].clientX - touchX;
    if (Math.abs(dx) > 60) nav(dx > 0 ? prev : next);
  }}>
  {#key item.id}
    <img
      src={itemFileUrl(item.id)}
      alt={item.title}
      class:loaded
      onload={() => (loaded = true)}
      in:fade={{ duration: ms(260) }} />
  {/key}
  <header>
    <button class="btn ghost icon" aria-label={t('Close')} onclick={() => back(`/library/${item.library_id}`)}><Icon name="x" size={22} /></button>
    <div class="grow">
      <strong class="ellipsis">{item.title}</strong>
      <span class="caption">{dayDate(item.added_at)}{siblings.length ? ` · ${idx + 1} / ${siblings.length}` : ''}</span>
    </div>
    <button class="btn ghost icon" class:fav={item.state.favorite} aria-pressed={item.state.favorite} aria-label={item.state.favorite ? t('Remove from favorites') : t('Add to favorites')} onclick={() => toggleFavorite(item)}>
      <Icon name={item.state.favorite ? 'heart-fill' : 'heart'} size={22} />
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
