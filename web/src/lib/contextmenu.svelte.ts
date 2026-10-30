// Context menus: one open at a time, shown by the host in App.svelte. Opened by right-click,
// a long press on touch screens, or Shift+F10 / the Menu key, through `use:contextMenu`.
import type { IconName } from './icons';

export type MenuItem =
  | {
      kind?: 'item';
      label: string;
      icon?: IconName;
      danger?: boolean;
      disabled?: boolean;
      /** A check mark (checkbox rows); `radio` draws it as a choice among several. */
      checked?: boolean;
      radio?: boolean;
      /** A key hint shown at the end of the row ("F2"). */
      shortcut?: string;
      /** Highlighted (the desktop's accent rows, e.g. "Choose another folder…"). */
      accent?: boolean;
      onselect: () => void;
    }
  | {
      kind: 'submenu';
      label: string;
      icon?: IconName;
      disabled?: boolean;
      /** Rows, or a function that loads them when the submenu opens. */
      items: MenuItem[] | (() => Promise<MenuItem[]>);
    }
  | { kind: 'sep' }
  | { kind: 'header'; label: string };

export interface OpenMenu {
  items: MenuItem[];
  x: number;
  y: number;
  /** What the menu is about (a torrent's hash): it closes when that leaves the list. */
  key?: string;
  /** Gets the focus back when the menu closes. */
  returnFocus?: HTMLElement;
  /** Opened by touch on a narrow screen: shown as a sheet at the bottom. */
  sheet: boolean;
  title?: string;
  /** Changes with every open (the host starts over). */
  seq: number;
}

export const ctxmenu = $state<{ current: OpenMenu | null }>({ current: null });
let seq = 0;

export function openMenu(
  items: MenuItem[],
  at: { x: number; y: number } | HTMLElement,
  opts: { key?: string; returnFocus?: HTMLElement; touch?: boolean; title?: string } = {},
) {
  // Separators at the ends or next to each other (entries left out by conditions) go.
  const rows = tidy(items);
  if (!rows.length) return;
  let x: number, y: number;
  if (at instanceof HTMLElement) {
    const r = at.getBoundingClientRect();
    x = r.left + Math.min(24, r.width / 2);
    y = r.top + Math.min(r.height, 40);
  } else {
    ({ x, y } = at);
  }
  const narrow = typeof matchMedia === 'function' && matchMedia('(max-width: 640px)').matches;
  ctxmenu.current = {
    items: rows,
    x,
    y,
    key: opts.key,
    returnFocus: opts.returnFocus,
    sheet: !!opts.touch && narrow,
    title: opts.title,
    seq: ++seq,
  };
}

export function closeMenu(focus = true) {
  const c = ctxmenu.current;
  ctxmenu.current = null;
  if (focus && c?.returnFocus?.isConnected) c.returnFocus.focus({ preventScroll: true });
}

/** Closes the menu when it is about `key` (which went away). */
export function closeMenuFor(key: string) {
  if (ctxmenu.current?.key === key) closeMenu(false);
}

/** Drops leading, trailing and repeated separators (and headers with nothing under them). */
export function tidy(items: MenuItem[]): MenuItem[] {
  const next = (i: number) => items[i + 1]?.kind;
  const kept = items.filter((it, i) => it.kind !== 'header' || (i + 1 < items.length && next(i) !== 'sep' && next(i) !== 'header'));
  const out: MenuItem[] = [];
  for (const it of kept) {
    if (it.kind === 'sep') {
      if (out.length && out[out.length - 1].kind !== 'sep') out.push(it);
    } else {
      out.push(it);
    }
  }
  while (out.length && out[out.length - 1].kind === 'sep') out.pop();
  return out;
}

/** Where the browser's own menu is better: text fields and selected text. */
function native(e: Event): boolean {
  const t = e.target as HTMLElement | null;
  if (t?.closest('input, textarea, select, [contenteditable="true"]')) return true;
  const sel = typeof getSelection === 'function' ? getSelection() : null;
  return !!sel && !sel.isCollapsed && !!t && sel.containsNode(t, true);
}

export interface ContextMenuParams {
  /** The rows for this event (null: let the browser's menu through). */
  items: (e: Event) => MenuItem[] | null;
  /** Runs before the menu opens (select the row that was right-clicked). */
  before?: (e: Event) => void;
  /** What the menu is about, asked after `before` (it may change the selection). */
  key?: () => string | undefined;
  title?: string;
  /** What gets the focus back (default: the element itself, when it can take it). */
  returnFocus?: () => HTMLElement | undefined;
}

const LONG_PRESS_MS = 450;
const MOVE_PX = 10;

/** `use:contextMenu={{ items, before }}` on a row, a card or a list. */
export function contextMenu(node: HTMLElement, params: ContextMenuParams) {
  let p = params;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let start: { x: number; y: number } | null = null;
  let opened = 0;
  let swallowClick = false;
  let touch = false;

  function show(e: Event, x: number, y: number, byTouch: boolean): boolean {
    const items = p.items(e);
    if (!items) return false;
    e.preventDefault();
    p.before?.(e);
    const focus = p.returnFocus?.() ?? (node.tabIndex >= 0 ? node : undefined);
    openMenu(items, { x, y }, { key: p.key?.(), returnFocus: focus, touch: byTouch, title: p.title });
    opened = Date.now();
    return true;
  }
  const cancel = () => {
    if (timer) clearTimeout(timer);
    timer = null;
    start = null;
  };
  function oncontextmenu(e: MouseEvent) {
    if (native(e)) return;
    // Android follows a long press with `contextmenu` too: one menu.
    if (Date.now() - opened < 800) {
      e.preventDefault();
      return;
    }
    cancel();
    show(e, e.clientX, e.clientY, touch);
  }
  function onpointerdown(e: PointerEvent) {
    touch = e.pointerType === 'touch';
    if (!touch || native(e)) return;
    cancel();
    start = { x: e.clientX, y: e.clientY };
    const ev = e;
    timer = setTimeout(() => {
      timer = null;
      if (start && show(ev, start.x, start.y, true)) {
        swallowClick = true;
        setTimeout(() => (swallowClick = false), 600);
        navigator.vibrate?.(10);
      }
    }, LONG_PRESS_MS);
  }
  function onpointermove(e: PointerEvent) {
    if (start && Math.hypot(e.clientX - start.x, e.clientY - start.y) > MOVE_PX) cancel();
  }
  // The click that ends a long press must not open the card or row too.
  function onclick(e: MouseEvent) {
    if (swallowClick) {
      swallowClick = false;
      e.preventDefault();
      e.stopPropagation();
    }
  }
  function onkeydown(e: KeyboardEvent) {
    if (!(e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey))) return;
    const target = (e.target as HTMLElement) ?? node;
    const r = target.getBoundingClientRect();
    touch = false;
    show(e, r.left + Math.min(24, r.width / 2), r.top + Math.min(r.height, 40), false);
  }
  node.style.setProperty('-webkit-touch-callout', 'none');
  node.addEventListener('contextmenu', oncontextmenu);
  node.addEventListener('pointerdown', onpointerdown);
  node.addEventListener('pointermove', onpointermove);
  node.addEventListener('pointerup', cancel);
  node.addEventListener('pointercancel', cancel);
  node.addEventListener('click', onclick, true);
  node.addEventListener('keydown', onkeydown);
  return {
    update(next: ContextMenuParams) {
      p = next;
    },
    destroy() {
      cancel();
      node.removeEventListener('contextmenu', oncontextmenu);
      node.removeEventListener('pointerdown', onpointerdown);
      node.removeEventListener('pointermove', onpointermove);
      node.removeEventListener('pointerup', cancel);
      node.removeEventListener('pointercancel', cancel);
      node.removeEventListener('click', onclick, true);
      node.removeEventListener('keydown', onkeydown);
    },
  };
}
