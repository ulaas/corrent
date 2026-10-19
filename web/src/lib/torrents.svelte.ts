// The live torrent list (WebSocket `torrents` topic) and session statistics (`session`).
import { applyDelta, normalize } from './delta';
import { events } from './events.svelte';
import { t } from './i18n.svelte';
import { toast } from './toast.svelte';
import { api } from './api';
import { can } from './session.svelte';
import { Perm, type Labels, type SessionStats, type Torrent } from './types';

const map = new Map<string, Torrent>();

export const live = $state<{
  list: Torrent[];
  labels: Labels;
  stats: SessionStats | null;
  loaded: boolean;
}>({ list: [], labels: { categories: {}, tags: [] }, stats: null, loaded: false });

let users = 0;
let unsub: (() => void) | null = null;
let releaseTimer: ReturnType<typeof setTimeout> | null = null;
let wired = false;

function publish() {
  live.list = [...map.values()];
}

function wire() {
  if (wired) return;
  wired = true;
  events.subscribe({ topic: 'session' });
  events.on((e) => {
    switch (e.t) {
      case 'torrents_full':
        map.clear();
        for (const x of e.torrents) map.set(x.hash, normalize(x));
        live.labels = e.labels;
        live.loaded = true;
        publish();
        break;
      case 'torrents_delta':
        applyDelta(map, e.delta);
        publish();
        break;
      case 'labels':
        live.labels = e.labels;
        break;
      case 'session':
        live.stats = e.stats;
        break;
      case 'added':
        toast(t('Added “{name}”', { name: e.name }), 'success');
        break;
      case 'completed':
        toast(t('“{name}” finished downloading', { name: e.name }), 'success');
        break;
      case 'removed':
        if (e.undoable && can(Perm.TORRENTS_MANAGE_ALL))
          toast(t('Removed “{name}”', { name: e.name }), 'info', {
            label: t('Undo'),
            run: () => void api.post('/torrents/actions', { action: 'undo_remove', hashes: [] }),
          });
        break;
      case 'notice':
        toast(e.text, e.severity === 'warning' ? 'warning' : e.severity);
        break;
    }
  });
}

/** Starts the session feed (call once signed in). */
export function startSessionFeed() {
  wire();
}

/** Keeps the torrent list live while the returned function isn't called. */
export function useTorrents(): () => void {
  wire();
  if (releaseTimer) {
    clearTimeout(releaseTimer);
    releaseTimer = null;
  }
  if (users++ === 0 && !unsub) unsub = events.subscribe({ topic: 'torrents' });
  return () => {
    if (--users > 0) return;
    // Keep it a little while: people often come straight back.
    releaseTimer = setTimeout(() => {
      releaseTimer = null;
      if (users === 0 && unsub) {
        unsub();
        unsub = null;
        live.loaded = false;
      }
    }, 30_000);
  };
}
