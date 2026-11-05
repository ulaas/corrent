<script lang="ts">
  // Sound processing: presets, equalizer, loudness, meters. In the player's menu, the music player's
  // Sound tab and Settings -> Appearance (the same settings everywhere, they follow the account).
  import Icon from './Icon.svelte';
  import Toggle from './Toggle.svelte';
  import { prefs } from '../lib/prefs.svelte';
  import { t, tk } from '../lib/i18n.svelte';
  import { applyPreset, defaultCrossfeed, defaultNight, flatEq, flatten, PRESETS, type Loudness } from '../lib/audiofxmodel';

  const fx = $derived(prefs.audioFx);
  const LABEL = { night: tk('Night mode'), dialogue: tk('Dialogue boost'), crossfeed: tk('Headphone crossfeed') } as const;

  let saving = $state(false);
  let name = $state('');
  let tuning = $state(false);

  const snap = () => $state.snapshot(prefs.audioFx);
  /** A value changed by hand: on, and no longer exactly the preset. */
  function touched() {
    prefs.audioFx.enabled = true;
    prefs.audioFx.preset = null;
  }

  function usePreset(p: { name: string; fx: typeof prefs.audioFx }) {
    prefs.audioFx = { ...$state.snapshot(p.fx), enabled: true, preset: p.name };
  }

  function save(e: Event) {
    e.preventDefault();
    const n = name.trim();
    if (!n) return;
    const copy = { ...snap(), preset: n };
    prefs.audioPresets = [...prefs.audioPresets.filter((p) => p.name !== n), { name: n, fx: copy }];
    prefs.audioFx.preset = n;
    name = '';
    saving = false;
  }

  const hzLabel = (hz: number) => (hz >= 1000 ? `${hz / 1000}k` : String(hz));
</script>

<div class="sound">
  <div class="line">
    <Toggle checked={fx.enabled} label={t('Sound processing')} onchange={(v: boolean) => (prefs.audioFx.enabled = v)} />
  </div>

  <div class="chips" role="group" aria-label={t('Presets')}>
    <button class="fxchip" class:on={!fx.enabled && !fx.preset} aria-pressed={!fx.enabled && !fx.preset} onclick={() => (prefs.audioFx = flatten(snap()))}>{t('Flat')}</button>
    {#each PRESETS as p (p)}
      <button class="fxchip" class:on={fx.preset === p} aria-pressed={fx.preset === p} onclick={() => (prefs.audioFx = applyPreset(snap(), p))}>{t(LABEL[p])}</button>
    {/each}
    {#each prefs.audioPresets as p (p.name)}
      <span class="fxchip own" class:on={fx.preset === p.name}>
        <button aria-pressed={fx.preset === p.name} onclick={() => usePreset(p)}>{p.name}</button>
        <button class="x" aria-label={t('Remove preset {name}', { name: p.name })} onclick={() => (prefs.audioPresets = prefs.audioPresets.filter((x) => x.name !== p.name))}><Icon name="x" size={12} /></button>
      </span>
    {/each}
    {#if saving}
      <form class="savebox" onsubmit={save}>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input sm" autofocus bind:value={name} placeholder={t('Preset name')} aria-label={t('Preset name')} />
        <button class="fxchip on" disabled={!name.trim()}>{t('Save')}</button>
      </form>
    {:else}
      <button class="fxchip" onclick={() => (saving = true)}>{t('Save as…')}</button>
    {/if}
  </div>

  <div class="eqhead">
    <span class="grow small">{t('Equalizer')}</span>
    <button class="linklike small" onclick={() => (tuning = !tuning)} aria-expanded={tuning}>{t('Fine tuning')}</button>
    <button class="linklike small" onclick={() => ((prefs.audioFx.eq = flatEq()), touched())}>{t('Reset')}</button>
  </div>
  <div class="eq" role="group" aria-label={t('Equalizer')}>
    {#each fx.eq as b, i (i)}
      <label class="band">
        <span class="db num">{b.db > 0 ? '+' : ''}{Math.round(b.db)}</span>
        <input
          type="range"
          min="-12"
          max="12"
          step="0.5"
          value={b.db}
          aria-label={t('{hz} Hz', { hz: b.hz })}
          aria-valuetext="{b.db} dB"
          oninput={(e) => {
            prefs.audioFx.eq[i].db = +e.currentTarget.value;
            touched();
          }} />
        <span class="hz num">{hzLabel(b.hz)}</span>
      </label>
    {/each}
  </div>

  {#if tuning}
    <div class="tune">
      <div class="row3 head small"><span>{t('Frequency')}</span><span>Q</span><span>dB</span></div>
      {#each fx.eq as b, i (i)}
        <div class="row3">
          <input class="input sm num" type="number" min="20" max="22000" value={b.hz} aria-label={t('Frequency')} onchange={(e) => ((prefs.audioFx.eq[i].hz = Math.max(20, Math.min(22000, +e.currentTarget.value || b.hz))), touched())} />
          <input class="input sm num" type="number" min="0.1" max="10" step="0.1" value={b.q} aria-label="Q" onchange={(e) => ((prefs.audioFx.eq[i].q = Math.max(0.1, Math.min(10, +e.currentTarget.value || 1))), touched())} />
          <input class="input sm num" type="number" min="-24" max="24" step="0.5" value={b.db} aria-label="dB" onchange={(e) => ((prefs.audioFx.eq[i].db = Math.max(-24, Math.min(24, +e.currentTarget.value || 0))), touched())} />
        </div>
      {/each}
    </div>
  {/if}

  <div class="line">
    <Toggle checked={!!fx.night} label={t('Night mode')} onchange={(v: boolean) => ((prefs.audioFx.night = v ? defaultNight() : null), touched())} />
  </div>
  {#if fx.night && tuning}
    <div class="sliders">
      <label><span class="small">{t('Threshold')}</span><input type="range" min="-60" max="0" step="1" bind:value={prefs.audioFx.night!.threshold_db} oninput={touched} /><span class="num small">{fx.night.threshold_db} dB</span></label>
      <label><span class="small">{t('Ratio')}</span><input type="range" min="1" max="20" step="0.5" bind:value={prefs.audioFx.night!.ratio} oninput={touched} /><span class="num small">{fx.night.ratio}:1</span></label>
      <label><span class="small">{t('Makeup')}</span><input type="range" min="0" max="24" step="0.5" bind:value={prefs.audioFx.night!.makeup_db} oninput={touched} /><span class="num small">{fx.night.makeup_db} dB</span></label>
    </div>
  {/if}
  <div class="line">
    <Toggle checked={fx.dialogue} label={t('Dialogue boost')} onchange={(v: boolean) => ((prefs.audioFx.dialogue = v), touched())} />
  </div>
  <div class="line">
    <Toggle checked={!!fx.crossfeed} label={t('Headphone crossfeed')} onchange={(v: boolean) => ((prefs.audioFx.crossfeed = v ? defaultCrossfeed() : null), touched())} />
  </div>
  {#if fx.crossfeed && tuning}
    <div class="sliders">
      <label><span class="small">{t('Strength')}</span><input type="range" min="0" max="1" step="0.05" bind:value={prefs.audioFx.crossfeed!.strength} oninput={touched} /><span class="num small">{fx.crossfeed.strength.toFixed(2)}</span></label>
      <label><span class="small">{t('Range')}</span><input type="range" min="0" max="1" step="0.05" bind:value={prefs.audioFx.crossfeed!.range} oninput={touched} /><span class="num small">{fx.crossfeed.range.toFixed(2)}</span></label>
    </div>
  {/if}

  <div class="line">
    <span class="grow">{t('Even out loudness')}</span>
    <div class="seg" role="radiogroup" aria-label={t('Even out loudness')}>
      {#each [['off', t('Off')], ['live', t('Live')]] as [v, l] (v)}
        <button role="radio" aria-checked={fx.loudness === v} class:on={fx.loudness === v} onclick={() => ((prefs.audioFx.loudness = v as Loudness), (prefs.audioFx.enabled ||= v === 'live'))}>{l}</button>
      {/each}
    </div>
  </div>
  <div class="line">
    <Toggle checked={fx.meters} label={t('Show meters')} onchange={(v: boolean) => (prefs.audioFx.meters = v)} />
  </div>
</div>

<style>
  .sound {
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: var(--fs-small, 14px);
  }
  .line {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 32px;
  }
  .line :global(.toggle) {
    flex: 1;
    justify-content: space-between;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .fxchip {
    padding: 4px 10px;
    border-radius: 999px;
    border: 1px solid var(--border-hi);
    font-size: 12px;
    color: var(--text);
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .fxchip.on {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  .fxchip button {
    color: inherit;
    font: inherit;
  }
  .fxchip .x {
    display: inline-grid;
    place-items: center;
    opacity: 0.7;
  }
  .savebox {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .savebox .input {
    width: 130px;
    height: 28px;
  }
  .eqhead {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 4px;
  }
  .linklike {
    color: var(--text-dim);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .eq {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    gap: 2px;
  }
  .band {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }
  .band .db,
  .band .hz {
    font-size: 10px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .band input {
    writing-mode: vertical-lr;
    direction: rtl;
    width: 22px;
    height: 104px;
    accent-color: var(--accent);
    touch-action: none;
  }
  .tune {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .row3 {
    display: grid;
    grid-template-columns: 1.4fr 1fr 1fr;
    gap: 6px;
  }
  .row3.head {
    color: var(--text-dim);
  }
  .row3 .input {
    width: 100%;
    height: 28px;
    padding: 0 6px;
  }
  .sliders {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-left: 8px;
  }
  .sliders label {
    display: grid;
    grid-template-columns: 76px 1fr 56px;
    align-items: center;
    gap: 8px;
  }
  .sliders input {
    width: 100%;
    accent-color: var(--accent);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border-hi);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    padding: 3px 12px;
    font-size: 12px;
    color: var(--text);
  }
  .seg button.on {
    background: var(--accent);
    color: var(--on-accent);
  }
</style>
