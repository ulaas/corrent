<script lang="ts">
  import Icon from '../Icon.svelte';
  import { t } from '../../lib/i18n.svelte';

  let { tags = $bindable([]), known = [] }: { tags?: string[]; known?: string[] } = $props();
  let text = $state('');
  const listId = `tags-${Math.random().toString(36).slice(2, 7)}`;

  function add() {
    const v = text.trim().replace(/,$/, '');
    if (v && !tags.includes(v)) tags = [...tags, v];
    text = '';
  }
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      add();
    } else if (e.key === 'Backspace' && !text && tags.length) {
      tags = tags.slice(0, -1);
    }
  }
</script>

<div class="tags input">
  {#each tags as tag (tag)}
    <span class="chip on">{tag}<button type="button" aria-label={t('Remove tag {tag}', { tag })} onclick={() => (tags = tags.filter((x) => x !== tag))}><Icon name="x" size={12} /></button></span>
  {/each}
  <input bind:value={text} {onkeydown} onblur={add} list={listId} placeholder={tags.length ? '' : t('Add tags…')} aria-label={t('Tags')} />
  <datalist id={listId}>{#each known.filter((k) => !tags.includes(k)) as k (k)}<option value={k}></option>{/each}</datalist>
</div>

<style>
  .tags {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    height: auto;
    min-height: var(--control-h);
    padding: 4px 8px;
  }
  .tags:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .chip {
    height: 24px;
    padding: 0 6px 0 10px;
  }
  .chip button {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
  }
  .chip button:hover {
    background: var(--hover);
  }
  input {
    flex: 1;
    min-width: 100px;
    border: 0;
    background: transparent;
    height: 26px;
    outline: none;
  }
</style>
