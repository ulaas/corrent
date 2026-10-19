<script lang="ts">
  // The persistent music player: keeps playing while you browse.
  import { fly } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import Img from './Img.svelte';
  import Seek from './Seek.svelte';
  import { music } from '../lib/player.svelte';
  import { ms, prefs } from '../lib/prefs.svelte';
  import { clock } from '../lib/format';
  import { t } from '../lib/i18n.svelte';

  const cur = $derived(music.current);
  const art = $derived(cur?.art ? { kind: 'poster', sha: cur.art, width: 400, height: 400 } : null);
</script>

{#if music.active && cur}
  <section class="mini" aria-label={t('Music player')} transition:fly={{ y: 90, duration: ms(300) }}>
    <div class="seek-top">
      <Seek
        value={music.time}
        max={music.duration}
        onseek={(v) => music.seek(v)}
        label={t('Seek')} />
    </div>
    <div class="row-main">
      <button class="now" onclick={() => (music.expanded = !music.expanded)} aria-expanded={music.expanded} aria-label={t('Show queue')}>
        <span class="art">
          {#if art}<Img image={art} w={200} />{:else}<Icon name="music-note" size={22} />{/if}
        </span>
        <span class="meta">
          <span class="title ellipsis">{cur.title}</span>
          <span class="artist ellipsis">{[cur.artist, cur.album].filter(Boolean).join(' · ')}</span>
        </span>
      </button>

      <div class="controls">
        <button class="btn ghost icon sm hide-sm" class:on={music.shuffle} aria-pressed={music.shuffle} aria-label={t('Shuffle')} title={t('Shuffle')} onclick={() => music.toggleShuffle()}><Icon name="shuffle" /></button>
        <button class="btn ghost icon" aria-label={t('Previous')} title={t('Previous')} onclick={() => music.prev()}><Icon name="skip-back-fill" /></button>
        <button class="play" aria-label={music.playing ? t('Pause') : t('Play')} onclick={() => music.toggle()}>
          {#if music.loading}<span class="ring"></span>{/if}
          <Icon name={music.playing ? 'pause-fill' : 'play-fill'} size={22} />
        </button>
        <button class="btn ghost icon" aria-label={t('Next')} title={t('Next')} onclick={() => music.next()}><Icon name="skip-forward-fill" /></button>
        <button
          class="btn ghost icon sm hide-sm"
          class:on={music.repeat !== 'off'}
          aria-label={music.repeat === 'one' ? t('Repeat one') : music.repeat === 'all' ? t('Repeat all') : t('Repeat off')}
          title={music.repeat === 'one' ? t('Repeat one') : music.repeat === 'all' ? t('Repeat all') : t('Repeat off')}
          onclick={() => music.cycleRepeat()}>
          <Icon name="repeat" />{#if music.repeat === 'one'}<span class="one">1</span>{/if}
        </button>
      </div>

      <div class="side">
        <span class="time num hide-sm">{clock(music.time * 1000)} / {clock(music.duration * 1000)}</span>
        <div class="vol hide-sm">
          <button class="btn ghost icon sm" aria-label={prefs.volume ? t('Mute') : t('Unmute')} onclick={() => music.setVolume(prefs.volume ? 0 : 1)}>
            <Icon name={prefs.volume === 0 ? 'speaker-x' : prefs.volume < 0.5 ? 'speaker-low' : 'speaker-high'} />
          </button>
          <input
            class="range"
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={prefs.volume}
            style:--fill="{prefs.volume * 100}%"
            aria-label={t('Volume')}
            oninput={(e) => music.setVolume(+e.currentTarget.value)} />
        </div>
        <button class="btn ghost icon sm" class:on={music.expanded} aria-label={t('Queue')} title={t('Queue')} onclick={() => (music.expanded = !music.expanded)}><Icon name="queue" /></button>
        <button class="btn ghost icon sm" aria-label={t('Close player')} title={t('Close player')} onclick={() => music.close()}><Icon name="x" /></button>
      </div>
    </div>
  </section>

  {#if music.expanded}
    <aside class="queue" aria-label={t('Queue')} transition:fly={{ y: 30, duration: ms(240) }}>
      <header>
        <h3>{t('Up next')}</h3>
        <span class="caption muted">{music.order.length === 1 ? t('1 track') : t('{n} tracks', { n: music.order.length })}</span>
      </header>
      <ol>
        {#each music.order as qi, i (qi)}
          {@const e = music.queue[qi]}
          <li class:current={i === music.pos}>
            <button class="pick" onclick={() => music.jump(i)}>
              <span class="n num">{#if i === music.pos && music.playing}<Icon name="speaker-high" size={14} />{:else}{i + 1}{/if}</span>
              <span class="grow">
                <span class="ellipsis t">{e.title}</span>
                <span class="ellipsis caption muted">{e.artist}</span>
              </span>
              <span class="caption muted num">{clock(e.runtime_ms)}</span>
            </button>
            {#if i !== music.pos}
              <button class="btn ghost icon sm" aria-label={t('Remove from queue')} onclick={() => music.removeAt(i)}><Icon name="x" size={14} /></button>
            {/if}
          </li>
        {/each}
      </ol>
    </aside>
  {/if}
{/if}

<style>
  .mini {
    position: fixed;
    left: var(--sidebar-w);
    right: 0;
    bottom: 0;
    height: var(--player-h);
    z-index: 34;
    background: color-mix(in srgb, var(--bg1) 88%, transparent);
    backdrop-filter: blur(20px) saturate(1.5);
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
  }
  .seek-top {
    margin-top: -9px;
    padding: 0 0;
  }
  .row-main {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 16px;
    padding: 0 var(--page-pad) 6px;
  }
  .now {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    text-align: left;
    border-radius: var(--r-md);
  }
  .art {
    position: relative;
    flex: none;
    width: 54px;
    height: 54px;
    border-radius: var(--r-sm);
    overflow: hidden;
    background: var(--bg3);
    display: grid;
    place-items: center;
    color: var(--text-dim);
    box-shadow: var(--shadow-2);
  }
  .meta {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .title {
    font-weight: 600;
  }
  .artist {
    font-size: var(--fs-caption);
    color: var(--text-dim);
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .play {
    position: relative;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--text);
    color: var(--bg0);
    transition: transform var(--dur-micro) var(--ease-emphasized);
  }
  .play:hover {
    transform: scale(1.07);
  }
  .play:active {
    transform: scale(0.95);
  }
  .ring {
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    border: 2px solid transparent;
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }
  .on {
    color: var(--accent) !important;
  }
  .one {
    font-size: 9px;
    font-weight: 700;
    margin-left: -6px;
    margin-top: 10px;
  }
  .side {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
  }
  .time {
    font-size: var(--fs-caption);
    color: var(--text-dim);
    margin-right: 8px;
  }
  .vol {
    display: flex;
    align-items: center;
    width: 140px;
  }
  .queue {
    position: fixed;
    right: 16px;
    bottom: calc(var(--player-h) + 12px);
    width: min(400px, calc(100vw - 32px));
    max-height: min(60dvh, 560px);
    z-index: 33;
    display: flex;
    flex-direction: column;
    background: var(--bg2);
    border: 1px solid var(--border-hi);
    border-radius: var(--r-xl);
    box-shadow: var(--shadow-4);
    overflow: hidden;
  }
  .queue header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 16px 18px 10px;
  }
  .queue ol {
    list-style: none;
    margin: 0;
    padding: 0 8px 10px;
    overflow-y: auto;
  }
  .queue li {
    display: flex;
    align-items: center;
    border-radius: var(--r-md);
  }
  .queue li:hover {
    background: var(--hover);
  }
  .queue li.current {
    background: var(--accent-soft);
  }
  .pick {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    text-align: left;
  }
  .n {
    width: 22px;
    text-align: center;
    color: var(--text-dim);
    font-size: var(--fs-caption);
  }
  .current .n,
  .current .t {
    color: var(--accent);
  }
  .t {
    display: block;
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .queue .caption {
    display: block;
  }
  @media (max-width: 900px) {
    .mini {
      left: 0;
    }
  }
  @media (max-width: 760px) {
    .mini {
      bottom: calc(var(--bottom-nav) + env(safe-area-inset-bottom));
      height: 72px;
    }
    .row-main {
      grid-template-columns: 1fr auto auto;
      gap: 6px;
    }
    .hide-sm {
      display: none !important;
    }
    .art {
      width: 44px;
      height: 44px;
    }
    .queue {
      bottom: calc(var(--bottom-nav) + 84px);
    }
  }
</style>
