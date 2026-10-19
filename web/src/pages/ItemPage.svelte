<script lang="ts">
  import { fade } from 'svelte/transition';
  import Icon from '../components/Icon.svelte';
  import Img from '../components/Img.svelte';
  import Card from '../components/Card.svelte';
  import Spinner from '../components/Spinner.svelte';
  import PhotoViewer from '../components/PhotoViewer.svelte';
  import { api, errorText } from '../lib/api';
  import { back, go, link } from '../lib/router.svelte';
  import { backdrop, image, isPlayable, KIND_LABEL, playHref, shapeFor, watched } from '../lib/media';
  import { bytes, clock, runtime } from '../lib/format';
  import { toggleFavorite, togglePlayed } from '../lib/itemstate';
  import { music, toEntry, tracksOf } from '../lib/player.svelte';
  import { library } from '../lib/libraries.svelte';
  import { cardWidth } from '../lib/cards.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { Item, ItemDetail, Page } from '../lib/types';

  let { params }: { params: Record<string, string> } = $props();
  const id = $derived(+params.id);

  let item = $state<ItemDetail | null>(null);
  let children = $state<Item[]>([]);
  let parent = $state<ItemDetail | null>(null);
  let season = $state<number | null>(null);
  let episodes = $state<Item[]>([]);
  let error = $state('');

  async function load(id: number) {
    item = null;
    error = '';
    children = [];
    episodes = [];
    parent = null;
    try {
      const d = await api.get<ItemDetail>(`/items/${id}`);
      if (d.kind === 'track' && d.parent_id) {
        go(`/item/${d.parent_id}`, { replace: true });
        return;
      }
      item = d;
      if (['show', 'season', 'artist', 'album', 'folder'].includes(d.kind)) {
        const p = await api.get<Page<Item>>('/items', { parent_id: id, sort: 'index', limit: 1000 });
        children = p.items;
        if (d.kind === 'show' && children.length) {
          const next = children.find((s) => (s.state.unplayed ?? 0) > 0) ?? children[0];
          await pickSeason(next.id);
        }
      }
      if ((d.kind === 'episode' || d.kind === 'season' || d.kind === 'album') && d.parent_id) {
        api.get<ItemDetail>(`/items/${d.parent_id}`).then((p) => (parent = p)).catch(() => {});
      }
    } catch (e) {
      error = errorText(e);
    }
  }

  async function pickSeason(sid: number) {
    season = sid;
    const p = await api.get<Page<Item>>('/items', { parent_id: sid, sort: 'index', limit: 1000 });
    if (season === sid) episodes = p.items;
  }

  $effect(() => {
    void load(id);
  });

  const bg = $derived(item ? (backdrop(item) ?? backdrop(parent)) : undefined);
  const poster = $derived(item ? (image(item, 'poster') ?? image(parent, 'poster')) : undefined);
  const file = $derived(item?.files.find((f) => f.role === 'main') ?? item?.files[0]);
  const video = $derived(file?.streams.find((s) => s.kind === 'video'));
  const audio = $derived(file?.streams.filter((s) => s.kind === 'audio') ?? []);
  const subs = $derived(file?.streams.filter((s) => s.kind === 'subtitle') ?? []);
  const progress = $derived(item ? watched(item) : 0);
  const resumable = $derived(!!item && item.state.position_ms > 5000 && !item.state.played);
  const cast = $derived(item?.credits.filter((c) => c.role === 'actor' || c.role === 'cast').slice(0, 20) ?? []);
  const crew = $derived(item?.credits.filter((c) => c.role !== 'actor' && c.role !== 'cast').slice(0, 6) ?? []);
  const square = $derived(item?.kind === 'album' || item?.kind === 'artist');

  /** First unplayed episode of the show (for "Play" on a show). */
  async function playShow() {
    if (!item) return;
    const target = episodes.find((e) => !e.state.played) ?? episodes[0];
    if (target) go(playHref(target.id));
  }

  async function playAlbum(shuffle = false, start = 0) {
    if (!item) return;
    try {
      const tracks = item.kind === 'album' ? children.filter((c) => c.kind === 'track') : await tracksOf(item.id);
      if (!tracks.length) return;
      if (shuffle !== music.shuffle) music.toggleShuffle();
      music.play(tracks.map((x) => toEntry(x, item)), shuffle ? Math.floor(Math.random() * tracks.length) : start);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  function playBook() {
    if (!item) return;
    music.play([toEntry(item, item)]);
  }

  const lib = $derived(item ? library(item.library_id) : undefined);
  const albumTime = $derived(children.reduce((a, c) => a + (c.runtime_ms ?? 0), 0));
  const isCurrent = (tid: number) => music.current?.id === tid;
</script>

{#if error}
  <div class="page"><div class="empty"><div class="empty-icon"><Icon name="warning-circle" size={30} /></div><h3>{error}</h3>
    <button class="btn" onclick={() => back()}><Icon name="arrow-left" />{t('Back')}</button></div></div>
{:else if !item}
  <div class="loading"><Spinner size={28} /></div>
{:else if item.kind === 'photo'}
  <PhotoViewer {item} />
{:else}
  <article class="detail" class:square>
    <div class="backdrop">
      {#if bg}<div class="bgimg" in:fade={{ duration: ms(700) }}><Img image={bg} w={0} eager /></div>{/if}
      <div class="veil"></div>
    </div>

    <div class="page head-wrap">
      <button class="btn ghost back" onclick={() => back(lib ? `/library/${lib.id}` : '/')}><Icon name="arrow-left" />{t('Back')}</button>
      <div class="head">
        <div class="poster" class:sq={square}>
          {#if poster}<Img image={poster} w={400} eager />{:else}<div class="ph"><Icon name={square ? 'music-notes' : 'film-strip'} size={44} /></div>{/if}
          {#if progress > 0 && !item.state.played}<div class="prog"><div style:width="{progress * 100}%"></div></div>{/if}
        </div>
        <div class="info">
          {#if parent && (item.kind === 'episode' || item.kind === 'season')}
            <a class="parent" href="/item/{parent.id}" use:link>{parent.title}</a>
          {/if}
          <span class="kind">{t(KIND_LABEL[item.kind])}{item.kind === 'episode' && item.index != null ? ` ${item.index}` : ''}</span>
          <h1>{item.title}</h1>
          {#if item.original_title && item.original_title !== item.title}<p class="muted">{item.original_title}</p>{/if}
          <p class="meta">
            {#if item.year}<span>{item.year}</span>{/if}
            {#if item.kind === 'album' && item.context?.[0]}<span>{item.context[0]}</span>{/if}
            {#if item.runtime_ms}<span>{runtime(item.runtime_ms)}</span>{/if}
            {#if item.kind === 'album' && albumTime}<span>{children.length === 1 ? t('1 track') : t('{n} tracks', { n: children.length })} · {runtime(albumTime)}</span>{/if}
            {#if item.child_count && (item.kind === 'show' || item.kind === 'artist')}
              <span>{item.kind === 'show' ? (item.child_count === 1 ? t('1 season') : t('{n} seasons', { n: item.child_count })) : item.child_count === 1 ? t('1 album') : t('{n} albums', { n: item.child_count })}</span>
            {/if}
            {#if item.official_rating}<span class="rating">{item.official_rating}</span>{/if}
            {#if item.rating}<span class="star"><Icon name="sparkle" size={14} />{item.rating.toFixed(1)}</span>{/if}
            {#if video?.height}<span class="tag">{video.height >= 2000 ? '4K' : video.height >= 1000 ? 'HD 1080p' : video.height >= 700 ? 'HD 720p' : `${video.height}p`}</span>{/if}
            {#if item.partial}<span class="badge warn"><Icon name="download-simple" size={12} />{t('Still downloading')}</span>{/if}
          </p>
          {#if item.genres.length}
            <p class="genres">{#each item.genres as g (g)}<span class="chip">{g}</span>{/each}</p>
          {/if}

          <div class="actions">
            {#if item.kind === 'show'}
              <button class="btn primary lg" onclick={playShow} disabled={!episodes.length}><Icon name="play-fill" />{t('Play')}</button>
            {:else if item.kind === 'album' || item.kind === 'artist'}
              <button class="btn primary lg" onclick={() => playAlbum(false)}><Icon name="play-fill" />{t('Play')}</button>
              <button class="btn lg" onclick={() => playAlbum(true)}><Icon name="shuffle" />{t('Shuffle')}</button>
            {:else if item.kind === 'book'}
              <button class="btn primary lg" onclick={playBook}><Icon name="play-fill" />{resumable ? t('Resume') : t('Listen')}</button>
            {:else if isPlayable(item.kind)}
              <button class="btn primary lg" onclick={() => go(playHref(item!.id))}>
                <Icon name="play-fill" />{resumable ? t('Resume from {time}', { time: clock(item.state.position_ms) }) : t('Play')}
              </button>
              {#if resumable}
                <button class="btn lg" onclick={() => go(playHref(item!.id, false))}><Icon name="arrow-counter-clockwise" />{t('From the start')}</button>
              {/if}
            {/if}
            <button
              class="btn icon lg round"
              class:fav={item.state.favorite}
              aria-pressed={item.state.favorite}
              aria-label={item.state.favorite ? t('Remove from favorites') : t('Add to favorites')}
              title={item.state.favorite ? t('Remove from favorites') : t('Add to favorites')}
              onclick={() => toggleFavorite(item!)}><Icon name={item.state.favorite ? 'heart-fill' : 'heart'} size={20} /></button>
            <button
              class="btn icon lg round"
              class:done={item.state.played}
              aria-pressed={item.state.played}
              aria-label={item.state.played ? t('Mark as unplayed') : t('Mark as played')}
              title={item.state.played ? t('Mark as unplayed') : t('Mark as played')}
              onclick={() => togglePlayed(item!)}><Icon name={item.state.played ? 'check-circle-fill' : 'check-circle'} size={20} /></button>
          </div>

          {#if item.overview}<p class="overview">{item.overview}</p>{/if}
          {#if crew.length}
            <dl class="crew">
              {#each crew as c (c.person_id + c.role)}<div><dt>{c.role}</dt><dd>{c.name}</dd></div>{/each}
            </dl>
          {/if}
        </div>
      </div>
    </div>

    <div class="page body">
      {#if item.kind === 'show' && children.length}
        <div class="seasons" role="tablist" aria-label={t('Seasons')}>
          {#each children as s (s.id)}
            <button role="tab" aria-selected={season === s.id} class:on={season === s.id} onclick={() => pickSeason(s.id)}>{s.title}</button>
          {/each}
        </div>
        <div class="episodes stagger">
          {#each episodes as e, i (e.id)}
            {@const thumb = image(e, 'thumb', 'backdrop')}
            <a class="episode" href={playHref(e.id)} use:link style:--i={i}>
              <span class="ethumb">
                {#if thumb}<Img image={thumb} w={400} />{:else}<span class="ph"><Icon name="television-simple" size={26} /></span>{/if}
                <span class="eplay"><Icon name="play-fill" size={22} /></span>
                {#if watched(e) > 0 && !e.state.played}<span class="prog"><span style:width="{watched(e) * 100}%"></span></span>{/if}
              </span>
              <span class="etext">
                <span class="etitle">{e.index != null ? `${e.index}. ` : ''}{e.title}</span>
                <span class="caption muted">{runtime(e.runtime_ms)}</span>
              </span>
              {#if e.state.played}<span class="edone" title={t('Played')}><Icon name="check-circle-fill" size={20} /></span>{/if}
            </a>
          {/each}
        </div>
      {:else if item.kind === 'season'}
        <div class="episodes stagger">
          {#each children as e, i (e.id)}
            <a class="episode" href={playHref(e.id)} use:link style:--i={i}>
              <span class="ethumb">
                {#if image(e, 'thumb')}<Img image={image(e, 'thumb')} w={400} />{:else}<span class="ph"><Icon name="television-simple" size={26} /></span>{/if}
                <span class="eplay"><Icon name="play-fill" size={22} /></span>
              </span>
              <span class="etext"><span class="etitle">{e.index != null ? `${e.index}. ` : ''}{e.title}</span><span class="caption muted">{runtime(e.runtime_ms)}</span></span>
              {#if e.state.played}<span class="edone"><Icon name="check-circle-fill" size={20} /></span>{/if}
            </a>
          {/each}
        </div>
      {:else if item.kind === 'album'}
        <ol class="tracks">
          {#each children as tr, i (tr.id)}
            <li class:current={isCurrent(tr.id)}>
              <button class="track" onclick={() => playAlbum(false, i)}>
                <span class="tn num">
                  {#if isCurrent(tr.id) && music.playing}<span class="eq" aria-hidden="true"><i></i><i></i><i></i></span>{:else}<span class="idx">{tr.index ?? i + 1}</span><Icon name="play-fill" size={14} />{/if}
                </span>
                <span class="grow ellipsis tt">{tr.title}</span>
                <span class="caption muted num">{clock(tr.runtime_ms)}</span>
              </button>
              <button class="btn ghost icon sm" aria-label={t('Add to queue')} title={t('Add to queue')} onclick={() => music.enqueue([toEntry(tr, item)])}><Icon name="plus" size={16} /></button>
            </li>
          {/each}
        </ol>
      {:else if children.length}
        <div class="grid" style:--w="{cardWidth(shapeFor(children[0].kind))}px">
          {#each children as c (c.id)}<Card item={c} shape={shapeFor(c.kind)} />{/each}
        </div>
      {/if}

      {#if cast.length}
        <h2 class="section-title">{t('Cast')}</h2>
        <div class="cast">
          {#each cast as c (c.person_id)}
            <div class="person">
              <span class="pavatar">{c.name.split(/\s+/).map((w) => w[0]).join('').slice(0, 2)}</span>
              <span class="pname ellipsis">{c.name}</span>
              {#if c.character}<span class="caption muted ellipsis">{c.character}</span>{/if}
            </div>
          {/each}
        </div>
      {/if}

      {#if file && isPlayable(item.kind)}
        <h2 class="section-title">{t('Media info')}</h2>
        <div class="media-info panel">
          <div><span class="k">{t('File')}</span><span class="v ellipsis" title={file.path}>{file.path.split(/[\\/]/).pop()}</span></div>
          <div><span class="k">{t('Size')}</span><span class="v">{bytes(file.size)}</span></div>
          {#if file.container}<div><span class="k">{t('Container')}</span><span class="v">{file.container.toUpperCase()}</span></div>{/if}
          {#if video}<div><span class="k">{t('Video')}</span><span class="v">{[video.codec?.toUpperCase(), video.width && video.height ? `${video.width}×${video.height}` : '', video.fps ? `${video.fps.toFixed(2)} fps` : ''].filter(Boolean).join(' · ')}</span></div>{/if}
          {#each audio as a (a.index)}<div><span class="k">{t('Audio')}</span><span class="v">{[a.codec?.toUpperCase(), a.lang, a.channels ? t('{n} ch', { n: a.channels }) : '', a.title].filter(Boolean).join(' · ')}</span></div>{/each}
          {#each subs as s (s.index)}<div><span class="k">{t('Subtitles')}</span><span class="v">{[s.lang, s.title, s.codec, s.external ? t('external') : ''].filter(Boolean).join(' · ')}</span></div>{/each}
          {#if file.missing}<div><span class="k">{t('Status')}</span><span class="v badge err">{t('File missing')}</span></div>{/if}
        </div>
        {#if file.chapters.length > 1}
          <h2 class="section-title">{t('Chapters')}</h2>
          <ol class="chapters">
            {#each file.chapters as c, i (i)}
              <li><a href="/play/{item.id}?t={Math.floor(c.start_ms / 1000)}" use:link><span class="num muted">{clock(c.start_ms)}</span>{c.title || t('Chapter {n}', { n: i + 1 })}</a></li>
            {/each}
          </ol>
        {/if}
      {/if}
    </div>
  </article>
{/if}

<style>
  .loading {
    display: grid;
    place-items: center;
    padding: 160px 0;
  }
  .detail {
    position: relative;
  }
  .backdrop {
    position: absolute;
    inset: 0 0 auto 0;
    height: min(78vh, 720px);
    overflow: hidden;
    z-index: 0;
  }
  .bgimg {
    position: absolute;
    inset: 0;
  }
  .veil {
    position: absolute;
    inset: 0;
    background:
      linear-gradient(to top, var(--bg0) 0%, color-mix(in srgb, var(--bg0) 70%, transparent) 45%, color-mix(in srgb, var(--bg0) 35%, transparent) 100%),
      linear-gradient(to right, color-mix(in srgb, var(--bg0) 80%, transparent), transparent 70%);
  }
  .head-wrap {
    position: relative;
    z-index: 1;
    padding-bottom: 0;
  }
  .back {
    margin: 0 0 18px -12px;
  }
  .head {
    display: grid;
    grid-template-columns: clamp(160px, 22vw, 280px) 1fr;
    gap: clamp(20px, 3vw, 44px);
    align-items: end;
    padding-top: clamp(40px, 12vh, 160px);
  }
  .poster {
    position: relative;
    aspect-ratio: 2/3;
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--bg2);
    box-shadow: var(--shadow-4);
  }
  .poster.sq {
    aspect-ratio: 1;
  }
  .ph {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--text-mute);
    background: linear-gradient(160deg, var(--bg3), var(--bg2));
  }
  .prog {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 4px;
    background: rgba(0, 0, 0, 0.5);
  }
  .prog > * {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .info {
    min-width: 0;
  }
  .parent {
    display: inline-block;
    font-weight: 600;
    color: var(--accent);
    margin-bottom: 4px;
  }
  .kind {
    display: block;
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    font-weight: 600;
    color: var(--text-dim);
    margin-bottom: 6px;
  }
  .info h1 {
    font-size: clamp(28px, 4.4vw, var(--fs-hero));
    letter-spacing: -0.03em;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    margin-top: 12px;
    color: var(--text-dim);
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .rating,
  .tag {
    border: 1px solid var(--border-hi);
    padding: 0 6px;
    border-radius: 4px;
    font-size: 12px;
  }
  .star {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    color: var(--warn);
  }
  .genres {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 14px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 22px;
  }
  .round {
    border-radius: 50%;
    width: 46px;
  }
  .fav {
    color: #ff5c8a;
  }
  .done {
    color: var(--success);
  }
  .overview {
    margin-top: 22px;
    max-width: 760px;
    line-height: 1.65;
    color: color-mix(in srgb, var(--text) 88%, transparent);
  }
  .crew {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 32px;
    margin: 18px 0 0;
  }
  .crew dt {
    font-size: 12px;
    color: var(--text-dim);
    text-transform: capitalize;
  }
  .crew dd {
    margin: 0;
    font-weight: 600;
  }
  .body {
    position: relative;
    z-index: 1;
    padding-top: var(--sp-8);
  }
  .seasons {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    margin-bottom: 18px;
    padding-bottom: 4px;
  }
  .seasons button {
    flex: none;
    padding: 8px 16px;
    border-radius: var(--r-pill);
    font-weight: 600;
    font-size: var(--fs-small);
    color: var(--text-dim);
    border: 1px solid var(--border);
    transition: all var(--dur-micro);
  }
  .seasons button:hover {
    color: var(--text);
  }
  .seasons button.on {
    background: var(--text);
    color: var(--bg0);
    border-color: var(--text);
  }
  .episodes {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 18px;
  }
  .episode {
    display: flex;
    flex-direction: column;
    gap: 10px;
    position: relative;
    border-radius: var(--r-md);
  }
  .ethumb {
    position: relative;
    aspect-ratio: 16/9;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--bg2);
    box-shadow: var(--shadow-1);
    transition: transform var(--dur-short) var(--ease-standard), box-shadow var(--dur-short);
  }
  .episode:hover .ethumb {
    transform: translateY(calc(-3px * var(--motion)));
    box-shadow: var(--shadow-3);
  }
  .eplay {
    position: absolute;
    inset: 0;
    margin: auto;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    opacity: 0;
    transform: scale(0.85);
    transition: all var(--dur-short) var(--ease-emphasized);
  }
  .episode:hover .eplay,
  .episode:focus-visible .eplay {
    opacity: 1;
    transform: none;
  }
  .etext {
    display: flex;
    flex-direction: column;
  }
  .etitle {
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .edone {
    position: absolute;
    top: 8px;
    right: 8px;
    color: var(--success);
    filter: drop-shadow(0 1px 3px rgba(0, 0, 0, 0.6));
  }
  .tracks {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 920px;
  }
  .tracks li {
    display: flex;
    align-items: center;
    border-radius: var(--r-md);
    padding-right: 8px;
  }
  .tracks li:hover {
    background: var(--hover);
  }
  .tracks li.current .tt,
  .tracks li.current .tn {
    color: var(--accent);
  }
  .track {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 12px;
    text-align: left;
  }
  .tn {
    width: 24px;
    display: grid;
    place-items: center;
    color: var(--text-dim);
  }
  .tn :global(.icon) {
    display: none;
  }
  .track:hover .tn .idx {
    display: none;
  }
  .track:hover .tn :global(.icon) {
    display: block;
  }
  .tt {
    font-weight: 600;
  }
  .eq {
    display: flex;
    gap: 2px;
    align-items: flex-end;
    height: 14px;
  }
  .eq i {
    width: 3px;
    background: var(--accent);
    animation: eq 0.9s ease-in-out infinite;
  }
  .eq i:nth-child(2) {
    animation-delay: -0.3s;
  }
  .eq i:nth-child(3) {
    animation-delay: -0.6s;
  }
  @keyframes eq {
    0%,
    100% {
      height: 4px;
    }
    50% {
      height: 14px;
    }
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--w), 1fr));
    gap: 20px;
  }
  .cast {
    display: flex;
    gap: 18px;
    overflow-x: auto;
    padding-bottom: 8px;
  }
  .person {
    flex: none;
    width: 110px;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 4px;
  }
  .pavatar {
    width: 84px;
    height: 84px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--bg3);
    color: var(--text-dim);
    font-weight: 600;
    font-size: 22px;
    margin-bottom: 4px;
  }
  .pname {
    font-weight: 600;
    font-size: var(--fs-small);
    max-width: 100%;
  }
  .person .caption {
    max-width: 100%;
  }
  .media-info {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 14px 24px;
    padding: 18px 20px;
    max-width: 1000px;
  }
  .media-info div {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .k {
    font-size: 12px;
    color: var(--text-dim);
  }
  .v {
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .chapters {
    list-style: none;
    padding: 0;
    margin: 0;
    columns: 2 280px;
  }
  .chapters a {
    display: flex;
    gap: 14px;
    padding: 6px 8px;
    border-radius: var(--r-sm);
  }
  .chapters a:hover {
    background: var(--hover);
  }
  @media (max-width: 700px) {
    .head {
      grid-template-columns: 1fr;
      padding-top: 24px;
    }
    .poster {
      width: 52%;
      max-width: 220px;
    }
    .square .poster {
      width: 70%;
    }
  }
</style>
