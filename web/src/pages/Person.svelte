<script lang="ts">
  // Everything a person is credited in (as the desktop's person page).
  import ItemGrid from '../components/ItemGrid.svelte';
  import { api, errorText } from '../lib/api';
  import { route } from '../lib/router.svelte';
  import type { Item, Page } from '../lib/types';

  let { params }: { params: Record<string, string> } = $props();
  let items = $state<Item[] | null>(null);
  let error = $state('');

  $effect(() => {
    const id = params.id;
    items = null;
    error = '';
    api.get<Page<Item>>('/items', { person_id: id, limit: 500 }).then(
      (p) => (items = p.items),
      (e) => (error = errorText(e)),
    );
  });
</script>

<ItemGrid title={route.query.get('name') ?? ''} icon="user" {items} {error} />
