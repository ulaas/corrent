<script lang="ts">
  import { ICONS, type IconName } from '../lib/icons';

  let { name, size = 18, class: cls = '' }: { name: IconName; size?: number; class?: string } = $props();
</script>

<svg
  class="icon {cls}"
  viewBox="0 0 256 256"
  width={size}
  height={size}
  fill="currentColor"
  aria-hidden="true"
  focusable="false">{@html ICONS[name]}</svg>

<style>
  .icon {
    flex: none;
    display: inline-block;
    vertical-align: middle;
  }
</style>
