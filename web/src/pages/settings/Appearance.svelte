<script lang="ts">
  import { flip } from 'svelte/animate';
  import Icon from '../../components/Icon.svelte';
  import { accents, fixedAccentPresets } from 'virtual:tokens';
  import { api } from '../../lib/api';
  import { ms, prefs, resetPrefs, scheme, type CardSize, type Density, type ThemeMode } from '../../lib/prefs.svelte';
  import { libs } from '../../lib/libraries.svelte';
  import { homeRowTitle } from '../../lib/media';
  import { LANGS, langSetting, setLang, t, tk, type Lang } from '../../lib/i18n.svelte';
  import type { HomeRow } from '../../lib/types';

  const THEMES: { id: ThemeMode; label: string; icon: 'desktop' | 'moon' | 'sun' }[] = [
    { id: 'system', label: tk('System'), icon: 'desktop' },
    { id: 'dark', label: tk('Dark'), icon: 'moon' },
    { id: 'light', label: tk('Light'), icon: 'sun' },
  ];
  const PRESETS: { id: string; label: string; swatch: string[] }[] = [
    { id: 'corrent', label: 'Corrent', swatch: ['#0F1115', '#1D212B', '#4F8CFF', '#E6E8EE'] },
    { id: 'nord', label: 'Nord', swatch: ['#2E3440', '#3B4252', '#88C0D0', '#ECEFF4'] },
    { id: 'solarized', label: 'Solarized', swatch: ['#002B36', '#073642', '#B58900', '#EEE8D5'] },
    { id: 'high_contrast', label: tk('High contrast'), swatch: ['#000000', '#1F1F1F', '#FFD400', '#FFFFFF'] },
  ];
  const DENSITIES: { id: Density; label: string }[] = [
    { id: 'compact', label: tk('Compact') },
    { id: 'comfortable', label: tk('Comfortable') },
    { id: 'spacious', label: tk('Spacious') },
  ];
  const SIZES: { id: CardSize; label: string }[] = [
    { id: 'small', label: tk('Small') },
    { id: 'medium', label: tk('Medium') },
    { id: 'large', label: tk('Large') },
  ];

  let lang = $state<Lang | ''>(langSetting());
  let rows = $state<HomeRow[]>([]);
  api.get<HomeRow[]>('/home').then((r) => (rows = r)).catch(() => {});

  // Every known row id, in the user's order.
  const order = $derived.by(() => {
    const ids = rows.map((r) => r.id);
    const known = prefs.homeOrder.filter((id) => ids.includes(id));
    return [...known, ...ids.filter((id) => !known.includes(id))];
  });

  function move(id: string, d: number) {
    const o = [...order];
    const i = o.indexOf(id), j = i + d;
    if (j < 0 || j >= o.length) return;
    [o[i], o[j]] = [o[j], o[i]];
    prefs.homeOrder = o;
  }
  function toggleRow(id: string) {
    prefs.homeHidden = prefs.homeHidden.includes(id) ? prefs.homeHidden.filter((x) => x !== id) : [...prefs.homeHidden, id];
  }
  const rowTitle = (id: string) => {
    const r = rows.find((x) => x.id === id);
    const lib = id.startsWith('recent:') ? libs.list.find((l) => l.id === +id.slice(7)) : undefined;
    return homeRowTitle(id, r?.title ?? id, lib?.name);
  };
  const fixed = $derived(fixedAccentPresets.includes(prefs.preset));
</script>

<p class="lead muted">{t('These settings are saved in this browser.')}</p>

<div class="sgroup">
  <h3>{t('Theme')}</h3>
  <div class="srow wide">
    <div class="themes" role="radiogroup" aria-label={t('Theme')}>
      {#each THEMES as th (th.id)}
        <button role="radio" aria-checked={prefs.theme === th.id} class="theme {th.id}" class:on={prefs.theme === th.id} onclick={() => (prefs.theme = th.id)}>
          <span class="mock"><span></span><span></span><span></span></span>
          <span class="tl"><Icon name={th.icon} size={16} />{t(th.label)}</span>
        </button>
      {/each}
    </div>
  </div>
  <div class="srow wide">
    <div class="stext"><span class="slabel">{t('Palette')}</span></div>
    <div class="presets" role="radiogroup" aria-label={t('Palette')}>
      {#each PRESETS as p (p.id)}
        <button role="radio" aria-checked={prefs.preset === p.id} class="preset" class:on={prefs.preset === p.id} onclick={() => (prefs.preset = p.id)}>
          <span class="sw">{#each p.swatch as c, i (i)}<span style:background={c}></span>{/each}</span>
          <span>{p.id === 'high_contrast' ? t(p.label) : p.label}</span>
        </button>
      {/each}
    </div>
  </div>
  <div class="srow wide">
    <div class="stext"><span class="slabel">{t('Accent colour')}</span>{#if fixed}<span class="shint">{t('High contrast uses a fixed accent colour.')}</span>{/if}</div>
    <div class="accents" role="radiogroup" aria-label={t('Accent colour')}>
      {#each Object.entries(accents) as [name, c] (name)}
        <button role="radio" aria-checked={prefs.accent === name} aria-label={name} title={name} class="acc" class:on={prefs.accent === name} disabled={fixed} style:--c={c[scheme()]} onclick={() => (prefs.accent = name)}>
          {#if prefs.accent === name}<Icon name="check" size={16} />{/if}
        </button>
      {/each}
      <label class="acc custom" class:on={prefs.accent === 'custom'} style:--c={prefs.customAccent} title={t('Custom colour')}>
        <input type="color" value={prefs.customAccent} disabled={fixed} aria-label={t('Custom colour')} oninput={(e) => ((prefs.customAccent = e.currentTarget.value), (prefs.accent = 'custom'))} />
        <Icon name="palette" size={16} />
      </label>
    </div>
  </div>
</div>

<div class="sgroup">
  <h3>{t('Layout and motion')}</h3>
  <div class="srow"><div class="stext"><span class="slabel">{t('Density')}</span></div>
    <div class="sctl seg">{#each DENSITIES as d (d.id)}<button class:on={prefs.density === d.id} aria-pressed={prefs.density === d.id} onclick={() => (prefs.density = d.id)}>{t(d.label)}</button>{/each}</div></div>
  <div class="srow"><div class="stext"><span class="slabel">{t('Card size')}</span></div>
    <div class="sctl seg">{#each SIZES as d (d.id)}<button class:on={prefs.cardSize === d.id} aria-pressed={prefs.cardSize === d.id} onclick={() => (prefs.cardSize = d.id)}>{t(d.label)}</button>{/each}</div></div>
  <div class="srow"><div class="stext"><span class="slabel">{t('Corner radius')}</span></div>
    <div class="sctl"><input class="range" type="range" min="0" max="2" step="0.1" bind:value={prefs.radius} style:--fill="{(prefs.radius / 2) * 100}%" aria-label={t('Corner radius')} /><span class="val num">{Math.round(prefs.radius * 100)}%</span></div></div>
  <div class="srow"><div class="stext"><span class="slabel">{t('Text size')}</span></div>
    <div class="sctl"><input class="range" type="range" min="0.85" max="1.3" step="0.05" bind:value={prefs.fontScale} style:--fill="{((prefs.fontScale - 0.85) / 0.45) * 100}%" aria-label={t('Text size')} /><span class="val num">{Math.round(prefs.fontScale * 100)}%</span></div></div>
  <div class="srow"><div class="stext"><span class="slabel">{t('Motion')}</span><span class="shint">{t('Animations are always off when your system asks for reduced motion.')}</span></div>
    <div class="sctl"><input class="range" type="range" min="0" max="1.5" step="0.25" bind:value={prefs.motion} style:--fill="{(prefs.motion / 1.5) * 100}%" aria-label={t('Motion')} /><span class="val">{prefs.motion === 0 ? t('Off') : `${Math.round(prefs.motion * 100)}%`}</span></div></div>
</div>

<div class="sgroup">
  <h3>{t('Home page')}</h3>
  <p class="sdesc">{t('Choose which rows to show and their order.')}</p>
  {#if !order.length}
    <div class="srow"><span class="muted small">{t('Rows appear here once your libraries have items.')}</span></div>
  {/if}
  <ul class="rows">
    {#each order as id, i (id)}
      <li animate:flip={{ duration: ms(200) }} class:hidden={prefs.homeHidden.includes(id)}>
        <input type="checkbox" class="check" checked={!prefs.homeHidden.includes(id)} aria-label={t('Show {row}', { row: rowTitle(id) })} onchange={() => toggleRow(id)} />
        <span class="grow ellipsis">{rowTitle(id)}</span>
        <button class="btn ghost icon sm" disabled={i === 0} aria-label={t('Move up')} onclick={() => move(id, -1)}><Icon name="caret-up" /></button>
        <button class="btn ghost icon sm" disabled={i === order.length - 1} aria-label={t('Move down')} onclick={() => move(id, 1)}><Icon name="caret-down" /></button>
      </li>
    {/each}
  </ul>
</div>

<div class="sgroup">
  <h3>{t('Language')}</h3>
  <div class="srow"><div class="stext"><span class="slabel">{t('Language')}</span><span class="shint">{t('Automatic follows your browser.')}</span></div>
    <div class="sctl"><select class="select" bind:value={lang} onchange={() => setLang(lang)}>
      {#each LANGS as l (l.id)}<option value={l.id}>{l.id ? l.label : t('Automatic')}</option>{/each}
    </select></div></div>
</div>

<button class="btn ghost" onclick={resetPrefs}><Icon name="arrow-counter-clockwise" />{t('Reset appearance')}</button>

<style>
  .lead {
    margin: -10px 0 18px;
  }
  .themes {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 160px));
    gap: 12px;
  }
  .theme {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    border-radius: var(--r-lg);
    border: 2px solid var(--border);
    transition: border-color var(--dur-micro), transform var(--dur-micro);
  }
  .theme:hover {
    border-color: var(--border-hi);
  }
  .theme.on {
    border-color: var(--accent);
  }
  .mock {
    display: grid;
    grid-template-columns: 1fr 3fr;
    grid-template-rows: 1fr 1fr;
    gap: 4px;
    height: 64px;
    padding: 6px;
    border-radius: var(--r-md);
    background: #0f1115;
  }
  .mock span {
    border-radius: 4px;
    background: #262b36;
  }
  .mock span:first-child {
    grid-row: span 2;
    background: #1d212b;
  }
  .mock span:nth-child(2) {
    background: #4f8cff;
  }
  .light .mock {
    background: #f3f5f9;
  }
  .light .mock span {
    background: #e9ecf2;
  }
  .light .mock span:first-child {
    background: #fff;
  }
  .system .mock {
    background: linear-gradient(135deg, #0f1115 50%, #f3f5f9 50%);
  }
  .tl {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 8px;
  }
  .preset {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 14px 8px 8px;
    border-radius: var(--r-md);
    border: 2px solid var(--border);
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .preset.on {
    border-color: var(--accent);
  }
  .sw {
    display: flex;
    border-radius: 6px;
    overflow: hidden;
  }
  .sw span {
    width: 12px;
    height: 24px;
  }
  .accents {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 8px;
  }
  .acc {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--c);
    display: grid;
    place-items: center;
    color: #fff;
    box-shadow: inset 0 0 0 2px rgba(255, 255, 255, 0.15);
    transition: transform var(--dur-micro) var(--ease-emphasized);
    position: relative;
    cursor: pointer;
  }
  .acc:hover:not(:disabled) {
    transform: scale(1.1);
  }
  .acc.on {
    box-shadow:
      0 0 0 3px var(--bg1),
      0 0 0 5px var(--c);
  }
  .acc:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .custom {
    background: conic-gradient(#ff5c5c, #f5b942, #3ddc84, #2dd4bf, #4f8cff, #8b7cff, #ff6b9d, #ff5c5c);
  }
  .custom.on {
    background: var(--c);
  }
  .custom input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: pointer;
  }
  .seg {
    display: inline-flex;
    gap: 0;
    padding: 3px;
    border-radius: var(--r-pill);
    background: var(--bg0);
    border: 1px solid var(--border);
    justify-self: end;
  }
  .seg button {
    padding: 5px 12px;
    border-radius: var(--r-pill);
    font-size: var(--fs-caption);
    font-weight: 600;
    color: var(--text-dim);
  }
  .seg button.on {
    background: var(--bg3);
    color: var(--text);
    box-shadow: var(--shadow-1);
  }
  .val {
    width: 48px;
    text-align: right;
    font-size: var(--fs-caption);
    color: var(--text-dim);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0 12px 12px;
  }
  .rows li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px;
    border-radius: var(--r-md);
    font-weight: 600;
    font-size: var(--fs-small);
  }
  .rows li:hover {
    background: var(--hover);
  }
  .rows li.hidden span {
    color: var(--text-mute);
  }
  @media (max-width: 640px) {
    .themes {
      grid-template-columns: repeat(3, 1fr);
    }
    .seg {
      justify-self: start;
    }
  }
</style>
