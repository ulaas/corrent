<script lang="ts">
  import Icon from '../../components/Icon.svelte';
  import Spinner from '../../components/Spinner.svelte';
  import Toggle from '../../components/Toggle.svelte';
  import { api, errorText, BASE } from '../../lib/api';
  import { toast } from '../../lib/toast.svelte';
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
  let error = $state('');
  const writable = $derived(can(Perm.SETTINGS_WRITE));
  const admin = $derived(can(Perm.SETTINGS_WRITE) && can(Perm.USERS_MANAGE));

  async function load() {
    try {
      tasks = await api.get<TaskInfo[]>('/tasks');
      if (admin) backups = await api.get<BackupFile[]>('/backups');
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
      <p class="sdesc">{t('A backup holds the database, settings, certificates and keys. To restore, stop the server and unzip it into its data folder. Keep backups private.')}</p>
      {#if !backups.length}
        <div class="srow"><span class="muted small">{t('No backups yet.')}</span></div>
      {/if}
      {#each backups as b (b.name)}
        <div class="srow">
          <div class="stext"><span class="slabel mono">{b.name}</span><span class="shint">{date(b.created)} · {bytes(b.size)}</span></div>
          <div class="sctl"><a class="btn sm" href="{BASE}/backups/{encodeURIComponent(b.name)}" download><Icon name="download-simple" size={14} />{t('Download')}</a></div>
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
