<script lang="ts">
  import { fade } from 'svelte/transition';
  import Shelf from '../components/Shelf.svelte';
  import Icon from '../components/Icon.svelte';
  import Img from '../components/Img.svelte';
  import { api } from '../lib/api';
  import { link, go } from '../lib/router.svelte';
  import { libs, library } from '../lib/libraries.svelte';
  import { backdrop, homeRowTitle, image, isPlayable, itemHref, playHref, subtitle } from '../lib/media';
  import { runtime } from '../lib/format';
  import { prefs, ms, motion } from '../lib/prefs.svelte';
  import { can } from '../lib/session.svelte';
  import { t } from '../lib/i18n.svelte';
  import { Perm, type HomeRow, type Item, type ItemDetail } from '../lib/types';

  let rows = $state<HomeRow[] | null>(null);
  let failed = $state(false);
  let heroIndex = $state(0);
  let heroDetail = $state<Record<number, ItemDetail>>({});

  async function load() {
    try {
      rows = await api.get<HomeRow[]>('/home');
      failed = false;
    } catch {
      failed = true;
      rows ??= [];
    }
  }
  void load();

  // Refresh when a library changes (new items) — at most every few seconds.
  let last = 0;
  $effect(() => {
    void libs.changed;
    if (Date.now() - last > 4000 && rows) void load();
    last = Date.now();
  });

  const ordered = $derived.by(() => {
    const list = (rows ?? []).filter((r) => r.items.length && !prefs.homeHidden.includes(r.id));
    const pos = (id: string) => {
      const i = prefs.homeOrder.indexOf(id);
      return i < 0 ? 1000 : i;
    };
    return list.map((r, i) => ({ r, i })).sort((a, b) => pos(a.r.id) - pos(b.r.id) || a.i - b.i).map((x) => x.r);
  });

  /** Items for the hero: things with a backdrop, continue-watching first. */
  const heroes = $derived.by(() => {
    const seen = new Set<number>();
    const out: Item[] = [];
    for (const r of ordered) {
      for (const it of r.items) {
        if (out.length >= 5) break;
        if (!seen.has(it.id) && backdrop(it)) {
          seen.add(it.id);
          out.push(it);
        }
      }
    }
    return out;
  });
  const hero = $derived(heroes[heroIndex % Math.max(1, heroes.length)]);

  $effect(() => {
    const h = hero;
    if (!h || heroDetail[h.id]) return;
    api.get<ItemDetail>(`/items/${h.id}`).then((d) => (heroDetail[h.id] = d)).catch(() => {});
  });

  $effect(() => {
    if (heroes.length < 2 || motion() === 0) return;
    const timer = setInterval(() => (heroIndex = (heroIndex + 1) % heroes.length), 9000);
    return () => clearInterval(timer);
  });

  function rowHref(id: string): string | undefined {
    if (id.startsWith('recent:')) return `/library/${id.slice(7)}?sort=added&desc=1`;
    return undefined;
  }
</script>

{#if rows === null}
  <div class="page">
    <div class="skeleton hero-skel"></div>
    {#each [0, 1] as r (r)}
      <div class="skeleton line"></div>
      <div class="skel-row">
        {#each Array(7) as _, i (i)}<div class="skeleton card-skel"></div>{/each}
      </div>
    {/each}
  </div>
{:else}
  {#if hero}
    {@const d = heroDetail[hero.id]}
    {@const logo = image(d ?? hero, 'logo')}
    <section class="hero" aria-label={t('Featured')}>
      {#key hero.id}
        <div class="bg" transition:fade={{ duration: ms(900) }}>
          <Img image={backdrop(hero)} w={0} eager />
        </div>
      {/key}
      <div class="veil"></div>
      <div class="hero-body">
        {#key hero.id}
          <div class="hero-text" in:fade={{ duration: ms(500), delay: ms(200) }}>
            {#if logo}
              <div class="logo"><Img image={logo} w={800} fit="contain" alt={hero.title} /></div>
            {:else}
              <h1>{hero.title}</h1>
            {/if}
            <p class="meta">
              {#if subtitle(hero)}<span>{subtitle(hero)}</span>{/if}
              {#if hero.runtime_ms}<span>{runtime(hero.runtime_ms)}</span>{/if}
              {#if d?.official_rating}<span class="rating">{d.official_rating}</span>{/if}
              {#each (d?.genres ?? []).slice(0, 3) as g (g)}<span>{g}</span>{/each}
            </p>
            {#if d?.overview}<p class="overview">{d.overview}</p>{/if}
            <div class="row wrap cta">
              {#if isPlayable(hero.kind)}
                <button class="btn primary lg" onclick={() => go(playHref(hero.id))}>
                  <Icon name="play-fill" />
                  {hero.state.position_ms > 0 && !hero.state.played ? t('Resume') : t('Play')}
                </button>
              {/if}
              <a class="btn lg glass" href={itemHref(hero)} use:link><Icon name="info" />{t('More info')}</a>
            </div>
          </div>
        {/key}
        {#if heroes.length > 1}
          <div class="dots" role="tablist" aria-label={t('Featured')}>
            {#each heroes as h, i (h.id)}
              <button
                role="tab"
                aria-selected={i === heroIndex % heroes.length}
                aria-label={h.title}
                class:on={i === heroIndex % heroes.length}
                onclick={() => (heroIndex = i)}></button>
            {/each}
          </div>
        {/if}
      </div>
    </section>
  {/if}

  <div class="page rows" class:no-hero={!hero}>
    {#if !hero}
      <div class="page-head"><h1>{t('Home')}</h1></div>
    {/if}
    {#each ordered as r (r.id)}
      {@const lib = r.id.startsWith('recent:') ? library(+r.id.slice(7)) : undefined}
      <Shelf title={homeRowTitle(r.id, r.title, lib?.name)} items={r.items} href={rowHref(r.id)} />
    {/each}

    {#if !ordered.length}
      <div class="empty">
        <div class="empty-icon"><Icon name="books" size={32} /></div>
        {#if failed}
          <h3>{t("Couldn't load your home page.")}</h3>
          <button class="btn" onclick={load}><Icon name="arrow-clockwise" />{t('Try again')}</button>
        {:else if !libs.list.length}
          <h3>{t('No libraries yet')}</h3>
          <p>{t('Add a folder with movies, shows, music or photos and Corrent will organise it.')}</p>
          {#if can(Perm.LIBRARY_MANAGE)}
            <a class="btn primary" href="/settings/libraries" use:link><Icon name="plus" />{t('Add a library')}</a>
          {/if}
        {:else}
          <h3>{t('Nothing here yet')}</h3>
          <p>{t('Your libraries are being scanned. New items show up here.')}</p>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .hero {
    position: relative;
    height: clamp(380px, 62vh, 640px);
    margin-bottom: calc(-1 * var(--sp-10));
    overflow: hidden;
    isolation: isolate;
  }
  .bg {
    position: absolute;
    inset: 0;
    z-index: -2;
    animation: drift 24s ease-in-out infinite alternate;
  }
  :global(:root[data-motion='off']) .bg {
    animation: none;
  }
  @keyframes drift {
    from {
      transform: scale(1.04) translate(0, 0);
    }
    to {
      transform: scale(1.1) translate(-1.5%, -1%);
    }
  }
  .veil {
    position: absolute;
    inset: 0;
    z-index: -1;
    background:
      linear-gradient(to top, var(--bg0) 2%, color-mix(in srgb, var(--bg0) 55%, transparent) 38%, transparent 70%),
      linear-gradient(to right, color-mix(in srgb, var(--bg0) 85%, transparent) 0%, transparent 62%);
  }
  .hero-body {
    position: absolute;
    inset: auto 0 0 0;
    padding: 0 var(--page-pad) calc(var(--sp-10) + 48px);
    max-width: var(--content-max);
    margin: 0 auto;
  }
  .hero-text {
    max-width: 640px;
  }
  .hero h1 {
    font-size: clamp(32px, 5vw, var(--fs-hero));
    letter-spacing: -0.03em;
    text-shadow: 0 2px 24px rgba(0, 0, 0, 0.35);
  }
  .logo {
    position: relative;
    width: min(420px, 70vw);
    height: 140px;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    margin-top: 12px;
    color: var(--text-dim);
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .rating {
    border: 1px solid var(--border-hi);
    padding: 0 6px;
    border-radius: 4px;
  }
  .overview {
    margin-top: 12px;
    color: color-mix(in srgb, var(--text) 85%, transparent);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    max-width: 560px;
  }
  .cta {
    margin-top: 22px;
    gap: 12px;
  }
  .glass {
    --btn-bg: color-mix(in srgb, var(--bg3) 60%, transparent);
    backdrop-filter: blur(10px);
  }
  .dots {
    display: flex;
    gap: 8px;
    margin-top: 26px;
  }
  .dots button {
    width: 8px;
    height: 8px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--text) 30%, transparent);
    transition:
      width var(--dur-page) var(--ease-standard),
      background var(--dur-page);
  }
  .dots button.on {
    width: 28px;
    background: var(--accent);
  }
  .rows {
    position: relative;
  }
  .no-hero {
    padding-top: var(--sp-6);
  }
  .hero-skel {
    height: 320px;
    margin-bottom: 32px;
    border-radius: var(--r-xl);
  }
  .line {
    height: 22px;
    width: 220px;
    margin-bottom: 14px;
  }
  .skel-row {
    display: flex;
    gap: 16px;
    overflow: hidden;
    margin-bottom: 32px;
  }
  .card-skel {
    flex: none;
    width: 160px;
    aspect-ratio: 2/3;
  }
  @media (max-width: 760px) {
    .hero {
      height: 440px;
    }
    .hero-body {
      padding-bottom: calc(var(--sp-10) + 24px);
    }
  }
</style>
