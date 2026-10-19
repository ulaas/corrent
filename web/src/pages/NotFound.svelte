<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import { link } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';
</script>

<div class="page">
  <div class="empty">
    <div class="empty-icon"><Icon name="warning-circle" size={30} /></div>
    <h3>{t('Page not found')}</h3>
    <a class="btn primary" href="/" use:link><Icon name="house" />{t('Go home')}</a>
  </div>
</div>
