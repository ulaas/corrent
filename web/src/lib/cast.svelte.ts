// Casting from the web player: the server finds Chromecasts on its network and plays the item
// on one; this side picks the device, follows its status and drives it.
import { everyVisible } from './visibility.svelte';
import { api, ApiError, errorText } from './api';
import { t } from './i18n.svelte';
import type { CastConfig, CastDevice, CastStatus } from './types';

/** A failed cast request, for people: the server's reason, since a code's general text would
 *  mislead where the same code means several things. Permissions still read as permissions. */
export function castErrorText(e: unknown): string {
  if (!(e instanceof ApiError) || e.code === 'forbidden' || !e.detail) return errorText(e);
  return e.detail === 'casting is turned off in the settings' ? t('Casting is turned off in the settings.') : e.detail;
}

export function castDevices(refresh = false): Promise<CastDevice[]> {
  return api.get<CastDevice[]>(`/cast/devices${refresh ? '?refresh=true' : ''}`);
}

/** What is casting that the signed-in user could control; `mine`: only what they started. */
export function castsActive(mine = false): Promise<CastStatus[]> {
  return api.get<CastStatus[]>(`/cast/active${mine ? '?mine=true' : ''}`);
}

export const castConfig = () => api.get<CastConfig>('/cast/config');
export const saveCastConfig = (c: CastConfig) => api.put<CastConfig>('/cast/config', c);
export const stopCastOn = (deviceId: string) =>
  api.post(`/cast/${encodeURIComponent(deviceId)}/command`, { action: 'stop' });

/** Volume changes sent while dragging, at most this often. */
const VOLUME_EVERY = 150;
/** How long a volume set here is shown before the device's own report takes over. */
const VOLUME_SHOWN = 1500;

export class Cast {
  status = $state<CastStatus | null>(null);
  /** The volume last set here, until the device has caught up. */
  volumeSet = $state<number | null>(null);
  private stop: (() => void) | null = null;
  private volumeTimer: ReturnType<typeof setTimeout> | null = null;
  private volumeClear: ReturnType<typeof setTimeout> | null = null;
  private volumeAt = 0;
  /** Bumped by every play request: answers to older polls are dropped. */
  private generation = 0;
  private starting = 0;
  private closed = false;

  constructor(readonly device: CastDevice) {}

  /** Follows a cast that is already running. */
  attach(status: CastStatus) {
    this.status = status;
    this.stop ??= everyVisible(() => void this.refresh(), 1000);
  }

  async start(itemId: number, positionMs: number, audioTrack?: number | null, subtitle?: number | null) {
    const before = this.status;
    this.generation++;
    this.starting++;
    // At once: from here on the status is about this item, not how the one before ended.
    if (before) this.status = { ...before, state: 'loading', item_id: itemId, finished: false, error: undefined };
    try {
      const s = await api.post<CastStatus>(`/cast/${encodeURIComponent(this.device.id)}/play`, {
        item_id: itemId,
        position_ms: Math.round(positionMs),
        audio_track: audioTrack ?? null,
        subtitle: subtitle ?? null,
      });
      if (this.closed) return;
      this.generation++;
      this.status = s;
      this.stop ??= everyVisible(() => void this.refresh(), 1000);
    } catch (e) {
      if (!this.closed) this.status = before;
      throw e;
    } finally {
      this.starting--;
    }
  }

  async refresh() {
    if (this.starting || this.closed) return;
    const at = this.generation;
    try {
      const s = await api.get<CastStatus>(`/cast/${encodeURIComponent(this.device.id)}`);
      if (at === this.generation) this.status = s;
    } catch {
      // Gone (the server restarted): keep the last status.
    }
  }

  /** The device's position in seconds (between polls it moves on while playing). */
  position(): number {
    return (this.status?.position_ms ?? 0) / 1000;
  }

  /** The device's volume, 0 to 1. */
  volume(): number | null {
    return this.volumeSet ?? this.status?.volume ?? null;
  }

  setVolume(level: number) {
    // Its level isn't ours to change: the slider would only jump back.
    if (this.status?.volume_fixed) return;
    level = Math.max(0, Math.min(1, level));
    this.volumeSet = level;
    if (this.volumeClear) clearTimeout(this.volumeClear);
    this.volumeClear = setTimeout(() => (this.volumeSet = null), VOLUME_SHOWN);
    if (this.volumeTimer) clearTimeout(this.volumeTimer);
    const wait = this.volumeAt + VOLUME_EVERY - Date.now();
    this.volumeTimer = setTimeout(() => {
      this.volumeAt = Date.now();
      void this.post({ action: 'volume', level }).catch(() => {});
    }, Math.max(0, wait));
  }

  send(action: 'play' | 'pause' | 'stop'): Promise<unknown>;
  send(action: 'seek', positionS: number): Promise<unknown>;
  send(action: 'mute', muted: boolean): Promise<unknown>;
  send(action: string, value?: number | boolean) {
    if (action === 'seek') return this.post({ action, position_ms: Math.round(((value as number) ?? 0) * 1000) });
    if (action === 'mute') return this.post({ action, muted: value });
    return this.post({ action });
  }

  private post(body: object) {
    return api.post(`/cast/${encodeURIComponent(this.device.id)}/command`, body).then(() => this.refresh());
  }

  close() {
    this.closed = true;
    this.stop?.();
    this.stop = null;
    if (this.volumeTimer) clearTimeout(this.volumeTimer);
    if (this.volumeClear) clearTimeout(this.volumeClear);
  }
}
