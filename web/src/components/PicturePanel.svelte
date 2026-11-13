<script lang="ts">
  // Video effects: looks, the picture controls, the effects by group, saved looks. In the player's
  // controls; the same sheet as the desktop's (src/app/media/picture.rs). What a browser can't do
  // to a <video> is listed, switched off ("Only in the desktop app").
  import Icon from './Icon.svelte';
  import Toggle from './Toggle.svelte';
  import { prefs } from '../lib/prefs.svelte';
  import { t } from '../lib/i18n.svelte';
  import { ADJUST, CATALOGUE, GROUPS, PRESETS, applyPreset, defaultVideoFx, hasEffects, isOn, presetOnWeb, setValue, toggle, value, type Adjust, type Param, type VideoFx, type VideoPreset } from '../lib/videofxmodel';
  import { ADJUST_LABEL, GROUP_LABEL, HINT, KINDS, LABEL, PRESET_LABEL, paramLabel } from '../lib/videofxlabels';

  const fx = $derived(prefs.videoFx);
  const original = $derived(!hasEffects(fx) && fx.preset == null);
  const desktopOnly = $derived(t('Only in the desktop app'));

  let saving = $state(false);
  let name = $state('');
  let renaming = $state<string | null>(null);
  let newName = $state('');

  const snap = () => $state.snapshot(prefs.videoFx) as VideoFx;
  /** Changed by hand: no longer the look, and on (the switch itself only compares). */
  function byHand(next: VideoFx) {
    prefs.videoFx = { ...next, preset: null, enabled: true };
  }
  function setAdjust(key: keyof Adjust, v: number) {
    const now = snap();
    byHand({ ...now, adjust: { ...now.adjust, [key]: Math.round(Math.max(-100, Math.min(100, v))) } });
  }

  function useLook(p: VideoPreset) {
    prefs.videoFx = { ...($state.snapshot(p.fx) as VideoFx), enabled: true, preset: p.name };
  }
  function save(e: Event) {
    e.preventDefault();
    const n = name.trim();
    if (!n) return;
    prefs.videoLooks = [...prefs.videoLooks.filter((p) => p.name !== n), { name: n, fx: { ...snap(), preset: n } }];
    prefs.videoFx.preset = n;
    name = '';
    saving = false;
  }
  function rename(e: Event) {
    e.preventDefault();
    const [from, to] = [renaming, newName.trim()];
    renaming = null;
    if (!from || !to || to === from) return;
    prefs.videoLooks = prefs.videoLooks.filter((p) => p.name !== to).map((p) => (p.name === from ? { name: to, fx: { ...p.fx, preset: to } } : p));
    if (prefs.videoFx.preset === from) prefs.videoFx.preset = to;
  }
  function remove(n: string) {
    prefs.videoLooks = prefs.videoLooks.filter((p) => p.name !== n);
    if (prefs.videoFx.preset === n) prefs.videoFx.preset = null;
  }

  /** A value as its slider shows it: as many decimals as its steps have. */
  const valueText = (p: Param, v: number) => v.toFixed(p.step >= 1 ? 0 : p.step >= 0.1 ? 1 : 2);
</script>

<div class="picture">
  <div class="line">
    <Toggle checked={fx.enabled} label={t('On (off to compare)')} onchange={(v: boolean) => (prefs.videoFx.enabled = v)} />
  </div>

  <div class="chips" role="group" aria-label={t('Looks')}>
    <button class="fxchip" class:on={original} aria-pressed={original} onclick={() => (prefs.videoFx = defaultVideoFx())}>{t('Original')}</button>
    {#each PRESETS as p (p)}
      {@const can = presetOnWeb(p) !== 'none'}
      <button class="fxchip" class:on={fx.preset === p} aria-pressed={fx.preset === p} disabled={!can} title={can ? undefined : desktopOnly} onclick={() => (prefs.videoFx = applyPreset(snap(), p))}>{t(PRESET_LABEL[p] ?? p)}</button>
    {/each}
    {#each prefs.videoLooks as p (p.name)}
      <button class="fxchip" class:on={fx.preset === p.name} aria-pressed={fx.preset === p.name} onclick={() => useLook(p)}>{p.name}</button>
    {/each}
  </div>

  <h4>{t('Adjust')}</h4>
  <div class="sliders">
    {#each ADJUST as key (key)}
      <label>
        <span class="small">{t(ADJUST_LABEL[key])}</span>
        <input type="range" min="-100" max="100" step="1" value={fx.adjust[key]} oninput={(e) => setAdjust(key, +e.currentTarget.value)} />
        <span class="num small">{fx.adjust[key] > 0 ? '+' : ''}{fx.adjust[key]}</span>
      </label>
    {/each}
  </div>
  <div class="line" title={t('Removes the bands in skies and dark scenes of compressed video.')}>
    <Toggle checked={fx.deband} disabled label={t('Smooth gradients')} hint={desktopOnly} />
  </div>

  {#each GROUPS as group (group)}
    <h4>{t(GROUP_LABEL[group])}</h4>
    {#each CATALOGUE.filter((s) => s.group === group) as s (s.id)}
      {@const on = isOn(fx, s.id)}
      <div class="line" title={t(HINT[s.id] ?? '')}>
        <Toggle checked={on} disabled={!s.web} label={t(LABEL[s.id] ?? s.id)} hint={s.web ? '' : desktopOnly} onchange={(v: boolean) => byHand(toggle(snap(), s.id, v))} />
      </div>
      {#if on && s.web && s.params.length}
        <div class="sliders">
          {#each s.params as p (p.key)}
            {@const v = value(fx, s.id, p.key)}
            {#if p.choice}
              <div class="chips kinds" role="radiogroup" aria-label={t(LABEL[s.id] ?? s.id)}>
                {#each KINDS as kind, i (kind)}
                  <button class="fxchip" role="radio" class:on={v === i} aria-checked={v === i} onclick={() => byHand(setValue(snap(), s.id, p.key, i))}>{t(kind)}</button>
                {/each}
              </div>
            {:else}
              <label class:off={!p.web} title={p.web ? undefined : desktopOnly}>
                <span class="small">{t(paramLabel(s.id, p.key))}</span>
                <input type="range" min={p.min} max={p.max} step={p.step} value={v} disabled={!p.web} oninput={(e) => byHand(setValue(snap(), s.id, p.key, +e.currentTarget.value))} />
                <span class="num small">{valueText(p, v)}</span>
              </label>
            {/if}
          {/each}
        </div>
      {/if}
    {/each}
  {/each}

  <h4>{t('Saved looks')}</h4>
  {#each prefs.videoLooks as p (p.name)}
    {#if renaming === p.name}
      <form class="savebox" onsubmit={rename}>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input sm" autofocus bind:value={newName} placeholder={t('Look name')} aria-label={t('Look name')} onkeydown={(e) => e.key === 'Escape' && (e.stopPropagation(), (renaming = null))} />
        <button class="fxchip on" disabled={!newName.trim()}>{t('Rename')}</button>
      </form>
    {:else}
      <div class="look">
        <button class="grow lookname" class:on={fx.preset === p.name} onclick={() => useLook(p)}>{p.name}</button>
        <button class="btn ghost icon sm" aria-label={t('Rename look {name}', { name: p.name })} title={t('Rename')} onclick={() => ((newName = p.name), (renaming = p.name))}><Icon name="pencil-simple" size={14} /></button>
        <button class="btn ghost icon sm" aria-label={t('Remove look {name}', { name: p.name })} title={t('Remove')} onclick={() => remove(p.name)}><Icon name="x" size={14} /></button>
      </div>
    {/if}
  {/each}
  {#if saving}
    <form class="savebox" onsubmit={save}>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input sm" autofocus bind:value={name} placeholder={t('Look name')} aria-label={t('Look name')} onkeydown={(e) => e.key === 'Escape' && (e.stopPropagation(), (saving = false))} />
      <button class="fxchip on" disabled={!name.trim()}>{t('Save')}</button>
    </form>
  {:else}
    <div><button class="fxchip" onclick={() => (saving = true)}>{t('Save these effects')}</button></div>
  {/if}
</div>

<style>
  .picture {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: var(--fs-small, 14px);
  }
  h4 {
    margin: 8px 0 0;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-dim);
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
  }
  .fxchip.on {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
  }
  .fxchip:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .sliders {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .line + .sliders {
    padding-left: 8px;
  }
  .sliders label {
    display: grid;
    grid-template-columns: 84px 1fr 40px;
    align-items: center;
    gap: 8px;
  }
  .sliders label.off {
    opacity: 0.45;
  }
  .sliders label .num {
    text-align: right;
  }
  .sliders input {
    width: 100%;
    accent-color: var(--accent);
  }
  .kinds {
    padding: 2px 0;
  }
  .look {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .lookname {
    text-align: left;
    color: var(--text);
    padding: 4px 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lookname.on {
    color: var(--accent);
  }
  .savebox {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .savebox .input {
    width: 150px;
    height: 28px;
  }
</style>
