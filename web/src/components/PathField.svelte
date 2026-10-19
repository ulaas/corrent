<script lang="ts">
  // A path text box with a "browse" button that opens the server folder picker.
  import Icon from './Icon.svelte';
  import FolderPicker from './FolderPicker.svelte';
  import { t } from '../lib/i18n.svelte';

  let {
    value = $bindable(''),
    placeholder = '',
    label = '',
    id = undefined,
  }: { value?: string; placeholder?: string; label?: string; id?: string } = $props();
  let picking = $state(false);
</script>

<span class="pf">
  <input class="input mono" {id} bind:value {placeholder} aria-label={label || undefined} spellcheck="false" />
  <button type="button" class="btn icon" aria-label={t('Browse…')} title={t('Browse…')} onclick={() => (picking = true)}><Icon name="folder-open" /></button>
</span>
{#if picking}
  <FolderPicker start={value || placeholder} onpick={(p) => ((value = p), (picking = false))} onclose={() => (picking = false)} />
{/if}

<style>
  .pf {
    display: flex;
    gap: 8px;
    min-width: 0;
  }
  .pf .input {
    flex: 1;
    min-width: 0;
    font-size: 13px;
  }
</style>
