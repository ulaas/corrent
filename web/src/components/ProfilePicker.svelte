<script lang="ts">
  // "Who's watching?" (as the desktop's, src/app/media/profiles.rs): the server's profiles, and
  // a PIN pad for those that have one (digits can be typed too). Switching gives this browser a
  // session for that profile and starts the app over.
  import Modal from './Modal.svelte';
  import Icon from './Icon.svelte';
  import Spinner from './Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { session } from '../lib/session.svelte';
  import { t } from '../lib/i18n.svelte';

  interface Profile {
    id: number;
    name: string;
    display_name: string;
    role: string;
    has_pin: boolean;
  }

  let { onclose }: { onclose: () => void } = $props();

  let list = $state<Profile[] | null>(null);
  let error = $state('');
  let pinFor = $state<Profile | null>(null);
  let pin = $state('');
  let busy = $state(false);

  api.get<Profile[]>('/auth/profiles').then(
    (l) => (list = l),
    (e) => (error = errorText(e)),
  );

  const nameOf = (p: Profile) => p.display_name || p.name;
  const initials = (p: Profile) => nameOf(p).split(/\s+/).map((w) => w[0]).join('').slice(0, 2).toUpperCase();

  async function choose(p: Profile, withPin?: string) {
    if (p.id === session.user?.id && !p.has_pin) return onclose();
    busy = true;
    error = '';
    try {
      await api.post('/auth/profile', withPin ? { user_id: p.id, pin: withPin } : { user_id: p.id });
      // A new session cookie: everything starts over as the other person.
      location.assign('/');
    } catch (e) {
      error = errorText(e);
      pin = '';
      busy = false;
    }
  }

  function pick(p: Profile) {
    if (p.has_pin && p.id !== session.user?.id) {
      pinFor = p;
      pin = '';
      error = '';
    } else {
      void choose(p);
    }
  }

  function press(d: string) {
    if (pin.length < 8) pin += d;
  }

  function onkeydown(e: KeyboardEvent) {
    if (!pinFor || busy) return;
    if (/^\d$/.test(e.key)) press(e.key);
    else if (e.key === 'Backspace') pin = pin.slice(0, -1);
    else if (e.key === 'Enter' && pin.length >= 4) void choose(pinFor, pin);
    else return;
    e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<Modal title={pinFor ? t('PIN for {name}', { name: nameOf(pinFor) }) : t("Who's watching?")} size="sm" {onclose}>
  {#if !pinFor}
    {#if !list}
      {#if error}<p class="form-error">{error}</p>{:else}<div class="center"><Spinner /></div>{/if}
    {:else}
      <div class="profiles">
        {#each list as p (p.id)}
          <button class="profile" class:me={p.id === session.user?.id} disabled={busy} onclick={() => pick(p)}>
            <span class="avatar">{initials(p)}</span>
            <span class="ellipsis pname">{nameOf(p)}{#if p.has_pin}<Icon name="lock" size={12} />{/if}</span>
          </button>
        {/each}
      </div>
    {/if}
  {:else}
    <div class="dots" aria-live="polite">{'•'.repeat(pin.length) || ' '}</div>
    <div class="pad">
      {#each ['1', '2', '3', '4', '5', '6', '7', '8', '9'] as d (d)}
        <button class="key" disabled={busy} onclick={() => press(d)}>{d}</button>
      {/each}
      <button class="key" disabled={busy} aria-label={t('Delete')} onclick={() => (pin = pin.slice(0, -1))}><Icon name="caret-left" size={20} /></button>
      <button class="key" disabled={busy} onclick={() => press('0')}>0</button>
      <button class="key ok" disabled={busy || pin.length < 4} aria-label={t('OK')} onclick={() => choose(pinFor!, pin)}>
        {#if busy}<Spinner size={16} />{:else}<Icon name="check" size={20} />{/if}
      </button>
    </div>
  {/if}
  {#if error && (list || pinFor)}<p class="form-error" role="alert">{error}</p>{/if}
  {#snippet footer()}
    {#if pinFor}<button class="btn" onclick={() => ((pinFor = null), (error = ''))}>{t('Back')}</button>{/if}
    <button class="btn" onclick={onclose}>{t('Cancel')}</button>
  {/snippet}
</Modal>

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 24px 0;
  }
  .profiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    gap: 12px;
  }
  .profile {
    display: grid;
    justify-items: center;
    gap: 8px;
    padding: 14px 8px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--bg2);
    color: var(--text);
  }
  .profile:hover:not(:disabled),
  .profile:focus-visible {
    border-color: var(--accent);
    outline: none;
  }
  .profile.me {
    border-color: var(--border-hi);
  }
  .avatar {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-weight: 700;
    font-size: 20px;
    color: var(--on-accent, #fff);
    background: var(--accent);
  }
  .pname {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    max-width: 100%;
    font-size: var(--fs-small);
  }
  .dots {
    text-align: center;
    font-size: 28px;
    letter-spacing: 0.3em;
    min-height: 40px;
  }
  .pad {
    display: grid;
    grid-template-columns: repeat(3, 72px);
    gap: 10px;
    justify-content: center;
  }
  .key {
    height: 56px;
    border-radius: var(--r-md);
    border: 1px solid var(--border);
    background: var(--bg2);
    color: var(--text);
    font-size: 20px;
    display: grid;
    place-items: center;
  }
  .key:hover:not(:disabled),
  .key:focus-visible {
    border-color: var(--accent);
    outline: none;
  }
  .key.ok:not(:disabled) {
    background: var(--accent);
    color: var(--on-accent, #fff);
  }
</style>
