<script lang="ts">
  import { extensions } from '../../lib/extensions.svelte';
  import { t } from '../../lib/i18n.svelte';

  let { section }: { section: string } = $props();
  const page = $derived(extensions.pages.find((p) => `ext-${p.id}` === section));
  let el: HTMLElement | undefined = $state();

  $effect(() => {
    if (!el || !page) return;
    let done: void | (() => void);
    try {
      done = page.render(el);
    } catch (e) {
      el.textContent = String(e);
    }
    return () => {
      if (typeof done === 'function') done();
      if (el) el.replaceChildren();
    };
  });
</script>

{#if page}
  <div class="ext" bind:this={el}></div>
{:else}
  <p class="muted">{t('This extension isn’t loaded.')}</p>
{/if}
