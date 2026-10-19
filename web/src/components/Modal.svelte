<script lang="ts">
  import type { Snippet } from 'svelte';
  import { fade, fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import Icon from './Icon.svelte';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';

  let {
    title = '',
    size = 'md',
    onclose,
    children,
    footer,
    dismissable = true,
  }: {
    title?: string;
    size?: 'sm' | 'md' | 'lg' | 'xl';
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
    dismissable?: boolean;
  } = $props();

  let dialog: HTMLDivElement | undefined = $state();
  const opener = document.activeElement as HTMLElement | null;
  const id = `dlg-${Math.random().toString(36).slice(2, 8)}`;

  const FOCUSABLE =
    'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  $effect(() => {
    if (!dialog) return;
    const first =
      dialog.querySelector<HTMLElement>('[data-autofocus]') ??
      dialog.querySelector<HTMLElement>('.body ' + FOCUSABLE.split(', ').join(', .body ')) ??
      dialog.querySelector<HTMLElement>(FOCUSABLE);
    first?.focus();
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = '';
      opener?.focus?.();
    };
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && dismissable) {
      e.stopPropagation();
      onclose();
    } else if (e.key === 'Tab' && dialog) {
      const items = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
      if (!items.length) return;
      const first = items[0], last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<div class="layer" role="presentation" {onkeydown}>
  <div
    class="scrim"
    role="presentation"
    transition:fade={{ duration: ms(180) }}
    onclick={() => dismissable && onclose()}></div>
  <div
    bind:this={dialog}
    class="dialog {size}"
    role="dialog"
    aria-modal="true"
    aria-labelledby={title ? id : undefined}
    transition:fly={{ y: 24, duration: ms(240), easing: cubicOut }}>
    {#if title}
      <header>
        <h2 {id}>{title}</h2>
        {#if dismissable}
          <button class="btn ghost icon sm" onclick={onclose} aria-label={t('Close')}><Icon name="x" /></button>
        {/if}
      </header>
    {/if}
    <div class="body">{@render children()}</div>
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .layer {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .scrim {
    position: absolute;
    inset: 0;
    background: var(--scrim);
    backdrop-filter: blur(6px);
  }
  .dialog {
    position: relative;
    width: 100%;
    max-height: min(88dvh, 900px);
    display: flex;
    flex-direction: column;
    background: var(--bg1);
    border: 1px solid var(--border);
    border-radius: var(--r-xl);
    box-shadow: var(--shadow-4);
    overflow: hidden;
  }
  .sm {
    max-width: 420px;
  }
  .md {
    max-width: 560px;
  }
  .lg {
    max-width: 820px;
  }
  .xl {
    max-width: 1100px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 18px 20px 6px 24px;
  }
  header h2 {
    flex: 1;
    font-size: var(--fs-title);
  }
  .body {
    padding: 14px 24px 20px;
    overflow: auto;
    flex: 1 1 auto;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 10px;
    padding: 14px 24px;
    border-top: 1px solid var(--border);
    background: var(--bg2);
  }
  @media (max-width: 640px) {
    .layer {
      padding: 0;
      place-items: end stretch;
    }
    .dialog {
      max-width: none;
      max-height: 92dvh;
      border-radius: var(--r-xl) var(--r-xl) 0 0;
      border-bottom: 0;
    }
    .body {
      padding: 12px 18px 18px;
    }
    footer {
      padding: 12px 18px calc(12px + env(safe-area-inset-bottom));
    }
  }
</style>
