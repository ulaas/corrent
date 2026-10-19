<script lang="ts">
  let {
    checked = $bindable(false),
    label = '',
    hint = '',
    disabled = false,
    onchange,
  }: { checked?: boolean; label?: string; hint?: string; disabled?: boolean; onchange?: (v: boolean) => void } =
    $props();
</script>

<label class="toggle" class:disabled>
  <span class="text">
    {#if label}<span class="label">{label}</span>{/if}
    {#if hint}<span class="hint">{hint}</span>{/if}
  </span>
  <input
    type="checkbox"
    role="switch"
    bind:checked
    {disabled}
    onchange={() => onchange?.(checked)} />
  <span class="track" aria-hidden="true"><span class="thumb"></span></span>
</label>

<style>
  .toggle {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    cursor: pointer;
    min-height: 32px;
    position: relative;
  }
  .disabled {
    opacity: 0.5;
    cursor: default;
  }
  .text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .label {
    font-weight: var(--fw-semibold);
    font-size: var(--fs-small);
  }
  .hint {
    color: var(--text-dim);
    font-size: var(--fs-caption);
  }
  input {
    position: absolute;
    opacity: 0;
    width: 1px;
    height: 1px;
  }
  .track {
    flex: none;
    width: 40px;
    height: 24px;
    border-radius: 12px;
    background: var(--bg3);
    border: 1px solid var(--border-hi);
    position: relative;
    transition:
      background var(--dur-short) var(--ease-standard),
      border-color var(--dur-short);
  }
  .thumb {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--text-dim);
    transition:
      transform var(--dur-short) var(--ease-emphasized),
      background var(--dur-short);
  }
  input:checked + .track {
    background: var(--accent);
    border-color: var(--accent);
  }
  input:checked + .track .thumb {
    transform: translateX(16px);
    background: var(--on-accent);
  }
  input:focus-visible + .track {
    outline: var(--focus-width) solid var(--accent);
    outline-offset: var(--focus-offset);
  }
</style>
