//! Users, passwords, tokens, devices, pairing codes and the audit log, stored in `corrent.db`.
//!
//! Passwords are argon2id. Tokens are 32 random bytes shown once to their owner and stored only
//! as SHA-256. Web sessions slide for 12 h, device tokens for 90 days.

use std::sync::Arc;

use aws_lc_rs::{digest, rand};
use corrent_core::util::now_unix;
use corrent_db::rusqlite::{self, OptionalExtension, params};
use corrent_db::{Db, DbError};
use corrent_proto::auth::{AuditEntry, Device, DeviceInfo, NewUser, PairingCode, Perms, Role, User, UserUpdate};
use corrent_proto::{ApiError, ErrorCode};

pub const WEB_SESSION_SECS: i64 = 12 * 3600;
pub const DEVICE_TOKEN_SECS: i64 = 90 * 24 * 3600;
const PAIRING_SECS: i64 = 10 * 60;
/// Wrong guesses (from anyone) before every outstanding code is burned. Each address also gets
/// only five (see `Limiter::may_pair`), so one client can't burn other people's codes.
const PAIRING_MAX_ATTEMPTS: i64 = 50;
const PAIRING_MAX_OUTSTANDING: i64 = 3;
const LOCKOUT_AFTER: i64 = 10;
const LOCKOUT_SECS: i64 = 15 * 60;
const AUDIT_KEEP: i64 = 20_000;
/// Only refresh `last_seen`/expiry once a minute per token (reads stay reads).
const TOUCH_EVERY: i64 = 60;

/// The request for a new administrator (CLI setup).
pub fn new_admin(name: &str, password: &str) -> NewUser {
    NewUser {
        name: name.to_string(),
        display_name: String::new(),
        password: password.to_string(),
        role: Role::Admin,
        perms: None,
    }
}

/// argon2id cost. `STANDARD` for real use; tests use `FAST`.
#[derive(Clone, Copy, Debug)]
pub struct HashCost {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl HashCost {
    pub const STANDARD: HashCost = HashCost { m_kib: 64 * 1024, t: 3, p: 1 };
    pub const FAST: HashCost = HashCost { m_kib: 1024, t: 1, p: 1 };
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::fill(&mut b).expect("OS random generator");
    b
}

/// `argon2id$m=<kib>,t=<t>,p=<p>$<salt hex>$<hash hex>`.
pub fn hash_password(password: &str, cost: HashCost) -> String {
    let salt = random_bytes::<16>();
    let mut out = [0u8; 32];
    argon2_into(password, &salt, cost, &mut out);
    format!("argon2id$m={},t={},p={}${}${}", cost.m_kib, cost.t, cost.p, hex(&salt), hex(&out))
}

fn argon2_into(password: &str, salt: &[u8], cost: HashCost, out: &mut [u8]) -> bool {
    let Ok(params) = argon2::Params::new(cost.m_kib, cost.t, cost.p, Some(out.len())) else { return false };
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, out)
        .is_ok()
}

/// Verifies argon2id hashes and, for migrated qBittorrent-API accounts, the old PBKDF2 format.
pub fn verify_password(stored: &str, password: &str) -> bool {
    if stored.starts_with("pbkdf2-sha256$") {
        return crate::webapi::auth::verify_password(stored, password);
    }
    let mut parts = stored.split('$');
    let (Some("argon2id"), Some(params), Some(salt), Some(hash), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let mut cost = HashCost { m_kib: 0, t: 0, p: 0 };
    for kv in params.split(',') {
        match kv.split_once('=') {
            Some(("m", v)) => cost.m_kib = v.parse().unwrap_or(0),
            Some(("t", v)) => cost.t = v.parse().unwrap_or(0),
            Some(("p", v)) => cost.p = v.parse().unwrap_or(0),
            _ => return false,
        }
    }
    let (Some(salt), Some(hash)) = (unhex(salt), unhex(hash)) else { return false };
    if hash.is_empty() || hash.len() > 64 {
        return false;
    }
    let mut out = vec![0u8; hash.len()];
    argon2_into(password, &salt, cost, &mut out) && constant_eq(&out, &hash)
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn token_hash(token: &str) -> Vec<u8> {
    digest::digest(&digest::SHA256, token.as_bytes()).as_ref().to_vec()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Web,
    Device,
}

impl SessionKind {
    fn as_str(self) -> &'static str {
        match self {
            SessionKind::Web => "web",
            SessionKind::Device => "device",
        }
    }
}

/// Who is making a request (from a verified token).
#[derive(Clone, Debug)]
pub struct Principal {
    pub user: User,
    pub session_id: i64,
    pub device_id: Option<i64>,
    pub kind: SessionKind,
    pub csrf: String,
}

impl Principal {
    pub fn can(&self, p: Perms) -> bool {
        !self.user.disabled && self.user.perms.contains(p)
    }
}

/// A freshly issued token (the secret is only available here).
#[derive(Clone, Debug)]
pub struct Issued {
    pub token: String,
    pub csrf: String,
    pub expires_at: i64,
    pub device_id: Option<i64>,
}

fn db_err(e: DbError) -> ApiError {
    tracing::error!("identity database: {e}");
    ApiError::new(ErrorCode::Internal, e.to_string())
}

fn role_from(s: &str) -> Role {
    match s {
        "admin" => Role::Admin,
        "guest" => Role::Guest,
        "kids" => Role::Kids,
        _ => Role::User,
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::Admin => "admin",
        Role::User => "user",
        Role::Guest => "guest",
        Role::Kids => "kids",
    }
}

const USER_COLS: &str = "id, name, display_name, role, perms, disabled, max_rating, created_at";

fn user_row(r: &rusqlite::Row) -> rusqlite::Result<User> {
    let role = role_from(&r.get::<_, String>(3)?);
    let perms: Option<u32> = r.get(4)?;
    Ok(User {
        id: r.get(0)?,
        name: r.get(1)?,
        display_name: r.get(2)?,
        role,
        perms: perms.map(Perms).unwrap_or_else(|| role.default_perms()),
        disabled: r.get::<_, i64>(5)? != 0,
        max_rating: r.get(6)?,
        created_at: r.get(7)?,
    })
}

fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n.len() <= 64 && !n.chars().any(|c| c.is_control())
}

/// The identity store. Cheap to clone.
#[derive(Clone)]
pub struct Identity {
    db: Db,
    cost: HashCost,
    /// argon2 at 64 MiB per hash: at most two at once.
    hashing: Arc<tokio::sync::Semaphore>,
    /// A hash at the real cost, checked for unknown user names so their timing matches.
    dummy: Arc<tokio::sync::OnceCell<String>>,
}

impl Identity {
    pub fn new(db: Db, cost: HashCost) -> Identity {
        Identity { db, cost, hashing: Arc::new(tokio::sync::Semaphore::new(2)), dummy: Default::default() }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    async fn hash(&self, password: String) -> Result<String, ApiError> {
        let _permit = self.hashing.acquire().await;
        let cost = self.cost;
        tokio::task::spawn_blocking(move || hash_password(&password, cost))
            .await
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("hashing failed: {e}")))
    }

    async fn verify(&self, stored: String, password: String) -> bool {
        let _permit = self.hashing.acquire().await;
        tokio::task::spawn_blocking(move || verify_password(&stored, &password)).await.unwrap_or(false)
    }

    /// A stable random id for this server (created on first use).
    pub async fn server_id(&self) -> Result<String, ApiError> {
        let id = hex(&random_bytes::<12>());
        self.db
            .write(move |t| {
                t.execute("INSERT OR IGNORE INTO kv(key, value) VALUES ('server_id', ?1)", [&id])?;
                t.query_row("SELECT value FROM kv WHERE key = 'server_id'", [], |r| r.get(0))
            })
            .await
            .map_err(db_err)
    }

    pub async fn user_count(&self) -> Result<i64, ApiError> {
        self.db.read(|c| c.query_row("SELECT count(*) FROM users", [], |r| r.get(0))).await.map_err(db_err)
    }

    pub async fn create_user(&self, u: NewUser) -> Result<User, ApiError> {
        if !valid_name(&u.name) {
            return Err(ApiError::new(ErrorCode::BadRequest, "invalid user name"));
        }
        if u.password.len() < 8 {
            return Err(ApiError::new(ErrorCode::BadRequest, "password too short").with("min", 8));
        }
        let hash = self.hash(u.password).await?;
        self.insert_user(u.name, u.display_name, hash, u.role, u.perms).await
    }

    async fn insert_user(
        &self,
        name: String,
        display: String,
        pw_hash: String,
        role: Role,
        perms: Option<Perms>,
    ) -> Result<User, ApiError> {
        let now = now_unix();
        let name = name.trim().to_string();
        let r = self
            .db
            .write(move |t| {
                t.execute(
                    "INSERT INTO users(name, display_name, pw_hash, role, perms, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![name, display, pw_hash, role_str(role), perms.map(|p| p.0), now],
                )?;
                let id = t.last_insert_rowid();
                t.query_row(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"), [id], user_row)
            })
            .await;
        match r {
            Ok(u) => Ok(u),
            Err(DbError::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(ApiError::new(ErrorCode::Conflict, "user exists"))
            }
            Err(e) => Err(db_err(e)),
        }
    }

    /// First-run setup: creates the administrator only while no user exists (atomically, so
    /// two simultaneous setups can't make two admins).
    pub async fn create_first_admin(&self, mut u: NewUser) -> Result<User, ApiError> {
        if !valid_name(&u.name) {
            return Err(ApiError::new(ErrorCode::BadRequest, "invalid user name"));
        }
        if u.password.len() < 8 {
            return Err(ApiError::new(ErrorCode::BadRequest, "password too short").with("min", 8));
        }
        u.role = Role::Admin;
        let hash = self.hash(u.password.clone()).await?;
        let (name, display, now) = (u.name.trim().to_string(), u.display_name.clone(), now_unix());
        let id = self
            .db
            .write(move |t| {
                let n = t.execute(
                    "INSERT INTO users(name, display_name, pw_hash, role, created_at)                      SELECT ?1, ?2, ?3, 'admin', ?4 WHERE NOT EXISTS (SELECT 1 FROM users)",
                    params![name, display, hash, now],
                )?;
                Ok((n == 1).then(|| t.last_insert_rowid()))
            })
            .await
            .map_err(db_err)?
            .ok_or_else(|| ApiError::new(ErrorCode::Conflict, "already set up"))?;
        self.user(id).await
    }

    /// A device token for the desktop app hosting this server in-process: it signs in as the
    /// first administrator (creating "owner" with a random password when there is none) and
    /// replaces the token it issued at its previous start.
    pub async fn local_device_token(&self) -> Result<(User, Issued), ApiError> {
        let admin = self.users().await?.into_iter().find(|u| u.role == Role::Admin && !u.disabled);
        let admin = match admin {
            Some(a) => a,
            None => {
                let pw = hex(&random_bytes::<24>());
                self.create_first_admin(NewUser {
                    name: "owner".into(),
                    display_name: String::new(),
                    password: pw,
                    role: Role::Admin,
                    perms: None,
                })
                .await?
            }
        };
        let uid = admin.id;
        self.db
            .write(move |t| t.execute("DELETE FROM devices WHERE user_id = ?1 AND client = 'corrent-local'", [uid]))
            .await
            .map_err(db_err)?;
        let dev = DeviceInfo {
            name: "This computer".into(),
            platform: std::env::consts::OS.into(),
            client: "corrent-local".into(),
        };
        let issued = self.issue(uid, Some(dev), "127.0.0.1".into(), "Corrent desktop".into()).await?;
        Ok((admin, issued))
    }

    /// Checks a user's current password (self-service password changes).
    pub async fn check_password(&self, id: i64, password: &str) -> Result<bool, ApiError> {
        let stored: Option<String> = self
            .db
            .read(move |c| c.query_row("SELECT pw_hash FROM users WHERE id = ?1", [id], |r| r.get(0)).optional())
            .await
            .map_err(db_err)?;
        match stored {
            Some(s) => Ok(self.verify(s, password.to_string()).await),
            None => Ok(false),
        }
    }

    /// First start with an existing qBittorrent-API password: that account becomes the admin
    /// (its PBKDF2 hash is upgraded to argon2id at the first sign-in).
    pub async fn adopt_legacy_admin(&self, name: &str, pbkdf2_hash: &str) -> Result<Option<User>, ApiError> {
        if self.user_count().await? > 0 || pbkdf2_hash.is_empty() {
            return Ok(None);
        }
        let name = if valid_name(name) { name.to_string() } else { "admin".into() };
        self.insert_user(name, String::new(), pbkdf2_hash.to_string(), Role::Admin, None).await.map(Some)
    }

    pub async fn users(&self) -> Result<Vec<User>, ApiError> {
        self.db
            .read(|c| {
                let mut st = c.prepare(&format!("SELECT {USER_COLS} FROM users ORDER BY name"))?;
                st.query_map([], user_row)?.collect()
            })
            .await
            .map_err(db_err)
    }

    pub async fn user(&self, id: i64) -> Result<User, ApiError> {
        self.db
            .read(move |c| {
                c.query_row(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"), [id], user_row).optional()
            })
            .await
            .map_err(db_err)?
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such user"))
    }

    pub async fn update_user(&self, id: i64, u: UserUpdate) -> Result<User, ApiError> {
        let pw = match u.password {
            Some(p) if p.len() < 8 => {
                return Err(ApiError::new(ErrorCode::BadRequest, "password too short").with("min", 8));
            }
            Some(p) => Some(self.hash(p).await?),
            None => None,
        };
        let demotes_or_disables = u.role.is_some_and(|r| r != Role::Admin) || u.disabled == Some(true);
        let r = self
            .db
            .write(move |t| {
                if demotes_or_disables {
                    let was_admin: bool =
                        t.query_row("SELECT role = 'admin' FROM users WHERE id = ?1", [id], |r| r.get(0))?;
                    let admins: i64 =
                        t.query_row("SELECT count(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |r| {
                            r.get(0)
                        })?;
                    if was_admin && admins <= 1 {
                        return Ok(Err("the last administrator can't be demoted or disabled"));
                    }
                }
                if let Some(d) = &u.display_name {
                    t.execute("UPDATE users SET display_name = ?1 WHERE id = ?2", params![d, id])?;
                }
                if let Some(h) = &pw {
                    t.execute("UPDATE users SET pw_hash = ?1 WHERE id = ?2", params![h, id])?;
                    // A new password signs out everywhere else.
                    t.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
                }
                if let Some(r) = u.role {
                    t.execute("UPDATE users SET role = ?1 WHERE id = ?2", params![role_str(r), id])?;
                }
                if let Some(p) = u.perms {
                    t.execute("UPDATE users SET perms = ?1 WHERE id = ?2", params![p.0, id])?;
                }
                if let Some(d) = u.disabled {
                    t.execute("UPDATE users SET disabled = ?1 WHERE id = ?2", params![d as i64, id])?;
                    if d {
                        t.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
                    }
                }
                Ok(Ok(()))
            })
            .await
            .map_err(db_err)?;
        r.map_err(|m| ApiError::new(ErrorCode::Conflict, m))?;
        self.user(id).await
    }

    pub async fn delete_user(&self, id: i64) -> Result<(), ApiError> {
        let r = self
            .db
            .write(move |t| {
                let admins: i64 =
                    t.query_row("SELECT count(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |r| r.get(0))?;
                let is_admin: Option<bool> =
                    t.query_row("SELECT role = 'admin' FROM users WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                match is_admin {
                    None => Ok(Err(ErrorCode::NotFound)),
                    Some(true) if admins <= 1 => Ok(Err(ErrorCode::Conflict)),
                    Some(_) => {
                        t.execute("DELETE FROM users WHERE id = ?1", [id])?;
                        Ok(Ok(()))
                    }
                }
            })
            .await
            .map_err(db_err)?;
        r.map_err(|c| ApiError::new(c, "can't delete that user"))
    }

    /// Checks a password with per-account lockout. Disabled users can't sign in.
    pub async fn authenticate(&self, name: &str, password: &str) -> Result<User, ApiError> {
        let n = name.trim().to_string();
        let now = now_unix();
        let row = self
            .db
            .read(move |c| {
                c.query_row(
                    &format!("SELECT {USER_COLS}, pw_hash, locked_until FROM users WHERE name = ?1"),
                    [n],
                    |r| Ok((user_row(r)?, r.get::<_, String>(8)?, r.get::<_, i64>(9)?)),
                )
                .optional()
            })
            .await
            .map_err(db_err)?;
        let Some((user, stored, locked_until)) = row else {
            // Same work as a real check, so timing doesn't reveal which names exist.
            let cost = self.cost;
            let dummy = self
                .dummy
                .get_or_init(|| async move {
                    tokio::task::spawn_blocking(move || hash_password("not a password", cost)).await.unwrap_or_default()
                })
                .await
                .clone();
            let _ = self.verify(dummy, password.to_string()).await;
            return Err(ApiError::new(ErrorCode::InvalidCredentials, "invalid credentials"));
        };
        if locked_until > now {
            return Err(ApiError::new(ErrorCode::AccountLocked, "account locked").with("until", locked_until));
        }
        let ok = self.verify(stored.clone(), password.to_string()).await;
        let id = user.id;
        if !ok || user.disabled {
            self.db
                .write(move |t| {
                    t.execute("UPDATE users SET failed_logins = failed_logins + 1 WHERE id = ?1", [id])?;
                    t.execute(
                        "UPDATE users SET locked_until = ?1, failed_logins = 0 WHERE id = ?2 AND failed_logins >= ?3",
                        params![now + LOCKOUT_SECS, id, LOCKOUT_AFTER],
                    )
                })
                .await
                .map_err(db_err)?;
            return Err(ApiError::new(ErrorCode::InvalidCredentials, "invalid credentials"));
        }
        let upgrade = if stored.starts_with("argon2id$") { None } else { Some(self.hash(password.to_string()).await?) };
        self.db
            .write(move |t| {
                t.execute("UPDATE users SET failed_logins = 0, locked_until = 0 WHERE id = ?1", [id])?;
                if let Some(h) = upgrade {
                    t.execute("UPDATE users SET pw_hash = ?1 WHERE id = ?2", params![h, id])?;
                }
                Ok(())
            })
            .await
            .map_err(db_err)?;
        Ok(user)
    }

    /// Issues a web session or (with `device`) a new device and its token.
    pub async fn issue(
        &self,
        user_id: i64,
        device: Option<DeviceInfo>,
        ip: String,
        user_agent: String,
    ) -> Result<Issued, ApiError> {
        let token = format!("ct_{}", hex(&random_bytes::<32>()));
        let csrf = hex(&random_bytes::<16>());
        let th = token_hash(&token);
        let now = now_unix();
        let (kind, ttl) = if device.is_some() {
            (SessionKind::Device, DEVICE_TOKEN_SECS)
        } else {
            (SessionKind::Web, WEB_SESSION_SECS)
        };
        let expires_at = now + ttl;
        let c2 = csrf.clone();
        let device_id = self
            .db
            .write(move |t| {
                let device_id = match device {
                    Some(d) => {
                        t.execute(
                            "INSERT INTO devices(user_id, name, platform, client, created_at, last_seen, last_ip) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                            params![user_id, d.name.chars().take(80).collect::<String>(), d.platform, d.client, now, ip],
                        )?;
                        Some(t.last_insert_rowid())
                    }
                    None => None,
                };
                t.execute(
                    "INSERT INTO sessions(token_hash, user_id, device_id, kind, csrf, created_at, expires_at, last_seen, ip, user_agent) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?6, ?8, ?9)",
                    params![th, user_id, device_id, kind.as_str(), c2, now, expires_at, ip, user_agent.chars().take(200).collect::<String>()],
                )?;
                t.execute("DELETE FROM sessions WHERE expires_at < ?1", [now])?;
                Ok(device_id)
            })
            .await
            .map_err(db_err)?;
        Ok(Issued { token, csrf, expires_at, device_id })
    }

    /// Resolves a token to its user; slides the expiry. `None` for unknown, expired or disabled.
    pub async fn verify_token(&self, token: &str, ip: &str) -> Result<Option<Principal>, ApiError> {
        if !token.starts_with("ct_") || token.len() > 80 {
            return Ok(None);
        }
        let th = token_hash(token);
        let now = now_unix();
        let row = self
            .db
            .read(move |c| {
                c.query_row(
                    &format!(
                        "SELECT {}, s.id, s.device_id, s.kind, s.csrf, s.expires_at, s.last_seen \
                         FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1",
                        USER_COLS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ")
                    ),
                    [th],
                    |r| {
                        Ok((
                            user_row(r)?,
                            r.get::<_, i64>(8)?,
                            r.get::<_, Option<i64>>(9)?,
                            r.get::<_, String>(10)?,
                            r.get::<_, String>(11)?,
                            r.get::<_, i64>(12)?,
                            r.get::<_, i64>(13)?,
                        ))
                    },
                )
                .optional()
            })
            .await
            .map_err(db_err)?;
        let Some((user, sid, device_id, kind, csrf, expires_at, last_seen)) = row else { return Ok(None) };
        if expires_at < now || user.disabled {
            return Ok(None);
        }
        let kind = if kind == "device" { SessionKind::Device } else { SessionKind::Web };
        if now - last_seen >= TOUCH_EVERY {
            let ttl = if kind == SessionKind::Device { DEVICE_TOKEN_SECS } else { WEB_SESSION_SECS };
            let ip = ip.to_string();
            self.db
                .write(move |t| {
                    t.execute(
                        "UPDATE sessions SET last_seen = ?1, expires_at = ?2, ip = ?3 WHERE id = ?4",
                        params![now, now + ttl, ip, sid],
                    )?;
                    if let Some(d) = device_id {
                        t.execute(
                            "UPDATE devices SET last_seen = ?1, last_ip = ?2 WHERE id = ?3",
                            params![now, ip, d],
                        )?;
                    }
                    Ok(())
                })
                .await
                .map_err(db_err)?;
        }
        Ok(Some(Principal { user, session_id: sid, device_id, kind, csrf }))
    }

    pub async fn revoke_session(&self, session_id: i64) -> Result<(), ApiError> {
        self.db
            .write(move |t| t.execute("DELETE FROM sessions WHERE id = ?1", [session_id]).map(|_| ()))
            .await
            .map_err(db_err)
    }

    pub async fn devices(&self, user_id: Option<i64>) -> Result<Vec<Device>, ApiError> {
        self.db
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT id, user_id, name, platform, client, created_at, last_seen, last_ip FROM devices \
                     WHERE ?1 IS NULL OR user_id = ?1 ORDER BY last_seen DESC",
                )?;
                st.query_map([user_id], |r| {
                    Ok(Device {
                        id: r.get(0)?,
                        user_id: r.get(1)?,
                        name: r.get(2)?,
                        platform: r.get(3)?,
                        client: r.get(4)?,
                        created_at: r.get(5)?,
                        last_seen: r.get(6)?,
                        last_ip: r.get(7)?,
                        current: false,
                    })
                })?
                .collect()
            })
            .await
            .map_err(db_err)
    }

    /// Removes a device and its token. `owner` limits it to that user's devices.
    pub async fn delete_device(&self, id: i64, owner: Option<i64>) -> Result<(), ApiError> {
        let n = self
            .db
            .write(move |t| {
                t.execute("DELETE FROM devices WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)", params![id, owner])
            })
            .await
            .map_err(db_err)?;
        if n == 0 { Err(ApiError::new(ErrorCode::NotFound, "no such device")) } else { Ok(()) }
    }

    /// A 6-digit, single-use pairing code for `user_id` (at most 3 outstanding).
    pub async fn pairing_code(&self, user_id: i64) -> Result<(String, i64), ApiError> {
        let b = random_bytes::<4>();
        let code = format!("{:06}", u32::from_le_bytes(b) % 1_000_000);
        let now = now_unix();
        let expires = now + PAIRING_SECS;
        let c2 = code.clone();
        self.db
            .write(move |t| {
                t.execute(
                    "DELETE FROM pairing_codes WHERE expires_at < ?1 OR attempts >= ?2",
                    params![now, PAIRING_MAX_ATTEMPTS],
                )?;
                // Drop the oldest beyond the limit, and any equal code.
                t.execute("DELETE FROM pairing_codes WHERE code = ?1", [&c2])?;
                t.execute(
                    "DELETE FROM pairing_codes WHERE user_id = ?1 AND code NOT IN \
                     (SELECT code FROM pairing_codes WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2)",
                    params![user_id, PAIRING_MAX_OUTSTANDING - 1],
                )?;
                t.execute(
                    "INSERT INTO pairing_codes(code, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
                    params![c2, user_id, now, expires],
                )
            })
            .await
            .map_err(db_err)?;
        Ok((code, expires))
    }

    /// Redeems a pairing code. A wrong guess counts against every outstanding code, so after
    /// fifty wrong guesses all codes are burned (10⁶ combinations can't be walked).
    pub async fn redeem_pairing(&self, code: &str) -> Result<User, ApiError> {
        let code = code.trim().to_string();
        let now = now_unix();
        let uid = self
            .db
            .write(move |t| {
                let uid: Option<i64> = t
                    .query_row(
                        "SELECT user_id FROM pairing_codes WHERE code = ?1 AND expires_at >= ?2 AND attempts < ?3",
                        params![code, now, PAIRING_MAX_ATTEMPTS],
                        |r| r.get(0),
                    )
                    .optional()?;
                match uid {
                    Some(_) => {
                        t.execute("DELETE FROM pairing_codes WHERE code = ?1", [&code])?;
                    }
                    None => {
                        t.execute("UPDATE pairing_codes SET attempts = attempts + 1", [])?;
                        t.execute("DELETE FROM pairing_codes WHERE attempts >= ?1", [PAIRING_MAX_ATTEMPTS])?;
                    }
                }
                Ok(uid)
            })
            .await
            .map_err(db_err)?;
        let Some(uid) = uid else { return Err(ApiError::new(ErrorCode::PairingCodeInvalid, "invalid pairing code")) };
        let u = self.user(uid).await?;
        if u.disabled {
            return Err(ApiError::new(ErrorCode::PairingCodeInvalid, "invalid pairing code"));
        }
        Ok(u)
    }

    pub async fn audit(&self, user: Option<&str>, ip: &str, action: &str, target: &str, ok: bool, details: &str) {
        let (user, ip, action, target, details) =
            (user.map(str::to_string), ip.to_string(), action.to_string(), target.to_string(), details.to_string());
        let now = now_unix();
        let r = self
            .db
            .write(move |t| {
                t.execute(
                    "INSERT INTO audit(ts, user, ip, action, target, ok, details) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![now, user, ip, action, target, ok as i64, details],
                )?;
                let id = t.last_insert_rowid();
                if id % 500 == 0 {
                    t.execute("DELETE FROM audit WHERE id <= ?1", [id - AUDIT_KEEP])?;
                }
                Ok(())
            })
            .await;
        if let Err(e) = r {
            tracing::error!("audit log: {e}");
        }
    }

    /// Newest first; `before` pages backwards by id.
    pub async fn audit_log(&self, limit: u32, before: Option<i64>) -> Result<Vec<AuditEntry>, ApiError> {
        let limit = limit.clamp(1, 1000);
        self.db
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT id, ts, user, ip, action, target, ok, details FROM audit \
                     WHERE ?1 IS NULL OR id < ?1 ORDER BY id DESC LIMIT ?2",
                )?;
                st.query_map(params![before, limit], |r| {
                    Ok(AuditEntry {
                        id: r.get(0)?,
                        ts: r.get(1)?,
                        user: r.get(2)?,
                        ip: r.get(3)?,
                        action: r.get(4)?,
                        target: r.get(5)?,
                        ok: r.get::<_, i64>(6)? != 0,
                        details: r.get(7)?,
                    })
                })?
                .collect()
            })
            .await
            .map_err(db_err)
    }

    /// A `corrent://pair` link for a code.
    pub fn pairing_link(
        code: &str,
        expires_at: i64,
        host: &str,
        server_id: &str,
        fingerprint: Option<&str>,
    ) -> PairingCode {
        let mut link = format!(
            "corrent://pair?host={}&id={server_id}&code={code}",
            url::form_urlencoded::byte_serialize(host.as_bytes()).collect::<String>()
        );
        if let Some(fp) = fingerprint {
            link.push_str(&format!("&fp={fp}"));
        }
        PairingCode { code: code.to_string(), expires_at, link }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Identity {
        let d = std::env::temp_dir().join(format!("corrent-ident-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Identity::new(Db::open(&d.join("corrent.db")).unwrap(), HashCost::FAST)
    }

    fn new_user(name: &str, role: Role) -> NewUser {
        NewUser { name: name.into(), display_name: String::new(), password: "correct horse".into(), role, perms: None }
    }

    #[test]
    fn password_hashes_round_trip() {
        let h = hash_password("secret pw", HashCost::FAST);
        assert!(h.starts_with("argon2id$m=1024,t=1,p=1$"));
        assert!(verify_password(&h, "secret pw"));
        assert!(!verify_password(&h, "Secret pw"));
        assert!(!verify_password("argon2id$m=1,t=1$zz$zz", "x"));
        let legacy = crate::webapi::auth::hash_password("old one");
        assert!(verify_password(&legacy, "old one"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn users_tokens_and_lockout() {
        let id = scratch("users");
        let admin = id.create_user(new_user("Admin", Role::Admin)).await.unwrap();
        assert!(admin.perms.contains(Perms::USERS_MANAGE));
        assert_eq!(id.create_user(new_user("admin", Role::User)).await.unwrap_err().code, ErrorCode::Conflict);
        let u = id.authenticate("admin", "correct horse").await.unwrap();
        assert_eq!(u.id, admin.id);
        let issued = id.issue(u.id, None, "127.0.0.1".into(), "test".into()).await.unwrap();
        let p = id.verify_token(&issued.token, "127.0.0.1").await.unwrap().unwrap();
        assert_eq!((p.user.id, p.kind), (admin.id, SessionKind::Web));
        assert!(id.verify_token("ct_nope", "127.0.0.1").await.unwrap().is_none());
        id.revoke_session(p.session_id).await.unwrap();
        assert!(id.verify_token(&issued.token, "127.0.0.1").await.unwrap().is_none());
        // Lockout after 10 failures, even with the right password afterwards.
        for _ in 0..10 {
            assert_eq!(id.authenticate("admin", "wrong").await.unwrap_err().code, ErrorCode::InvalidCredentials);
        }
        assert_eq!(id.authenticate("admin", "correct horse").await.unwrap_err().code, ErrorCode::AccountLocked);
        // The last admin can't be demoted or deleted.
        let upd = UserUpdate { role: Some(Role::User), ..Default::default() };
        assert_eq!(id.update_user(admin.id, upd).await.unwrap_err().code, ErrorCode::Conflict);
        assert_eq!(id.delete_user(admin.id).await.unwrap_err().code, ErrorCode::Conflict);
        let _ = std::fs::remove_dir_all(id.db().path().parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn devices_and_pairing() {
        let id = scratch("pair");
        let u = id.create_user(new_user("ulas", Role::User)).await.unwrap();
        let (code, _) = id.pairing_code(u.id).await.unwrap();
        assert_eq!(code.len(), 6);
        assert_eq!(id.redeem_pairing(&code).await.unwrap().id, u.id);
        assert!(id.redeem_pairing(&code).await.is_err(), "single use");
        // Fifty wrong guesses burn outstanding codes.
        let (code, _) = id.pairing_code(u.id).await.unwrap();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        for _ in 0..50 {
            assert!(id.redeem_pairing(wrong).await.is_err());
        }
        assert!(id.redeem_pairing(&code).await.is_err(), "burned");
        let dev = DeviceInfo { name: "Laptop".into(), platform: "linux".into(), client: "corrent".into() };
        let issued = id.issue(u.id, Some(dev), "10.0.0.2".into(), String::new()).await.unwrap();
        let p = id.verify_token(&issued.token, "10.0.0.2").await.unwrap().unwrap();
        assert_eq!(p.kind, SessionKind::Device);
        let devs = id.devices(Some(u.id)).await.unwrap();
        assert_eq!(devs.len(), 1);
        id.delete_device(devs[0].id, Some(u.id)).await.unwrap();
        assert!(id.verify_token(&issued.token, "10.0.0.2").await.unwrap().is_none(), "token revoked with the device");
        let _ = std::fs::remove_dir_all(id.db().path().parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_admin_is_adopted_and_upgraded() {
        let id = scratch("legacy");
        let legacy = crate::webapi::auth::hash_password("sonarr pw");
        let u = id.adopt_legacy_admin("sonarr", &legacy).await.unwrap().unwrap();
        assert_eq!(u.role, Role::Admin);
        assert!(id.adopt_legacy_admin("x", &legacy).await.unwrap().is_none(), "only when no users exist");
        id.authenticate("sonarr", "sonarr pw").await.unwrap();
        let stored: String =
            id.db().read(|c| c.query_row("SELECT pw_hash FROM users", [], |r| r.get(0))).await.unwrap();
        assert!(stored.starts_with("argon2id$"), "upgraded at sign-in");
        id.audit(Some("sonarr"), "127.0.0.1", "login", "", true, "").await;
        assert_eq!(id.audit_log(10, None).await.unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(id.db().path().parent().unwrap());
    }
}
