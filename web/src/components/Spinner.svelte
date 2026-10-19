<script lang="ts">
  import { t } from '../lib/i18n.svelte';
  let { size = 20, label = '' }: { size?: number; label?: string } = $props();
</script>

<span class="spinner" role="status" aria-label={label || t('Loading…')} style:--s="{size}px"></span>

<style>
  .spinner {
    display: inline-block;
    width: var(--s);
    height: var(--s);
    border-radius: 50%;
    border: 2px solid var(--bg3);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
    flex: none;
  }
</style>
