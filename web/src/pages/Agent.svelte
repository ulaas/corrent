<script lang="ts">
  // AI agent (as the desktop's, src/app/media/agent.rs): how to connect an MCP client to this
  // server, and the changes agents propose, each approved or rejected here.
  import Icon from '../components/Icon.svelte';
  import Spinner from '../components/Spinner.svelte';
  import { api, errorText } from '../lib/api';
  import { date } from '../lib/format';
  import { toast } from '../lib/toast.svelte';
  import { t } from '../lib/i18n.svelte';

  interface Proposal {
    id: number;
    created_at: number;
    tool: string;
    summary: string;
    status: 'pending' | 'applied' | 'failed' | 'rejected';
    decided_at?: number;
    result?: string;
  }

  let list = $state<Proposal[] | null>(null);
  let error = $state('');
  let deciding = $state<number | null>(null);

  async function load() {
    try {
      list = await api.get<Proposal[]>('/agent/proposals');
      error = '';
    } catch (e) {
      error = errorText(e);
    }
  }
  $effect(() => {
    void load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  });

  async function decide(p: Proposal, approve: boolean) {
    deciding = p.id;
    try {
      const r = await api.post<Proposal>(`/agent/proposals/${p.id}/${approve ? 'approve' : 'reject'}`, {});
      if (r.status === 'failed') toast(t("Couldn't do it: {why}", { why: r.result ?? '' }), 'error');
      else if (r.status === 'applied') toast(t('Done.'), 'success');
      await load();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      deciding = null;
    }
  }

  const pending = $derived(list?.filter((p) => p.status === 'pending') ?? []);
  const earlier = $derived(list?.filter((p) => p.status !== 'pending').slice(0, 50) ?? []);

  // Connecting: the server's MCP endpoint, with a token of its own (a paired device).
  const endpoint = `${location.origin}/api/v1/mcp`;
  let token = $state('');
  let making = $state(false);
  async function makeToken() {
    making = true;
    try {
      const code = await api.post<{ code: string }>('/auth/pair/code', {});
      const r = await api.post<{ token: string }>('/auth/pair', { code: code.code, device: { name: 'AI agent', platform: 'mcp', client: 'mcp' } });
      token = r.token;
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      making = false;
    }
  }
  const claudeCode = $derived(`claude mcp add --transport http corrent ${endpoint} --header "Authorization: Bearer ${token || '<token>'}"`);
  const others = $derived(
    JSON.stringify({ mcpServers: { corrent: { type: 'http', url: endpoint, headers: { Authorization: `Bearer ${token || '<token>'}` } } } }, null, 2),
  );
  const copy = (text: string) => void navigator.clipboard?.writeText(text).then(() => toast(t('Copied'), 'success'));
</script>

<div class="page">
  <div class="page-head"><h1>{t('AI agent')}</h1></div>

  <section class="panel connect">
    <h3><Icon name="sparkle" size={18} />{t('Connect an AI agent')}</h3>
    <p class="muted">
      {t('AI agents (Claude Code, Claude Desktop and other MCP clients) can look through your libraries and downloads and propose changes: identify movies, fix titles, put downloads into libraries, pick thumbnails. Nothing changes until you approve it below, and nothing is ever deleted.')}
    </p>
    <p class="caption muted">{t('The agent signs in as you with a token of its own; remove it any time in Settings → Devices.')}</p>
    {#if !token}
      <button class="btn" disabled={making} onclick={makeToken}>{#if making}<Spinner size={14} />{:else}<Icon name="key" size={16} />{/if}{t('Create a token for an agent')}</button>
    {:else}
      <p class="caption warn">{t('Copy it now: the token is shown only once.')}</p>
    {/if}
    {#each [[t('Claude Code (a terminal)'), claudeCode], [t('Claude Desktop and others (their MCP settings)'), others]] as [title, text] (title)}
      <div class="snippet">
        <div class="row"><strong class="grow">{title}</strong><button class="btn sm" onclick={() => copy(text)}><Icon name="copy" size={14} />{t('Copy')}</button></div>
        <pre class="mono">{text}</pre>
      </div>
    {/each}
  </section>

  <h2 class="section-title">{t('Waiting for you')}</h2>
  {#if error}
    <p class="form-error">{error}</p>
  {:else if !list}
    <Spinner />
  {:else if !pending.length}
    <p class="muted">{t('Nothing to decide. Proposals from agents show up here.')}</p>
  {:else}
    <div class="stack">
      {#each pending as p (p.id)}
        <div class="panel proposal">
          <p>{p.summary}</p>
          <div class="row">
            <span class="caption muted grow">{date(p.created_at)}</span>
            {#if deciding === p.id}
              <Spinner size={16} />
            {:else}
              <button class="btn" disabled={deciding != null} onclick={() => decide(p, false)}>{t('Reject')}</button>
              <button class="btn primary" disabled={deciding != null} onclick={() => decide(p, true)}><Icon name="check" size={16} />{t('Approve')}</button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if earlier.length}
    <h2 class="section-title">{t('Earlier')}</h2>
    <ul class="earlier">
      {#each earlier as p (p.id)}
        <li title={p.result || p.summary}>
          <span class="caption st {p.status}">{p.status === 'applied' ? t('Done') : p.status === 'failed' ? t('Failed') : t('Rejected')}</span>
          <span class="ellipsis muted">{p.summary}</span>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .connect {
    display: grid;
    gap: 10px;
    padding: 18px 20px;
    margin-bottom: 24px;
  }
  .connect h3 {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .connect > .btn {
    justify-self: start;
  }
  .snippet {
    display: grid;
    gap: 6px;
  }
  pre {
    margin: 0;
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--bg1);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-all;
    font-variant-ligatures: none;
  }
  .proposal {
    display: grid;
    gap: 8px;
    padding: 14px 16px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .earlier {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 6px;
  }
  .earlier li {
    display: flex;
    gap: 10px;
    min-width: 0;
  }
  .st {
    flex: none;
    width: 70px;
  }
  .st.applied {
    color: var(--success);
  }
  .st.failed {
    color: var(--error);
  }
  .st.rejected {
    color: var(--text-mute);
  }
  .warn {
    color: var(--warn);
  }
</style>
