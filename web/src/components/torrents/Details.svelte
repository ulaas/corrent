<script lang="ts">
  import Icon from '../Icon.svelte';
  import Progress from '../Progress.svelte';
  import Spinner from '../Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { events } from '../../lib/events.svelte';
  import { bytes, date, duration, num, percent, rate } from '../../lib/format';
  import { pieceBuckets, STATE_LABEL, stateColor } from '../../lib/torrentlist';
  import { toast } from '../../lib/toast.svelte';
  import { t, tk } from '../../lib/i18n.svelte';
  import type { FileEntry, Torrent, TorrentDetail } from '../../lib/types';
  import { contextMenu, type MenuItem } from '../../lib/contextmenu.svelte';
  import { points, speeds } from '../../lib/speedhistory.svelte';

  let {
    torrent,
    onclose,
    initialTab = 'general',
  }: { torrent: Torrent; onclose: () => void; initialTab?: 'general' | 'files' | 'peers' | 'trackers' | 'pieces' | 'speed' } = $props();

  // The Speed tab (as on the desktop): this torrent's or all torrents' last minutes.
  let scope = $state<'torrent' | 'all'>('torrent');
  const hist = $derived(scope === 'torrent' ? (speeds.per[torrent.hash] ?? []) : speeds.all);
  const peak = $derived(Math.max(1024, ...hist.map(([d, u]) => Math.max(d, u))));
  const CW = 600, CH = 160;

  const TABS = [
    { id: 'general', label: tk('General') },
    { id: 'files', label: tk('Files') },
    { id: 'peers', label: tk('Peers') },
    { id: 'trackers', label: tk('Trackers') },
    { id: 'pieces', label: tk('Pieces') },
    { id: 'speed', label: tk('Speed') },
  ] as const;
  // svelte-ignore state_referenced_locally
  let tab = $state<(typeof TABS)[number]['id']>(initialTab);
  let detail = $state<TorrentDetail | null>(null);
  let selectedFiles = $state<Set<number>>(new Set());
  let newTrackers = $state('');
  let canvas: HTMLCanvasElement | undefined = $state();

  $effect(() => {
    const hash = torrent.hash;
    detail = null;
    selectedFiles = new Set();
    api.get<TorrentDetail>(`/torrents/${hash}/detail`).then((d) => (detail = d)).catch(() => {});
    const off = events.on((e) => {
      if (e.t === 'torrent_detail' && e.detail.hash === hash) detail = e.detail;
    });
    const unsub = events.subscribe({ topic: 'torrent', arg: hash });
    return () => {
      off();
      unsub();
    };
  });

  // Pieces bar
  $effect(() => {
    if (!canvas || tab !== 'pieces' || !detail) return;
    const w = canvas.clientWidth, h = canvas.clientHeight;
    const dpr = devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    const ctx = canvas.getContext('2d')!;
    const css = getComputedStyle(canvas);
    const have = css.getPropertyValue('--accent').trim() || '#4f8cff';
    const miss = css.getPropertyValue('--bg3').trim() || '#262b36';
    const n = Math.max(1, Math.min(Math.round(w * dpr), detail.piece_count || 1));
    const b = pieceBuckets(detail.pieces, detail.piece_count, n);
    ctx.fillStyle = miss;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = have;
    const bw = canvas.width / n;
    for (let i = 0; i < n; i++) {
      if (b[i] <= 0) continue;
      ctx.globalAlpha = 0.35 + 0.65 * b[i];
      ctx.fillRect(i * bw, 0, Math.ceil(bw), canvas.height);
    }
    ctx.globalAlpha = 1;
  });

  const PRIO = [
    { v: 1, label: tk('High') },
    { v: 0, label: tk('Normal') },
    { v: -1, label: tk('Low') },
  ];

  async function setPriority(files: number[], p: number | 'skip' | 'download') {
    if (!detail) return;
    try {
      if (p === 'skip' || p === 'download') {
        const inc = new Set(detail.files.filter((f) => f.included).map((f) => f.index));
        for (const i of files) p === 'skip' ? inc.delete(i) : inc.add(i);
        if (!inc.size) {
          toast(t('At least one file must be downloaded.'), 'warning');
          return;
        }
        await api.post(`/torrents/${torrent.hash}/files`, { files: [...inc].sort((a, b) => a - b), only: true });
      } else {
        await api.post(`/torrents/${torrent.hash}/files`, { files, priority: p });
      }
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  // Right-click on a file (as on the desktop): Priority ▸, for the selected files when it is one of them.
  function fileMenu(f: FileEntry) {
    return (): MenuItem[] => {
      const files = selectedFiles.has(f.index) ? [...selectedFiles] : [f.index];
      const list = detail?.files.filter((x) => files.includes(x.index)) ?? [f];
      const same = list.every((x) => x.priority === list[0].priority);
      return [
        {
          kind: 'submenu',
          label: t('Priority'),
          icon: 'flag',
          items: PRIO.map((p): MenuItem => ({
            label: t(p.label),
            radio: true,
            checked: same && list[0].priority === p.v,
            onselect: () => void setPriority(files, p.v),
          })),
        },
        { kind: 'sep' },
        list.every((x) => x.included)
          ? { label: t("Don't download"), icon: 'minus-circle', onselect: () => void setPriority(files, 'skip') }
          : { label: t('Download'), icon: 'download-simple', onselect: () => void setPriority(files, 'download') },
      ];
    };
  }

  async function choosePrio(f: FileEntry, v: string) {
    if (v === 'skip') return setPriority([f.index], 'skip');
    if (!f.included) await setPriority([f.index], 'download');
    await setPriority([f.index], +v);
  }

  function fileState(f: FileEntry): string {
    return !f.included ? 'skip' : String(f.priority);
  }

  async function addTrackers(e: Event) {
    e.preventDefault();
    const list = newTrackers.split(/\s+/).filter(Boolean);
    if (!list.length) return;
    try {
      await api.post(`/torrents/${torrent.hash}/trackers`, { trackers: list });
      newTrackers = '';
      toast(t('Trackers added'), 'success');
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  const allFiles = $derived(detail?.files ?? []);
  const allSelected = $derived(allFiles.length > 0 && selectedFiles.size === allFiles.length);

  function copy(text: string) {
    void navigator.clipboard?.writeText(text).then(() => toast(t('Copied'), 'success'));
  }
</script>

<section class="details" aria-label={t('Torrent details')}>
  <header>
    <span class="dot" style:background={stateColor(torrent.state)}></span>
    <h3 class="ellipsis grow" title={torrent.name}>{torrent.name}</h3>
    <button class="btn ghost icon sm" aria-label={t('Close details')} onclick={onclose}><Icon name="x" /></button>
  </header>
  <div class="tabs" role="tablist" aria-label={t('Torrent details')}>
    {#each TABS as tb (tb.id)}
      <button role="tab" aria-selected={tab === tb.id} class:on={tab === tb.id} onclick={() => (tab = tb.id)}>
        {t(tb.label)}
        {#if tb.id === 'files' && torrent.file_count}<span class="badge">{torrent.file_count}</span>{/if}
        {#if tb.id === 'peers' && torrent.peers_live}<span class="badge">{torrent.peers_live}</span>{/if}
      </button>
    {/each}
  </div>
  <div class="body" role="tabpanel">
    {#if tab === 'general'}
      <div class="general">
        <div class="big">
          <Progress value={torrent.progress} color={stateColor(torrent.state)} height={8} glow label={t('Progress')} />
          <div class="row big-line">
            <strong class="num">{percent(torrent.progress)}</strong>
            <span class="muted">{bytes(torrent.done)} / {bytes(torrent.size)}</span>
            <span class="spacer"></span>
            <span class="badge" style:color={stateColor(torrent.state)}>{t(STATE_LABEL[torrent.state])}</span>
          </div>
          {#if torrent.error}<p class="form-error"><Icon name="warning-circle" size={16} />{torrent.error}</p>{/if}
        </div>
        <dl class="kv">
          <div><dt>{t('Download speed')}</dt><dd class="num">{rate(torrent.down_bps)}</dd></div>
          <div><dt>{t('Upload speed')}</dt><dd class="num">{rate(torrent.up_bps)}</dd></div>
          <div><dt>{t('Time left')}</dt><dd>{torrent.eta_secs != null ? duration(torrent.eta_secs) : '–'}</dd></div>
          <div><dt>{t('Ratio')}</dt><dd class="num">{num(torrent.ratio, 2)}</dd></div>
          <div><dt>{t('Downloaded')}</dt><dd class="num">{bytes(torrent.downloaded)}</dd></div>
          <div><dt>{t('Uploaded')}</dt><dd class="num">{bytes(torrent.uploaded)}</dd></div>
          <div><dt>{t('Peers')}</dt><dd class="num">{t('{live} connected ({seen} known)', { live: torrent.peers_live, seen: torrent.peers_seen })}</dd></div>
          <div><dt>{t('Seeding time')}</dt><dd>{torrent.seeding_secs ? duration(torrent.seeding_secs) : '–'}</dd></div>
          <div><dt>{t('Added')}</dt><dd>{date(torrent.added_at)}{torrent.added_by ? ` · ${torrent.added_by}` : ''}</dd></div>
          <div><dt>{t('Completed')}</dt><dd>{date(torrent.completed_at)}</dd></div>
          <div><dt>{t('Pieces')}</dt><dd class="num">{torrent.piece_count ? `${num(torrent.piece_count)} × ${bytes(torrent.piece_size)}` : '–'}</dd></div>
          <div><dt>{t('Limits')}</dt><dd class="num">↓ {torrent.down_limit_kib ? `${torrent.down_limit_kib} KiB/s` : '∞'} · ↑ {torrent.up_limit_kib ? `${torrent.up_limit_kib} KiB/s` : '∞'}</dd></div>
          {#if torrent.save_path}<div class="wide"><dt>{t('Save path')}</dt><dd class="mono ellipsis" title={torrent.content_path}>{torrent.save_path}</dd></div>{/if}
          <div class="wide"><dt>{t('Info hash')}</dt><dd class="mono"><button class="copy" onclick={() => copy(torrent.hash)} title={t('Copy')}>{torrent.hash}<Icon name="copy" size={14} /></button></dd></div>
          {#if torrent.comment}<div class="wide"><dt>{t('Comment')}</dt><dd>{torrent.comment}</dd></div>{/if}
          {#if torrent.created_by}<div><dt>{t('Created by')}</dt><dd>{torrent.created_by}</dd></div>{/if}
        </dl>
      </div>
    {:else if tab === 'files'}
      {#if !detail}
        <div class="center"><Spinner /></div>
      {:else if !detail.files.length}
        <p class="muted center">{t('The file list appears once the metadata has arrived.')}</p>
      {:else}
        {#if selectedFiles.size}
          <div class="filebar">
            <span class="small">{t('{n} selected', { n: selectedFiles.size })}</span>
            {#each PRIO as p (p.v)}<button class="btn sm" onclick={() => setPriority([...selectedFiles], p.v)}>{t(p.label)}</button>{/each}
            <button class="btn sm" onclick={() => setPriority([...selectedFiles], 'download')}>{t('Download')}</button>
            <button class="btn sm" onclick={() => setPriority([...selectedFiles], 'skip')}>{t("Don't download")}</button>
          </div>
        {/if}
        <table class="table">
          <thead><tr>
            <th style="width: 32px"><input type="checkbox" class="check" checked={allSelected} indeterminate={selectedFiles.size > 0 && !allSelected} aria-label={t('Select all files')} onchange={() => (selectedFiles = allSelected ? new Set() : new Set(allFiles.map((f) => f.index)))} /></th>
            <th>{t('Name')}</th><th class="r">{t('Size')}</th><th style="width: 160px">{t('Progress')}</th><th style="width: 150px">{t('Priority')}</th>
          </tr></thead>
          <tbody>
            {#each detail.files as f (f.index)}
              <tr class:skipped={!f.included} use:contextMenu={{ items: fileMenu(f), title: f.path[f.path.length - 1] }}>
                <td><input type="checkbox" class="check" checked={selectedFiles.has(f.index)} aria-label={f.path.join('/')} onchange={() => { const s = new Set(selectedFiles); s.has(f.index) ? s.delete(f.index) : s.add(f.index); selectedFiles = s; }} /></td>
                <td class="fname" title={f.path.join('/')}>{#if f.path.length > 1}<span class="faint">{f.path.slice(0, -1).join('/')}/</span>{/if}{f.path[f.path.length - 1]}</td>
                <td class="r num">{bytes(f.len)}</td>
                <td><div class="fp"><Progress value={f.len ? f.done / f.len : 0} height={4} /><span class="caption num">{percent(f.len ? f.done / f.len : 0, 0)}</span></div></td>
                <td>
                  <select class="select prio" value={fileState(f)} aria-label={t('Priority')} onchange={(e) => choosePrio(f, e.currentTarget.value)}>
                    {#each PRIO as p (p.v)}<option value={String(p.v)}>{t(p.label)}</option>{/each}
                    <option value="skip">{t("Don't download")}</option>
                  </select>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {:else if tab === 'speed'}
      <div class="speedhead">
        <div class="seg" role="radiogroup" aria-label={t('Speed')}>
          <button role="radio" aria-checked={scope === 'torrent'} class:on={scope === 'torrent'} onclick={() => (scope = 'torrent')}>{t('This torrent')}</button>
          <button role="radio" aria-checked={scope === 'all'} class:on={scope === 'all'} onclick={() => (scope = 'all')}>{t('All torrents')}</button>
        </div>
        {#if hist.length}
          {@const [d, u] = hist[hist.length - 1]}
          <span class="num down">↓ {rate(d)}</span><span class="num up">↑ {rate(u)}</span>
        {/if}
      </div>
      {#if hist.length < 3}
        <p class="muted center">{t('Collecting speed data…')}</p>
      {:else}
        <svg class="chart" viewBox="0 0 {CW} {CH}" preserveAspectRatio="none" role="img" aria-label={t('Speed')}>
          <line x1="0" x2={CW} y1={CH / 2} y2={CH / 2} class="grid" />
          <polyline class="down" points={points(hist.map((s) => s[0]), peak, CW, CH)} />
          <polyline class="up" points={points(hist.map((s) => s[1]), peak, CW, CH)} />
        </svg>
        <div class="axis caption muted"><span>{t('Download')} / {t('Upload')} · {rate(peak)}</span><span>{t('now')}</span></div>
      {/if}
    {:else if tab === 'peers'}
      {#if !detail}
        <div class="center"><Spinner /></div>
      {:else if !detail.peers.length}
        <p class="muted center">{t('No peers connected.')}</p>
      {:else}
        <table class="table">
          <thead><tr><th>{t('Address')}</th><th>{t('Client')}</th><th>{t('Type')}</th><th class="r">↓</th><th class="r">↑</th><th class="r">{t('Downloaded')}</th><th class="r">{t('Uploaded')}</th></tr></thead>
          <tbody>
            {#each detail.peers as p (p.addr)}
              <tr><td class="mono">{p.addr}</td><td class="ellipsis">{p.client ?? '–'}</td><td><span class="badge">{p.kind}</span></td><td class="r num">{rate(p.down_bps)}</td><td class="r num">{rate(p.up_bps)}</td><td class="r num">{bytes(p.downloaded)}</td><td class="r num">{bytes(p.uploaded)}</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {:else if tab === 'trackers'}
      {#if torrent.trackers.length}
        <ul class="trackers">{#each torrent.trackers as tr (tr)}<li class="mono ellipsis"><Icon name="broadcast" size={14} />{tr}</li>{/each}</ul>
      {:else}
        <p class="muted">{t('No trackers (or you may not see them).')}</p>
      {/if}
      <form class="addtr" onsubmit={addTrackers}>
        <textarea class="input mono" bind:value={newTrackers} rows="2" placeholder="udp://tracker.example:1337/announce" aria-label={t('Add trackers')}></textarea>
        <button class="btn" disabled={!newTrackers.trim()}><Icon name="plus" />{t('Add trackers')}</button>
      </form>
    {:else if tab === 'pieces'}
      {#if !detail}
        <div class="center"><Spinner /></div>
      {:else}
        <canvas bind:this={canvas} class="pieces" aria-label={t('{n} pieces, {p} done', { n: detail.piece_count, p: percent(torrent.progress) })}></canvas>
        <p class="caption muted">{t('{n} pieces of {size}', { n: num(detail.piece_count), size: bytes(torrent.piece_size) })}</p>
      {/if}
    {/if}
  </div>
</section>

<style>
  .details {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--bg1);
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 16px 6px;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex: none;
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
  }
  .tabs button {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 10px 12px;
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .tabs button:hover {
    color: var(--text);
  }
  .tabs button.on {
    color: var(--text);
  }
  .tabs button.on::after {
    content: '';
    position: absolute;
    left: 10px;
    right: 10px;
    bottom: -1px;
    height: 2px;
    border-radius: 2px;
    background: var(--accent);
  }
  .body {
    flex: 1;
    overflow: auto;
    padding: 14px 16px;
    min-height: 0;
  }
  .center {
    display: grid;
    place-items: center;
    padding: 30px 0;
  }
  .big {
    margin-bottom: 18px;
  }
  .big-line {
    margin-top: 8px;
  }
  .kv {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 12px 24px;
    margin: 0;
  }
  .kv .wide {
    grid-column: 1 / -1;
  }
  dt {
    font-size: 12px;
    color: var(--text-dim);
  }
  dd {
    margin: 2px 0 0;
    font-size: var(--fs-small);
    font-weight: 600;
    min-width: 0;
  }
  .copy {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    word-break: break-all;
    text-align: left;
  }
  .copy:hover {
    color: var(--accent);
  }
  .filebar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-bottom: 10px;
  }
  .fname {
    max-width: 420px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .skipped td {
    opacity: 0.5;
  }
  .fp {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .fp .caption {
    width: 36px;
    text-align: right;
  }
  .prio {
    height: 28px;
    font-size: 12px;
  }
  .trackers {
    list-style: none;
    padding: 0;
    margin: 0 0 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .trackers li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .addtr {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .addtr textarea {
    min-height: 40px;
    font-size: 12px;
  }
  .pieces {
    width: 100%;
    height: 48px;
    border-radius: var(--r-sm);
    display: block;
    margin-bottom: 8px;
  }
  .speedhead {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 10px;
    font-size: var(--fs-small);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden;
    margin-right: auto;
  }
  .seg button {
    padding: 4px 10px;
    font-size: var(--fs-small);
    color: var(--text-dim);
  }
  .seg button.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .speedhead .down,
  .chart .down {
    color: var(--accent);
    stroke: var(--accent);
  }
  .speedhead .up,
  .chart .up {
    color: var(--success);
    stroke: var(--success);
  }
  .chart {
    width: 100%;
    height: 160px;
    display: block;
  }
  .chart polyline {
    fill: none;
    stroke-width: 1.8;
    vector-effect: non-scaling-stroke;
  }
  .chart .grid {
    stroke: var(--border);
    stroke-dasharray: 4 4;
    vector-effect: non-scaling-stroke;
  }
  .axis {
    display: flex;
    justify-content: space-between;
    margin-top: 4px;
  }
</style>
