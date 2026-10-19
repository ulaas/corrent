import { tk } from './i18n.svelte';
import type { Role } from './types';

export const ROLE_LABEL: Record<Role, string> = {
  admin: tk('Administrator'),
  user: tk('User'),
  guest: tk('Guest'),
  kids: tk('Kids'),
};

export const ROLE_HINT: Record<Role, string> = {
  admin: tk('Everything, including users and server settings.'),
  user: tk('Plays media, adds and manages their own torrents.'),
  guest: tk('Plays media only.'),
  kids: tk('Plays media from allowed libraries only.'),
};

export const PERM_LABELS: { bit: number; label: string }[] = [
  { bit: 1, label: tk('Add torrents') },
  { bit: 1 << 1, label: tk('Manage own torrents') },
  { bit: 1 << 2, label: tk('Manage all torrents') },
  { bit: 1 << 3, label: tk('Manage libraries') },
  { bit: 1 << 4, label: tk('Transcode media') },
  { bit: 1 << 5, label: tk('See server settings') },
  { bit: 1 << 6, label: tk('Change server settings') },
  { bit: 1 << 7, label: tk('Manage users') },
  { bit: 1 << 8, label: tk('Manage plugins') },
  { bit: 1 << 9, label: tk('qBittorrent API (Sonarr/Radarr)') },
];

/** Default permissions of each role (corrent_proto::auth::Role::default_perms). */
export function defaultPerms(role: Role): number {
  switch (role) {
    case 'admin':
      return (1 << 10) - 1;
    case 'user':
      return 1 | (1 << 1) | (1 << 4) | (1 << 5);
    default:
      return 1 << 4;
  }
}
