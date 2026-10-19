<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText, BASE } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
  import { confirm } from '../../lib/confirm.svelte';
  import { can } from '../../lib/session.svelte';
  import { ago, bytes, date } from '../../lib/format';
  import { t, tk } from '../../lib/i18n.svelte';
  import { Perm, type BackupFile, type TaskInfo } from '../../lib/types';

  const NAMES: Record<string, string> = {
    scan_libraries: tk('Scan the libraries'),
    monitor_shows: tk('Look for new episodes of followed shows'),
    refresh_metadata: tk('Look up metadata and artwork'),
    wanted_search: tk('Search for wanted releases'),
    cleanup_cache: tk('Clean up the transcoding cache'),
    backup: tk('Back up the server'),
    podcasts: tk('Download new podcast episodes'),
  };
  const EVERY: [number, string][] = [
    [0, tk('Only by hand')],
    [15, tk('Every 15 minutes')],
    [60, tk('Every hour')],
    [360, tk('Every 6 hours')],
    [1440, tk('Every day')],
    [10080, tk('Every week')],
  ];

  let tasks = $state<TaskInfo[] | null>(null);
  let backups = $state<BackupFile[]>([]);
  let pendingRestore = $state<string | null>(null);
  let uploading = $state(false);
  let picker: HTMLInputElement | undefined = $state();
  let error = $state('');
  const writable = $derived(can(Perm.SETTINGS_WRITE));
  const admin = $derived(can(Perm.SETTINGS_WRITE) && can(Perm.USERS_MANAGE));

  async function load() {
    try {
      tasks = await api.get<TaskInfo[]>('/tasks');
      if (admin) {
        backups = await api.get<BackupFile[]>('/backups');
        pendingRestore = (await api.get<{ pending: string | null }>('/backups/restore')).pending;
      }
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  void load();
  $effect(() => {
    const h = setInterval(load, 5000);
    return () => clearInterval(h);
  });

  async function restore(b: BackupFile) {
    const r = await confirm({
      title: t('Restore “{name}”?', { name: b.name }),
      text: t('The server goes back to this backup when it next starts: accounts, libraries, watch history and settings. The current files are kept in the data folder (before-restore-…).'),
      ok: t('Restore at restart'),
      danger: true,
    });
    if (!r.ok) return;
    try {
      pendingRestore = (await api.post<{ pending: string | null }>(`/backups/${encodeURIComponent(b.name)}/restore`)).pending;
      toast(t('Restart the server to finish restoring.'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function cancelRestore() {
    try {
      pendingRestore = (await api.del<{ pending: string | null }>('/backups/restore')).pending;
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function upload(files: FileList | null) {
    const f = files?.[0];
    if (!f) return;
    uploading = true;
    try {
      await api.post<BackupFile>('/backups/upload', f);
      toast(t('Backup uploaded'), 'success');
      void load();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      uploading = false;
      if (picker) picker.value = '';
    }
  }

  async function run(task: TaskInfo) {
    task.running = true;
    try {
      await api.post(`/tasks/${task.id}/run`);
      toast(t('Started'), 'success');
    } catch (e) {
      toast(errorText(e), 'error');
    }
    void load();
  }

  async function change(task: TaskInfo, body: { enabled?: boolean; every_min?: number }) {
    try {
      tasks = await api.put<TaskInfo[]>(`/tasks/${task.id}`, body);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
</script>

{#if error}
  <p class="form-error">{error}</p>
{:else if !tasks}
  <div class="center"><Spinner /></div>
{:else}
  <div class="sgroup">
    <h3>{t('Scheduled tasks')}</h3>
    {#each tasks as task (task.id)}
      <div class="srow task">
        <div class="stext">
          <span class="slabel">{t(NAMES[task.id] ?? task.id)}</span>
          <span class="shint">
            {#if task.running}
              <Spinner size={12} /> {t('Running…')}
            {:else if task.last?.finished}
              {#if task.last.ok}<Icon name="check-circle" size={13} />{:else}<Icon name="warning-circle" size={13} />{/if}
              {t('Last run {when}', { when: ago(task.last.started) })}{#if task.last.message} · {task.last.message}{/if}
            {:else}
              {t('Not run yet')}
            {/if}
            {#if task.enabled && task.next_at && !task.running} · {t('next {when}', { when: ago(task.next_at) })}{/if}
          </span>
        </div>
        <div class="sctl">
          <select class="select" disabled={!writable} value={task.every_min} onchange={(e) => change(task, { every_min: Number((e.currentTarget as HTMLSelectElement).value) })}>
            {#each EVERY as [m, label] (m)}<option value={m}>{t(label)}</option>{/each}
            {#if !EVERY.some(([m]) => m === task.every_min)}<option value={task.every_min}>{t('Every {n} minutes', { n: task.every_min })}</option>{/if}
          </select>
          <Toggle checked={task.enabled} disabled={!writable} label={t('On')} onchange={(v: boolean) => change(task, { enabled: v })} />
          <button class="btn sm" disabled={!writable || task.running} onclick={() => run(task)}><Icon name="play" size={14} />{t('Run now')}</button>
        </div>
      </div>
    {/each}
  </div>

  {#if admin}
    <div class="sgroup">
      <h3>{t('Backups')}</h3>
      <p class="sdesc">{t('A backup holds the database, settings, certificates and keys. Restoring one takes effect when the server next starts. Keep backups private.')}</p>
      {#if pendingRestore}
        <div class="srow">
          <div class="stext"><span class="slabel">{t('Restart the server to finish restoring.')}</span><span class="shint mono">{pendingRestore}</span></div>
          <div class="sctl"><button class="btn sm" onclick={cancelRestore}>{t('Cancel')}</button></div>
        </div>
      {/if}
      <div class="srow">
        <div class="stext"><span class="slabel">{t('Restore from a file')}</span><span class="shint">{t('A backup downloaded from this or another Corrent server.')}</span></div>
        <div class="sctl">
          <input type="file" accept=".zip,application/zip" hidden bind:this={picker} onchange={(e) => upload((e.currentTarget as HTMLInputElement).files)} />
          <button class="btn sm" disabled={uploading} onclick={() => picker?.click()}>{#if uploading}<Spinner size={14} />{:else}<Icon name="upload-simple" size={14} />{/if}{t('Upload…')}</button>
        </div>
      </div>
      {#if !backups.length}
        <div class="srow"><span class="muted small">{t('No backups yet.')}</span></div>
      {/if}
      {#each backups as b (b.name)}
        <div class="srow">
          <div class="stext"><span class="slabel mono">{b.name}</span><span class="shint">{date(b.created)} · {bytes(b.size)}</span></div>
          <div class="sctl">
            <a class="btn sm" href="{BASE}/backups/{encodeURIComponent(b.name)}" download><Icon name="download-simple" size={14} />{t('Download')}</a>
            <button class="btn sm" onclick={() => restore(b)}><Icon name="clock-counter-clockwise" size={14} />{t('Restore')}</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 60px 0;
  }
  .task {
    grid-template-columns: 1fr auto;
  }
  .sctl {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .shint {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    flex-wrap: wrap;
  }
</style>
