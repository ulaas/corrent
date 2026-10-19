//! Notifications: server events ("download done", "device paired", …) sent to webhooks,
//! Discord, ntfy or notifier plugins, as the administrator configured (`notify_targets`).

use std::sync::Arc;
use std::time::Duration;

use corrent_db::rusqlite::{self, OptionalExtension, params};
use corrent_proto::tasks::{NotifyKind, NotifyTarget};

use crate::state::AppState;

fn kind_str(k: NotifyKind) -> &'static str {
    match k {
        NotifyKind::Webhook => "webhook",
        NotifyKind::Discord => "discord",
        NotifyKind::Ntfy => "ntfy",
        NotifyKind::Plugin => "plugin",
    }
}

fn kind_of(s: &str) -> NotifyKind {
    match s {
        "discord" => NotifyKind::Discord,
        "ntfy" => NotifyKind::Ntfy,
        "plugin" => NotifyKind::Plugin,
        _ => NotifyKind::Webhook,
    }
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<NotifyTarget> {
    let events: String = r.get(5)?;
    Ok(NotifyTarget {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: kind_of(&r.get::<_, String>(2)?),
        url: r.get(3)?,
        plugin: r.get(4)?,
        events: events.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
        enabled: r.get::<_, i64>(6)? != 0,
    })
}

const COLS: &str = "id, name, kind, url, plugin, events, enabled";

pub async fn list(db: &corrent_db::Db) -> Result<Vec<NotifyTarget>, String> {
    db.read(|c| {
        let mut s = c.prepare(&format!("SELECT {COLS} FROM notify_targets ORDER BY id"))?;
        s.query_map([], row)?.collect()
    })
    .await
    .map_err(|e| e.to_string())
}

pub fn validate(t: &NotifyTarget) -> Result<(), &'static str> {
    if t.name.trim().is_empty() {
        return Err("a name is needed");
    }
    match t.kind {
        NotifyKind::Plugin if t.plugin.trim().is_empty() => Err("choose a notifier plugin"),
        NotifyKind::Plugin => Ok(()),
        _ => match url::Url::parse(t.url.trim()) {
            Ok(u) if matches!(u.scheme(), "http" | "https") => Ok(()),
            _ => Err("not an http(s) URL"),
        },
    }
}

/// Inserts (`id` 0) or replaces a target; returns it as stored.
pub async fn save(db: &corrent_db::Db, t: NotifyTarget) -> Result<Option<NotifyTarget>, String> {
    let now = corrent_core::util::now_unix();
    db.write(move |tx| {
        let events = t.events.join(",");
        let id = if t.id == 0 {
            tx.execute(
                "INSERT INTO notify_targets(name, kind, url, plugin, events, enabled, created_at)                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![t.name.trim(), kind_str(t.kind), t.url.trim(), t.plugin, events, t.enabled as i64, now],
            )?;
            tx.last_insert_rowid()
        } else {
            tx.execute(
                "UPDATE notify_targets SET name = ?1, kind = ?2, url = ?3, plugin = ?4, events = ?5, enabled = ?6                  WHERE id = ?7",
                params![t.name.trim(), kind_str(t.kind), t.url.trim(), t.plugin, events, t.enabled as i64, t.id],
            )?;
            t.id
        };
        tx.query_row(&format!("SELECT {COLS} FROM notify_targets WHERE id = ?1"), [id], row).optional()
    })
    .await
    .map_err(|e| e.to_string())
}

pub async fn delete(db: &corrent_db::Db, id: i64) -> Result<bool, String> {
    db.write(move |tx| Ok(tx.execute("DELETE FROM notify_targets WHERE id = ?1", [id])? > 0))
        .await
        .map_err(|e| e.to_string())
}

/// Delivers one notification to one target.
pub async fn deliver(st: &AppState, t: &NotifyTarget, event: &str, title: &str, body: &str) -> Result<(), String> {
    let send = |req: reqwest::RequestBuilder| async move {
        let res = req.timeout(Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?;
        if res.status().is_success() { Ok(()) } else { Err(format!("HTTP {}", res.status())) }
    };
    match t.kind {
        NotifyKind::Webhook => {
            let j = serde_json::json!({
                "event": event, "title": title, "body": body,
                "time": corrent_core::util::now_unix(), "server": st.cfg.load().display_name(),
            });
            send(st.http.post(&t.url).json(&j)).await
        }
        NotifyKind::Discord => {
            let content = if body.is_empty() { format!("**{title}**") } else { format!("**{title}**\n{body}") };
            send(st.http.post(&t.url).json(&serde_json::json!({ "content": content }))).await
        }
        NotifyKind::Ntfy => {
            // ntfy takes the message as the body and the title in a header (ASCII only there).
            let ascii_title: String = title.chars().filter(|c| c.is_ascii() && !c.is_control()).collect();
            send(
                st.http
                    .post(&t.url)
                    .header("Title", ascii_title)
                    .header("Tags", event)
                    .body(format!("{title}\n{body}")),
            )
            .await
        }
        NotifyKind::Plugin => {
            let host = st.plugins.as_ref().ok_or("plugins are unavailable")?;
            host.notify(&t.plugin, title.to_string(), body.to_string()).await.map_err(|e| e.to_string())
        }
    }
}

/// Sends `event` to every enabled target that wants it (in the background; failures are logged).
pub fn send(st: &Arc<AppState>, event: &'static str, title: String, body: String) {
    send_on(&tokio::runtime::Handle::current(), st, event, title, body);
}

/// [`send`] from a thread outside the runtime (engine events).
pub fn send_on(rt: &tokio::runtime::Handle, st: &Arc<AppState>, event: &'static str, title: String, body: String) {
    let st = st.clone();
    rt.spawn(async move {
        let Ok(targets) = list(st.identity.db()).await else { return };
        for t in
            targets.into_iter().filter(|t| t.enabled && (t.events.is_empty() || t.events.iter().any(|e| e == event)))
        {
            if let Err(e) = deliver(&st, &t, event, &title, &body).await {
                tracing::warn!("notification to {:?}: {e}", t.name);
            }
        }
    });
}
