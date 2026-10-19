<script lang="ts">
  import Modal from './Modal.svelte';
  import { confirms } from '../lib/confirm.svelte';
  import { t } from '../lib/i18n.svelte';

  let checked = $state(false);
  const req = $derived(confirms[0]);

  function finish(ok: boolean) {
    const r = confirms.shift();
    r?.resolve({ ok, checked });
    checked = false;
  }
</script>

{#if req}
  <Modal title={req.title} size="sm" onclose={() => finish(false)}>
    {#if req.text}<p class="muted">{req.text}</p>{/if}
    {#if req.check}
      <label class="row check-row"><input type="checkbox" class="check" bind:checked /> {req.check}</label>
    {/if}
    {#snippet footer()}
      <button class="btn" onclick={() => finish(false)}>{t('Cancel')}</button>
      <button class="btn {req.danger ? 'danger solid' : 'primary'}" data-autofocus onclick={() => finish(true)}
        >{req.ok}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .check-row {
    margin-top: 16px;
    font-size: var(--fs-small);
    cursor: pointer;
  }
</style>
