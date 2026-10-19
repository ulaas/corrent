<script lang="ts">
  import { flip } from 'svelte/animate';
  import { fly } from 'svelte/transition';
  import Icon from './Icon.svelte';
  import { dismiss, toasts } from '../lib/toast.svelte';
  import { ms } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import type { IconName } from '../lib/icons';

  const ICON: Record<string, IconName> = {
    info: 'info',
    success: 'check-circle',
    warning: 'warning',
    error: 'warning-circle',
  };
</script>

<div class="toasts" aria-live="polite" role="status">
  {#each toasts as toast (toast.id)}
    <div
      class="toast {toast.kind}"
      animate:flip={{ duration: ms(200) }}
      in:fly={{ y: 16, duration: ms(240) }}
      out:fly={{ x: 40, duration: ms(180) }}>
      <Icon name={ICON[toast.kind]} size={20} />
      <span class="text">{toast.text}</span>
      {#if toast.action}
        <button
          class="btn sm"
          onclick={() => {
            toast.action?.run();
            dismiss(toast.id);
          }}>{toast.action.label}</button>
      {/if}
      <button class="btn ghost icon sm" aria-label={t('Dismiss')} onclick={() => dismiss(toast.id)}
        ><Icon name="x" size={16} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 20px;
    bottom: calc(20px + var(--player-h));
    z-index: 200;
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: min(420px, calc(100vw - 32px));
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 10px 10px 16px;
    border-radius: var(--r-lg);
    background: color-mix(in srgb, var(--bg2) 92%, transparent);
    backdrop-filter: blur(14px);
    border: 1px solid var(--border-hi);
    box-shadow: var(--shadow-3);
    font-size: var(--fs-small);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .info :global(.icon:first-child) {
    color: var(--accent);
  }
  .success :global(.icon:first-child) {
    color: var(--success);
  }
  .warning :global(.icon:first-child) {
    color: var(--warn);
  }
  .error :global(.icon:first-child) {
    color: var(--error);
  }
  @media (max-width: 640px) {
    .toasts {
      right: 16px;
      bottom: calc(76px + var(--player-h));
    }
  }
</style>
