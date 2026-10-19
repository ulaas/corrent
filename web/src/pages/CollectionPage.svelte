<script lang="ts">
  import PlaylistPage from './PlaylistPage.svelte';
  let { params }: { params: Record<string, string> } = $props();
</script>

<PlaylistPage {params} collection />
