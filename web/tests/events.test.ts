import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

class FakeSocket {
  static OPEN = 1;
  static all: FakeSocket[] = [];
  readyState = FakeSocket.OPEN;
  sent: string[] = [];
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((m: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  binaryType = '';
  constructor(public url: string) {
    FakeSocket.all.push(this);
  }
  send(s: string) {
    this.sent.push(s);
  }
  close() {
    // A half-open socket doesn't report back: onclose stays silent.
    this.closed = true;
  }
}

const listeners = new Map<string, () => void>();

async function fresh() {
  vi.resetModules();
  FakeSocket.all = [];
  listeners.clear();
  vi.stubGlobal('WebSocket', FakeSocket);
  vi.stubGlobal('location', { protocol: 'http:', host: 'x' });
  vi.stubGlobal('addEventListener', (k: string, f: () => void) => listeners.set(k, f));
  vi.stubGlobal('document', { visibilityState: 'visible', addEventListener: (k: string, f: () => void) => listeners.set(k, f) });
  return await import('../src/lib/events.svelte');
}

describe('the live link', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('gives up on a socket that stops answering pings and reconnects', async () => {
    const { events, link } = await fresh();
    events.start();
    FakeSocket.all[0].onopen?.();
    expect(link.connected).toBe(true);
    // Two pings go out unanswered; the third tick gives up.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeSocket.all[0].sent.filter((s) => s.includes('ping')).length).toBe(2);
    expect(FakeSocket.all[0].closed).toBe(false);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(FakeSocket.all[0].closed).toBe(true);
    expect(link.connected).toBe(false);
    await vi.advanceTimersByTimeAsync(1000);
    expect(FakeSocket.all.length).toBe(2);
    FakeSocket.all[1].onopen?.();
    expect(link.connected).toBe(true);
    expect(link.generation).toBe(1);
  });

  it('a pong keeps the socket', async () => {
    const { events } = await fresh();
    events.start();
    const ws = FakeSocket.all[0];
    ws.onopen?.();
    for (let i = 0; i < 5; i++) {
      await vi.advanceTimersByTimeAsync(30_000);
      ws.onmessage?.({ data: JSON.stringify({ t: 'pong', id: i }) });
    }
    expect(ws.closed).toBe(false);
  });

  it('coming back online skips the backoff', async () => {
    const { events } = await fresh();
    events.start();
    FakeSocket.all[0].onopen?.();
    FakeSocket.all[0].onclose?.();
    expect(FakeSocket.all.length).toBe(1);
    listeners.get('online')?.();
    expect(FakeSocket.all.length).toBe(2);
    // Only one connection: the backoff timer was cancelled.
    await vi.advanceTimersByTimeAsync(5000);
    expect(FakeSocket.all.length).toBe(2);
  });

  it('a socket that looks open is tested when the tab comes back', async () => {
    const { events, link } = await fresh();
    events.start();
    const ws = FakeSocket.all[0];
    ws.onopen?.();
    listeners.get('visibilitychange')?.();
    expect(ws.sent.some((s) => s.includes('ping'))).toBe(true);
    await vi.advanceTimersByTimeAsync(4100);
    expect(ws.closed).toBe(true);
    expect(link.connected).toBe(false);
  });

  it('the first connection is no reconnect, and reopen listeners run on later ones', async () => {
    const { events, link } = await fresh();
    let reopened = 0;
    events.onReopen(() => reopened++);
    events.start();
    FakeSocket.all[0].onopen?.();
    expect(link.generation).toBe(0);
    expect(reopened).toBe(0);
    FakeSocket.all[0].onclose?.();
    await vi.advanceTimersByTimeAsync(1000);
    FakeSocket.all[1].onopen?.();
    expect(link.generation).toBe(1);
    expect(reopened).toBe(1);
  });
});
