<script lang="ts">
  // One level of a context menu (submenus are nested MenuLists): placed on screen, scrolling
  // when long, with keyboard navigation (↑↓ Home End, → opens a submenu, ← and Escape close a
  // level, Enter/Space, first letter jumps).
  import { onMount, tick } from 'svelte';
  import { scale } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import Spinner from './Spinner.svelte';
  import MenuList from './MenuList.svelte';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import { jump, place, placeSub, step } from '../lib/menuplace';
  import type { MenuItem } from '../lib/contextmenu.svelte';

  let {
    items,
    at,
    row = null,
    onchoose,
    onback,
    label = '',
    focusFirst = false,
  }: {
    items: MenuItem[];
    /** Where the pointer was (the first level). */
    at?: { x: number; y: number };
    /** The row this submenu belongs to. */
    row?: DOMRect | null;
    /** A row was chosen (closes every level). */
    onchoose: (run: () => void) => void;
    /** Escape / ← on this level. */
    onback: (byKey: boolean) => void;
    label?: string;
    focusFirst?: boolean;
  } = $props();

  let el: HTMLDivElement | undefined = $state();
  let pos = $state({ left: -9999, top: -9999 });
  let active = $state(-1);
  let sub = $state<{ index: number; items: MenuItem[] | null; row: DOMRect; byKey: boolean } | null>(null);
  let hoverTimer: ReturnType<typeof setTimeout> | null = null;

  const enabled = $derived(items.map((it) => (it.kind === undefined || it.kind === 'item' || it.kind === 'submenu') && !it.disabled));
  const labels = $derived(items.map((it) => ('label' in it && it.kind !== 'header' ? it.label : null)));

  onMount(() => {
    if (!el) return;
    const r = el.getBoundingClientRect();
    pos = row ? placeSub(row, r.width, r.height, innerWidth, innerHeight) : place(at?.x ?? 0, at?.y ?? 0, r.width, r.height, innerWidth, innerHeight);
    el.focus({ preventScroll: true });
    if (focusFirst) focusRow(step(enabled, -1, 1));
    return () => {
      if (hoverTimer) clearTimeout(hoverTimer);
    };
  });

  function rowEl(i: number): HTMLElement | null {
    return el?.querySelector<HTMLElement>(`[data-i="${i}"]`) ?? null;
  }

  function focusRow(i: number) {
    if (i < 0) return;
    active = i;
    rowEl(i)?.focus({ preventScroll: false });
  }

  async function openSub(i: number, byKey: boolean) {
    const it = items[i];
    if (it.kind !== 'submenu' || it.disabled) return;
    if (sub?.index === i) return;
    const r = rowEl(i)?.getBoundingClientRect();
    if (!r) return;
    if (Array.isArray(it.items)) {
      sub = { index: i, items: it.items, row: r, byKey };
      return;
    }
    sub = { index: i, items: null, row: r, byKey };
    try {
      const loaded = await it.items();
      if (sub?.index === i) sub = { ...sub, items: loaded };
    } catch {
      if (sub?.index === i) sub = { ...sub, items: [] };
    }
  }

  function activate(i: number, byKey: boolean) {
    const it = items[i];
    if (!enabled[i]) return;
    if (it.kind === 'submenu') {
      void openSub(i, byKey);
    } else if (it.kind === undefined || it.kind === 'item') {
      onchoose(it.onselect);
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (sub) return; // the submenu has the keys
    const k = e.key;
    if (k === 'ArrowDown') focusRow(step(enabled, active, 1));
    else if (k === 'ArrowUp') focusRow(step(enabled, active < 0 ? 0 : active, -1));
    else if (k === 'Home') focusRow(step(enabled, -1, 1));
    else if (k === 'End') focusRow(step(enabled, 0, -1));
    else if (k === 'ArrowRight') {
      if (active >= 0 && items[active].kind === 'submenu') void openSub(active, true);
    } else if (k === 'ArrowLeft') {
      if (row) onback(true);
    } else if (k === 'Escape') onback(true);
    else if (k === 'Enter' || k === ' ') {
      if (active >= 0) activate(active, true);
    } else if (k === 'Tab') onback(false);
    else if (k.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) focusRow(jump(labels, active, k));
    else return;
    e.preventDefault();
    e.stopPropagation();
  }

  function onenter(i: number) {
    active = i;
    if (hoverTimer) clearTimeout(hoverTimer);
    if (items[i].kind === 'submenu') {
      hoverTimer = setTimeout(() => void openSub(i, false), 150);
    } else if (sub) {
      hoverTimer = setTimeout(() => (sub = null), 250);
    }
  }

  async function closeSub(byKey: boolean) {
    const i = sub?.index ?? -1;
    sub = null;
    if (byKey) {
      await tick();
      focusRow(i);
    }
  }
</script>

<div
  bind:this={el}
  class="cm"
  role="menu"
  tabindex="-1"
  aria-label={label || undefined}
  style:left="{pos.left}px"
  style:top="{pos.top}px"
  {onkeydown}
  in:scale={{ start: 0.97, duration: ms(120) }}>
  {#each items as it, i (i)}
    {#if it.kind === 'sep'}
      <div class="sep" role="separator"></div>
    {:else if it.kind === 'header'}
      <div class="mh" role="presentation">{it.label}</div>
    {:else if it.kind === 'submenu'}
      <button
        class="mi"
        class:open={sub?.index === i}
        data-i={i}
        role="menuitem"
        aria-haspopup="menu"
        aria-expanded={sub?.index === i}
        disabled={it.disabled}
        tabindex={active === i ? 0 : -1}
        onpointerenter={() => onenter(i)}
        onclick={() => openSub(i, false)}>
        <span class="ic">{#if it.icon}<Icon name={it.icon} size={16} />{/if}</span>
        <span class="lb">{it.label}</span>
        <Icon name="caret-right" size={14} class="caret" />
      </button>
    {:else}
      <button
        class="mi"
        class:danger={it.danger}
        class:accent={it.accent}
        data-i={i}
        role={it.checked === undefined ? 'menuitem' : it.radio ? 'menuitemradio' : 'menuitemcheckbox'}
        aria-checked={it.checked === undefined ? undefined : it.checked}
        disabled={it.disabled}
        tabindex={active === i ? 0 : -1}
        onpointerenter={() => onenter(i)}
        onclick={() => activate(i, false)}>
        <span class="ic">
          {#if it.checked !== undefined}
            {#if it.checked}<Icon name="check" size={16} />{/if}
          {:else if it.icon}<Icon name={it.icon} size={16} />{/if}
        </span>
        <span class="lb">{it.label}</span>
        {#if it.shortcut}<kbd>{it.shortcut}</kbd>{/if}
      </button>
    {/if}
  {/each}
</div>

{#if sub}
  {#if sub.items}
    <MenuList
      items={sub.items.length ? sub.items : [{ label: t('Nothing here'), disabled: true, onselect: () => {} }]}
      row={sub.row}
      {onchoose}
      onback={closeSub}
      focusFirst={sub.byKey} />
  {:else}
    <div class="cm loading" style:left="{sub.row.right - 4}px" style:top="{sub.row.top - 6}px"><Spinner size={14} /><span>{t('Loading…')}</span></div>
  {/if}
{/if}

<style>
  .cm {
    position: fixed;
    z-index: 150;
    min-width: 200px;
    max-width: min(360px, calc(100vw - 16px));
    max-height: calc(100dvh - 16px);
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 6px;
    border-radius: var(--r-lg);
    background: var(--bg2);
    border: 1px solid var(--border-hi);
    box-shadow: var(--shadow-3);
    transform-origin: top left;
    outline: none;
  }
  .loading {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 10px 12px;
    font-size: var(--fs-small);
    color: var(--text-dim);
  }
  .mi {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-small);
    text-align: left;
    color: var(--text);
    white-space: nowrap;
  }
  .mi:hover:not(:disabled),
  .mi:focus-visible,
  .mi.open {
    background: var(--hover);
    outline: none;
  }
  .mi:disabled {
    color: var(--text-mute);
    cursor: default;
  }
  .mi.danger {
    color: var(--error);
  }
  .mi.accent {
    color: var(--accent);
  }
  .mi[aria-checked='true'] {
    color: var(--accent);
  }
  .ic {
    display: inline-flex;
    width: 16px;
    flex: none;
  }
  .lb {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mi :global(.caret) {
    color: var(--text-mute);
  }
  kbd {
    font: inherit;
    font-size: 11px;
    color: var(--text-mute);
  }
  .sep {
    height: 1px;
    margin: 6px 4px;
    background: var(--border);
  }
  .mh {
    padding: 8px 10px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-mute);
  }
</style>
