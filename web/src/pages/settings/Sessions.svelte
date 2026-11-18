<script lang="ts">
  // Streaming performance (GET /playback/stats; admins see everyone's): the server's load, each
  // conversion with whether it keeps up with its player, and what is being cast. A server
  // without the route (404) gets the plain list of GET /playback/sessions.
  import { everyVisible } from '../../lib/visibility.svelte';
  import Icon from '../../components/Icon.svelte';
  import Progress from '../../components/Progress.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, ApiError, errorText } from '../../lib/api';
  import { bytes, clock, num } from '../../lib/format';
  import { link } from '../../lib/router.svelte';
  import { bandwidth, health, type Health, pushSample, type Reading, sparkPoints } from '../../lib/streamhealth';
  import { toast } from '../../lib/toast.svelte';
  import { t, tk } from '../../lib/i18n.svelte';
  import type { CastStatus, PlaybackSession, PlaybackStats } from '../../lib/types';

  let stats = $state<PlaybackStats | null>(null);
  /** An older server's list (no `/playback/stats`). */
  let list = $state<PlaybackSession[] | null>(null);
  let old = $state(false);
  let error = $state('');

  /** Bits per second each conversion sends now, by session id, and all of them together. */
  let bps = $state<Record<string, number>>({});
  let total = $state(0);
  let cpuHist = $state<number[]>([]);
  let outHist = $state<number[]>([]);
  const readings = new Map<string, Reading>();

  function took(s: PlaybackStats) {
    const at = performance.now();
    const now: Record<string, number> = {};
    let sum = 0;
    for (const x of s.sessions) {
      const r = { bytes: x.sent_bytes ?? 0, at };
      now[x.id] = bandwidth(readings.get(x.id), r);
      sum += now[x.id];
      readings.set(x.id, r);
    }
    for (const id of [...readings.keys()]) if (!(id in now)) readings.delete(id);
    bps = now;
    total = sum;
    cpuHist = pushSample(cpuHist, s.cpu_percent);
    outHist = pushSample(outHist, sum);
    stats = s;
  }

  let busy = false;
  async function load() {
    if (busy) return;
    busy = true;
    try {
      if (!old) {
        try {
          took(await api.get<PlaybackStats>('/playback/stats'));
          error = '';
          return;
        } catch (e) {
          if (!(e instanceof ApiError) || e.status !== 404) throw e;
          old = true;
        }
      }
      list = await api.get<PlaybackSession[]>('/playback/sessions');
      error = '';
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
  $effect(() => {
    void load();
    return everyVisible(load, 2000);
  });

  async function stop(s: PlaybackSession) {
    try {
      await api.del(`/playback/sessions/${s.id}`);
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  const SW = 120;
  const SH = 28;

  function bitrate(v: number): string {
    return v >= 1_000_000 ? t('{n} Mbit/s', { n: num(v / 1_000_000, 1) }) : t('{n} kbit/s', { n: num(Math.round(v / 1000)) });
  }

  /** "720p · 4.0 Mbit/s video · 192 kbit/s audio", or "sound only · 192 kbit/s". */
  function target(s: PlaybackSession): string {
    const parts: string[] = [];
    if (s.video) {
      if (s.height) parts.push(`${s.height}p`);
      if (s.video_bitrate) parts.push(t('{rate} video', { rate: bitrate(s.video_bitrate) }));
      if (s.audio_bitrate) parts.push(t('{rate} audio', { rate: bitrate(s.audio_bitrate) }));
    } else {
      parts.push(t('sound only'));
      if (s.audio_bitrate) parts.push(bitrate(s.audio_bitrate));
    }
    return parts.join(' · ');
  }

  function verdict(s: PlaybackSession): Health {
    return health({ produced: s.produced, idle_secs: s.idle_secs, encoding: !!s.encoding, speed: s.speed ?? 0, ahead_secs: s.ahead_secs ?? 0 });
  }
  const VERDICT: Record<Health, { label: string; tone: string }> = {
    smooth: { label: tk('Smooth'), tone: 'ok' },
    barely: { label: tk('Barely keeping up'), tone: 'warn' },
    slow: { label: tk('Too slow'), tone: 'err' },
    starting: { label: tk('Starting'), tone: '' },
    waiting: { label: tk('Waiting for the player'), tone: '' },
  };

  function castState(c: CastStatus): string {
    switch (c.state) {
      case 'playing':
        return t('Playing');
      case 'paused':
        return t('Paused');
      case 'buffering':
        return t('Buffering');
      case 'connecting':
        return t('Connecting…');
      case 'loading':
        return t('Loading…');
      case 'error':
        return t('Error');
      default:
        return t('Stopped');
    }
  }

  const cpu = $derived(Math.max(0, Math.min(100, stats?.cpu_percent ?? 0)));
  const outPeak = $derived(Math.max(1_000_000, ...outHist));
</script>

{#if error && !stats && !list}
  <p class="form-error">{error}</p>
{:else if stats}
  <p class="muted small lead">{t('What the server is converting right now and whether it keeps up with the players. Files played as they are cost the server almost nothing and are not listed.')}</p>
  {#if error}<p class="form-error">{error}</p>{/if}

  <div class="sgroup">
    <h3>{t('Server load')}</h3>
    <div class="srow">
      <div class="stext">
        <span class="slabel">{t('Processor')}</span>
        <span class="shint">{t('{n} cores', { n: stats.cores })}</span>
      </div>
      <div class="sctl meter">
        <div class="mline">
          {#if cpuHist.length > 1}
            <svg class="spark" viewBox="0 0 {SW} {SH}" preserveAspectRatio="none" aria-hidden="true"><polyline points={sparkPoints(cpuHist, 100, SW, SH)} /></svg>
          {/if}
          <span class="num mval">{num(cpu)}%</span>
        </div>
        <Progress value={cpu / 100} color={cpu >= 90 ? 'var(--error)' : cpu >= 75 ? 'var(--warn)' : 'var(--accent)'} label={t('Processor')} />
      </div>
    </div>
    {#if stats.memory_total > 0}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Memory')}</span></div>
        <div class="sctl meter">
          <div class="mline"><span class="num mval">{t('{used} of {total}', { used: bytes(stats.memory_used), total: bytes(stats.memory_total) })}</span></div>
          <Progress value={stats.memory_used / stats.memory_total} label={t('Memory')} />
        </div>
      </div>
    {/if}
    <div class="srow">
      <div class="stext">
        <span class="slabel">{t('Sending now')}</span>
      </div>
      <div class="sctl meter">
        <div class="mline">
          {#if outHist.length > 1}
            <svg class="spark out" viewBox="0 0 {SW} {SH}" preserveAspectRatio="none" aria-hidden="true"><polyline points={sparkPoints(outHist, outPeak, SW, SH)} /></svg>
          {/if}
          <span class="num mval">{bitrate(total)}</span>
        </div>
      </div>
    </div>
    <div class="srow">
      <div class="stext">
        <span class="slabel">{t('Conversions')}</span>
        <span class="shint">{stats.encoder ? t('Encoder in use: {name}', { name: stats.encoder }) : t('No working video encoder was found: only sound can be converted.')}</span>
      </div>
      <div class="sctl"><span class="num mval">{t('{n} of {max}', { n: stats.sessions.length, max: stats.max_sessions })}</span></div>
    </div>
  </div>

  <div class="sgroup">
    <h3>{t('Conversions')}</h3>
    {#each stats.sessions as s (s.id)}
      {@const v = verdict(s)}
      {@const speed = s.speed ?? 0}
      <div class="srow wide">
        <div class="head">
          <span class="ic"><Icon name={s.video ? 'film-strip' : 'music-note'} size={20} /></span>
          <div class="stext grow">
            {#if s.item_id}<a class="slabel text-link" href="/item/{s.item_id}" use:link>{s.title}</a>{:else}<span class="slabel">{s.title}</span>{/if}
            <span class="shint">{s.user} · {target(s)} · {s.encoder}</span>
          </div>
          <span class="chip verdict {VERDICT[v].tone}"><span class="dot"></span>{t(VERDICT[v].label)}</span>
          <button class="btn sm danger" onclick={() => stop(s)}><Icon name="stop" size={14} />{t('Stop')}</button>
        </div>
        {#if v === 'slow'}
          <p class="hint">{t('The server can’t convert this fast enough. Choose a lower quality, or a lighter encoding effort in Settings → Transcoding.')}</p>
        {/if}
        <dl class="facts">
          <div>
            <dt>{t('Encoding speed')}</dt>
            <dd class="num">
              {#if speed > 0}
                {t('{n}× real time', { n: num(speed, 1) })}{#if s.video && s.fps}<span class="sub"> · {t('{n} fps', { n: num(Math.round(speed * s.fps)) })}</span>{/if}
              {:else if s.encoding}–{:else}{t('Resting')}{/if}
            </dd>
          </div>
          <div><dt>{t('Buffer ahead')}</dt><dd class="num">{t('{s} s', { s: num(Math.max(0, Math.round(s.ahead_secs ?? 0))) })}</dd></div>
          <div><dt>{t('Bandwidth')}</dt><dd class="num">{bitrate(bps[s.id] ?? 0)}</dd></div>
          <div><dt>{t('Position')}</dt><dd class="num">{s.position_secs != null ? clock(s.position_secs * 1000) : '–'}</dd></div>
        </dl>
        <Progress value={s.segments ? s.produced / s.segments : 0} height={4} label={t('{n} of {total} segments', { n: s.produced, total: s.segments })} />
      </div>
    {:else}
      <div class="srow wide"><span class="muted small">{t('Nothing is being transcoded')}</span></div>
    {/each}
  </div>

  {#if stats.casts.length}
    <div class="sgroup">
      <h3>{t('Casting')}</h3>
      {#each stats.casts as c (c.device.id)}
        <div class="srow wide">
          <div class="head">
            <span class="ic"><Icon name={c.device.kind === 'dlna' ? 'television-simple' : 'screencast'} size={20} /></span>
            <div class="stext grow">
              <span class="slabel">{c.device.name}{#if c.device.kind === 'dlna'} <span class="badge">{t('TV')}</span>{/if}</span>
              <span class="shint">
                {#if c.title}{#if c.item_id}<a class="text-link" href="/item/{c.item_id}" use:link>{c.title}</a>{:else}{c.title}{/if}{#if c.by} · {c.by}{/if}{:else}{c.by}{/if}
              </span>
            </div>
            <span class="num small muted">{clock(c.position_ms)}{#if c.duration_ms} / {clock(c.duration_ms)}{/if}</span>
            <span class="badge {c.state === 'error' ? 'err' : c.state === 'playing' ? 'ok' : ''}">{castState(c)}</span>
          </div>
        </div>
      {/each}
    </div>
  {/if}
{:else if list}
  <p class="muted small lead">{t('Media the server is transcoding right now. Direct play does not show up here.')}</p>
  {#if error}<p class="form-error">{error}</p>{/if}
  {#if !list.length}
    <div class="empty"><div class="empty-icon"><Icon name="broadcast" size={30} /></div><h3>{t('Nothing is being transcoded')}</h3></div>
  {:else}
    <div class="sgroup">
      {#each list as s (s.id)}
        <div class="srow srow2">
          <div class="row">
            <span class="ic"><Icon name={s.video ? 'film-strip' : 'music-note'} size={20} /></span>
            <div class="stext">
              {#if s.item_id}<a class="slabel text-link" href="/item/{s.item_id}" use:link>{s.title}</a>{:else}<span class="slabel">{s.title}</span>{/if}
              <span class="shint">{s.user} · {s.encoder} · {t('{n} of {total} segments', { n: s.produced, total: s.segments })}{s.idle_secs > 5 ? ` · ${t('idle {n} s', { n: s.idle_secs })}` : ''}</span>
              <div class="bar"><Progress value={s.segments ? s.produced / s.segments : 0} height={4} /></div>
            </div>
          </div>
          <div class="sctl"><button class="btn sm danger" onclick={() => stop(s)}><Icon name="stop" size={14} />{t('Stop')}</button></div>
        </div>
      {/each}
    </div>
  {/if}
{:else}
  <Spinner />
{/if}

<style>
  .lead {
    margin-bottom: 16px;
  }
  .srow2 {
    grid-template-columns: 1fr auto;
  }
  .ic {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  .bar {
    margin-top: 6px;
    width: min(320px, 100%);
  }
  .meter {
    flex-direction: column;
    align-items: stretch;
    gap: 6px;
  }
  .mline {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 12px;
    min-height: 28px;
  }
  .mval {
    font-weight: var(--fw-semibold);
    font-size: var(--fs-small);
    white-space: nowrap;
  }
  .spark {
    width: 120px;
    height: 28px;
    flex: none;
  }
  .spark polyline {
    fill: none;
    stroke: var(--accent);
    stroke-width: 1.6;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
  .spark.out polyline {
    stroke: var(--success);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .head .grow {
    flex: 1 1 200px;
  }
  .verdict {
    flex: none;
    color: var(--text);
    white-space: nowrap;
  }
  .verdict .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-dim);
  }
  .verdict.ok .dot {
    background: var(--success);
  }
  .verdict.warn {
    border-color: color-mix(in srgb, var(--warn) 50%, transparent);
  }
  .verdict.warn .dot {
    background: var(--warn);
  }
  .verdict.err {
    border-color: color-mix(in srgb, var(--error) 50%, transparent);
  }
  .verdict.err .dot {
    background: var(--error);
  }
  .hint {
    margin: 0;
    color: var(--error);
    font-size: var(--fs-caption);
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 8px 20px;
    margin: 4px 0;
  }
  .facts dt {
    color: var(--text-dim);
    font-size: var(--fs-caption);
  }
  .facts dd {
    margin: 0;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-small);
  }
  .facts .sub {
    color: var(--text-dim);
    font-weight: normal;
  }
</style>
