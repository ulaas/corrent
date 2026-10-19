<script lang="ts">
  // The server's log: recent lines (GET /server/log) plus live lines from the `log` topic.
  import Icon from '../../components/Icon.svelte';
  import { api, errorText } from '../../lib/api';
  import { events } from '../../lib/events.svelte';
  import { time } from '../../lib/format';
  import { t } from '../../lib/i18n.svelte';

  interface Line {
    time: number;
    level: string;
    target: string;
    message: string;
  }
  const MAX = 2000;
  let lines = $state<Line[]>([]);
  let error = $state('');
  let level = $state('');
  let filter = $state('');
  let follow = $state(true);
  let paused = $state(false);
  let box: HTMLDivElement | undefined = $state();

  api.get<Line[]>('/server/log').then((l) => (lines = [...l].reverse())).catch((e) => (error = errorText(e)));

  $effect(() => {
    const off = events.on((e) => {
      if (e.t !== 'log' || paused) return;
      lines.push({ time: e.time, level: e.level, target: e.target, message: e.message });
      if (lines.length > MAX) lines.splice(0, lines.length - MAX);
    });
    const unsub = events.subscribe({ topic: 'log' });
    return () => {
      off();
      unsub();
    };
  });

  const shown = $derived.by(() => {
    const w = filter.toLowerCase();
    const order = ['trace', 'debug', 'info', 'warn', 'error'];
    const min = level ? order.indexOf(level) : 0;
    return lines.filter((l) => order.indexOf(l.level.toLowerCase()) >= min && (!w || `${l.target} ${l.message}`.toLowerCase().includes(w)));
  });

  $effect(() => {
    void shown.length;
    if (follow && box) queueMicrotask(() => box && (box.scrollTop = box.scrollHeight));
  });
</script>

<div class="tools">
  <select class="select lvl" bind:value={level} aria-label={t('Level')}>
    <option value="">{t('All levels')}</option>
    <option value="info">info+</option>
    <option value="warn">warn+</option>
    <option value="error">error</option>
  </select>
  <input class="input" bind:value={filter} placeholder={t('Filter')} aria-label={t('Filter')} />
  <label class="row small"><input type="checkbox" class="check" bind:checked={follow} />{t('Follow')}</label>
  <button class="btn" onclick={() => (paused = !paused)}><Icon name={paused ? 'play' : 'pause'} />{paused ? t('Resume') : t('Pause')}</button>
  <button class="btn ghost" onclick={() => (lines = [])}>{t('Clear')}</button>
</div>
{#if error}<p class="form-error">{error}</p>{/if}
<div class="log mono" bind:this={box} role="log" aria-live="off">
  {#each shown as l, i (i)}
    <div class="ln {l.level.toLowerCase()}"><span class="tm">{time(l.time)}</span><span class="lv">{l.level.toUpperCase()}</span><span class="tg">{l.target}</span><span class="msg">{l.message}</span></div>
  {:else}
    <p class="faint">{t('No log lines.')}</p>
  {/each}
</div>

<style>
  .tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }
  .tools .input {
    width: 240px;
  }
  .lvl {
    width: auto;
  }
  .log {
    height: calc(100dvh - 300px);
    min-height: 320px;
    overflow: auto;
    padding: 10px 14px;
    border-radius: var(--r-lg);
    background: var(--bg0);
    border: 1px solid var(--border);
    font-size: 12px;
    line-height: 1.6;
  }
  .ln {
    display: flex;
    gap: 10px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .tm {
    color: var(--text-mute);
    flex: none;
  }
  .lv {
    width: 44px;
    flex: none;
    font-weight: 600;
    color: var(--text-dim);
  }
  .tg {
    color: var(--queued);
    flex: none;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .warn .lv {
    color: var(--warn);
  }
  .error .lv,
  .error .msg {
    color: var(--error);
  }
  .debug .msg,
  .trace .msg {
    color: var(--text-dim);
  }
</style>
