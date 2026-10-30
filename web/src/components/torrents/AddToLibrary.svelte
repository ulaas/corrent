<script lang="ts">
  // "Add to library…" for a torrent (as the desktop's dialog, src/app/dialogs.rs): a library,
  // how the files go there, and where each would end up (a dry run, asked again on changes).
  import Modal from '../Modal.svelte';
  import Icon from '../Icon.svelte';
  import Spinner from '../Spinner.svelte';
  import { api, errorText } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { t, tk } from '../../lib/i18n.svelte';
  import type { Library, Torrent } from '../../lib/types';

  type Action = 'move_out' | 'hardlink' | 'copy' | 'move_download';
  interface Plan {
    folder: string;
    files: { from: string; to: string; exists: boolean }[];
    warnings?: string[];
  }

  let { torrent, onclose }: { torrent: Torrent; onclose: () => void } = $props();

  const CHOICES: { a: Action; name: string; help: string }[] = [
    { a: 'move_out', name: tk('Move into the library'), help: tk('Clean names. The torrent is removed and stops seeding.') },
    { a: 'hardlink', name: tk('Link into the library'), help: tk('Clean names, no extra space on the same drive. Keeps seeding.') },
    { a: 'copy', name: tk('Copy into the library'), help: tk('Clean names. Keeps seeding; takes the space twice.') },
    { a: 'move_download', name: tk('Move the whole download'), help: tk("Keeps seeding from the library, with the release's own names.") },
  ];

  const finished = $derived(torrent.progress >= 1);
  let libs = $state<Library[] | null>(null);
  let library = $state<number | null>(null);
  // svelte-ignore state_referenced_locally
  let action = $state<Action>(torrent.progress >= 1 ? 'move_out' : 'move_download');
  let plan = $state<Plan | null>(null);
  let planError = $state('');
  let error = $state('');
  let busy = $state(false);

  api.get<Library[]>('/libraries').then(
    (l) => {
      libs = l.filter((x) => x.kind === 'movies' || x.kind === 'shows' || x.kind === 'videos');
      library = libs[0]?.id ?? null;
    },
    (e) => (error = errorText(e)),
  );

  // Only a finished download can be split into clean names.
  $effect(() => {
    if (!finished && action !== 'move_download') action = 'move_download';
  });

  // Where the files go, for the library and way chosen now.
  let asked = 0;
  $effect(() => {
    const lib = library, a = action, hash = torrent.hash;
    if (lib == null) return;
    const n = ++asked;
    plan = null;
    planError = '';
    api.post<Plan>(`/torrents/${hash}/import`, { library_id: lib, action: a, dry_run: true }).then(
      (p) => n === asked && (plan = p),
      (e) => n === asked && (planError = errorText(e)),
    );
  });

  function shown(to: string, base: string): string {
    return to.startsWith(base) ? to.slice(base.length).replace(/^[\\/]+/, '') : to;
  }

  async function go() {
    if (library == null) return;
    busy = true;
    error = '';
    try {
      await api.post(`/torrents/${torrent.hash}/import`, { library_id: library, action, dry_run: false });
      toast(t('Added to {library}.', { library: libs?.find((l) => l.id === library)?.name ?? '' }), 'success');
      onclose();
    } catch (e) {
      error = errorText(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={t('Add to library')} size="md" {onclose}>
  {#if !libs}
    {#if error}<p class="form-error">{error}</p>{:else}<div class="center"><Spinner /></div>{/if}
  {:else if !libs.length}
    <p class="muted">{t('There is no movie, show or video library yet: add one in Manage libraries.')}</p>
  {:else}
    <div class="stack">
      <fieldset class="group">
        <legend class="label">{t('Library')}</legend>
        <div class="scroll libs">
          {#each libs as l (l.id)}
            <label class="choice"><input type="radio" bind:group={library} value={l.id} /><span><strong>{l.name}</strong> <span class="mono caption muted">{l.folders[0] ?? ''}</span></span></label>
          {/each}
        </div>
      </fieldset>
      <fieldset class="group">
        <legend class="label">{t('How')}</legend>
        {#each CHOICES as c (c.a)}
          {@const allowed = finished || c.a === 'move_download'}
          <label class="choice" class:off={!allowed}>
            <input type="radio" bind:group={action} value={c.a} disabled={!allowed} />
            <span><span>{t(c.name)}</span><span class="caption muted block">{allowed ? t(c.help) : t('Once the download has finished.')}</span></span>
          </label>
        {/each}
      </fieldset>
      <div class="group">
        <span class="label">{t('Where the files go')}</span>
        <div class="scroll plan">
          {#if planError}
            <p class="form-error">{planError}</p>
          {:else if !plan}
            <Spinner size={16} />
          {:else}
            {#each plan.files as f (f.from)}
              <div class="mono caption" class:muted={f.exists}><Icon name="arrow-right" size={12} /> {shown(f.to, plan.folder)}</div>
            {/each}
            {#each plan.warnings ?? [] as w (w)}
              <div class="caption warn"><Icon name="warning" size={12} /> {w}</div>
            {/each}
          {/if}
        </div>
      </div>
      {#if error}<p class="form-error" role="alert">{error}</p>{/if}
    </div>
  {/if}
  {#snippet footer()}
    <button class="btn" onclick={onclose} disabled={busy}>{t('Cancel')}</button>
    {#if libs?.length}
      <button class="btn primary" onclick={go} disabled={!plan || busy}>{#if busy}<Spinner size={14} />{/if}{t('Add to library')}</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 30px 0;
  }
  .group {
    border: 0;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .scroll {
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  .libs {
    max-height: 132px;
  }
  .plan {
    max-height: 180px;
    padding: 8px 10px;
    border-radius: var(--r-md);
    background: var(--bg2);
    display: grid;
    gap: 2px;
  }
  .choice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 4px 0;
    font-size: var(--fs-small);
    cursor: pointer;
  }
  .choice.off {
    opacity: 0.6;
    cursor: default;
  }
  .choice input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .block {
    display: block;
  }
  .warn {
    color: var(--warn);
  }
</style>
