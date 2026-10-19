<script lang="ts">
  import { fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { login, session, setup } from '../lib/session.svelte';
  import { errorText } from '../lib/api';
  import { ms } from '../lib/prefs.svelte';
  import { LANGS, langSetting, setLang, t, type Lang } from '../lib/i18n.svelte';

  const firstRun = $derived(!!session.info?.setup_required);

  let name = $state('');
  let display = $state('');
  let password = $state('');
  let password2 = $state('');
  let show = $state(false);
  let busy = $state(false);
  let error = $state('');
  let lang = $state<Lang | ''>(langSetting());

  async function submit(e: Event) {
    e.preventDefault();
    error = '';
    if (firstRun) {
      if (password.length < 8) {
        error = t('Use at least 8 characters for the password.');
        return;
      }
      if (password !== password2) {
        error = t("The passwords don't match.");
        return;
      }
    }
    busy = true;
    try {
      if (firstRun) await setup(name.trim(), display.trim(), password);
      else await login(name.trim(), password);
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="auth">
  <div class="glow g1"></div>
  <div class="glow g2"></div>
  <div class="card" in:fly={{ y: 20, duration: ms(420), easing: cubicOut }}>
    <div class="logo">
      <img src="/icons/corrent-128.png" alt="" width="56" height="56" />
    </div>
    {#if session.info === null}
      <h1>{t("Can't reach the server.")}</h1>
      <p class="muted lead">{t('Check that Corrent is running, then reload this page.')}</p>
      <button class="btn primary lg" onclick={() => location.reload()}><Icon name="arrow-clockwise" />{t('Reload')}</button>
    {:else}
      <h1>{firstRun ? t('Welcome to Corrent') : t('Sign in')}</h1>
      <p class="muted lead">
        {#if firstRun}
          {t('Create the administrator account for {server}.', { server: session.info.name })}
        {:else if session.expired}
          {t('Your session has ended. Please sign in again.')}
        {:else}
          {session.info.name}
        {/if}
      </p>

      <form onsubmit={submit} class="stack">
        <label class="field">
          <span class="label">{t('User name')}</span>
          <input class="input" bind:value={name} autocomplete="username" required autocapitalize="none" spellcheck="false" />
        </label>
        {#if firstRun}
          <label class="field">
            <span class="label">{t('Display name')}</span>
            <input class="input" bind:value={display} autocomplete="name" placeholder={t('Optional')} />
          </label>
        {/if}
        <label class="field">
          <span class="label">{t('Password')}</span>
          <span class="pw">
            <input
              class="input"
              type={show ? 'text' : 'password'}
              bind:value={password}
              autocomplete={firstRun ? 'new-password' : 'current-password'}
              required />
            <button
              type="button"
              class="btn ghost icon sm"
              aria-label={show ? t('Hide password') : t('Show password')}
              onclick={() => (show = !show)}><Icon name={show ? 'eye-slash' : 'eye'} /></button>
          </span>
        </label>
        {#if firstRun}
          <label class="field">
            <span class="label">{t('Repeat password')}</span>
            <input class="input" type={show ? 'text' : 'password'} bind:value={password2} autocomplete="new-password" required />
          </label>
        {/if}
        {#if error}
          <p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{error}</p>
        {/if}
        <button class="btn primary lg" type="submit" disabled={busy || !name.trim() || !password}>
          {#if busy}<Spinner size={16} />{/if}
          {firstRun ? t('Create account') : t('Sign in')}
        </button>
      </form>
      {#if firstRun}
        <p class="caption faint note"><Icon name="lock" size={14} />{t('Setup only works on the server itself. Other devices sign in afterwards.')}</p>
      {/if}
    {/if}
    <div class="lang">
      <Icon name="translate" size={16} />
      <select class="select" bind:value={lang} onchange={() => setLang(lang)} aria-label={t('Language')}>
        {#each LANGS as l (l.id)}<option value={l.id}>{l.id ? l.label : t('Automatic')}</option>{/each}
      </select>
    </div>
  </div>
</div>

<style>
  .auth {
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: 24px 16px;
    position: relative;
    overflow: hidden;
    background:
      radial-gradient(1200px 600px at 10% -10%, var(--accent-softer), transparent 60%),
      var(--bg0);
  }
  .glow {
    position: absolute;
    width: 520px;
    height: 520px;
    border-radius: 50%;
    filter: blur(90px);
    opacity: 0.45;
    pointer-events: none;
  }
  .g1 {
    background: var(--accent);
    top: -220px;
    right: -120px;
    opacity: 0.22;
  }
  .g2 {
    background: var(--queued);
    bottom: -260px;
    left: -160px;
    opacity: 0.16;
  }
  .card {
    position: relative;
    width: min(420px, 100%);
    padding: 36px 32px 24px;
    border-radius: var(--r-xl);
    background: color-mix(in srgb, var(--bg1) 86%, transparent);
    backdrop-filter: blur(20px);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-4);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .logo {
    width: 72px;
    height: 72px;
    margin-bottom: 14px;
    display: grid;
    place-items: center;
    border-radius: 22px;
    background: var(--bg2);
    border: 1px solid var(--border);
    box-shadow: 0 10px 30px color-mix(in srgb, var(--accent) 22%, transparent);
  }
  .logo img {
    border-radius: 14px;
  }
  h1 {
    font-size: 26px;
  }
  .lead {
    margin-bottom: 18px;
  }
  .pw {
    position: relative;
    display: flex;
  }
  .pw .btn {
    position: absolute;
    right: 4px;
    top: 50%;
    transform: translateY(-50%);
  }
  .pw .input {
    padding-right: 42px;
  }
  form .btn.primary {
    margin-top: 8px;
  }
  .note {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 14px;
  }
  .lang {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 20px;
    color: var(--text-dim);
  }
  .lang .select {
    width: auto;
    height: 32px;
    background-color: transparent;
    border-color: transparent;
  }
</style>
