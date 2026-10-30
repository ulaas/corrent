<script lang="ts">
  // A titled page of cards (a person's titles, a home row in full).
  import Card from './Card.svelte';
  import Icon from './Icon.svelte';
  import Spinner from './Spinner.svelte';
  import { back } from '../lib/router.svelte';
  import { cardWidth } from '../lib/cards.svelte';
  import { shapeFor } from '../lib/media';
  import { t } from '../lib/i18n.svelte';
  import type { IconName } from '../lib/icons';
  import type { Item } from '../lib/types';

  let { title, icon, items, error = '' }: { title: string; icon: IconName; items: Item[] | null; error?: string } = $props();
</script>

<div class="page">
  <button class="btn ghost back" onclick={() => back()}><Icon name="arrow-left" />{t('Back')}</button>
  <div class="page-head"><h1>{title}</h1></div>
  {#if error}
    <div class="empty"><div class="empty-icon"><Icon name="warning-circle" size={30} /></div><h3>{t("Couldn't load this item")}</h3><p>{error}</p></div>
  {:else if !items}
    <div class="empty"><Spinner /></div>
  {:else if !items.length}
    <div class="empty"><div class="empty-icon"><Icon name={icon} size={30} /></div><h3>{t('Nothing here yet')}</h3></div>
  {:else}
    <div class="grid" style:--w="{cardWidth(shapeFor(items[0].kind))}px">
      {#each items as c (c.id)}<Card item={c} shape={shapeFor(c.kind)} />{/each}
    </div>
  {/if}
</div>

<style>
  .back {
    margin-bottom: 8px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--w), 1fr));
    gap: 20px;
  }
</style>
