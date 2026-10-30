// HTTP client for /api/v1: JSON in and out, the session's CSRF token on every change, and
// ApiError bodies turned into translated messages.
import { t, tk } from './i18n.svelte';
import type { ApiErrorBody } from './types';

export const BASE = '/api/v1';

let csrf = '';
let onUnauthorized: (() => void) | null = null;

export function setCsrf(token: string | undefined | null) {
  csrf = token ?? '';
}

export function setUnauthorizedHandler(f: () => void) {
  onUnauthorized = f;
}

const CODE_TEXT: Record<string, string> = {
  bad_request: tk('The request was not valid.'),
  unauthorized: tk('Please sign in again.'),
  forbidden: tk("You don't have permission to do that."),
  not_found: tk('Not found.'),
  conflict: tk('That already exists.'),
  too_many_requests: tk('Too many attempts. Wait a moment and try again.'),
  payload_too_large: tk('That is too large to upload.'),
  remote_access_disabled: tk('Remote access is turned off on this server.'),
  invalid_credentials: tk('Wrong user name or password.'),
  account_locked: tk('Too many failed sign-ins. The account is locked for a while.'),
  pairing_code_invalid: tk('The pairing code is wrong or has expired.'),
  engine_unavailable: tk("The torrent engine isn't running."),
  timeout: tk('That took too long. Try again later.'),
  no_space: tk('The server is running out of disk space.'),
  internal: tk('Something went wrong on the server.'),
};

export class ApiError extends Error {
  code: string;
  status: number;
  detail: string;
  params: Record<string, string>;
  constructor(status: number, body: ApiErrorBody | null) {
    const code = body?.code ?? (status === 0 ? 'network' : 'internal');
    super(body?.message ?? `HTTP ${status}`);
    this.code = code;
    this.status = status;
    this.detail = body?.message ?? '';
    this.params = body?.params ?? {};
  }
  /** Text for people, in the current language. */
  get text(): string {
    if (this.code === 'network') return t("Can't reach the server.");
    const base = CODE_TEXT[this.code];
    return base ? t(base) : this.detail || t('Something went wrong on the server.');
  }
}

/** A message for any thrown value. */
export function errorText(e: unknown): string {
  if (e instanceof ApiError) {
    // Bad requests carry useful English details (e.g. a rule's regex error).
    if (e.code === 'bad_request' && e.detail) return `${e.text} ${e.detail}`;
    return e.text;
  }
  if (e instanceof DOMException && e.name === 'AbortError') return t('Cancelled.');
  return e instanceof Error ? e.message : String(e);
}

type Query = Record<string, string | number | boolean | null | undefined>;

export function url(path: string, query?: Query): string {
  const q = new URLSearchParams();
  if (query) {
    for (const [k, v] of Object.entries(query)) if (v !== undefined && v !== null && v !== '') q.set(k, String(v));
  }
  const s = q.toString();
  return BASE + path + (s ? `?${s}` : '');
}

export interface RequestOptions {
  query?: Query;
  body?: unknown;
  signal?: AbortSignal;
  /** Don't treat 401 as "signed out" (sign-in itself). */
  quiet401?: boolean;
  /** Finish even when the page closes (leaving a room). */
  keepalive?: boolean;
}

async function request<T>(method: string, path: string, o: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  let body: BodyInit | undefined;
  if (o.body instanceof FormData) {
    body = o.body;
  } else if (o.body instanceof Blob) {
    headers['Content-Type'] = o.body.type || 'application/octet-stream';
    body = o.body;
  } else if (o.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    body = JSON.stringify(o.body);
  }
  if (method !== 'GET' && csrf) headers['X-Corrent-CSRF'] = csrf;
  let res: Response;
  try {
    res = await fetch(url(path, o.query), { method, headers, body, signal: o.signal, credentials: 'same-origin', keepalive: o.keepalive });
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') throw e;
    throw new ApiError(0, null);
  }
  if (!res.ok) {
    let err: ApiErrorBody | null = null;
    try {
      err = await res.json();
    } catch {
      /* not JSON */
    }
    const e = new ApiError(res.status, err);
    if (res.status === 401 && !o.quiet401 && e.code === 'unauthorized') onUnauthorized?.();
    throw e;
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export const api = {
  get: <T>(path: string, query?: Query, signal?: AbortSignal) => request<T>('GET', path, { query, signal }),
  post: <T>(path: string, body?: unknown, o?: RequestOptions) => request<T>('POST', path, { ...o, body }),
  put: <T>(path: string, body?: unknown) => request<T>('PUT', path, { body }),
  patch: <T>(path: string, body?: unknown) => request<T>('PATCH', path, { body }),
  del: <T>(path: string, o?: RequestOptions) => request<T>('DELETE', path, { ...o }),
};

/** Artwork URL for a width bucket the server caches (200, 400, 800); none = the original. */
export function imageUrl(sha: string, w?: 200 | 400 | 800): string {
  return `${BASE}/images/${sha}` + (w ? `?w=${w}` : '');
}

export function itemFileUrl(id: number): string {
  return `${BASE}/items/${id}/file`;
}

export function fileUrl(id: number): string {
  return `${BASE}/files/${id}`;
}
