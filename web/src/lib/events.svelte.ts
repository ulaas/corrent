// The realtime channel (`/api/v1/events`): one WebSocket with reference-counted topic
// subscriptions, ordered delivery (compressed binary frames are inflated in order), keep-alive
// pings and reconnects with backoff.
import type { ServerEvent, Topic } from './types';

type Listener = (e: ServerEvent) => void;

const key = (t: Topic) => ('arg' in t ? `${t.topic}:${t.arg}` : t.topic);

export const link = $state({ connected: false, everConnected: false, attempts: 0 });

class Events {
  private ws: WebSocket | null = null;
  private topics = new Map<string, { topic: Topic; count: number }>();
  private listeners = new Set<Listener>();
  private chain: Promise<void> = Promise.resolve();
  private retry = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private ping: ReturnType<typeof setInterval> | null = null;
  private wanted = false;
  private pingId = 0;

  start() {
    this.wanted = true;
    if (!this.ws) this.connect();
  }

  stop() {
    this.wanted = false;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.ws?.close(1000);
    this.ws = null;
    link.connected = false;
  }

  /** Subscribes to a topic while the returned function is not called. */
  subscribe(topic: Topic): () => void {
    const k = key(topic);
    const cur = this.topics.get(k);
    if (cur) cur.count++;
    else {
      this.topics.set(k, { topic, count: 1 });
      this.send({ t: 'subscribe', topics: [topic] });
    }
    let done = false;
    return () => {
      if (done) return;
      done = true;
      const c = this.topics.get(k);
      if (!c) return;
      if (--c.count <= 0) {
        this.topics.delete(k);
        this.send({ t: 'unsubscribe', topics: [topic] });
      }
    };
  }

  on(f: Listener): () => void {
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }

  private send(msg: unknown) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  private connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}/api/v1/events?rate_ms=1000`);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    ws.onopen = () => {
      this.retry = 0;
      link.connected = true;
      link.everConnected = true;
      link.attempts = 0;
      const topics = [...this.topics.values()].map((t) => t.topic);
      if (topics.length) this.send({ t: 'subscribe', topics });
      this.ping = setInterval(() => this.send({ t: 'ping', id: ++this.pingId }), 30_000);
    };
    ws.onmessage = (m) => {
      const data = m.data;
      // Keep the order: a compressed frame takes a moment to inflate.
      this.chain = this.chain
        .then(async () => {
          const text = typeof data === 'string' ? data : await inflate(data as ArrayBuffer);
          this.dispatch(JSON.parse(text) as ServerEvent);
        })
        .catch((e) => console.warn('events: bad message', e));
    };
    ws.onclose = () => {
      if (this.ping) clearInterval(this.ping);
      this.ping = null;
      if (this.ws === ws) this.ws = null;
      link.connected = false;
      if (!this.wanted) return;
      const delay = Math.min(30_000, 500 * 2 ** this.retry) * (0.75 + Math.random() * 0.5);
      this.retry = Math.min(this.retry + 1, 6);
      link.attempts++;
      this.timer = setTimeout(() => {
        this.timer = null;
        if (this.wanted) this.connect();
      }, delay);
    };
  }

  private dispatch(e: ServerEvent) {
    if (e.t === 'bye') this.ws?.close();
    for (const f of this.listeners) {
      try {
        f(e);
      } catch (err) {
        console.error('events listener', err);
      }
    }
  }
}

/** zlib (RFC 1950) → text. */
export async function inflate(buf: ArrayBuffer): Promise<string> {
  const stream = new Blob([buf]).stream().pipeThrough(new DecompressionStream('deflate'));
  return await new Response(stream).text();
}

export const events = new Events();
