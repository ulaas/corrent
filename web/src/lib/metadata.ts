// Metadata refreshes and provider tests, in words (the same as the desktop app's).
import { t } from './i18n.svelte';
import type { ToastKind } from './toast.svelte';
import type { MetadataRun, PluginInfo, ProviderTest, RefreshOutcome } from './types';

const NO_PROVIDER = 'No metadata provider is set up. Add an API key in Settings → Metadata.';

/** An item's refresh, as a toast. */
export function outcomeText(o: RefreshOutcome): [ToastKind, string] {
  if (o.provider) return ['success', t('Details updated from {provider}.', { provider: o.provider })];
  if (o.no_providers) return ['warning', t(NO_PROVIDER)];
  const e = o.errors[0];
  if (e) return ['error', t('{provider} failed: {error}', { provider: e.provider, error: e.error })];
  return ['info', t('No provider found a clear match. Use Identify… to choose one.')];
}

/** A library run's status lines: [tone, text]. */
export function runLines(run: MetadataRun): ['muted' | 'warn' | 'error', string][] {
  const out: ['muted' | 'warn' | 'error', string][] = [];
  if (run.running) {
    out.push(['muted', run.total ? t('Looking up metadata… {done} of {total}', { done: run.done, total: run.total }) : t('Looking up metadata…')]);
  } else if (run.finished_at) {
    out.push([
      'muted',
      run.total
        ? t('Metadata: {matched} identified, {unmatched} without a clear match.', { matched: run.matched, unmatched: run.unmatched })
        : t('Metadata: nothing to look up.'),
    ]);
  }
  if (run.no_providers) out.push(['warn', t(NO_PROVIDER)]);
  for (const e of run.errors) out.push(['error', t('{provider} failed: {error}', { provider: e.provider, error: e.error })]);
  return out;
}

/** A provider test's answer: [worked, text]. */
export function testText(r: ProviderTest | { failed: string }): [boolean, string] {
  if ('failed' in r) return [false, t('Test failed: {error}', { error: r.failed })];
  if (r.ok) {
    const found = r.year ? `${r.found ?? ''} (${r.year})` : (r.found ?? '');
    let text = t('Works: looked up {query}, found “{found}”.', { query: r.query, found });
    if (r.artwork || r.credits) text += ' ' + t('{images} images, {people} people.', { images: r.artwork, people: r.credits });
    return [true, text];
  }
  if (r.error) return [false, t('Test failed: {error}', { error: r.error })];
  return [false, t('It answered, but found nothing for {query}.', { query: r.query })];
}

/** Ready providers that identify titles (supplements only add to their matches). */
export function identifying(list: PluginInfo[]): string[] {
  return list
    .filter((p) => p.manifest.kinds.includes('metadata') && !p.manifest.supplements && !p.manifest.id.startsWith('example'))
    .filter((p) => p.enabled && !p.missing.length)
    .map((p) => p.manifest.name);
}
