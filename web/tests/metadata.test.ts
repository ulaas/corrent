import { describe, expect, it } from 'vitest';
import { identifying, outcomeText, runLines, testText } from '../src/lib/metadata';
import type { MetadataRun, PluginInfo } from '../src/lib/types';

const run = (r: Partial<MetadataRun>): MetadataRun => ({
  library_id: 1,
  running: false,
  mode: 'missing',
  done: 0,
  total: 0,
  matched: 0,
  unmatched: 0,
  no_providers: false,
  errors: [],
  started_at: 1,
  ...r,
});

describe('metadata refresh texts', () => {
  it('says what an item refresh came to', () => {
    expect(outcomeText({ provider: 'tmdb', no_providers: false, errors: [] })).toEqual(['success', 'Details updated from tmdb.']);
    expect(outcomeText({ no_providers: true, errors: [] })[0]).toBe('warning');
    expect(outcomeText({ no_providers: false, errors: [{ provider: 'tmdb', error: 'TMDB refused the API key' }] })).toEqual([
      'error',
      'tmdb failed: TMDB refused the API key',
    ]);
    expect(outcomeText({ no_providers: false, errors: [] })[0]).toBe('info');
  });

  it('shows progress, the result and every provider error', () => {
    expect(runLines(run({ running: true, done: 2, total: 5 }))).toEqual([['muted', 'Looking up metadata… 2 of 5']]);
    const done = runLines(run({ finished_at: 2, total: 3, matched: 1, unmatched: 2, errors: [{ provider: 'omdb', error: 'Invalid API key!' }] }));
    expect(done).toEqual([
      ['muted', 'Metadata: 1 identified, 2 without a clear match.'],
      ['error', 'omdb failed: Invalid API key!'],
    ]);
    expect(runLines(run({}))).toEqual([]);
  });

  it('words provider tests', () => {
    const base = { query: 'The Matrix (1999)', artwork: 0, credits: 0, results: 1 };
    expect(testText({ ...base, ok: true, found: 'The Matrix', year: 1999, artwork: 3, credits: 10 })).toEqual([
      true,
      'Works: looked up The Matrix (1999), found “The Matrix (1999)”. 3 images, 10 people.',
    ]);
    expect(testText({ ...base, ok: false, error: '401' })).toEqual([false, 'Test failed: 401']);
    expect(testText({ ...base, ok: false, results: 0 })[1]).toContain('found nothing');
    expect(testText({ failed: 'offline' })).toEqual([false, 'Test failed: offline']);
  });

  it('lists only ready providers that identify titles', () => {
    const p = (id: string, extra: Partial<PluginInfo> = {}, supplements = false): PluginInfo => ({
      manifest: { id, name: id.toUpperCase(), version: '1', description: '', author: '', kinds: ['metadata'], permissions: { hosts: [], allow_local_network: false }, settings: [], supplements },
      enabled: true,
      builtin: true,
      settings: {},
      missing: [],
      failures: 0,
      ...extra,
    });
    expect(identifying([p('tmdb', { missing: ['api_key'] }), p('omdb', {}, true), p('musicbrainz'), p('tvdb', { enabled: false })])).toEqual(['MUSICBRAINZ']);
  });
});
