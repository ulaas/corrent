<script lang="ts">
  // A home row in full ("See all" for Continue watching, Next up and the like).
  import ItemGrid from '../components/ItemGrid.svelte';
  import { api, errorText } from '../lib/api';
  import { homeRowTitle } from '../lib/media';
  import { libs } from '../lib/libraries.svelte';
  import type { HomeRow } from '../lib/types';

  let { params }: { params: Record<string, string> } = $props();
  let row = $state<HomeRow | null>(null);
  let error = $state('');

  $effect(() => {
    void libs.changed;
    const id = params.id;
    api.get<HomeRow[]>('/home').then(
      (rows) => (row = rows.find((r) => r.id === id) ?? { id, title: '', items: [] }),
      (e) => (error = errorText(e)),
    );
  });
  const title = $derived(row ? homeRowTitle(row.id, row.title) : '');
</script>

<ItemGrid {title} icon="rows" items={row?.items ?? null} {error} />
