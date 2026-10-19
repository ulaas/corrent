// Casting from the web player: the server finds Chromecasts on its network and plays the item
// on one; this side picks the device, follows its status and sends play/pause/seek/stop.
import { api } from './api';
import type { CastDevice, CastStatus } from './types';

export function castDevices(refresh = false): Promise<CastDevice[]> {
  return api.get<CastDevice[]>(`/cast/devices${refresh ? '?refresh=true' : ''}`);
}

export class Cast {
  status = $state<CastStatus | null>(null);
  private timer: ReturnType<typeof setInterval> | null = null;

  constructor(readonly device: CastDevice) {}

  async start(itemId: number, positionMs: number, audioTrack?: number | null) {
    this.status = await api.post<CastStatus>(`/cast/${encodeURIComponent(this.device.id)}/play`, {
      item_id: itemId,
      position_ms: Math.round(positionMs),
      audio_track: audioTrack ?? null,
    });
    this.timer ??= setInterval(() => void this.refresh(), 1000);
  }

  async refresh() {
    try {
      this.status = await api.get<CastStatus>(`/cast/${encodeURIComponent(this.device.id)}`);
    } catch {
      // Gone (the server restarted): keep the last status.
    }
  }

  /** The device's position in seconds (between polls it moves on while playing). */
  position(): number {
    return (this.status?.position_ms ?? 0) / 1000;
  }

  send(action: 'play' | 'pause' | 'stop'): Promise<unknown>;
  send(action: 'seek', positionS: number): Promise<unknown>;
  send(action: string, positionS?: number) {
    const body = action === 'seek' ? { action, position_ms: Math.round((positionS ?? 0) * 1000) } : { action };
    return api.post(`/cast/${encodeURIComponent(this.device.id)}/command`, body).then(() => this.refresh());
  }

  close() {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }
}
