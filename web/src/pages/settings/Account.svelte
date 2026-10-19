<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { logout, refreshMe, session } from '../../lib/session.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { dayDate } from '../../lib/format';
  import { ROLE_LABEL } from '../../lib/roles';
  import { t } from '../../lib/i18n.svelte';
  import type { User } from '../../lib/types';

  const me = $derived(session.user!);
  // svelte-ignore state_referenced_locally
  let display = $state(session.user?.display_name ?? '');
  let current = $state('');
  let next = $state('');
  let next2 = $state('');
  let busy = $state('');
  let pwError = $state('');

  async function saveName(e: Event) {
    e.preventDefault();
    busy = 'name';
    try {
      await api.patch<User>(`/users/${me.id}`, { display_name: display.trim() });
      await refreshMe();
      toast(t('Saved'), 'success');
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = '';
    }
  }

  async function changePassword(e: Event) {
    e.preventDefault();
    pwError = '';
    if (next.length < 8) return void (pwError = t('Use at least 8 characters for the password.'));
    if (next !== next2) return void (pwError = t("The passwords don't match."));
    busy = 'pw';
    try {
      await api.patch(`/users/${me.id}`, { password: next, current_password: current });
      current = next = next2 = '';
      toast(t('Password changed'), 'success');
    } catch (err) {
      pwError = errorText(err);
    } finally {
      busy = '';
    }
  }
</script>

<div class="sgroup profile">
  <div class="who">
    <span class="avatar">{(me.display_name || me.name).slice(0, 2).toUpperCase()}</span>
    <div>
      <h3>{me.display_name || me.name}</h3>
      <p class="muted small">{me.name} · {t(ROLE_LABEL[me.role])} · {t('Member since {date}', { date: dayDate(me.created_at) })}</p>
    </div>
  </div>
  <form class="srow" onsubmit={saveName}>
    <div class="stext"><span class="slabel">{t('Display name')}</span><span class="shint">{t('Shown instead of your user name.')}</span></div>
    <div class="sctl"><input class="input" bind:value={display} aria-label={t('Display name')} /><button class="btn" disabled={busy === 'name' || display.trim() === me.display_name}>{t('Save')}</button></div>
  </form>
</div>

<form class="sgroup" onsubmit={changePassword}>
  <h3>{t('Change password')}</h3>
  <div class="srow"><div class="stext"><span class="slabel">{t('Current password')}</span></div><div class="sctl"><input class="input" type="password" bind:value={current} autocomplete="current-password" required /></div></div>
  <div class="srow"><div class="stext"><span class="slabel">{t('New password')}</span><span class="shint">{t('At least 8 characters.')}</span></div><div class="sctl"><input class="input" type="password" bind:value={next} autocomplete="new-password" required /></div></div>
  <div class="srow"><div class="stext"><span class="slabel">{t('Repeat new password')}</span></div><div class="sctl"><input class="input" type="password" bind:value={next2} autocomplete="new-password" required /></div></div>
  <div class="srow wide end">
    {#if pwError}<p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{pwError}</p>{/if}
    <button class="btn primary" disabled={busy === 'pw' || !current || !next}>{#if busy === 'pw'}<Spinner size={14} />{/if}{t('Change password')}</button>
  </div>
</form>

<div class="sgroup">
  <div class="srow">
    <div class="stext"><span class="slabel">{t('Sign out')}</span><span class="shint">{t('End this browser session.')}</span></div>
    <div class="sctl"><button class="btn danger" onclick={logout}><Icon name="sign-out" />{t('Sign out')}</button></div>
  </div>
</div>

<style>
  .who {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 20px;
  }
  .avatar {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 40%, var(--queued)));
    color: var(--on-accent);
    font-weight: 600;
    font-size: 20px;
  }
  .end {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 14px;
  }
</style>
