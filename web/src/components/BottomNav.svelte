<script lang="ts">
  // Phone navigation along the bottom edge.
  import Icon from './Icon.svelte';
  import { link } from '../lib/router.svelte';
  import { can } from '../lib/session.svelte';
  import { t } from '../lib/i18n.svelte';
  import { Perm } from '../lib/types';

  let { current, onmore }: { current: string; onmore: () => void } = $props();
</script>

<nav class="bottom" aria-label={t('Quick navigation')}>
  <a href="/" use:link class:active={current === 'home'}><Icon name="house" size={22} /><span>{t('Home')}</span></a>
  <a href="/find" use:link class:active={current === 'find'}
    ><Icon name="magnifying-glass" size={22} /><span>{t('Find')}</span></a>
  {#if can(Perm.TORRENTS_ADD) || can(Perm.TORRENTS_MANAGE_OWN)}
    <a href="/torrents" use:link class:active={current === 'torrents'}
      ><Icon name="download-simple" size={22} /><span>{t('Torrents')}</span></a>
  {/if}
  <a href="/settings" use:link class:active={current === 'settings'}
    ><Icon name="gear-six" size={22} /><span>{t('Settings')}</span></a>
  <button onclick={onmore}><Icon name="list" size={22} /><span>{t('More')}</span></button>
</nav>

<style>
  .bottom {
    display: none;
  }
  @media (max-width: 760px) {
    .bottom {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 35;
      display: flex;
      height: calc(var(--bottom-nav) + env(safe-area-inset-bottom));
      padding-bottom: env(safe-area-inset-bottom);
      background: var(--glass);
      backdrop-filter: blur(16px) saturate(1.4);
      border-top: 1px solid var(--border);
    }
    a,
    button {
      flex: 1;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 3px;
      font-size: 11px;
      font-weight: 600;
      color: var(--text-mute);
      transition: color var(--dur-micro);
    }
    .active {
      color: var(--accent);
    }
  }
</style>
