// Who is signed in, the server's identity, and what the user may do.
import { api, setCsrf, setUnauthorizedHandler } from './api';
import { events } from './events.svelte';
import { syncPrefs } from './prefs.svelte';
import { Perm, type LoginResponse, type ServerInfo, type User } from './types';

export const session = $state<{
  ready: boolean;
  info: ServerInfo | null;
  user: User | null;
  /** Set when the session ended on its own (so the sign-in page can say why). */
  expired: boolean;
}>({ ready: false, info: null, user: null, expired: false });

export function can(p: number): boolean {
  return !!session.user && (session.user.perms & p) === p;
}

export const isAdmin = () => can(Perm.SETTINGS_WRITE);

function signedIn(r: LoginResponse) {
  setCsrf(r.csrf);
  session.user = r.user;
  session.expired = false;
  events.start();
  void syncPrefs();
}

export async function boot(): Promise<void> {
  setUnauthorizedHandler(() => {
    if (session.user) {
      session.user = null;
      session.expired = true;
      events.stop();
    }
  });
  try {
    session.info = await api.get<ServerInfo>('/server/info');
  } catch {
    session.info = null;
  }
  if (session.info && !session.info.setup_required) {
    try {
      signedIn(await api.get<LoginResponse>('/auth/me'));
    } catch {
      session.user = null;
    }
  }
  session.ready = true;
}

export async function login(username: string, password: string) {
  signedIn(await api.post<LoginResponse>('/auth/login', { username, password }, { quiet401: true }));
}

export async function setup(name: string, display_name: string, password: string) {
  const r = await api.post<LoginResponse>('/auth/setup', { name, display_name, password, role: 'admin' });
  if (session.info) session.info.setup_required = false;
  signedIn(r);
}

export async function logout() {
  try {
    await api.post('/auth/logout');
  } catch {
    /* already gone */
  }
  events.stop();
  setCsrf('');
  session.user = null;
}

export async function refreshMe() {
  const r = await api.get<LoginResponse>('/auth/me');
  setCsrf(r.csrf);
  session.user = r.user;
}
