// Watching together: a room on the server shares play, pause and seek. This side sends what the
// person does and follows what the room says (with the server's clock, so late joiners and slow
// networks line up within about half a second).
import { api } from './api';
import { events } from './events.svelte';
import type { RoomState } from './types';

/** Positions this far apart are corrected with a seek. */
const DRIFT_S = 0.75;

export class SyncRoom {
  state = $state<RoomState | null>(null);
  private offset = 0;
  /** Set while this side applies the room's state (those changes aren't sent back). */
  private applying = false;
  private stop: (() => void)[] = [];

  constructor(
    readonly id: string,
    private video: () => HTMLVideoElement | undefined,
  ) {}

  static async create(itemId: number, positionMs: number, video: () => HTMLVideoElement | undefined): Promise<SyncRoom> {
    const paused = video()?.paused ?? true;
    const s = await api.post<RoomState>('/syncplay', { item_id: itemId, position_ms: Math.round(positionMs), paused });
    const r = new SyncRoom(s.id, video);
    r.listen();
    r.adopt(s, 0);
    return r;
  }

  static async join(id: string, video: () => HTMLVideoElement | undefined): Promise<SyncRoom> {
    const r = new SyncRoom(id, video);
    const sent = Date.now();
    const s = await api.post<RoomState>(`/syncplay/${encodeURIComponent(id)}/join`);
    r.listen();
    r.adopt(s, (Date.now() - sent) / 2);
    return r;
  }

  private listen() {
    this.stop.push(events.subscribe({ topic: 'room', arg: this.id }));
    this.stop.push(
      events.on((e) => {
        if (e.t === 'sync_play' && e.room.id === this.id) this.adopt(e.room, 0);
      }),
    );
  }

  /** `latency`: half the round trip of the request that brought `s`. */
  private adopt(s: RoomState, latency: number) {
    this.offset = s.server_now_ms + latency - Date.now();
    this.state = s;
    this.apply();
  }

  /** Where the room is now (seconds). */
  position(): number {
    const s = this.state;
    if (!s) return 0;
    const now = Date.now() + this.offset;
    return (s.paused ? s.position_ms : s.position_ms + Math.max(0, now - s.at_ms)) / 1000;
  }

  apply() {
    const v = this.video();
    const s = this.state;
    if (!v || !s) return;
    this.applying = true;
    const target = this.position();
    if (Math.abs(v.currentTime - target) > DRIFT_S) v.currentTime = target;
    if (s.paused && !v.paused) v.pause();
    if (!s.paused && v.paused) void v.play().catch(() => {});
    setTimeout(() => (this.applying = false), 400);
  }

  /** The person played, paused or seeked here. */
  send(action: 'play' | 'pause' | 'seek', positionS: number) {
    if (this.applying) return;
    // Pressing play while the room plays (autoplay was blocked here): catch up, don't rewind
    // everyone to this player's stale position.
    if (action === 'play' && this.state && !this.state.paused) {
      this.apply();
      return;
    }
    void api.post(`/syncplay/${encodeURIComponent(this.id)}/command`, { action, position_ms: Math.round(positionS * 1000) }).catch(() => {});
  }

  leave() {
    if (this.left) return;
    this.left = true;
    for (const f of this.stop) f();
    this.stop = [];
    void api.post(`/syncplay/${encodeURIComponent(this.id)}/leave`, undefined, { keepalive: true }).catch(() => {});
  }
  private left = false;
}
