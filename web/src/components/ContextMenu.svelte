<script lang="ts">
  // The open context menu (see lib/contextmenu.svelte.ts): floating at the pointer, or on phones
  // (opened by touch) a sheet at the bottom where submenus open in place.
  import { fade, fly } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import Spinner from './Spinner.svelte';
  import MenuList from './MenuList.svelte';
  import { closeMenu, ctxmenu, type MenuItem } from '../lib/contextmenu.svelte';
  import { route } from '../lib/router.svelte';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';

  const cur = $derived(ctxmenu.current);

  // Run the chosen row after the menu is gone (it may open a dialog that takes the focus).
  function choose(run: () => void) {
    closeMenu(false);
    queueMicrotask(run);
  }

  // Anything outside the menu closes it: a click, scrolling the page, resizing, leaving the
  // window, going to another page.
  $effect(() => {
    if (!cur) return;
    const inside = (e: Event) => (e.target as Element | null)?.closest?.('.cm, .sheet') != null;
    const down = (e: PointerEvent) => {
      if (!inside(e)) closeMenu(false);
    };
    const scroll = (e: Event) => {
      if (!inside(e)) closeMenu(false);
    };
    const away = () => closeMenu(false);
    document.addEventListener('pointerdown', down, true);
    document.addEventListener('scroll', scroll, true);
    addEventListener('resize', away);
    addEventListener('blur', away);
    return () => {
      document.removeEventListener('pointerdown', down, true);
      document.removeEventListener('scroll', scroll, true);
      removeEventListener('resize', away);
      removeEventListener('blur', away);
    };
  });
  let lastRoute = '';
  $effect(() => {
    const now = `${route.path}?${route.query}#${route.key}`;
    if (lastRoute && now !== lastRoute) closeMenu(false);
    lastRoute = now;
  });

  // The sheet: a stack of levels (a submenu replaces the list, Back returns).
  let stack = $state<{ label: string; items: MenuItem[] | null }[]>([]);
  $effect(() => {
    void cur?.seq;
    stack = [];
  });
  const level = $derived(stack.length ? stack[stack.length - 1] : null);
  const rows = $derived(level ? level.items : (cur?.items ?? []));

  async function sheetOpen(it: MenuItem) {
    if (it.kind === 'submenu') {
      if (it.disabled) return;
      if (Array.isArray(it.items)) {
        stack = [...stack, { label: it.label, items: it.items }];
        return;
      }
      const at = stack.length;
      stack = [...stack, { label: it.label, items: null }];
      const loaded = await it.items().catch(() => [] as MenuItem[]);
      if (stack.length === at + 1) stack = [...stack.slice(0, at), { label: it.label, items: loaded }];
    } else if ((it.kind === undefined || it.kind === 'item') && !it.disabled) {
      choose(it.onselect);
    }
  }

  function sheetKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (stack.length) stack = stack.slice(0, -1);
      else closeMenu();
    }
  }
</script>

{#if cur && !cur.sheet}
  {#key cur.seq}
    <MenuList items={cur.items} at={{ x: cur.x, y: cur.y }} label={cur.title} onchoose={choose} onback={() => closeMenu()} />
  {/key}
{:else if cur}
  <div class="scrim" transition:fade={{ duration: ms(140) }} role="presentation" onclick={() => closeMenu(false)}></div>
  <div class="sheet" role="menu" tabindex="-1" aria-label={cur.title} onkeydown={sheetKey} transition:fly={{ y: 40, duration: ms(180) }}>
    <div class="grip" aria-hidden="true"></div>
    {#if level}
      <button class="row back" onclick={() => (stack = stack.slice(0, -1))}><Icon name="caret-left" size={18} /><span>{level.label}</span></button>
    {:else if cur.title}
      <div class="title">{cur.title}</div>
    {/if}
    {#if !rows}
      <div class="row muted"><Spinner size={16} />{t('Loading…')}</div>
    {:else if !rows.length}
      <div class="row muted">{t('Nothing here')}</div>
    {:else}
      {#each rows as it, i (i)}
        {#if it.kind === 'sep'}
          <div class="sep"></div>
        {:else if it.kind === 'header'}
          <div class="mh">{it.label}</div>
        {:else}
          <button
            class="row"
            class:danger={it.kind !== 'submenu' && it.danger}
            class:accent={it.kind !== 'submenu' && it.accent}
            role={it.kind !== 'submenu' && it.checked !== undefined ? (it.radio ? 'menuitemradio' : 'menuitemcheckbox') : 'menuitem'}
            aria-checked={it.kind !== 'submenu' && it.checked !== undefined ? it.checked : undefined}
            disabled={it.disabled}
            onclick={() => sheetOpen(it)}>
            <span class="ic">
              {#if it.kind !== 'submenu' && it.checked !== undefined}
                {#if it.checked}<Icon name="check" size={18} />{/if}
              {:else if it.icon}<Icon name={it.icon} size={18} />{/if}
            </span>
            <span class="lb">{it.label}</span>
            {#if it.kind === 'submenu'}<Icon name="caret-right" size={16} />{/if}
          </button>
        {/if}
      {/each}
    {/if}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 149;
    background: var(--scrim);
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 150;
    max-height: 80dvh;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 6px 8px calc(10px + env(safe-area-inset-bottom));
    background: var(--bg1);
    border: 1px solid var(--border);
    border-bottom: 0;
    border-radius: var(--r-xl) var(--r-xl) 0 0;
    box-shadow: var(--shadow-4);
    outline: none;
  }
  .grip {
    width: 36px;
    height: 4px;
    margin: 4px auto 8px;
    border-radius: 2px;
    background: var(--border-hi);
  }
  .title {
    padding: 4px 12px 8px;
    font-weight: 600;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    width: 100%;
    min-height: 48px;
    padding: 0 12px;
    border-radius: var(--r-md);
    text-align: left;
    color: var(--text);
  }
  .row:active:not(:disabled) {
    background: var(--hover);
  }
  .row:disabled {
    color: var(--text-mute);
  }
  .row.danger {
    color: var(--error);
  }
  .row.accent,
  .row[aria-checked='true'] {
    color: var(--accent);
  }
  .row.back {
    font-weight: 600;
  }
  .ic {
    display: inline-flex;
    width: 18px;
    flex: none;
  }
  .lb {
    flex: 1;
  }
  .muted {
    color: var(--text-dim);
  }
  .sep {
    height: 1px;
    margin: 6px 8px;
    background: var(--border);
  }
  .mh {
    padding: 10px 12px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-mute);
  }
</style>
