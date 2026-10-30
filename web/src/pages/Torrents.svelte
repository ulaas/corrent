<script lang="ts" module>
  import type { ListFilter, SortKey } from '../lib/torrentlist';
  // Kept while the app runs, so coming back restores the view.
  const view = $state<{ filter: ListFilter; sort: SortKey; desc: boolean }>({
    filter: { status: 'all', category: null, tag: null, text: '' },
    sort: 'queue',
    desc: false,
  });
</script>

<script lang="ts">
  import { fly } from 'svelte/transition';
  import Icon from '../components/Icon.svelte';
  import Menu from '../components/Menu.svelte';
  import Spinner from '../components/Spinner.svelte';
  import Progress from '../components/Progress.svelte';
  import AddTorrent from '../components/torrents/AddTorrent.svelte';
  import EditTorrent from '../components/torrents/EditTorrent.svelte';
  import LabelsManager from '../components/torrents/LabelsManager.svelte';
  import Details from '../components/torrents/Details.svelte';
  import FolderPicker from '../components/FolderPicker.svelte';
  import ColumnChooser from '../components/torrents/ColumnChooser.svelte';
  import { cell, col, fit, known, template } from '../lib/torrentcols';
  import { api, errorText } from '../lib/api';
  import { live, useTorrents } from '../lib/torrents.svelte';
  import { filterTorrents, sortTorrents, statusCounts, STATUS_FILTERS, STATE_LABEL, stateColor } from '../lib/torrentlist';
  import { bytes, duration, percent, rate } from '../lib/format';
  import { confirm } from '../lib/confirm.svelte';
  import { toast } from '../lib/toast.svelte';
  import { can, session } from '../lib/session.svelte';
  import { closeMenuFor, contextMenu, ctxmenu } from '../lib/contextmenu.svelte';
  import { isMedia, menuFor, type TorrentMenuActions } from '../lib/torrentmenu';
  import { ms, prefs } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import { Perm, type FileEntry, type Torrent, type TorrentDetail } from '../lib/types';
  import type { IconName } from '../lib/icons';

  $effect(() => useTorrents());

  let selected = $state<Set<string>>(new Set());
  let anchor = $state<string | null>(null);
  let focusHash = $state<string | null>(null);
  let detailHash = $state<string | null>(null);
  let adding = $state<{ text: string; files: File[] } | null>(null);
  let editing = $state<{ torrent: Torrent; focus: 'name' | 'speed' | 'seed' } | null>(null);
  let detailTab = $state<'general' | 'trackers'>('general');
  let labels = $state<false | { focus?: string; start?: string }>(false);
  let moving = $state<string[] | null>(null);
  let dropping = $state(false);
  let railOpen = $state(false);
  let choosing = $state(false);

  const rows = $derived(sortTorrents(filterTorrents(live.list, view.filter), view.sort, view.desc));
  const counts = $derived(statusCounts(live.list));
  const byHash = $derived(new Map(live.list.map((x) => [x.hash, x])));
  const detailTorrent = $derived(detailHash ? byHash.get(detailHash) : undefined);
  const sel = $derived([...selected].filter((h) => byHash.has(h)));
  const stats = $derived(live.stats);
  const canManage = $derived(can(Perm.TORRENTS_MANAGE_OWN) || can(Perm.TORRENTS_MANAGE_ALL));
  const canAdd = $derived(can(Perm.TORRENTS_ADD));
  const canAll = $derived(can(Perm.TORRENTS_MANAGE_ALL));

  const catCounts = $derived.by(() => {
    const m: Record<string, number> = {};
    for (const x of live.list) m[x.category ?? ''] = (m[x.category ?? ''] ?? 0) + 1;
    return m;
  });
  const tagCounts = $derived.by(() => {
    const m: Record<string, number> = {};
    for (const x of live.list) for (const g of x.tags) m[g] = (m[g] ?? 0) + 1;
    return m;
  });

  const STATUS_ICON: Record<string, IconName> = {
    all: 'rows',
    downloading: 'arrow-circle-down',
    seeding: 'arrow-circle-up',
    completed: 'check-circle',
    paused: 'pause-circle',
    active: 'sparkle',
    inactive: 'hourglass-medium',
    errored: 'warning-circle',
  };

  // ---- virtual list ----
  let scroller: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewH = $state(600);
  let listW = $state(1200);
  let phone = $state(false);
  const ROW = $derived(phone ? 78 : 56);
  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW) - 6));
  const last = $derived(Math.min(rows.length, Math.ceil((scrollTop + viewH) / ROW) + 6));
  const visible = $derived(rows.slice(first, last));
  // The chosen columns that fit (phones show a summary line under the name instead).
  const cols = $derived(phone ? [] : fit(known(prefs.torrentColumns), listW));
  const grid = $derived(phone ? '36px 1fr' : template(cols));

  $effect(() => {
    if (!scroller) return;
    const ro = new ResizeObserver(() => {
      viewH = scroller!.clientHeight;
      listW = scroller!.clientWidth;
      phone = innerWidth < 760;
    });
    ro.observe(scroller);
    return () => ro.disconnect();
  });

  // ---- selection ----
  function click(e: MouseEvent, x: Torrent) {
    const s = new Set(selected);
    if (e.shiftKey && anchor) {
      const a = rows.findIndex((r) => r.hash === anchor), b = rows.findIndex((r) => r.hash === x.hash);
      if (!(e.ctrlKey || e.metaKey)) s.clear();
      for (let i = Math.min(a, b); i <= Math.max(a, b); i++) s.add(rows[i].hash);
    } else if (e.ctrlKey || e.metaKey) {
      s.has(x.hash) ? s.delete(x.hash) : s.add(x.hash);
      anchor = x.hash;
    } else {
      s.clear();
      s.add(x.hash);
      anchor = x.hash;
      detailHash = x.hash;
    }
    focusHash = x.hash;
    selected = s;
  }

  function toggleOne(x: Torrent) {
    const s = new Set(selected);
    s.has(x.hash) ? s.delete(x.hash) : s.add(x.hash);
    selected = s;
    anchor = x.hash;
  }

  const allSel = $derived(rows.length > 0 && rows.every((r) => selected.has(r.hash)));
  function toggleAll() {
    selected = allSel ? new Set() : new Set(rows.map((r) => r.hash));
  }

  function onListKey(e: KeyboardEvent) {
    if (!rows.length) return;
    const i = Math.max(0, rows.findIndex((r) => r.hash === focusHash));
    let j = i;
    if (e.key === 'ArrowDown') j = Math.min(rows.length - 1, i + 1);
    else if (e.key === 'ArrowUp') j = Math.max(0, i - 1);
    else if (e.key === 'Home') j = 0;
    else if (e.key === 'End') j = rows.length - 1;
    else if (e.key === 'Enter') {
      if (focusHash) detailHash = focusHash;
      return;
    } else if (e.key === 'Delete' && sel.length) {
      void remove(sel, e.shiftKey);
      return;
    } else if (e.key === 'a' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      selected = new Set(rows.map((r) => r.hash));
      return;
    } else if (e.key === ' ' && (e.ctrlKey || e.metaKey) && focusHash) {
      e.preventDefault();
      toggleOne(byHash.get(focusHash)!);
      return;
    } else if (e.key === ' ' && sel.length && canManage) {
      // As on the desktop: pause what runs, else resume.
      e.preventDefault();
      const running = sel.some((h) => {
        const st = byHash.get(h)?.state;
        return st !== 'paused' && st !== 'queued' && st !== 'completed';
      });
      void act(running ? 'pause' : 'resume');
      return;
    } else if (e.key === 'F2' && sel.length === 1 && canManage) {
      e.preventDefault();
      const x = byHash.get(sel[0]);
      if (x) editing = { torrent: x, focus: 'name' };
      return;
    } else if (e.key === 'Escape' && sel.length) {
      e.preventDefault();
      selected = new Set();
      return;
    } else if (e.key === 'i' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      detailHash = detailHash ? null : (focusHash ?? sel[0] ?? null);
      return;
    } else return;
    e.preventDefault();
    focusHash = rows[j].hash;
    if (!e.shiftKey) {
      selected = new Set([rows[j].hash]);
      anchor = rows[j].hash;
    } else {
      const s = new Set(selected);
      s.add(rows[j].hash);
      selected = s;
    }
    const top = j * ROW;
    if (scroller) {
      if (top < scroller.scrollTop) scroller.scrollTop = top;
      else if (top + ROW > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = top + ROW - scroller.clientHeight;
    }
  }

  // ---- actions ----
  async function act(action: string, hashes = sel, done?: string) {
    if (!hashes.length && !action.endsWith('_all')) return;
    try {
      await api.post('/torrents/actions', { action, hashes });
      if (done) toast(done, 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function remove(hashes = sel, withData = false) {
    if (!hashes.length) return;
    const one = hashes.length === 1 ? byHash.get(hashes[0]) : null;
    const r = await confirm({
      title: one ? t('Remove “{name}”?', { name: one.name }) : t('Remove {n} torrents?', { n: hashes.length }),
      text: t('The torrents are removed from the list. Downloaded files are kept unless you choose otherwise.'),
      ok: t('Remove'),
      danger: true,
      check: t('Also delete the downloaded files'),
      checked: withData,
    });
    if (!r.ok) return;
    await act(r.checked ? 'remove_with_data' : 'remove', hashes);
    selected = new Set();
    if (detailHash && hashes.includes(detailHash)) detailHash = null;
  }

  async function move(to: string) {
    const hashes = moving ?? [];
    moving = null;
    try {
      await api.post('/torrents/move', { hashes, to });
      toast(hashes.length === 1 ? t('Moving 1 torrent…') : t('Moving {n} torrents…', { n: hashes.length }), 'info');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function altSpeed() {
    try {
      await api.post('/server/alt-speed', { on: !stats?.alt_speed });
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  // ---- context menu (as on the desktop, src/app/actions.rs) ----
  async function patchEach(list: Torrent[], body: (x: Torrent) => Record<string, unknown>) {
    try {
      await Promise.all(list.map((x) => api.patch(`/torrents/${x.hash}`, body(x))));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  const menuActions: TorrentMenuActions = {
    act: (action, hashes) => void act(action, hashes, action === 'recheck' ? t('Checking…') : undefined),
    move: (hashes) => (moving = hashes),
    media: async (hash) => {
      const d = await api.get<TorrentDetail>(`/torrents/${hash}/detail`);
      return d.files.filter((f) => f.included && isMedia(f.path[f.path.length - 1] ?? '')).sort((a, b) => b.len - a.len);
    },
    play: (hash: string, f: FileEntry) => void open(`/api/v1/torrents/${hash}/files/${f.index}/stream`, '_blank', 'noopener'),
    rename: (x) => (editing = { torrent: x, focus: 'name' }),
    limits: (x, section) => (editing = { torrent: x, focus: section }),
    setCategory: (list, category) => void patchEach(list, () => ({ category: category ?? '' })),
    setTag: (list, tag, on) => void patchEach(list, (x) => ({ tags: on ? [...new Set([...x.tags, tag])] : x.tags.filter((g) => g !== tag) })),
    clearTags: (list) => void patchEach(list, () => ({ tags: [] })),
    labels: () => (labels = {}),
    trackers: (x) => {
      detailTab = 'trackers';
      detailHash = x.hash;
    },
    copy: (text) => void navigator.clipboard?.writeText(text).then(() => toast(t('Copied'), 'success')),
    remove: (hashes, withData) => void remove(hashes, withData),
  };

  /** The row under the event (or the focused one, for the keyboard). */
  function rowOf(e: Event): Torrent | undefined {
    const h = (e.target as HTMLElement | null)?.closest<HTMLElement>('[data-hash]')?.dataset.hash ?? (e instanceof KeyboardEvent ? focusHash : null);
    return h ? byHash.get(h) : undefined;
  }
  function listMenu(e: Event) {
    const x = rowOf(e);
    if (!x) return null;
    const list = selected.has(x.hash) ? sel.map((h) => byHash.get(h)).filter((y): y is Torrent => !!y) : [x];
    return menuFor(
      list,
      {
        me: session.user?.name ?? null,
        manageAll: canAll,
        manageOwn: can(Perm.TORRENTS_MANAGE_OWN),
        categories: Object.keys(live.labels.categories),
        tags: live.labels.tags,
      },
      menuActions,
    );
  }
  // Right-clicking a row that isn't selected selects only it (as on the desktop).
  function selectFor(e: Event) {
    const x = rowOf(e);
    if (!x || selected.has(x.hash)) return;
    selected = new Set([x.hash]);
    anchor = x.hash;
    focusHash = x.hash;
  }
  // A menu about torrents that are gone closes.
  $effect(() => {
    const k = ctxmenu.current?.key;
    if (k && !byHash.has(k)) closeMenuFor(k);
  });

  // ---- sidebar menus (as on the desktop, src/app/sidebar.rs) ----
  async function removeCategory(c: string) {
    const keep = Object.fromEntries(Object.entries(live.labels.categories).filter(([k]) => k !== c && !k.startsWith(`${c}/`)));
    try {
      await api.put('/torrents/categories', keep);
      if (view.filter.category === c) view.filter.category = null;
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  async function removeTag(g: string) {
    try {
      await api.put('/torrents/tags', live.labels.tags.filter((x) => x !== g));
      if (view.filter.tag === g) view.filter.tag = null;
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  const categoryMenu = (c: string) => () =>
    canAll
      ? [
          { label: t('Edit…'), icon: 'pencil-simple' as const, onselect: () => (labels = { focus: c }) },
          { label: t('Add subcategory…'), icon: 'folder-plus' as const, onselect: () => (labels = { start: `${c}/` }) },
          { label: t('Remove category'), icon: 'trash' as const, onselect: () => void removeCategory(c) },
        ]
      : null;
  const tagMenu = (g: string) => () => (canAll ? [{ label: t('Remove tag'), icon: 'trash' as const, onselect: () => void removeTag(g) }] : null);

  function sortBy(k: SortKey) {
    if (view.sort === k) view.desc = !view.desc;
    else {
      view.sort = k;
      view.desc = !['name', 'queue', 'eta', 'category', 'tags', 'save_path', 'tracker'].includes(k);
    }
  }

  function ondrop(e: DragEvent) {
    e.preventDefault();
    dropping = false;
    const files = [...(e.dataTransfer?.files ?? [])].filter((f) => f.name.endsWith('.torrent'));
    const text = e.dataTransfer?.getData('text/plain') ?? '';
    if (files.length || /^magnet:|^https?:/.test(text)) adding = { text: files.length ? '' : text, files };
  }

  let filterInput: HTMLInputElement | undefined = $state();
  function onPageKey(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    if (e.shiftKey && e.key.toLowerCase() === 's' && (canAll || can(Perm.SETTINGS_WRITE))) {
      e.preventDefault();
      void altSpeed();
    } else if (!e.shiftKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      filterInput?.focus();
      filterInput?.select();
    }
  }

  function onpaste(e: ClipboardEvent) {
    if ((e.target as HTMLElement).closest('input, textarea') || adding || !canAdd) return;
    const text = e.clipboardData?.getData('text') ?? '';
    if (/^magnet:\?/i.test(text.trim())) adding = { text: text.trim(), files: [] };
  }
</script>

<svelte:window {onpaste} onkeydown={onPageKey} />

<div
  class="tpage"
  class:with-details={!!detailTorrent}
  role="region"
  aria-label={t('Torrents')}
  ondragover={(e) => { if (canAdd && e.dataTransfer?.types.includes('Files')) { e.preventDefault(); dropping = true; } }}
  ondragleave={(e) => { if (e.target === e.currentTarget) dropping = false; }}
  {ondrop}>
  <aside class="rail" class:open={railOpen} aria-label={t('Filters')}>
    <div class="rail-group">
      {#each STATUS_FILTERS as s (s.id)}
        <button class="ritem" class:on={view.filter.status === s.id} aria-pressed={view.filter.status === s.id} onclick={() => ((view.filter.status = s.id), (railOpen = false))}>
          <Icon name={STATUS_ICON[s.id]} size={17} /><span class="grow">{t(s.label)}</span><span class="cnt num">{counts[s.id]}</span>
        </button>
      {/each}
    </div>
    <div class="rail-title">{t('Categories')}</div>
    <div class="rail-group">
      <button class="ritem" class:on={view.filter.category === null} onclick={() => (view.filter.category = null)}><Icon name="tag" size={17} /><span class="grow">{t('All')}</span></button>
      <button class="ritem" class:on={view.filter.category === ''} onclick={() => (view.filter.category = '')}><Icon name="minus" size={17} /><span class="grow">{t('Uncategorized')}</span><span class="cnt num">{catCounts[''] ?? 0}</span></button>
      {#each Object.keys(live.labels.categories) as c (c)}
        <button class="ritem" class:on={view.filter.category === c} onclick={() => (view.filter.category = c)} use:contextMenu={{ items: categoryMenu(c), title: c }}><Icon name="folder" size={17} /><span class="grow ellipsis">{c}</span><span class="cnt num">{catCounts[c] ?? 0}</span></button>
      {/each}
    </div>
    {#if live.labels.tags.length}
      <div class="rail-title">{t('Tags')}</div>
      <div class="rail-group">
        <button class="ritem" class:on={view.filter.tag === null} onclick={() => (view.filter.tag = null)}><Icon name="tag" size={17} /><span class="grow">{t('All')}</span></button>
        {#each live.labels.tags as g (g)}
          <button class="ritem" class:on={view.filter.tag === g} onclick={() => (view.filter.tag = g)} use:contextMenu={{ items: tagMenu(g), title: g }}><Icon name="bookmark-simple" size={17} /><span class="grow ellipsis">{g}</span><span class="cnt num">{tagCounts[g] ?? 0}</span></button>
        {/each}
      </div>
    {/if}
    {#if canAll}
      <button class="btn ghost sm manage" onclick={() => (labels = {})}><Icon name="pencil-simple" size={14} />{t('Categories and tags')}</button>
    {/if}
  </aside>

  <section class="main">
    <div class="toolbar">
      <button class="btn icon rail-btn" aria-label={t('Filters')} onclick={() => (railOpen = !railOpen)}><Icon name="funnel" /></button>
      {#if canAdd}
        <button class="btn primary" onclick={() => (adding = { text: '', files: [] })}><Icon name="plus" />{t('Add')}</button>
      {/if}
      <div class="search">
        <Icon name="magnifying-glass" size={16} />
        <input bind:this={filterInput} bind:value={view.filter.text} placeholder={t('Filter torrents')} aria-label={t('Filter torrents')} />
      </div>
      {#if sel.length && canManage}
        <div class="selbar" transition:fly={{ x: -10, duration: ms(160) }}>
          <span class="small muted num">{t('{n} selected', { n: sel.length })}</span>
          <button class="btn icon" aria-label={t('Resume')} title={t('Resume')} onclick={() => act('resume')}><Icon name="play" /></button>
          <button class="btn icon" aria-label={t('Pause')} title={t('Pause')} onclick={() => act('pause')}><Icon name="pause" /></button>
          <button class="btn icon danger" aria-label={t('Remove')} title={t('Remove')} onclick={() => remove()}><Icon name="trash" /></button>
          <Menu label={t('More actions')} triggerClass="btn icon" width={230}>
            {#snippet trigger()}<Icon name="dots-three-vertical" />{/snippet}
            {#snippet children(close)}
              <button class="mi" role="menuitem" onclick={() => { close(); void act('force_start'); }}><Icon name="play-fill" />{t('Force start')}</button>
              <button class="mi" role="menuitem" onclick={() => { close(); void act('recheck', sel, t('Checking…')); }}><Icon name="check-square" />{t('Verify files')}</button>
              <button class="mi" role="menuitem" onclick={() => { close(); moving = sel; }}><Icon name="folder-open" />{t('Move to…')}</button>
              {#if sel.length === 1}
                <button class="mi" role="menuitem" onclick={() => { close(); const x = byHash.get(sel[0]); editing = x ? { torrent: x, focus: 'name' } : null; }}><Icon name="pencil-simple" />{t('Edit…')}</button>
              {/if}
              <div class="sep"></div>
              <div class="mh">{t('Queue')}</div>
              <button class="mi" role="menuitem" onclick={() => { close(); void act('queue_top'); }}><Icon name="arrow-line-up" />{t('Move to top')}</button>
              <button class="mi" role="menuitem" onclick={() => { close(); void act('queue_up'); }}><Icon name="arrow-up" />{t('Move up')}</button>
              <button class="mi" role="menuitem" onclick={() => { close(); void act('queue_down'); }}><Icon name="arrow-down" />{t('Move down')}</button>
              <button class="mi" role="menuitem" onclick={() => { close(); void act('queue_bottom'); }}><Icon name="arrow-line-down" />{t('Move to bottom')}</button>
            {/snippet}
          </Menu>
        </div>
      {/if}
      <span class="spacer"></span>
      {#if canAll}
        <Menu label={t('All torrents')} triggerClass="btn icon" width={220}>
          {#snippet trigger()}<Icon name="dots-three" />{/snippet}
          {#snippet children(close)}
            <button class="mi" role="menuitem" onclick={() => { close(); void act('resume_all', []); }}><Icon name="play" />{t('Resume all')}</button>
            <button class="mi" role="menuitem" onclick={() => { close(); void act('pause_all', []); }}><Icon name="pause" />{t('Pause all')}</button>
            <div class="sep"></div>
            <button class="mi" role="menuitem" onclick={() => { close(); labels = {}; }}><Icon name="tag" />{t('Categories and tags…')}</button>
          {/snippet}
        </Menu>
      {/if}
    </div>

    <!-- svelte-ignore a11y_interactive_supports_focus (right-click opens the column chooser, as on the desktop) -->
    <div class="list-head" role="row" style:grid-template-columns={grid} oncontextmenu={(e) => { e.preventDefault(); choosing = true; }}>
      <span class="c-check"><input type="checkbox" class="check" checked={allSel} indeterminate={sel.length > 0 && !allSel} onchange={toggleAll} aria-label={t('Select all')} /></span>
      <button class="c-name sortable" class:on={view.sort === 'name' || view.sort === 'queue'} onclick={() => sortBy(view.sort === 'name' ? 'name' : view.sort === 'queue' ? 'name' : 'queue')}>
        {view.sort === 'queue' ? t('Queue') : t('Name')}
        {#if view.sort === 'name' || view.sort === 'queue'}<Icon name={view.desc ? 'caret-down' : 'caret-up'} size={12} />{/if}
      </button>
      {#each cols as id (id)}
        <button class="cell sortable" class:right={col(id).right} class:on={view.sort === id} onclick={() => sortBy(id)}>
          {t(col(id).label)}{#if view.sort === id}<Icon name={view.desc ? 'caret-down' : 'caret-up'} size={12} />{/if}
        </button>
      {/each}
    </div>

    <div
      class="list"
      bind:this={scroller}
      onscroll={() => (scrollTop = scroller!.scrollTop)}
      role="grid"
      tabindex="0"
      aria-label={t('Torrents')}
      aria-rowcount={rows.length}
      aria-multiselectable="true"
      use:contextMenu={{ items: listMenu, before: selectFor, key: sel[0], title: t('Torrents') }}
      onkeydown={onListKey}>
      {#if !live.loaded}
        <div class="center"><Spinner size={26} /></div>
      {:else if !rows.length}
        <div class="empty">
          <div class="empty-icon"><Icon name="download-simple" size={30} /></div>
          {#if live.list.length}
            <h3>{t('No torrents match the filter.')}</h3>
            <button class="btn" onclick={() => (view.filter = { status: 'all', category: null, tag: null, text: '' })}>{t('Clear filters')}</button>
          {:else}
            <h3>{t('No torrents yet')}</h3>
            <p>{t('Add a magnet link or a .torrent file — or drop one anywhere on this page.')}</p>
            {#if canAdd}<button class="btn primary" onclick={() => (adding = { text: '', files: [] })}><Icon name="plus" />{t('Add torrents')}</button>{/if}
          {/if}
        </div>
      {:else}
        <div class="spacer-h" style:height="{rows.length * ROW}px">
          {#each visible as x, i (x.hash)}
            {@const idx = first + i}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus (keys are handled by the grid) -->
            <div
              class="trow"
              data-hash={x.hash}
              class:sel={selected.has(x.hash)}
              class:focus={focusHash === x.hash}
              class:detail={detailHash === x.hash}
              style:transform="translateY({idx * ROW}px)"
              style:height="{ROW}px"
              style:grid-template-columns={grid}
              role="row"
              aria-rowindex={idx + 1}
              aria-selected={selected.has(x.hash)}
              onclick={(e) => click(e, x)}
              ondblclick={() => (detailHash = x.hash)}>
              <span class="c-check" role="gridcell">
                <input type="checkbox" class="check" checked={selected.has(x.hash)} aria-label={x.name} tabindex="-1" onclick={(e) => e.stopPropagation()} onchange={() => toggleOne(x)} />
              </span>
              <span class="c-name" role="gridcell">
                <span class="nm">
                  <span class="sdot" style:background={stateColor(x.state)} title={t(STATE_LABEL[x.state])}></span>
                  <span class="ellipsis tname" title={x.name}>{x.name}</span>
                  <span class="labels">
                    {#if x.category}<span class="badge accent">{x.category}</span>{/if}
                    {#each x.tags.slice(0, 2) as g (g)}<span class="badge">{g}</span>{/each}
                  </span>
                </span>
                <span class="pb">
                  <Progress value={x.progress} color={stateColor(x.state)} height={4} />
                  <span class="pct num">{percent(x.progress)}</span>
                </span>
                <span class="mobile-line caption muted num">
                  {t(STATE_LABEL[x.state])} · {bytes(x.size)}
                  {#if x.down_bps}· ↓ {rate(x.down_bps)}{/if}
                  {#if x.up_bps}· ↑ {rate(x.up_bps)}{/if}
                  {#if x.eta_secs != null && x.state === 'downloading'}· {duration(x.eta_secs)}{/if}
                </span>
              </span>
              {#each cols as id (id)}
                {#if id === 'state'}
                  <span class="cell state" role="gridcell" style:color={stateColor(x.state)}>{x.error ? t('Error') : t(STATE_LABEL[x.state])}</span>
                {:else if id === 'peers'}
                  <span class="cell right num" role="gridcell">{x.peers_live}<span class="faint">/{x.peers_seen}</span></span>
                {:else}
                  {@const text = cell(x, id)}
                  <span class="cell num" class:right={col(id).right} class:live={id === 'down' && x.down_bps > 0} class:live-up={id === 'up' && x.up_bps > 0} role="gridcell" title={id === 'save_path' || id === 'tags' || id === 'tracker' ? text : undefined}>{text}</span>
                {/if}
              {/each}
            </div>
          {/each}
        </div>
      {/if}
    </div>

    {#if detailTorrent}
      <div class="dpanel" transition:fly={{ y: 40, duration: ms(220) }}>
        {#key detailTorrent.hash + detailTab}
          <Details torrent={detailTorrent} initialTab={detailTab} onclose={() => ((detailHash = null), (detailTab = 'general'))} />
        {/key}
      </div>
    {/if}

    <footer class="status" aria-label={t('Session')}>
      {#if stats}
        <span class="st"><Icon name="arrow-circle-down" size={15} /><span class="num">{t('{size}/s', { size: bytes(stats.down_bps) })}</span></span>
        <span class="st"><Icon name="arrow-circle-up" size={15} /><span class="num">{t('{size}/s', { size: bytes(stats.up_bps) })}</span></span>
        <span class="st hide-sm"><Icon name="users" size={15} /><span class="num">{stats.peers_live}</span></span>
        {#if stats.dht_nodes != null}<span class="st hide-sm" title={t('DHT nodes')}><Icon name="globe" size={15} /><span class="num">{stats.dht_nodes}</span></span>{/if}
        {#if stats.free_space != null}<span class="st hide-sm" title={t('Free space')}><Icon name="hard-drives" size={15} />{t('{size} free', { size: bytes(stats.free_space) })}</span>{/if}
        <span class="st hide-sm muted">{t('Session: ↓ {down} ↑ {up}', { down: bytes(stats.session_downloaded), up: bytes(stats.session_uploaded) })}</span>
        {#if stats.engine.state !== 'running'}
          <span class="badge {stats.engine.state === 'failed' ? 'err' : 'warn'}">{stats.engine.state === 'failed' ? t('Engine stopped') : t('Engine starting…')}</span>
        {/if}
        {#if stats.warning}<span class="badge warn" title={stats.warning}><Icon name="warning" size={12} />{stats.warning}</span>{/if}
      {/if}
      <span class="spacer"></span>
      {#if canAll || can(Perm.SETTINGS_WRITE)}
        <button class="altbtn" class:on={stats?.alt_speed} aria-pressed={!!stats?.alt_speed} onclick={altSpeed} title={t('Alternative speed limits')}>
          <Icon name="speedometer" size={16} />{t('Alt speed')}
        </button>
      {/if}
      <span class="caption muted num">{rows.length === live.list.length ? (live.list.length === 1 ? t('1 torrent') : t('{n} torrents', { n: live.list.length })) : t('{shown} of {n} torrents', { shown: rows.length, n: live.list.length })}</span>
    </footer>
  </section>

  {#if dropping}
    <div class="dropzone" transition:fly={{ y: 10, duration: ms(150) }}><Icon name="upload-simple" size={40} /><strong>{t('Drop .torrent files to add them')}</strong></div>
  {/if}
</div>

{#if adding}<AddTorrent initialText={adding.text} initialFiles={adding.files} onclose={() => (adding = null)} />{/if}
{#if editing}<EditTorrent torrent={editing.torrent} focus={editing.focus} onclose={() => (editing = null)} />{/if}
{#if choosing}<ColumnChooser onclose={() => (choosing = false)} />{/if}
{#if labels}<LabelsManager focus={labels.focus} start={labels.start} onclose={() => (labels = false)} />{/if}
{#if moving}
  <FolderPicker title={moving.length === 1 ? t('Move 1 torrent to…') : t('Move {n} torrents to…', { n: moving.length })} start={byHash.get(moving[0])?.save_path ?? ''} onpick={move} onclose={() => (moving = null)} />
{/if}

<style>
  .tpage {
    position: relative;
    display: grid;
    grid-template-columns: 228px 1fr;
    height: calc(100dvh - var(--topbar-h) - var(--player-h));
    min-height: 420px;
  }
  .rail {
    border-right: 1px solid var(--border);
    padding: 16px 10px;
    overflow-y: auto;
    background: var(--bg1);
  }
  .rail-title {
    margin: 18px 10px 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-mute);
  }
  .ritem {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border-radius: var(--r-md);
    color: var(--text-dim);
    font-size: var(--fs-small);
    font-weight: 600;
    text-align: left;
    transition: background var(--dur-micro), color var(--dur-micro);
  }
  .ritem:hover {
    background: var(--hover);
    color: var(--text);
  }
  .ritem.on {
    background: var(--accent-soft);
    color: var(--text);
  }
  .ritem.on :global(.icon) {
    color: var(--accent);
  }
  .cnt {
    font-size: 11px;
    color: var(--text-mute);
  }
  .manage {
    margin: 16px 6px 0;
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    position: relative;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .rail-btn {
    display: none;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--control-h);
    padding: 0 12px;
    border-radius: var(--r-md);
    border: 1px solid var(--border);
    background: var(--bg0);
    color: var(--text-dim);
    width: min(280px, 100%);
  }
  .search:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    outline: none;
    color: var(--text);
  }
  .selbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-left: 8px;
    margin-left: 4px;
    border-left: 1px solid var(--border);
  }
  .list-head,
  .trow {
    display: grid;
    align-items: center;
    gap: 0 8px;
    padding: 0 16px 0 8px;
  }
  .list-head {
    height: 34px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
    background: var(--bg1);
  }
  .sortable {
    display: inline-flex;
    align-items: center;
    justify-content: flex-start;
    gap: 4px;
    font: inherit;
    color: inherit;
    text-align: left;
    white-space: nowrap;
  }
  .sortable:hover,
  .sortable.on {
    color: var(--text);
  }
  .c-check {
    display: grid;
    place-items: center;
  }
  .tname {
    flex: 0 1 auto;
  }
  .labels {
    display: flex;
    gap: 4px;
    flex: 0 100 auto;
    min-width: 0;
    max-width: 40%;
    overflow: hidden;
  }
  .cell {
    min-width: 0;
    font-size: var(--fs-small);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cell.right {
    text-align: right;
    justify-content: flex-end;
  }
  .cell.state {
    font-weight: 600;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    min-height: 0;
    position: relative;
    outline: none;
  }
  .list:focus-visible {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
  .spacer-h {
    position: relative;
  }
  .trow {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    cursor: default;
    user-select: none;
    transition: background var(--dur-micro);
  }
  .trow:hover {
    background: var(--hover);
  }
  .trow.sel {
    background: var(--accent-softer);
  }
  .trow.detail {
    background: var(--accent-soft);
  }
  .list:focus-visible .trow.focus {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .trow .c-name {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .nm {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .sdot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .pb {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-left: 16px;
  }
  .pct {
    font-size: 11px;
    color: var(--text-dim);
    width: 46px;
    text-align: right;
    flex: none;
  }
  .mobile-line {
    display: none;
  }
  .live {
    color: var(--accent);
  }
  .live-up {
    color: var(--success);
  }
  .center {
    display: grid;
    place-items: center;
    padding: 80px 0;
  }
  .dpanel {
    height: 44%;
    min-height: 240px;
    border-top: 1px solid var(--border);
    box-shadow: 0 -10px 30px rgba(0, 0, 0, 0.12);
    flex: none;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 16px;
    height: 36px;
    padding: 0 16px;
    border-top: 1px solid var(--border);
    background: var(--bg1);
    font-size: 12px;
    flex: none;
    overflow: hidden;
    white-space: nowrap;
  }
  .st {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .altbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border-radius: var(--r-pill);
    border: 1px solid var(--border);
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }
  .altbtn.on {
    background: color-mix(in srgb, var(--warn) 18%, transparent);
    border-color: color-mix(in srgb, var(--warn) 45%, transparent);
    color: var(--warn);
  }
  .dropzone {
    position: absolute;
    inset: 12px;
    border: 2px dashed var(--accent);
    border-radius: var(--r-xl);
    background: color-mix(in srgb, var(--bg0) 80%, transparent);
    backdrop-filter: blur(4px);
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 12px;
    color: var(--accent);
    z-index: 20;
    pointer-events: none;
  }

  @media (max-width: 1000px) {
    .tpage {
      grid-template-columns: 1fr;
    }
    .rail {
      position: absolute;
      inset: 0 auto 0 0;
      width: 260px;
      z-index: 15;
      transform: translateX(-100%);
      transition: transform var(--dur-page) var(--ease-standard);
      box-shadow: var(--shadow-4);
    }
    .rail.open {
      transform: none;
    }
    .rail-btn {
      display: inline-flex;
    }
  }
  @media (max-width: 760px) {
    .tpage {
      height: calc(100dvh - var(--topbar-h) - var(--player-h) - var(--bottom-nav, 0px));
      height: calc(100dvh - var(--topbar-h) - max(var(--player-h), var(--bottom-nav, 0px)));
    }
    .list-head {
      display: none;
    }
    .list-head,
    .trow {
      padding: 0 12px 0 4px;
    }
    .pb {
      padding-left: 0;
    }
    .mobile-line {
      display: block;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .search {
      flex: 1;
      width: auto;
    }
    .selbar {
      order: 10;
      width: 100%;
      border-left: 0;
      padding-left: 0;
      margin-left: 0;
    }
    .dpanel {
      position: fixed;
      inset: 30% 0 0 0;
      height: auto;
      z-index: 45;
      border-radius: var(--r-xl) var(--r-xl) 0 0;
      overflow: hidden;
      box-shadow: var(--shadow-4);
    }
    .hide-sm {
      display: none;
    }
  }
</style>
