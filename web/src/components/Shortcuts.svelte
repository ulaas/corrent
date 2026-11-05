<script lang="ts">
  // The keyboard help (`?`): what each page's keys do.
  import Modal from './Modal.svelte';
  import { t, tk } from '../lib/i18n.svelte';

  let { onclose }: { onclose: () => void } = $props();

  const GROUPS: { title: string; rows: [string, string][] }[] = [
    {
      title: tk('Everywhere'),
      rows: [
        ['/', tk('Search')],
        ['?', tk('Keyboard shortcuts')],
      ],
    },
    {
      title: tk('Video player'),
      rows: [
        ['Space, K', tk('Play or pause')],
        ['← →', tk('Back or forward 5 seconds')],
        ['J L', tk('Back or forward 10 seconds')],
        ['↑ ↓', tk('Volume')],
        ['M', tk('Mute')],
        ['F', tk('Full screen')],
        ['N', tk('Next episode')],
        ['T', tk('Use this frame as the thumbnail')],
        ['I', tk('Picture and playback stats')],
        ['Shift+I', tk('Audio meters')],
        ['Esc', tk('Close')],
      ],
    },
    {
      title: tk('Torrents'),
      rows: [
        ['Space', tk('Pause or resume')],
        ['Ctrl+Space', tk('Select or deselect the row')],
        ['Ctrl+A', tk('Select all')],
        ['Del', tk('Remove…')],
        ['Shift+Del', tk('Remove with files…')],
        ['F2', tk('Rename…')],
        ['Enter, Ctrl+I', tk('Details')],
        ['Ctrl+F', tk('Filter the list')],
        ['Ctrl+Shift+S', tk('Alternative speeds')],
        ['Ctrl+V', tk('Add a pasted magnet or link')],
      ],
    },
  ];
</script>

<Modal title={t('Keyboard shortcuts')} size="lg" {onclose}>
  <div class="groups">
    {#each GROUPS as g (g.title)}
      <section>
        <h3>{t(g.title)}</h3>
        <dl>
          {#each g.rows as [keys, what] (keys)}
            <div><dt><kbd>{keys}</kbd></dt><dd>{t(what)}</dd></div>
          {/each}
        </dl>
      </section>
    {/each}
  </div>
</Modal>

<style>
  .groups {
    columns: 2 280px;
    column-gap: 28px;
  }
  section {
    break-inside: avoid;
    margin-bottom: 20px;
  }
  h3 {
    margin: 0 0 8px;
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--text-dim);
  }
  dl {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  dl div {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }
  dt {
    flex: 0 0 128px;
  }
  dd {
    margin: 0;
    font-size: var(--fs-small);
  }
  kbd {
    font-family: var(--font-mono);
    font-size: 12px;
    padding: 2px 6px;
    border-radius: var(--r-sm);
    background: var(--bg3);
    border: 1px solid var(--border);
    white-space: nowrap;
  }
</style>
