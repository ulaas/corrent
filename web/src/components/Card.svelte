<script lang="ts">
  import Icon from './Icon.svelte';
  import Img from './Img.svelte';
  import { link, go } from '../lib/router.svelte';
  import { cardImage, isPlayable, itemHref, playHref, subtitle, watched, type Shape, LIBRARY_ICON } from '../lib/media';
  import { toggleFavorite } from '../lib/itemstate';
  import { contextMenu } from '../lib/contextmenu.svelte';
  import { itemMenu } from '../lib/itemmenu';
  import { can } from '../lib/session.svelte';
  import { Perm } from '../lib/types';
  import { t } from '../lib/i18n.svelte';
  import type { Item } from '../lib/types';
  import type { IconName } from '../lib/icons';

  let { item, shape, onplay }: { item: Item; shape: Shape; onplay?: (item: Item) => void } = $props();

  const img = $derived(cardImage(item, shape));
  const progress = $derived(watched(item));
  const href = $derived(item.kind === 'track' && item.parent_id ? `/item/${item.parent_id}` : itemHref(item));
  const unplayed = $derived(item.state.unplayed ?? 0);
  const fallbackIcon: IconName = $derived(
    item.kind === 'album' || item.kind === 'track' || item.kind === 'artist'
      ? 'music-notes'
      : item.kind === 'photo'
        ? 'image'
        : item.kind === 'book'
          ? 'book-open'
          : item.kind === 'show' || item.kind === 'season' || item.kind === 'episode'
            ? LIBRARY_ICON.shows
            : 'film-strip',
  );

  function play(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    if (onplay) onplay(item);
    else go(playHref(item.id));
  }
</script>

<a {href} use:link class="card {shape}" title={item.title} use:contextMenu={{ items: () => itemMenu(item, { manage: can(Perm.LIBRARY_MANAGE), onplay }), title: item.title }}>
  <div class="art">
    {#if img}
      <Img image={img} w={shape === 'landscape' ? 400 : 400} alt="" />
    {:else}
      <div class="ph"><Icon name={fallbackIcon} size={36} /></div>
    {/if}
    <div class="shade"></div>
    {#if item.state.played}
      <span class="badge-corner done" title={t('Played')}><Icon name="check" size={14} /></span>
    {:else if unplayed > 0 && (item.kind === 'show' || item.kind === 'season')}
      <span class="badge-corner count num" title={t('{n} unplayed', { n: unplayed })}>{unplayed}</span>
    {/if}
    {#if item.partial}
      <span class="badge-dl" title={t('Still downloading')}><Icon name="download-simple" size={13} /></span>
    {/if}
    <div class="actions">
      {#if isPlayable(item.kind) && item.kind !== 'track'}
        <button class="play" aria-label={t('Play {title}', { title: item.title })} onclick={play}>
          <Icon name="play-fill" size={22} />
        </button>
      {:else if onplay}
        <button class="play" aria-label={t('Play {title}', { title: item.title })} onclick={play}>
          <Icon name="play-fill" size={22} />
        </button>
      {/if}
      <button
        class="fav"
        class:on={item.state.favorite}
        aria-pressed={item.state.favorite}
        aria-label={item.state.favorite ? t('Remove from favorites') : t('Add to favorites')}
        onclick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          void toggleFavorite(item);
        }}><Icon name={item.state.favorite ? 'heart-fill' : 'heart'} size={18} /></button>
    </div>
    {#if progress > 0 && !item.state.played}
      <div class="prog"><div style:width="{progress * 100}%"></div></div>
    {/if}
  </div>
  <div class="text">
    <span class="title ellipsis">{item.kind === 'episode' && item.index != null ? `${item.index}. ${item.title}` : item.title}</span>
    <span class="sub ellipsis">{subtitle(item) || ' '}</span>
  </div>
</a>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    border-radius: var(--r-md);
    outline-offset: 4px;
  }
  .art {
    position: relative;
    width: 100%;
    aspect-ratio: 2 / 3;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--bg2);
    box-shadow: var(--shadow-1);
    transform: translateZ(0);
    transition:
      transform var(--dur-short) var(--ease-standard),
      box-shadow var(--dur-short) var(--ease-standard);
  }
  .square .art {
    aspect-ratio: 1;
  }
  .landscape .art {
    aspect-ratio: 16 / 9;
  }
  .card:hover .art,
  .card:focus-visible .art {
    transform: translateY(calc(-4px * var(--motion))) scale(calc(1 + 0.02 * var(--motion)));
    box-shadow:
      var(--shadow-3),
      0 0 0 1px color-mix(in srgb, var(--text) 10%, transparent);
  }
  .ph {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--text-mute);
    background: linear-gradient(160deg, var(--bg3), var(--bg2));
  }
  .shade {
    position: absolute;
    inset: 0;
    background: linear-gradient(to top, rgba(0, 0, 0, 0.55), transparent 50%);
    opacity: 0;
    transition: opacity var(--dur-short);
  }
  .card:hover .shade,
  .card:focus-visible .shade {
    opacity: 1;
  }
  .badge-corner {
    position: absolute;
    top: 8px;
    right: 8px;
    min-width: 24px;
    height: 24px;
    padding: 0 7px;
    display: grid;
    place-items: center;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 700;
    box-shadow: var(--shadow-2);
  }
  .done {
    background: var(--success);
    color: #06210f;
    padding: 0;
  }
  .count {
    background: var(--accent);
    color: var(--on-accent);
  }
  .badge-dl {
    position: absolute;
    top: 8px;
    left: 8px;
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    border-radius: 12px;
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
  }
  .actions {
    position: absolute;
    inset: auto 8px 10px 8px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    opacity: 0;
    transform: translateY(6px);
    transition:
      opacity var(--dur-short),
      transform var(--dur-short) var(--ease-standard);
  }
  .card:hover .actions,
  .card:focus-within .actions {
    opacity: 1;
    transform: none;
  }
  .play {
    width: 44px;
    height: 44px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.4);
    transition: transform var(--dur-micro) var(--ease-emphasized);
  }
  .play:hover {
    transform: scale(1.08);
  }
  .fav {
    margin-left: auto;
    width: 34px;
    height: 34px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.5);
    color: #fff;
    backdrop-filter: blur(6px);
  }
  .fav.on {
    color: #ff5c8a;
  }
  .prog {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 4px;
    background: rgba(0, 0, 0, 0.5);
  }
  .prog div {
    height: 100%;
    background: var(--accent);
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    padding: 0 2px;
  }
  .title {
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .sub {
    font-size: var(--fs-caption);
    color: var(--text-dim);
  }
  @media (hover: none) {
    .actions {
      opacity: 1;
      transform: none;
    }
    .play {
      display: none;
    }
  }
</style>
