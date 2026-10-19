<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Modal from '../../components/Modal.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText } from '../../lib/api';
  import { libs } from '../../lib/libraries.svelte';
  import { confirm } from '../../lib/confirm.svelte';
  import { toast } from '../../lib/toast.svelte';
  import { session } from '../../lib/session.svelte';
  import { dayDate } from '../../lib/format';
  import { defaultPerms, PERM_LABELS, ROLE_HINT, ROLE_LABEL } from '../../lib/roles';
  import { t } from '../../lib/i18n.svelte';
  import type { Role, User } from '../../lib/types';

  const ROLES: Role[] = ['admin', 'user', 'guest', 'kids'];
  let users = $state<User[] | null>(null);
  let error = $state('');

  interface Draft {
    id: number | null;
    name: string;
    display_name: string;
    password: string;
    role: Role;
    perms: number;
    custom: boolean;
    disabled: boolean;
    libraries: number[];
  }
  let edit = $state<Draft | null>(null);
  let formError = $state('');
  let busy = $state(false);

  async function load() {
    try {
      users = await api.get<User[]>('/users');
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();

  function create() {
    formError = '';
    edit = { id: null, name: '', display_name: '', password: '', role: 'user', perms: defaultPerms('user'), custom: false, disabled: false, libraries: [] };
  }
  async function open(u: User) {
    formError = '';
    edit = { id: u.id, name: u.name, display_name: u.display_name, password: '', role: u.role, perms: u.perms, custom: u.perms !== defaultPerms(u.role), disabled: u.disabled, libraries: [] };
    try {
      const l = await api.get<number[]>(`/users/${u.id}/libraries`);
      if (edit?.id === u.id) edit.libraries = l;
    } catch {
      /* keep empty = all */
    }
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!edit) return;
    formError = '';
    if (edit.id == null && (!edit.name.trim() || edit.password.length < 8)) {
      formError = t('A user name and a password of at least 8 characters are needed.');
      return;
    }
    if (edit.id != null && edit.password && edit.password.length < 8) {
      formError = t('Use at least 8 characters for the password.');
      return;
    }
    busy = true;
    try {
      const perms = edit.custom ? edit.perms : undefined;
      let id = edit.id;
      if (id == null) {
        const u = await api.post<User>('/users', { name: edit.name.trim(), display_name: edit.display_name.trim(), password: edit.password, role: edit.role, perms });
        id = u.id;
      } else {
        const body: Record<string, unknown> = { display_name: edit.display_name.trim(), role: edit.role, disabled: edit.disabled, perms: edit.custom ? edit.perms : defaultPerms(edit.role) };
        if (edit.password) body.password = edit.password;
        await api.patch(`/users/${id}`, body);
      }
      await api.put(`/users/${id}/libraries`, edit.libraries);
      toast(t('Saved'), 'success');
      edit = null;
      void load();
    } catch (err) {
      formError = errorText(err);
    } finally {
      busy = false;
    }
  }

  async function remove(u: User) {
    const r = await confirm({ title: t('Delete “{name}”?', { name: u.display_name || u.name }), text: t('Their sessions, devices and watch history are deleted.'), ok: t('Delete'), danger: true });
    if (!r.ok) return;
    try {
      await api.del(`/users/${u.id}`);
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  function setRole(r: Role) {
    if (!edit) return;
    edit.role = r;
    if (!edit.custom) edit.perms = defaultPerms(r);
  }
</script>

<div class="head"><p class="muted small grow">{t('People who can sign in to this server.')}</p><button class="btn primary" onclick={create}><Icon name="user-plus" />{t('Add user')}</button></div>

{#if error}
  <p class="form-error">{error}</p>
{:else if !users}
  <Spinner />
{:else}
  <div class="sgroup">
    {#each users as u (u.id)}
      <div class="srow urow">
        <div class="row">
          <span class="avatar">{(u.display_name || u.name).slice(0, 2).toUpperCase()}</span>
          <div class="stext">
            <span class="slabel">{u.display_name || u.name}
              {#if u.id === session.user?.id}<span class="badge accent">{t('You')}</span>{/if}
              {#if u.disabled}<span class="badge err">{t('Disabled')}</span>{/if}</span>
            <span class="shint">{u.name} · {t(ROLE_LABEL[u.role])} · {t('Since {date}', { date: dayDate(u.created_at) })}</span>
          </div>
        </div>
        <div class="sctl">
          <button class="btn sm" onclick={() => open(u)}><Icon name="pencil-simple" size={14} />{t('Edit')}</button>
          {#if u.id !== session.user?.id}<button class="btn sm icon danger" aria-label={t('Delete')} title={t('Delete')} onclick={() => remove(u)}><Icon name="trash" size={16} /></button>{/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

{#if edit}
  <Modal title={edit.id == null ? t('Add user') : t('Edit {name}', { name: edit.name })} size="lg" onclose={() => (edit = null)}>
    <form id="user-form" class="stack" onsubmit={save}>
      <div class="grid2">
        <label class="field"><span class="label">{t('User name')}</span><input class="input" bind:value={edit.name} disabled={edit.id != null} autocomplete="off" autocapitalize="none" /></label>
        <label class="field"><span class="label">{t('Display name')}</span><input class="input" bind:value={edit.display_name} /></label>
      </div>
      <label class="field"><span class="label">{edit.id == null ? t('Password') : t('New password')}</span>
        <input class="input" type="password" bind:value={edit.password} autocomplete="new-password" placeholder={edit.id == null ? '' : t('Leave empty to keep it')} /></label>
      <div class="field">
        <span class="label">{t('Role')}</span>
        <div class="roles" role="radiogroup" aria-label={t('Role')}>
          {#each ROLES as r (r)}
            <button type="button" role="radio" aria-checked={edit.role === r} class="role" class:on={edit.role === r} onclick={() => setRole(r)}>
              <strong>{t(ROLE_LABEL[r])}</strong><span class="caption muted">{t(ROLE_HINT[r])}</span>
            </button>
          {/each}
        </div>
      </div>
      <Toggle bind:checked={edit.custom} label={t('Custom permissions')} onchange={(v) => !v && edit && (edit.perms = defaultPerms(edit.role))} />
      {#if edit.custom}
        <div class="perms">
          {#each PERM_LABELS as p (p.bit)}
            <label class="row small"><input type="checkbox" class="check" checked={(edit.perms & p.bit) !== 0} onchange={() => (edit!.perms ^= p.bit)} />{t(p.label)}</label>
          {/each}
        </div>
      {/if}
      {#if libs.list.length}
        <div class="field">
          <span class="label">{t('Libraries')}</span>
          <span class="hint">{t('None selected = every library.')}</span>
          <div class="row wrap">
            {#each libs.list as l (l.id)}
              {@const on = edit.libraries.includes(l.id)}
              <button type="button" class="chip" class:on aria-pressed={on} onclick={() => (edit!.libraries = on ? edit!.libraries.filter((x) => x !== l.id) : [...edit!.libraries, l.id])}>{l.name}</button>
            {/each}
          </div>
        </div>
      {/if}
      {#if edit.id != null && edit.id !== session.user?.id}
        <Toggle bind:checked={edit.disabled} label={t('Account disabled')} hint={t('A disabled account cannot sign in.')} />
      {/if}
      {#if formError}<p class="form-error" role="alert"><Icon name="warning-circle" size={16} />{formError}</p>{/if}
    </form>
    {#snippet footer()}
      <button class="btn" onclick={() => (edit = null)}>{t('Cancel')}</button>
      <button class="btn primary" type="submit" form="user-form" disabled={busy}>{#if busy}<Spinner size={14} />{/if}{edit?.id == null ? t('Add user') : t('Save')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-bottom: 18px;
  }
  .urow {
    grid-template-columns: 1fr auto;
  }
  .avatar {
    width: 38px;
    height: 38px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--bg3);
    font-weight: 600;
    font-size: 13px;
    flex: none;
  }
  .slabel {
    display: inline-flex;
    gap: 8px;
    align-items: center;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .roles {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }
  .role {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 10px 12px;
    border-radius: var(--r-md);
    border: 2px solid var(--border);
    text-align: left;
  }
  .role.on {
    border-color: var(--accent);
    background: var(--accent-softer);
  }
  .perms {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px 16px;
    padding: 12px;
    border-radius: var(--r-md);
    background: var(--bg2);
  }
  @media (max-width: 640px) {
    .grid2,
    .roles,
    .perms {
      grid-template-columns: 1fr;
    }
  }
</style>
