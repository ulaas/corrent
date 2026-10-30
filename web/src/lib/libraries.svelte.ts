// The libraries the user can see, kept fresh by `library` events; scan progress per library.
import { api } from './api';
import { events } from './events.svelte';
import type { Library, MetadataRun, ScanStatus } from './types';

export const libs = $state<{
  list: Library[];
  loaded: boolean;
  scans: Record<number, ScanStatus>;
  /** Each library's running or last metadata refresh. */
  metadata: Record<number, MetadataRun>;
  changed: number;
}>({
  list: [],
  loaded: false,
  scans: {},
  metadata: {},
  changed: 0,
});

let started = false;
let pending: ReturnType<typeof setTimeout> | null = null;

export async function loadLibraries() {
  try {
    libs.list = await api.get<Library[]>('/libraries');
  } catch {
    /* signed out or offline */
  }
  libs.loaded = true;
}

export function startLibraries() {
  if (started) return;
  started = true;
  events.subscribe({ topic: 'library' });
  events.on((e) => {
    if (e.t === 'scan_status') {
      libs.scans[e.status.library_id] = e.status;
      if (!e.status.running) refreshSoon();
    } else if (e.t === 'metadata_status') {
      libs.metadata[e.status.library_id] = e.status;
    } else if (e.t === 'library_changed') {
      libs.changed++;
      refreshSoon();
    }
  });
  void loadLibraries();
}

function refreshSoon() {
  if (pending) return;
  pending = setTimeout(() => {
    pending = null;
    void loadLibraries();
  }, 800);
}

export function library(id: number): Library | undefined {
  return libs.list.find((l) => l.id === id);
}
