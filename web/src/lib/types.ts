// Wire types: exactly serde's JSON for crates/corrent-proto (and the core types the API returns).

export type TorrentState =
  | 'metadata'
  | 'checking'
  | 'downloading'
  | 'stalled'
  | 'seeding'
  | 'completed'
  | 'paused'
  | 'queued'
  | 'error';

export interface Torrent {
  hash: string;
  name: string;
  state: TorrentState;
  error?: string;
  size: number;
  done: number;
  progress: number;
  uploaded: number;
  downloaded: number;
  down_bps: number;
  up_bps: number;
  eta_secs?: number | null;
  ratio: number;
  peers_live: number;
  peers_seen: number;
  peers_connecting: number;
  save_path: string;
  content_path: string;
  category?: string;
  tags: string[];
  added_by?: string;
  added_at: number;
  completed_at?: number;
  queue_pos: number;
  trackers: string[];
  has_metadata: boolean;
  file_count: number;
  piece_count: number;
  piece_size: number;
  comment?: string;
  created_by?: string;
  creation_date?: number;
  busy?: string;
  last_activity: number;
  seeding_secs: number;
  down_limit_kib: number;
  up_limit_kib: number;
  pending: boolean;
  restore_failed: boolean;
}

/** Fast-changing fields of one torrent; `eta_secs: null` clears the ETA. */
export type TorrentPatch = { hash: string } & Partial<
  Pick<
    Torrent,
    | 'state'
    | 'done'
    | 'progress'
    | 'uploaded'
    | 'downloaded'
    | 'down_bps'
    | 'up_bps'
    | 'ratio'
    | 'peers_live'
    | 'peers_seen'
    | 'peers_connecting'
    | 'last_activity'
    | 'seeding_secs'
    | 'queue_pos'
  >
> & { eta_secs?: number | null };

export interface TorrentsDelta {
  upserted?: Torrent[];
  patched?: TorrentPatch[];
  removed?: string[];
}

export interface FileEntry {
  index: number;
  path: string[];
  len: number;
  done: number;
  included: boolean;
  priority: number;
}

export interface Peer {
  addr: string;
  client?: string;
  kind: string;
  state: string;
  downloaded: number;
  uploaded: number;
  down_bps: number;
  up_bps: number;
}

export interface TorrentDetail {
  hash: string;
  files: FileEntry[];
  peers: Peer[];
  pieces?: number[];
  piece_count: number;
}

export interface Category {
  save_path?: string;
  down_limit_kib: number;
  up_limit_kib: number;
}

export interface Labels {
  categories: Record<string, Category>;
  tags: string[];
}

export type EngineStatus =
  | { state: 'starting' }
  | { state: 'running' }
  | { state: 'restarting' }
  | { state: 'failed'; error: string };

export interface SessionStats {
  engine: EngineStatus;
  down_bps: number;
  up_bps: number;
  dht_nodes?: number;
  listen_port?: number;
  peers_live: number;
  alt_speed: boolean;
  uptime_secs: number;
  session_downloaded: number;
  session_uploaded: number;
  all_time_downloaded: number;
  all_time_uploaded: number;
  free_space?: number;
  warning?: string;
  busy_ops: number;
  torrent_count: number;
}

export type Topic =
  | { topic: 'torrents' }
  | { topic: 'torrent'; arg: string }
  | { topic: 'session' }
  | { topic: 'log' }
  | { topic: 'library' }
  | { topic: 'room'; arg: string };

export type Severity = 'info' | 'success' | 'warning' | 'error';

export type ServerEvent =
  | { t: 'hello'; server_id: string; user: string }
  | { t: 'pong'; id: number }
  | { t: 'torrents_full'; torrents: Torrent[]; labels: Labels }
  | { t: 'torrents_delta'; delta: TorrentsDelta }
  | { t: 'labels'; labels: Labels }
  | { t: 'session'; stats: SessionStats }
  | { t: 'torrent_detail'; detail: TorrentDetail }
  | { t: 'notice'; severity: Severity; text: string; code?: string }
  | { t: 'added'; hash: string; name: string }
  | { t: 'completed'; hash: string; name: string; category?: string }
  | { t: 'removed'; hash: string; name: string; undoable: boolean }
  | { t: 'log'; level: string; target: string; message: string; time: number }
  | { t: 'library_changed'; library_id: number }
  | { t: 'scan_status'; status: ScanStatus }
  | { t: 'sync_play'; room: RoomState }
  | { t: 'bye' };

// ---- auth ----

export type Role = 'admin' | 'user' | 'guest' | 'kids';

export const Perm = {
  TORRENTS_ADD: 1,
  TORRENTS_MANAGE_OWN: 1 << 1,
  TORRENTS_MANAGE_ALL: 1 << 2,
  LIBRARY_MANAGE: 1 << 3,
  PLAYBACK_TRANSCODE: 1 << 4,
  SETTINGS_READ: 1 << 5,
  SETTINGS_WRITE: 1 << 6,
  USERS_MANAGE: 1 << 7,
  PLUGINS_MANAGE: 1 << 8,
  DOWNLOADER: 1 << 9,
} as const;

export interface User {
  id: number;
  name: string;
  display_name: string;
  role: Role;
  perms: number;
  disabled: boolean;
  max_rating?: string;
  created_at: number;
}

export interface LoginResponse {
  user: User;
  token?: string;
  csrf?: string;
  expires_at: number;
}

export interface Device {
  id: number;
  user_id: number;
  name: string;
  platform: string;
  client: string;
  created_at: number;
  last_seen: number;
  last_ip: string;
  current: boolean;
}

export interface PairingCode {
  code: string;
  expires_at: number;
  link: string;
  qr_svg?: string;
}

export interface RemoteConfig {
  remote_access: boolean;
  discovery: boolean;
  connect: boolean;
  connect_relay: boolean;
  port_mapping: boolean;
  dlna: boolean;
  dlna_user: string;
  allowed_networks: string[];
  trusted_proxies: string[];
  hostnames: string[];
}

export interface RemoteStatus {
  https_port: number;
  discovery: boolean;
  discovery_error?: string;
  connect_id?: string;
  connect_relay?: string;
  connect_error?: string;
  mapped_address?: string;
  port_mapping_error?: string;
  dlna_port?: number;
  dlna_error?: string;
}

export interface RemoteAccess {
  config: RemoteConfig;
  status: RemoteStatus;
}

export interface PluginSettingDef {
  key: string;
  label: string;
  help: string;
  secret: boolean;
  required: boolean;
  default?: string | null;
}

export interface PluginInfo {
  manifest: {
    id: string;
    name: string;
    version: string;
    description: string;
    author: string;
    kinds: string[];
    permissions: { hosts: string[]; allow_local_network: boolean; rate_per_sec?: number | null };
    settings: PluginSettingDef[];
  };
  enabled: boolean;
  builtin: boolean;
  settings: Record<string, string>;
  missing: string[];
  failures: number;
}

export interface AuditEntry {
  id: number;
  ts: number;
  user?: string;
  ip: string;
  action: string;
  target: string;
  ok: boolean;
  details?: string;
}

// ---- server ----

export interface ServerInfo {
  id: string;
  name: string;
  version: string;
  api_version: number;
  tls_fingerprint?: string;
  setup_required: boolean;
  features: string[];
}

export interface FsEntry {
  name: string;
  path: string;
  dir: boolean;
}

export interface FsListing {
  path: string;
  parent?: string;
  entries: FsEntry[];
  free_space?: number;
}

export interface ApiErrorBody {
  code: string;
  message: string;
  params?: Record<string, string>;
}

// ---- library ----

export type LibraryKind = 'movies' | 'shows' | 'music' | 'audiobooks' | 'videos' | 'photos';
export type ItemKind =
  | 'movie'
  | 'show'
  | 'season'
  | 'episode'
  | 'artist'
  | 'album'
  | 'track'
  | 'book'
  | 'video'
  | 'photo'
  | 'folder';

export type ImportMode = 'in_place' | 'hardlink' | 'copy';

export interface LibrarySettings {
  import_categories: string[];
  import_mode: ImportMode;
  watch: boolean;
  fetch_metadata: boolean;
  language: string;
  played_threshold: number;
}

export interface Library {
  id: number;
  name: string;
  kind: LibraryKind;
  folders: string[];
  settings: LibrarySettings;
  item_count: number;
  scanned_at?: number;
  scanning?: number;
}

export interface ImageRef {
  kind: string;
  sha: string;
  width: number;
  height: number;
  blurhash?: string;
}

export interface UserState {
  position_ms: number;
  played: boolean;
  play_count: number;
  last_played_at?: number;
  favorite: boolean;
  rating?: number;
  unplayed?: number;
}

export interface Item {
  id: number;
  library_id: number;
  kind: ItemKind;
  parent_id?: number;
  title: string;
  year?: number;
  index?: number;
  runtime_ms?: number;
  context?: string[];
  images?: ImageRef[];
  state: UserState;
  added_at: number;
  child_count?: number;
  partial?: boolean;
}

export interface Stream {
  index: number;
  kind: string;
  codec?: string | null;
  profile?: string | null;
  lang?: string | null;
  title?: string | null;
  default: boolean;
  forced: boolean;
  width?: number | null;
  height?: number | null;
  fps?: number | null;
  bitrate?: number | null;
  channels?: number | null;
  sample_rate?: number | null;
  external: boolean;
}

export interface Chapter {
  start_ms: number;
  title?: string;
}

export interface MediaFile {
  id: number;
  path: string;
  size: number;
  container?: string;
  duration_ms?: number;
  role: string;
  streams: Stream[];
  chapters: Chapter[];
  torrent?: string;
  missing?: boolean;
}

export interface Credit {
  person_id: number;
  name: string;
  role: string;
  character?: string;
}

export interface ItemDetail extends Item {
  original_title?: string;
  overview?: string;
  rating?: number;
  official_rating?: string;
  premiered?: string;
  genres: string[];
  credits: Credit[];
  files: MediaFile[];
  provider_ids: Record<string, string>;
  locked_fields: string[];
  extra: Record<string, unknown>;
}

export interface Page<T> {
  items: T[];
  total: number;
  next?: number;
}

export type SortBy = 'title' | 'added' | 'year' | 'played' | 'index';

export interface HomeRow {
  id: string;
  title: string;
  items: Item[];
}

export interface SearchResults {
  items: Item[];
  people: Credit[];
}

export interface ScanStatus {
  library_id: number;
  running: boolean;
  phase: string;
  done: number;
  total: number;
  added: number;
  updated: number;
  removed: number;
  error?: string;
}

// ---- RSS and search (corrent-core) ----

export interface Article {
  guid: string;
  title: string;
  torrent?: string | null;
  page?: string | null;
  published?: number | null;
  size?: number | null;
}

export interface FeedView {
  url: string;
  title: string;
  articles: { article: Article; downloaded: boolean }[];
  last_ok?: number | null;
  error?: string | null;
  loading: boolean;
  enabled: boolean;
}

export interface RssView {
  feeds: FeedView[];
}

export interface FeedConfig {
  url: string;
  name: string;
  enabled: boolean;
}

export interface RssRule {
  name: string;
  enabled: boolean;
  feeds: string[];
  must_contain: string;
  must_not_contain: string;
  use_regex: boolean;
  episode_filter: string;
  smart_episodes: boolean;
  category: string | null;
  save_path: string | null;
  start_paused: boolean;
}

export interface RssSettings {
  auto_download: boolean;
  refresh_minutes: number;
  max_articles: number;
  feeds: FeedConfig[];
  rules: RssRule[];
}

export interface SearchResult {
  provider: string;
  title: string;
  link?: string | null;
  page?: string | null;
  size?: number | null;
  seeders?: number | null;
  peers?: number | null;
  published?: number | null;
  info_hash?: string | null;
  category?: string | null;
}

export interface SearchResponse {
  results: SearchResult[];
  errors: [string, string][];
}

export interface TorznabProvider {
  name: string;
  url: string;
  api_key: string;
  enabled: boolean;
}

/** The server's settings document (corrent-core `Settings`); only the parts the client edits are typed. */
export interface ServerSettings {
  downloads: {
    save_path: string;
    start_paused: boolean;
    create_subfolder: boolean;
    [k: string]: unknown;
  };
  connection: {
    listen_port: number;
    random_port: boolean;
    upnp: boolean;
    listen_mode: 'Tcp' | 'Utp' | 'TcpAndUtp';
    ipv4_only: boolean;
    listen_address: string;
    proxy_enabled: boolean;
    proxy_host: string;
    proxy_port: number;
    proxy_username: string;
    proxy_password: string;
    max_peers_global: number;
    max_peers_per_torrent: number;
    blocklist_url: string;
    connect_timeout_secs: number;
    [k: string]: unknown;
  };
  speed: {
    download_limit: number;
    upload_limit: number;
    alt_download_limit: number;
    alt_upload_limit: number;
    alt_enabled: boolean;
    schedule: { enabled: boolean; days: number; from_minutes: number; to_minutes: number };
  };
  bittorrent: {
    dht: boolean;
    lsd: boolean;
    trackers_enabled: boolean;
    default_trackers: string[];
    fastresume: boolean;
  };
  queue: {
    enabled: boolean;
    max_active_downloads: number;
    max_active_uploads: number;
    max_active_total: number;
    ignore_slow: boolean;
    slow_down_kib: number;
    slow_up_kib: number;
  };
  seeding: {
    ratio_enabled: boolean;
    ratio: number;
    time_enabled: boolean;
    time_minutes: number;
    inactive_enabled: boolean;
    inactive_minutes: number;
    action: 'Pause' | 'Remove' | 'RemoveWithData';
  };
  rss: RssSettings;
  search: { providers: TorznabProvider[] };
  [k: string]: unknown;
}

export interface TaskRun {
  id: number;
  task: string;
  started: number;
  finished?: number;
  ok?: boolean;
  message: string;
  trigger: string;
}

export interface TaskInfo {
  id: string;
  enabled: boolean;
  every_min: number;
  running: boolean;
  last?: TaskRun;
  next_at?: number;
}

export type NotifyKind = 'webhook' | 'discord' | 'ntfy' | 'plugin';

export interface NotifyTarget {
  id: number;
  name: string;
  kind: NotifyKind;
  url: string;
  plugin: string;
  events: string[];
  enabled: boolean;
}

export interface BackupFile {
  name: string;
  size: number;
  created: number;
}

/** A watch-together room. */
export interface RoomState {
  id: string;
  item_id: number;
  paused: boolean;
  position_ms: number;
  at_ms: number;
  by: string;
  members: string[];
  server_now_ms: number;
}

/** A Chromecast the server found. */
export interface CastDevice {
  id: string;
  name: string;
  model: string;
}

export interface CastStatus {
  device: CastDevice;
  state: 'connecting' | 'loading' | 'buffering' | 'playing' | 'paused' | 'idle' | 'error';
  position_ms: number;
  duration_ms?: number;
  item_id?: number;
  title: string;
  by: string;
  error?: string;
}
