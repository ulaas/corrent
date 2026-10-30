<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { fade, fly } from 'svelte/transition';
  import Icon from '../components/Icon.svelte';
  import Seek from '../components/Seek.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { back, go, route, setQuery } from '../lib/router.svelte';
  import { SyncRoom } from '../lib/syncplay.svelte';
  import { Cast, castDevices } from '../lib/cast.svelte';
  import { toast } from '../lib/toast.svelte';
  import { attachSource, reportProgress, resolveSource, resumeAt, stopSession, type PlaybackInfo, type PlaybackOptions } from '../lib/player.svelte';
  import { music } from '../lib/player.svelte';
  import { backdrop, image, segmentAt, skipAt } from '../lib/media';
  import { clock } from '../lib/format';
  import { cueAt, parseTrickplay, type TrickCue } from '../lib/trickplay';
  import { imageUrl } from '../lib/api';
  import { ms, prefs } from '../lib/prefs.svelte';
  import { t, tk } from '../lib/i18n.svelte';
  import type { CastDevice, Item, ItemDetail, Page } from '../lib/types';

  let { params }: { params: Record<string, string> } = $props();
  const id = $derived(+params.id);

  let video: HTMLVideoElement | undefined = $state();
  let stage: HTMLDivElement | undefined = $state();
  let item = $state<ItemDetail | null>(null);
  let info = $state<PlaybackInfo | null>(null);
  let opts = $state<PlaybackOptions>({});
  let error = $state('');
  let playing = $state(false);
  let waiting = $state(true);
  let time = $state(0);
  let duration = $state(0);
  let buffered = $state<[number, number][]>([]);
  let muted = $state(false);
  let rate = $state(1);
  let chrome = $state(true);
  let fs = $state(false);
  let subtitle = $state<number | null>(null);
  let trick = $state<TrickCue[]>([]);
  let nextUp = $state<Item | null>(null);
  let countdown = $state(0);
  let menu = $state<'' | 'speed' | 'subs' | 'audio' | 'cast' | 'settings'>('');

  // ---- playback settings (as the desktop's, src/app/media/player.rs; browsers can't delay
  // audio or take mpv's screenshots) ----
  let subDelay = $state(0);
  let subScale = $state(1);
  type Aspect = '' | '16:9' | '4:3' | '2.35:1' | 'fill';
  let aspect = $state<Aspect>('');
  const RATIO: Record<string, number> = { '16:9': 16 / 9, '4:3': 4 / 3, '2.35:1': 2.35 };
  let stats = $state(false);
  let statsText = $state('');
  // Subtitle cues as they came, so the delay can be changed again.
  const cueTimes = new WeakMap<TextTrackCue, [number, number]>();
  function shiftCues() {
    if (!video) return;
    for (const tt of Array.from(video.textTracks)) {
      for (const c of Array.from(tt.cues ?? [])) {
        let o = cueTimes.get(c);
        if (!o) {
          o = [c.startTime, c.endTime];
          cueTimes.set(c, o);
        }
        c.startTime = Math.max(0, o[0] + subDelay);
        c.endTime = Math.max(0, o[1] + subDelay);
      }
    }
  }
  $effect(() => {
    void subDelay;
    shiftCues();
  });
  const round1 = (x: number) => Math.round(x * 10) / 10;
  function resetSettings() {
    subDelay = 0;
    subScale = 1;
    aspect = '';
    setRate(1);
  }
  $effect(() => {
    if (!stats) return;
    const tick = () => {
      if (!video) return;
      const q = video.getVideoPlaybackQuality?.();
      let ahead = 0;
      for (let i = 0; i < video.buffered.length; i++) {
        if (video.buffered.start(i) <= video.currentTime && video.currentTime <= video.buffered.end(i)) ahead = video.buffered.end(i) - video.currentTime;
      }
      statsText = [
        `${video.videoWidth}×${video.videoHeight}`,
        info?.method ?? '',
        q ? `${q.droppedVideoFrames}/${q.totalVideoFrames} dropped` : '',
        `${ahead.toFixed(1)} s buffered`,
      ]
        .filter(Boolean)
        .join(' · ');
    };
    tick();
    const timer = setInterval(tick, 1000);
    return () => clearInterval(timer);
  });
  // "Use this frame as the thumbnail" (T): taken on the server from the file, locked against
  // providers.
  const canThumb = $derived(!!item && (item.kind === 'movie' || item.kind === 'episode' || item.kind === 'video'));
  async function setThumbnail() {
    if (!item || !canThumb) return;
    try {
      await api.post(`/items/${item.id}/thumbnail`, { at_ms: Math.round(time * 1000), lock: true });
      toast(t('Thumbnail updated.'), 'success');
    } catch (e) {
      toast(t("Couldn't set the thumbnail: {why}", { why: errorText(e) }), 'error');
    }
  }
  let showTitle = $state(true);

  let detach: (() => void) | null = null;
  let stopReport: (() => void) | null = null;
  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  let countTimer: ReturnType<typeof setInterval> | null = null;
  let retried = false;

  const file = $derived(item?.files.find((f) => f.role === 'main') ?? item?.files[0]);
  const chapters = $derived(file?.chapters ?? []);
  const skippable = $derived(skipAt(chapters, time, duration) ?? segmentAt(file?.segments, time));
  const SKIP_LABEL = { intro: tk('Skip intro'), recap: tk('Skip recap'), credits: tk('Skip credits') };
  const audioTracks = $derived(file?.streams.filter((s) => s.kind === 'audio') ?? []);
  const subStreams = $derived(file?.streams.filter((s) => s.kind === 'subtitle') ?? []);
  const speeds = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2];
  /** External subtitle formats the server serves as WebVTT (`TEXT_SUBS` in v1/playback.rs). */
  const TEXT_SUBS = ['srt', 'subrip', 'ass', 'ssa', 'vtt', 'webvtt'];

  async function start(itemId: number, o: PlaybackOptions, at: number | null) {
    error = '';
    waiting = true;
    nextUp = null;
    clearCountdown();
    const el = video!;
    try {
      let restoredBurn = false;
      if (!item || item.id !== itemId) {
        item = await api.get<ItemDetail>(`/items/${itemId}`);
        if (music.playing) music.pause();
        o = restoreTracks(item, o);
        restoredBurn = o.burn_subtitle != null;
        opts = o;
      }
      const old = info;
      try {
        info = await resolveSource(itemId, o);
      } catch (e) {
        // Burning the remembered subtitle isn't possible (no transcoding): play without it.
        if (!restoredBurn) throw e;
        o = { ...o, burn_subtitle: undefined };
        opts = o;
        subtitle = null;
        info = await resolveSource(itemId, o);
      }
      stopSession(old && old.session_id !== info.session_id ? old : null);
      detach?.();
      detach = await attachSource(el, info, (msg) => {
        // Copied streams the browser can't take after all: transcode instead.
        if (info?.method === 'remux' && !retried) {
          retried = true;
          opts = { ...opts, force_transcode: true };
          void start(itemId, opts, el.currentTime || at);
          return;
        }
        error = t('Playback failed ({reason}).', { reason: msg });
      });
      duration = (info.duration_ms ?? item.runtime_ms ?? 0) / 1000;
      const from =
        at ??
        (route.query.get('from') === '0'
          ? 0
          : route.query.get('t')
            ? +route.query.get('t')!
            : resumeAt(item, info.duration_ms) / 1000);
      const seekTo = () => {
        if (from > 0) el.currentTime = from;
        el.playbackRate = rate;
        void el.play().catch(() => (playing = false));
      };
      if (el.readyState >= 1) seekTo();
      else el.addEventListener('loadedmetadata', seekTo, { once: true });
      void loadTrickplay();
      setMediaSession();
      void findNext();
    } catch (e) {
      error = errorText(e);
      waiting = false;
    }
  }

  /** The audio and subtitle tracks chosen last time for this item, when the file still has them.
   *  Subtitles the browser can't show as text (embedded or image ones) are burned in, as when
   *  chosen from the menu. */
  function restoreTracks(it: ItemDetail, o: PlaybackOptions): PlaybackOptions {
    const streams = (it.files.find((f) => f.role === 'main') ?? it.files[0])?.streams ?? [];
    const { audio_track: a, subtitle_track: s } = it.state;
    if (a != null && o.audio_track == null && streams.some((x) => x.kind === 'audio' && x.index === a)) o = { ...o, audio_track: a };
    const sub = streams.find((x) => x.kind === 'subtitle' && x.index === s);
    if (sub && o.burn_subtitle == null) {
      subtitle = sub.index;
      if (!(sub.external && TEXT_SUBS.includes(sub.codec ?? ''))) o = { ...o, burn_subtitle: sub.index };
    }
    return o;
  }

  /** Remembers the track chosen for the item (on every device). */
  function rememberTrack(body: { audio_track?: number; subtitle_track?: number }) {
    const it = item;
    if (!it) return;
    it.state = { ...it.state, ...body };
    void api.put(`/items/${it.id}/state`, body).catch(() => {});
  }

  async function loadTrickplay() {
    trick = [];
    if (!info?.trickplay) return;
    try {
      const r = await fetch(info.trickplay, { credentials: 'same-origin' });
      if (r.ok) trick = parseTrickplay(await r.text(), info.trickplay);
    } catch {
      /* optional */
    }
  }

  /** The next episode: the next one in the season, else the first of the next season. */
  async function findNext() {
    nextUp = null;
    const it = item;
    if (!it || it.kind !== 'episode' || !it.parent_id) return;
    try {
      const eps = (await api.get<Page<Item>>('/items', { parent_id: it.parent_id, sort: 'index', limit: 1000 })).items;
      const i = eps.findIndex((e) => e.id === it.id);
      if (i >= 0 && i + 1 < eps.length) return void (nextUp = eps[i + 1]);
      const season = await api.get<ItemDetail>(`/items/${it.parent_id}`);
      if (!season.parent_id) return;
      const seasons = (await api.get<Page<Item>>('/items', { parent_id: season.parent_id, sort: 'index', limit: 200 })).items;
      const si = seasons.findIndex((s) => s.id === season.id);
      if (si >= 0 && si + 1 < seasons.length) {
        const next = (await api.get<Page<Item>>('/items', { parent_id: seasons[si + 1].id, sort: 'index', limit: 1 })).items[0];
        if (next && item?.id === it.id) nextUp = next;
      }
    } catch {
      /* no auto-next */
    }
  }

  function setMediaSession() {
    if (!('mediaSession' in navigator) || !item) return;
    const img = image(item, 'poster', 'thumb') ?? backdrop(item);
    navigator.mediaSession.metadata = new MediaMetadata({
      title: item.title,
      artist: item.context?.[0] ?? '',
      album: item.context?.[1] ?? '',
      artwork: img ? [{ src: imageUrl(img.sha, 400), sizes: '400x600' }] : [],
    });
    navigator.mediaSession.setActionHandler('play', () => video?.play());
    navigator.mediaSession.setActionHandler('pause', () => video?.pause());
    navigator.mediaSession.setActionHandler('seekbackward', () => skip(-10));
    navigator.mediaSession.setActionHandler('seekforward', () => skip(10));
    navigator.mediaSession.setActionHandler('seekto', (d) => d.seekTime != null && seek(d.seekTime));
    navigator.mediaSession.setActionHandler('nexttrack', nextUp ? () => playNext() : null);
  }

  $effect(() => {
    if (!video) return;
    const el = video;
    el.volume = untrack(() => prefs.volume);
    stopReport = reportProgress(el, () => item?.id ?? null);
    return () => stopReport?.();
  });

  // (Re)start when the item changes.
  $effect(() => {
    const i = id;
    if (video) {
      untrack(() => {
        opts = {};
        subtitle = null;
        retried = false;
        void start(i, {}, null);
      });
    }
  });

  onDestroy(() => {
    detach?.();
    stopSession(info);
    clearCountdown();
    if (hideTimer) clearTimeout(hideTimer);
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    if ('mediaSession' in navigator) navigator.mediaSession.metadata = null;
  });

  function onError() {
    const err = video?.error;
    if (!err || !item) return;
    if ((info?.method === 'direct' || info?.method === 'remux') && !retried) {
      // The browser can't decode this file: ask the server to transcode it.
      retried = true;
      opts = { ...opts, force_transcode: true };
      void start(item.id, opts, time || null);
      return;
    }
    error = t("This video can't be played in the browser.");
    waiting = false;
  }

  function onTime() {
    if (!video) return;
    time = video.currentTime;
    if (!info?.duration_ms && isFinite(video.duration)) duration = video.duration;
    const b: [number, number][] = [];
    for (let i = 0; i < video.buffered.length; i++) b.push([video.buffered.start(i), video.buffered.end(i)]);
    buffered = b;
  }

  function onEnded() {
    playing = false;
    if (nextUp) {
      countdown = 8;
      countTimer = setInterval(() => {
        countdown--;
        if (countdown <= 0) playNext();
      }, 1000);
    } else {
      chrome = true;
    }
  }

  function clearCountdown() {
    if (countTimer) clearInterval(countTimer);
    countTimer = null;
    countdown = 0;
  }

  function playNext() {
    clearCountdown();
    if (nextUp) go(`/play/${nextUp.id}?from=0`, { replace: true });
  }

  // Watching together (?room=<id>).
  let room = $state<SyncRoom | null>(null);
  $effect(() => {
    const roomId = route.query.get('room');
    if (!roomId || room?.id === roomId) return;
    untrack(() => {
      room?.leave();
      SyncRoom.join(roomId, () => video)
        .then((r) => {
          room = r;
          // The room plays something else: follow it.
          if (r.state && r.state.item_id !== id) go(`/play/${r.state.item_id}?room=${encodeURIComponent(r.id)}`, { replace: true });
        })
        .catch((e) => toast(errorText(e), 'error'));
    });
  });
  onDestroy(() => room?.leave());
  // Closing the tab leaves too (else the room counts a ghost until it idles out).
  $effect(() => {
    const bye = () => room?.leave();
    window.addEventListener('pagehide', bye);
    return () => window.removeEventListener('pagehide', bye);
  });
  // Another item here (next episode): the room stays with its item, this player leaves it.
  $effect(() => {
    const cur = id;
    untrack(() => {
      if (room?.state && room.state.item_id !== cur && !route.query.get('room')) {
        room.leave();
        room = null;
      }
    });
  });
  async function watchTogether() {
    if (room) {
      void navigator.clipboard?.writeText(location.href).then(() => toast(t('Link copied: send it to whoever watches with you.'), 'success'));
      return;
    }
    try {
      room = await SyncRoom.create(id, time * 1000, () => video);
      setQuery({ room: room.id });
      void navigator.clipboard?.writeText(location.href).then(() => toast(t('Link copied: send it to whoever watches with you.'), 'success'));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  // Casting to a device the server found (the video here waits, paused).
  let cast = $state<Cast | null>(null);
  let castList = $state<CastDevice[] | null>(null);
  let castLoading = $state(false);
  async function openCast(refresh = false) {
    menu = 'cast';
    castLoading = true;
    try {
      castList = await castDevices(refresh);
    } catch (e) {
      castList = [];
      toast(errorText(e), 'error');
    } finally {
      castLoading = false;
    }
  }
  async function castTo(d: CastDevice) {
    menu = '';
    video?.pause();
    cast?.close();
    const c = new Cast(d);
    cast = c;
    try {
      await c.start(id, time * 1000, opts.audio_track);
    } catch (e) {
      toast(errorText(e), 'error');
      c.close();
      cast = null;
    }
  }
  function stopCast() {
    const c = cast;
    if (!c) return;
    const at = c.position();
    void c.send('stop').catch(() => {});
    c.close();
    cast = null;
    if (video && at > 0) video.currentTime = at;
  }
  $effect(() => {
    const s = cast?.status;
    if (!s) return;
    time = s.position_ms / 1000;
    if (s.duration_ms) duration = s.duration_ms / 1000;
    playing = s.state === 'playing' || s.state === 'buffering';
  });
  onDestroy(() => cast?.close());

  function toggle() {
    if (cast) {
      void cast.send(playing ? 'pause' : 'play').catch((e) => toast(errorText(e), 'error'));
      return;
    }
    if (!video) return;
    if (video.paused) void video.play();
    else video.pause();
  }

  function seek(s: number) {
    if (cast) {
      time = s;
      void cast.send('seek', s).catch((e) => toast(errorText(e), 'error'));
      return;
    }
    if (video) video.currentTime = Math.max(0, Math.min(s, duration || s));
    room?.send('seek', s);
  }
  function skip(d: number) {
    seek(time + d);
    poke();
  }

  function setVolume(v: number) {
    prefs.volume = Math.max(0, Math.min(1, v));
    if (video) {
      video.volume = prefs.volume;
      video.muted = false;
      muted = false;
    }
  }

  function toggleMute() {
    if (!video) return;
    video.muted = !video.muted;
    muted = video.muted;
  }

  function setRate(r: number) {
    rate = r;
    if (video) video.playbackRate = r;
    menu = '';
  }

  async function toggleFullscreen() {
    if (!stage) return;
    if (document.fullscreenElement) await document.exitFullscreen();
    else await stage.requestFullscreen().catch(() => {});
  }

  async function togglePip() {
    if (!video) return;
    try {
      if (document.pictureInPictureElement) await document.exitPictureInPicture();
      else await video.requestPictureInPicture();
    } catch {
      /* not allowed */
    }
  }

  function chooseSubtitle(idx: number | null) {
    menu = '';
    subtitle = idx;
    rememberTrack({ subtitle_track: idx ?? 0 });
    const track = idx == null ? null : info?.subtitles.find((s) => s.index === idx);
    if (idx != null && track && !track.url) {
      // Image subtitles: have the server burn them into the picture.
      opts = { ...opts, burn_subtitle: idx };
      void start(item!.id, opts, time);
      return;
    }
    if (opts.burn_subtitle != null) {
      opts = { ...opts, burn_subtitle: undefined };
      void start(item!.id, opts, time);
    }
    applyTracks();
  }

  function applyTracks() {
    if (!video) return;
    for (const tt of Array.from(video.textTracks)) {
      tt.mode = tt.id === `sub-${subtitle}` ? 'showing' : 'disabled';
    }
  }

  function chooseAudio(idx: number) {
    menu = '';
    rememberTrack({ audio_track: idx });
    opts = { ...opts, audio_track: idx };
    void start(item!.id, opts, time);
  }

  function poke() {
    chrome = true;
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (playing && !menu) chrome = false;
    }, 3000);
  }

  function onkeydown(e: KeyboardEvent) {
    if ((e.target as HTMLElement).closest('input, select, textarea')) return;
    const k = e.key.toLowerCase();
    if (k === ' ' || k === 'k') {
      if ((e.target as HTMLElement).closest('button') && k === ' ') return;
      toggle();
    } else if (k === 'arrowleft' || k === 'j') skip(k === 'j' ? -10 : -5);
    else if (k === 'arrowright' || k === 'l') skip(k === 'l' ? 10 : 5);
    else if (k === 'arrowup') setVolume(prefs.volume + 0.05);
    else if (k === 'arrowdown') setVolume(prefs.volume - 0.05);
    else if (k === 'f') void toggleFullscreen();
    else if (k === 'm') toggleMute();
    else if (k === 'n' && nextUp) playNext();
    else if (k === 't' && canThumb) void setThumbnail();
    else if (k === 'escape') {
      if (menu) menu = '';
      else if (!document.fullscreenElement) close();
      else return;
    } else return;
    e.preventDefault();
    poke();
  }

  function close() {
    back(item?.parent_id && item.kind === 'episode' ? `/item/${item.parent_id}` : item ? `/item/${item.id}` : '/');
  }

  const thumbAt = (s: number) => {
    const c = cueAt(trick, s);
    return c ? { url: c.url, x: c.x, y: c.y, w: c.w, h: c.h } : null;
  };

  const subLabel = (s: { lang?: string | null; title?: string | null; forced?: boolean; index: number }) =>
    [s.title || s.lang?.toUpperCase() || t('Track {n}', { n: s.index }), s.forced ? t('forced') : ''].filter(Boolean).join(' · ');

  $effect(() => {
    // Hide the title a while after the start.
    void id;
    showTitle = true;
    const h = setTimeout(() => (showTitle = false), 4000);
    return () => clearTimeout(h);
  });
</script>

<svelte:window {onkeydown} />
<svelte:document onfullscreenchange={() => (fs = !!document.fullscreenElement)} />

<div
  class="stage"
  class:chrome={chrome || !playing}
  bind:this={stage}
  role="application"
  aria-label={t('Video player')}
  onpointermove={poke}
  onpointerdown={poke}>
  <!-- svelte-ignore a11y_media_has_caption -->
  <video
    bind:this={video}
    class:fill={aspect === 'fill'}
    class:forced={!!RATIO[aspect]}
    style:--ratio={RATIO[aspect] ?? null}
    style:--sub-scale={subScale}
    playsinline
    onclick={toggle}
    ondblclick={toggleFullscreen}
    onplay={() => ((playing = true), poke(), room?.send('play', video?.currentTime ?? time))}
    onpause={() => ((playing = false), (chrome = true), room?.send('pause', video?.currentTime ?? time))}
    onwaiting={() => (waiting = true)}
    onplaying={() => (waiting = false)}
    oncanplay={() => (waiting = false)}
    ontimeupdate={onTime}
    onprogress={onTime}
    ondurationchange={onTime}
    onended={onEnded}
    onerror={onError}
    onloadedmetadata={() => (applyTracks(), room?.apply())}>
    {#each info?.subtitles.filter((s) => s.url) ?? [] as s (s.index)}
      <track id="sub-{s.index}" kind="subtitles" src={s.url} srclang={s.lang ?? undefined} label={subLabel(s)} onload={shiftCues} />
    {/each}
  </video>

  {#if stats && statsText}<div class="stats num">{statsText}</div>{/if}

  {#if waiting && !error}
    <div class="center" transition:fade={{ duration: ms(150) }}><Spinner size={44} /></div>
  {/if}

  {#if error}
    <div class="center err" role="alert">
      <Icon name="warning-circle" size={40} />
      <p>{error}</p>
      <div class="row">
        <button class="btn" onclick={close}><Icon name="arrow-left" />{t('Back')}</button>
        <button class="btn primary" onclick={() => item && start(item.id, { ...opts, force_transcode: true }, time || null)}><Icon name="arrow-clockwise" />{t('Try again')}</button>
      </div>
    </div>
  {/if}

  {#if countdown > 0 && nextUp}
    <div class="upnext" transition:fly={{ x: 40, duration: ms(260) }}>
      <span class="caption muted">{t('Next episode in {n} s', { n: countdown })}</span>
      <strong class="ellipsis">{nextUp.index != null ? `${nextUp.index}. ` : ''}{nextUp.title}</strong>
      <div class="row">
        <button class="btn primary sm" onclick={playNext}><Icon name="play-fill" size={14} />{t('Play now')}</button>
        <button class="btn sm" onclick={clearCountdown}>{t('Cancel')}</button>
      </div>
    </div>
  {/if}

  <div class="top">
    <button class="btn ghost icon" aria-label={t('Back')} onclick={close}><Icon name="arrow-left" size={22} /></button>
    <div class="ttl" class:hidden={!showTitle && playing && !chrome}>
      {#if item}
        <strong class="ellipsis">{item.title}</strong>
        {#if item.context?.length}<span class="caption ellipsis">{item.context.join(' · ')}</span>{/if}
      {/if}
    </div>
    {#if info?.method === 'hls'}<span class="badge" title={info.reasons.join(', ')}>{t('Transcoding')}</span>{/if}
    {#if info?.method === 'remux'}<span class="badge" title={info.reasons.join(', ')}>{t('Direct stream')}</span>{/if}
  </div>

  {#if cast}
    <div class="castbox" transition:fade={{ duration: ms(150) }}>
      <Icon name="screencast" size={36} />
      <div class="stext">
        <strong>{t('Playing on {name}', { name: cast.device.name })}</strong>
        <span class="small muted">
          {#if cast.status?.state === 'error'}{!cast.status.error || cast.status.error === 'cant_play' ? t('The device couldn’t play this.') : cast.status.error}
          {:else if cast.status?.state === 'connecting' || cast.status?.state === 'loading'}{t('Connecting…')}
          {:else if cast.status?.state === 'idle'}{t('Finished')}
          {/if}
        </span>
      </div>
      <button class="btn sm" onclick={stopCast}><Icon name="stop" size={14} />{t('Stop casting')}</button>
    </div>
  {:else if !playing && !waiting && !error && countdown === 0}
    <button class="bigplay" aria-label={t('Play')} onclick={toggle} transition:fade={{ duration: ms(150) }}><Icon name="play-fill" size={40} /></button>
  {/if}

  {#if skippable}
    <button class="btn skipbtn" class:raised={chrome} onclick={() => (skippable!.kind === 'credits' && duration - skippable!.end < 5 && nextUp ? playNext() : seek(skippable!.end))} transition:fade={{ duration: ms(150) }}>
      <Icon name="skip-forward-fill" size={16} />{t(SKIP_LABEL[skippable.kind])}
    </button>
  {/if}

  <div class="bottom">
    <Seek value={time} max={duration} onseek={seek} label={t('Seek')} {chapters} {buffered} thumb={trick.length ? thumbAt : undefined} big />
    <div class="bar">
      <button class="btn ghost icon" aria-label={playing ? t('Pause') : t('Play')} title={playing ? t('Pause') : t('Play')} onclick={toggle}><Icon name={playing ? 'pause-fill' : 'play-fill'} size={22} /></button>
      <button class="btn ghost icon" aria-label={t('Back 10 seconds')} title={t('Back 10 seconds')} onclick={() => skip(-10)}><Icon name="arrow-counter-clockwise" size={20} /></button>
      <button class="btn ghost icon" aria-label={t('Forward 10 seconds')} title={t('Forward 10 seconds')} onclick={() => skip(10)}><Icon name="arrow-clockwise" size={20} /></button>
      {#if nextUp}
        <button class="btn ghost icon" aria-label={t('Next episode')} title={t('Next episode')} onclick={playNext}><Icon name="skip-forward-fill" size={20} /></button>
      {/if}
      <div class="vol">
        <button class="btn ghost icon" aria-label={muted ? t('Unmute') : t('Mute')} title={muted ? t('Unmute') : t('Mute')} onclick={toggleMute}>
          <Icon name={muted || prefs.volume === 0 ? 'speaker-x' : prefs.volume < 0.5 ? 'speaker-low' : 'speaker-high'} size={20} />
        </button>
        <input class="range" type="range" min="0" max="1" step="0.01" value={muted ? 0 : prefs.volume} style:--fill="{(muted ? 0 : prefs.volume) * 100}%" aria-label={t('Volume')} oninput={(e) => setVolume(+e.currentTarget.value)} />
      </div>
      <span class="time num">{clock(time * 1000)} <span class="muted">/ {clock(duration * 1000)}</span></span>
      <span class="spacer"></span>

      <div class="pop">
        <button class="btn ghost sm" aria-haspopup="menu" aria-expanded={menu === 'speed'} aria-label={t('Playback speed')} title={t('Playback speed')} onclick={() => (menu = menu === 'speed' ? '' : 'speed')}><Icon name="gauge" size={20} /><span class="num">{rate}×</span></button>
        {#if menu === 'speed'}
          <div class="menu" role="menu" transition:fly={{ y: 8, duration: ms(140) }}>
            <div class="mh">{t('Playback speed')}</div>
            {#each speeds as s (s)}
              <button class="mi" role="menuitemradio" aria-checked={rate === s} onclick={() => setRate(s)}>{s === 1 ? t('Normal') : `${s}×`}{#if rate === s}<Icon name="check" size={14} />{/if}</button>
            {/each}
          </div>
        {/if}
      </div>

      <div class="pop">
        <button class="btn ghost icon" aria-haspopup="menu" aria-expanded={menu === 'settings'} aria-label={t('Playback settings')} title={t('Playback settings')} onclick={() => (menu = menu === 'settings' ? '' : 'settings')}><Icon name="sliders-horizontal" size={20} /></button>
        {#if menu === 'settings'}
          <div class="menu settings" role="menu" transition:fly={{ y: 8, duration: ms(140) }}>
            <div class="mh">{t('Playback settings')}</div>
            <div class="adj">
              <span class="grow">{t('Subtitle delay')}</span>
              <button class="btn ghost icon sm" aria-label="−0.1 s" onclick={() => (subDelay = round1(subDelay - 0.1))}><Icon name="minus" size={14} /></button>
              <span class="num val">{subDelay > 0 ? '+' : ''}{subDelay.toFixed(1)} s</span>
              <button class="btn ghost icon sm" aria-label="+0.1 s" onclick={() => (subDelay = round1(subDelay + 0.1))}><Icon name="plus" size={14} /></button>
            </div>
            <div class="adj">
              <span class="grow">{t('Subtitle size')}</span>
              <button class="btn ghost icon sm" aria-label="−10%" disabled={subScale <= 0.5} onclick={() => (subScale = round1(subScale - 0.1))}><Icon name="minus" size={14} /></button>
              <span class="num val">{Math.round(subScale * 100)}%</span>
              <button class="btn ghost icon sm" aria-label="+10%" disabled={subScale >= 3} onclick={() => (subScale = round1(subScale + 0.1))}><Icon name="plus" size={14} /></button>
            </div>
            <div class="mh">{t('Picture')}</div>
            <div class="chips">
              {#each [['', t('Original')], ['16:9', '16:9'], ['4:3', '4:3'], ['2.35:1', '2.35:1'], ['fill', t('Fill')]] as [v, l] (v)}
                <button class="chip" class:on={aspect === v} aria-pressed={aspect === v} onclick={() => (aspect = v as Aspect)}>{l}</button>
              {/each}
            </div>
            <div class="sep"></div>
            {#if canThumb}
              <button class="mi" role="menuitem" onclick={() => ((menu = ''), void setThumbnail())}><span>{t('Use this frame as the thumbnail (T)')}</span><Icon name="image" size={14} /></button>
            {/if}
            <button class="mi" role="menuitemcheckbox" aria-checked={stats} onclick={() => (stats = !stats)}>{t('Stats')}{#if stats}<Icon name="check" size={14} />{/if}</button>
            <button class="mi" role="menuitem" onclick={resetSettings}>{t('Reset')}</button>
          </div>
        {/if}
      </div>

      {#if (info?.subtitles.length ?? 0) > 0 || subStreams.length > 0}
        <div class="pop">
          <button class="btn ghost icon" class:on={subtitle != null} aria-haspopup="menu" aria-expanded={menu === 'subs'} aria-label={t('Subtitles')} title={t('Subtitles')} onclick={() => (menu = menu === 'subs' ? '' : 'subs')}><Icon name="closed-captioning" size={22} /></button>
          {#if menu === 'subs'}
            <div class="menu" role="menu" transition:fly={{ y: 8, duration: ms(140) }}>
              <div class="mh">{t('Subtitles')}</div>
              <button class="mi" role="menuitemradio" aria-checked={subtitle == null} onclick={() => chooseSubtitle(null)}>{t('Off')}</button>
              {#each info?.subtitles ?? [] as s (s.index)}
                <button class="mi" role="menuitemradio" aria-checked={subtitle === s.index} onclick={() => chooseSubtitle(s.index)}>
                  <span class="grow ellipsis">{subLabel(s)}</span>
                  {#if !s.url}<span class="badge">{t('burn in')}</span>{/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      {#if audioTracks.length > 1}
        <div class="pop">
          <button class="btn ghost icon" aria-haspopup="menu" aria-expanded={menu === 'audio'} aria-label={t('Audio track')} title={t('Audio track')} onclick={() => (menu = menu === 'audio' ? '' : 'audio')}><Icon name="speaker-high" size={20} /></button>
          {#if menu === 'audio'}
            <div class="menu" role="menu" transition:fly={{ y: 8, duration: ms(140) }}>
              <div class="mh">{t('Audio track')}</div>
              {#each audioTracks as a (a.index)}
                <button class="mi" role="menuitemradio" aria-checked={(opts.audio_track ?? audioTracks[0].index) === a.index} onclick={() => chooseAudio(a.index)}>
                  {[a.title, a.lang?.toUpperCase(), a.codec?.toUpperCase(), a.channels ? t('{n} ch', { n: a.channels }) : ''].filter(Boolean).join(' · ')}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <div class="pop">
        <button class="btn ghost icon" class:on={cast != null} aria-haspopup="menu" aria-expanded={menu === 'cast'} aria-label={t('Cast to a device')} title={t('Cast to a device')} onclick={() => (menu === 'cast' ? (menu = '') : void openCast())}><Icon name="screencast" size={20} /></button>
        {#if menu === 'cast'}
          <div class="menu" role="menu" transition:fly={{ y: 8, duration: ms(140) }}>
            <div class="mh">{t('Cast to')}</div>
            {#if castLoading}
              <div class="mi"><Spinner size={14} />{t('Looking for devices…')}</div>
            {:else if !castList?.length}
              <div class="mi muted">{t('No Chromecasts found on the server’s network.')}</div>
            {:else}
              {#each castList as d (d.id)}
                <button class="mi" role="menuitemradio" aria-checked={cast?.device.id === d.id} onclick={() => castTo(d)}>
                  <span class="grow ellipsis">{d.name}</span>{#if d.model}<span class="small muted">{d.model}</span>{/if}
                </button>
              {/each}
            {/if}
            {#if cast}<button class="mi" onclick={() => ((menu = ''), stopCast())}>{t('Stop casting')}</button>{/if}
            <button class="mi" onclick={() => openCast(true)}><Icon name="arrows-clockwise" size={14} />{t('Look again')}</button>
          </div>
        {/if}
      </div>
      <button class="btn ghost icon" class:on={room != null} aria-label={t('Watch together')} title={room ? t('Watching together: {n} people', { n: room.state?.members.length ?? 1 }) : t('Watch together')} onclick={watchTogether}><Icon name="users" size={20} /></button>
      {#if 'pictureInPictureEnabled' in document}
        <button class="btn ghost icon" aria-label={t('Picture in picture')} title={t('Picture in picture')} onclick={togglePip}><Icon name="picture-in-picture" size={20} /></button>
      {/if}
      <button class="btn ghost icon" aria-label={fs ? t('Exit full screen') : t('Full screen')} title={fs ? t('Exit full screen') : t('Full screen')} onclick={toggleFullscreen}><Icon name={fs ? 'corners-in' : 'corners-out'} size={20} /></button>
    </div>
  </div>
</div>

<style>
  .castbox {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 20px;
    border-radius: 14px;
    background: color-mix(in srgb, #000 70%, transparent);
    color: #fff;
    max-width: calc(100% - 32px);
  }
  .castbox .stext {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .skipbtn {
    position: absolute;
    right: 32px;
    bottom: 32px;
    z-index: 5;
    background: rgba(0, 0, 0, 0.75);
    border: 1px solid var(--accent);
    color: #fff;
    padding: 10px 18px;
    transition: bottom var(--motion-page, 0.25s);
  }
  .skipbtn.raised {
    bottom: 128px;
  }
  .stage {
    position: fixed;
    inset: 0;
    z-index: 90;
    background: #000;
    color: #fff;
    overflow: hidden;
    cursor: none;
    --text: #fff;
    --text-dim: rgba(255, 255, 255, 0.72);
    --hover: rgba(255, 255, 255, 0.12);
  }
  .stage.chrome {
    cursor: default;
  }
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
  video.fill {
    object-fit: cover;
  }
  video.forced {
    position: absolute;
    inset: 0;
    margin: auto;
    width: min(100%, calc(100dvh * var(--ratio)));
    height: min(100%, calc(100vw / var(--ratio)));
    object-fit: fill;
  }
  video::cue {
    font-family: var(--font-sans);
    background: rgba(0, 0, 0, 0.6);
    font-size: calc(1.1em * var(--sub-scale, 1));
  }
  .stats {
    position: absolute;
    top: 64px;
    left: 16px;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
    font-size: 12px;
    pointer-events: none;
  }
  .settings {
    min-width: 320px;
  }
  .adj {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    font-size: 14px;
  }
  .val {
    min-width: 56px;
    text-align: center;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 10px 8px;
  }
  .chip {
    padding: 4px 10px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    font-size: 12px;
    color: #fff;
  }
  .chip.on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .menu .sep {
    height: 1px;
    margin: 6px 4px;
    background: rgba(255, 255, 255, 0.12);
  }
  .center {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 14px;
    text-align: center;
    pointer-events: none;
  }
  .center.err {
    pointer-events: auto;
    background: rgba(0, 0, 0, 0.6);
  }
  .bigplay {
    position: absolute;
    inset: 0;
    margin: auto;
    width: 92px;
    height: 92px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--accent) 85%, transparent);
    color: var(--on-accent);
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.5);
    transition: transform var(--dur-micro) var(--ease-emphasized);
  }
  .bigplay:hover {
    transform: scale(1.06);
  }
  .top,
  .bottom {
    position: absolute;
    left: 0;
    right: 0;
    opacity: 0;
    transition:
      opacity var(--dur-page) var(--ease-standard),
      transform var(--dur-page) var(--ease-standard);
    pointer-events: none;
  }
  .chrome .top,
  .chrome .bottom,
  .stage:focus-within .bottom {
    opacity: 1;
    transform: none;
    pointer-events: auto;
  }
  .top {
    top: 0;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 20px 40px;
    background: linear-gradient(rgba(0, 0, 0, 0.7), transparent);
    transform: translateY(-8px);
  }
  .ttl {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    transition: opacity var(--dur-page);
  }
  .ttl strong {
    font-size: 18px;
  }
  .ttl .caption {
    color: var(--text-dim);
  }
  .bottom {
    bottom: 0;
    padding: 50px 20px 12px;
    background: linear-gradient(transparent, rgba(0, 0, 0, 0.8));
    transform: translateY(8px);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
  }
  .bar .btn {
    color: #fff;
  }
  .bar .btn.on {
    color: var(--accent);
  }
  .vol {
    display: flex;
    align-items: center;
    width: 150px;
  }
  .time {
    margin-left: 10px;
    font-size: 13px;
  }
  .time .muted {
    color: var(--text-dim);
  }
  .pop {
    position: relative;
  }
  .menu {
    position: absolute;
    bottom: calc(100% + 10px);
    right: 0;
    min-width: 200px;
    max-height: 50vh;
    overflow: auto;
    padding: 6px;
    border-radius: var(--r-lg);
    background: rgba(20, 22, 28, 0.94);
    backdrop-filter: blur(14px);
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: var(--shadow-4);
  }
  .mh {
    padding: 8px 10px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-dim);
  }
  .mi {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    font-size: 14px;
    text-align: left;
  }
  .mi:hover,
  .mi:focus-visible {
    background: var(--hover);
  }
  .mi[aria-checked='true'] {
    color: var(--accent);
  }
  .upnext {
    position: absolute;
    right: 24px;
    bottom: 120px;
    width: 300px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 16px;
    border-radius: var(--r-lg);
    background: rgba(20, 22, 28, 0.92);
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: var(--shadow-4);
  }
  .upnext .muted {
    color: var(--text-dim);
  }
  .badge {
    background: rgba(255, 255, 255, 0.14);
    color: #fff;
  }
  @media (max-width: 640px) {
    .vol,
    .time .muted {
      display: none;
    }
    .bottom {
      padding: 40px 10px 10px;
    }
  }
</style>
