<script lang="ts">
  // A popover menu: a trigger button and items (role="menuitem" buttons) with arrow-key
  // navigation; closes on Escape, outside clicks and after a choice.
  import type { Snippet } from 'svelte';
  import { scale } from 'svelte/transition';
  import { ms } from '../lib/prefs.svelte';

  let {
    label,
    trigger,
    children,
    align = 'end',
    triggerClass = 'btn ghost icon',
    width = 220,
  }: {
    label: string;
    trigger: Snippet;
    children: Snippet<[() => void]>;
    align?: 'start' | 'end';
    triggerClass?: string;
    width?: number;
  } = $props();

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let list: HTMLDivElement | undefined = $state();
  let btn: HTMLButtonElement | undefined = $state();
  let up = $state(false);

  const close = () => {
    open = false;
  };

  function toggle() {
    if (!open && btn) up = btn.getBoundingClientRect().bottom > innerHeight - 280;
    open = !open;
  }

  $effect(() => {
    if (!open) return;
    queueMicrotask(() => list?.querySelector<HTMLElement>('[role="menuitem"], [role="menuitemradio"]')?.focus());
    const onDoc = (e: PointerEvent) => {
      if (root && !root.contains(e.target as Node)) open = false;
    };
    document.addEventListener('pointerdown', onDoc, true);
    return () => document.removeEventListener('pointerdown', onDoc, true);
  });

  function onkeydown(e: KeyboardEvent) {
    if (!open) return;
    const items = [...(list?.querySelectorAll<HTMLElement>('[role="menuitem"], [role="menuitemradio"]') ?? [])];
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'Escape') {
      e.stopPropagation();
      open = false;
      btn?.focus();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      items[(i + 1) % items.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      items[(i - 1 + items.length) % items.length]?.focus();
    } else if (e.key === 'Tab') {
      open = false;
    }
  }
</script>

<div class="menu-root" bind:this={root} role="presentation" {onkeydown}>
  <button
    bind:this={btn}
    class={triggerClass}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={label}
    title={label}
    onclick={toggle}>{@render trigger()}</button>
  {#if open}
    <div
      bind:this={list}
      class="menu {align}"
      class:up
      role="menu"
      tabindex="-1"
      style:width="{width}px"
      transition:scale={{ start: 0.96, duration: ms(140) }}>
      {@render children(close)}
    </div>
  {/if}
</div>

<style>
  .menu-root {
    position: relative;
    display: inline-flex;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    z-index: 80;
    padding: 6px;
    border-radius: var(--r-lg);
    background: var(--bg2);
    border: 1px solid var(--border-hi);
    box-shadow: var(--shadow-3);
    transform-origin: top right;
  }
  .menu.up {
    top: auto;
    bottom: calc(100% + 6px);
    transform-origin: bottom right;
  }
  .end {
    right: 0;
  }
  .start {
    left: 0;
    transform-origin: top left;
  }
  .menu :global(.mi) {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-small);
    text-align: left;
    color: var(--text);
  }
  .menu :global(.mi:hover),
  .menu :global(.mi:focus-visible) {
    background: var(--hover);
    outline: none;
  }
  .menu :global(.mi.danger) {
    color: var(--error);
  }
  .menu :global(.mi[aria-checked='true']) {
    color: var(--accent);
  }
  .menu :global(.sep) {
    height: 1px;
    margin: 6px 4px;
    background: var(--border);
  }
  .menu :global(.mh) {
    padding: 8px 10px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-mute);
  }
</style>
