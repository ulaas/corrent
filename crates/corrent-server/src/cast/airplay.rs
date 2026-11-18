//! AirPlay receivers (Apple TVs, and TVs that take AirPlay 2 such as Samsung's and LG's): finding
//! them and reading what they say about themselves. "AirPlay" is Apple's name for the protocol;
//! this is an independent implementation, not affiliated with Apple.
//!
//! There is no public description of the sending side. What is here follows what receivers
//! announce (mDNS `_airplay._tcp` with a TXT record) as the open-source senders read it. Whether
//! a given TV plays a link sent to it, and after which kind of pairing, is not known in advance:
//! [`Receiver`] carries what the device says, and [`probe`] asks it the harmless questions.

pub mod conn;
pub mod pair;
pub mod play;
pub mod srp;
pub mod tlv8;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const KIND: &str = "airplay";
const SERVICE: &str = "_airplay._tcp.local.";

/// Feature bits (the `features` TXT key, 64 bits written as `0xLOW,0xHIGH`).
const FEATURE_VIDEO: u64 = 1 << 0;
const FEATURE_VIDEO_HLS: u64 = 1 << 4;
const FEATURE_LEGACY_PAIRING: u64 = 1 << 27;
const FEATURE_UNIFIED_MEDIA_CONTROL: u64 = 1 << 38;
const FEATURE_HOMEKIT_PAIRING: u64 = 1 << 46;
const FEATURE_TRANSIENT_PAIRING: u64 = 1 << 48;
/// Status bits (the `flags` TXT key; `sf` on older devices).
const FLAG_PIN_REQUIRED: u64 = 0x8;
const FLAG_PASSWORD_REQUIRED: u64 = 0x80;
const FLAG_ONE_TIME_PAIRING: u64 = 0x200;

/// What a receiver announces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    /// Its `deviceid` (a MAC address), the stable name for it.
    pub id: String,
    pub name: String,
    pub model: String,
    pub addr: SocketAddr,
    pub features: u64,
    pub flags: u64,
    /// `pw=true`: a password set by its owner.
    pub password: bool,
    /// The receiver's software (`srcvers`).
    pub version: String,
}

impl Receiver {
    /// Takes video by address (as opposed to mirroring or sound only).
    pub fn video(&self) -> bool {
        self.features & FEATURE_VIDEO != 0
    }

    pub fn hls(&self) -> bool {
        self.features & FEATURE_VIDEO_HLS != 0
    }

    /// A PIN from its screen, or its owner's password, comes first.
    pub fn needs_pin(&self) -> bool {
        self.flags & (FLAG_PIN_REQUIRED | FLAG_ONE_TIME_PAIRING) != 0
    }

    pub fn needs_password(&self) -> bool {
        self.password || self.flags & FLAG_PASSWORD_REQUIRED != 0
    }

    /// Pairs without asking anyone (the HomeKit "transient" pairing).
    pub fn transient_pairing(&self) -> bool {
        self.features & FEATURE_TRANSIENT_PAIRING != 0
    }

    /// The facts in words, for the log and the probe.
    pub fn describe(&self) -> String {
        let mut facts = Vec::new();
        for (on, text) in [
            (self.video(), "video"),
            (self.hls(), "HLS"),
            (self.features & FEATURE_LEGACY_PAIRING != 0, "legacy pairing"),
            (self.features & FEATURE_HOMEKIT_PAIRING != 0, "HomeKit pairing"),
            (self.transient_pairing(), "transient pairing"),
            (self.features & FEATURE_UNIFIED_MEDIA_CONTROL != 0, "unified media control"),
            (self.needs_pin(), "PIN required"),
            (self.needs_password(), "password required"),
        ] {
            if on {
                facts.push(text);
            }
        }
        format!(
            "{} · {} · {} · {} · {} · features {:#x} · flags {:#x} · {}",
            self.name,
            self.id,
            self.model,
            self.addr,
            self.version,
            self.features,
            self.flags,
            facts.join(", ")
        )
    }
}

/// `0x4A7FCA00,0xBC356BD0` (low 32 bits, then high) or one number.
pub fn parse_features(text: &str) -> Option<u64> {
    let hex = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok();
    match text.split_once(',') {
        Some((low, high)) => {
            let (low, high) = (hex(low)?, hex(high)?);
            (low <= u64::from(u32::MAX) && high <= u64::from(u32::MAX)).then_some(low | high << 32)
        }
        None => hex(text),
    }
}

/// `0x244` or `580`.
pub fn parse_flags(text: &str) -> Option<u64> {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}

/// The receivers that answer an mDNS search within `wait` (local IPv4 only).
pub fn browse(wait: Duration) -> anyhow::Result<Vec<Receiver>> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE)?;
    let deadline = Instant::now() + wait;
    let mut out: Vec<Receiver> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(s)) => {
                let p = s.get_properties();
                let get = |k: &str| p.get_property_val_str(k).unwrap_or_default().to_string();
                let Some(ip) = s.get_addresses().iter().map(|a| a.to_ip_addr()).find(|a| a.is_ipv4()) else { continue };
                if !crate::net::is_local(ip, &[]) {
                    continue;
                }
                let id = get("deviceid").to_ascii_lowercase();
                if id.is_empty() || out.iter().any(|r| r.id == id) {
                    continue;
                }
                // "Living Room._airplay._tcp.local." → "Living Room".
                let name =
                    s.get_fullname().strip_suffix(&format!(".{SERVICE}")).unwrap_or(s.get_fullname()).to_string();
                out.push(Receiver {
                    id,
                    name,
                    model: get("model"),
                    addr: SocketAddr::new(ip, s.get_port()),
                    features: parse_features(&get("features")).unwrap_or(0),
                    flags: parse_flags(&get("flags")).or_else(|| parse_flags(&get("sf"))).unwrap_or(0),
                    password: matches!(get("pw").as_str(), "true" | "1"),
                    version: get("srcvers"),
                });
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    out.sort_by_key(|r| r.name.to_lowercase());
    Ok(out)
}

/// What a receiver answers to the questions that change nothing on it: `GET /info` and
/// `GET /playback-info` (status, type and size of each answer). Nothing is played, and no
/// pairing is started (that can put a PIN on the screen).
pub async fn probe(r: &Receiver) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
    else {
        return out;
    };
    for path in ["/info", "/playback-info", "/server-info"] {
        let url = format!("http://{}{path}", r.addr);
        let res = client
            .get(&url)
            .header("User-Agent", "MediaControl/1.0")
            .header("X-Apple-Session-ID", "00000000-0000-0000-0000-000000000000")
            .send()
            .await;
        out.push(match res {
            Ok(res) => {
                let status = res.status();
                let kind = res
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-")
                    .to_string();
                let server =
                    res.headers().get(reqwest::header::SERVER).and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
                let body = crate::net::read_capped(res, 256 * 1024).await.unwrap_or_default();
                // The names in a binary property list are plain text in it: enough to see what
                // the device tells, without a parser.
                let words: Vec<String> = String::from_utf8_lossy(&body)
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'))
                    .filter(|w| w.len() >= 4)
                    .take(60)
                    .map(str::to_string)
                    .collect();
                format!("GET {path}: {status} · {kind} · server {server} · {} bytes · {}", body.len(), words.join(" "))
            }
            Err(e) => format!("GET {path}: {e}"),
        });
    }
    out
}

/// Takes a link through its play queue (bit 49), or the older way (bit 0, which Apple's own
/// devices have next to it): what [`session`] can play on.
const FEATURE_VIDEO_QUEUE: u64 = 1 << 49;

impl Receiver {
    pub fn playable(&self) -> bool {
        self.features & (FEATURE_VIDEO | FEATURE_VIDEO_QUEUE) != 0 && !self.needs_password()
    }

    /// The device as the cast list shows it.
    pub fn found(self) -> super::Found {
        let id: String = self.id.chars().filter(char::is_ascii_alphanumeric).collect();
        super::Found {
            device: corrent_proto::library::CastDevice {
                id: format!("airplay-{id}"),
                name: self.name.clone(),
                model: self.model.clone(),
                kind: KIND.into(),
                needs_pairing: false,
            },
            addr: self.addr,
            dlna: None,
            airplay: Some(std::sync::Arc::new(self)),
        }
    }
}

/// The pairings made with receivers, by their `deviceid`: `<data dir>/airplay-pairings.json`,
/// readable by the server's account only (they are the keys to play on those devices).
pub mod store {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::pair::Credentials;

    fn file(data_dir: &Path) -> PathBuf {
        data_dir.join("airplay-pairings.json")
    }

    fn all(data_dir: &Path) -> BTreeMap<String, Credentials> {
        std::fs::read_to_string(file(data_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn get(data_dir: &Path, device: &str) -> Option<Credentials> {
        all(data_dir).remove(device)
    }

    pub fn paired(data_dir: &Path) -> Vec<String> {
        all(data_dir).into_keys().collect()
    }

    /// `None` forgets the device.
    pub fn set(data_dir: &Path, device: &str, creds: Option<Credentials>) -> anyhow::Result<()> {
        let mut map = all(data_dir);
        match creds {
            Some(c) => map.insert(device.to_string(), c),
            None => map.remove(device),
        };
        crate::tls::write_private(&file(data_dir), &serde_json::to_string_pretty(&map)?)
    }
}

/// The actor of one receiver: like [`super::upnp::session`], for a device that reports on its
/// own channel and is asked where it is every second besides.
pub(super) async fn session(
    r: std::sync::Arc<Receiver>,
    status: std::sync::Arc<std::sync::Mutex<corrent_proto::library::CastStatus>>,
    mut rx: tokio::sync::mpsc::Receiver<super::Cmd>,
    st: std::sync::Arc<crate::state::AppState>,
) -> anyhow::Result<()> {
    use std::sync::atomic::Ordering;

    use corrent_proto::library::CastCommand;

    use self::play::{Event, Session};
    use super::{CANT_PLAY, CANT_REACH, Cmd, IDLE_EXIT, LOAD_DEADLINE, Media, report_progress};

    let set = |f: &mut dyn FnMut(&mut corrent_proto::library::CastStatus)| {
        f(&mut status.lock().unwrap_or_else(|e| e.into_inner()))
    };
    // Its volume is the TV's own: not ours to set over this channel.
    set(&mut |s| s.volume_fixed = true);
    let stop_hls = |m: &Option<Box<Media>>| {
        if let Some(id) = m.as_ref().and_then(|m| m.hls_session.clone()) {
            let hls = st.hls.clone();
            tokio::spawn(async move { hls.stop(&id).await });
        }
    };
    let creds = store::get(&st.paths.data_dir, &r.id)
        .ok_or_else(|| anyhow::anyhow!("this device isn't paired yet: pair it in the casting settings"))?;
    let local = super::local_ip_towards(r.addr).ok_or_else(|| anyhow::anyhow!("no route to the device"))?;
    let mut current: Option<Box<Media>> = None;
    // The connection to the device (closing it ends playback there) and the queued item's name.
    let mut live: Option<(Session, String)> = None;
    let mut played = false;
    let mut played_at = 0i64;
    let mut loading_since: Option<Instant> = None;
    let mut hits_at_load = None;
    let mut last_report = Instant::now();
    let mut idle_since: Option<Instant> = None;
    let mut ticks = 0i64;
    let mut beat = tokio::time::interval(Duration::from_secs(1));
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // What ends the current item: `None` while it goes on.
    enum End {
        Finished,
        Failed,
        Gone,
    }
    loop {
        let mut end: Option<End> = None;
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else {
                    if let Some(c) = &current {
                        report_progress(&st, c, played_at, c.duration_ms, false);
                    }
                    stop_hls(&current);
                    return Ok(());
                };
                match cmd {
                    Cmd::Load(m) => {
                        if let Some(c) = &current {
                            report_progress(&st, c, played_at, c.duration_ms, false);
                        }
                        stop_hls(&current);
                        // A new connection for every item: closing the old one clears the
                        // device's queue, which no command of ours is known to do.
                        live = None;
                        current = None;
                        set(&mut |s| s.state = "loading".into());
                        idle_since = None;
                        played = false;
                        played_at = (m.start_s * 1000.0) as i64;
                        hits_at_load = Some((m.hits.clone(), m.hits.load(Ordering::Relaxed)));
                        let hls = m.content_type.contains("mpegurl");
                        let opened = async {
                            let mut s = Session::open(r.addr, &creds, &r.id, local).await?;
                            let item = s.play(&m.url, m.start_s, hls).await?;
                            anyhow::Ok((s, item))
                        }
                        .await;
                        match opened {
                            Ok(l) => {
                                live = Some(l);
                                loading_since = Some(Instant::now());
                                current = Some(m);
                            }
                            Err(e) => {
                                tracing::info!("cast to {}: {e:#}", r.name);
                                stop_hls(&Some(m));
                                loading_since = None;
                                set(&mut |s| {
                                    s.state = "error".into();
                                    s.error = Some(format!("{e:#}"));
                                });
                                idle_since = Some(Instant::now());
                            }
                        }
                    }
                    Cmd::Failed(why) => {
                        loading_since = None;
                        set(&mut |s| {
                            s.state = "error".into();
                            s.error = Some(why.clone());
                        });
                        idle_since = Some(Instant::now());
                    }
                    Cmd::Tracks(_) => {}
                    Cmd::Control(CastCommand::Stop) => {
                        if let Some(c) = &current {
                            report_progress(&st, c, played_at, c.duration_ms, false);
                        }
                        stop_hls(&current);
                        return Ok(());
                    }
                    Cmd::Control(c) => {
                        let Some((s, item)) = live.as_mut() else { continue };
                        let done = match c {
                            CastCommand::Play => s.set_rate(1.0).await,
                            CastCommand::Pause => s.set_rate(0.0).await,
                            CastCommand::Seek { position_ms } => s.seek(item, position_ms.max(0) as f64 / 1000.0).await,
                            _ => Ok(()),
                        };
                        if let Err(e) = done {
                            tracing::debug!("cast to {}: {e:#}", r.name);
                        }
                    }
                }
            }
            e = async {
                match live.as_mut() {
                    Some((s, _)) => s.events.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(m) = current.as_ref() else { continue };
                match e {
                    Some(Event::State { name, position, duration, reason, .. }) => {
                        if let Some(p) = position {
                            played_at = (p * 1000.0) as i64;
                        }
                        match name.as_str() {
                            "playing" | "paused" => {
                                played = true;
                                loading_since = None;
                                set(&mut |s| {
                                    s.state = name.clone();
                                    s.position_ms = played_at;
                                    if let Some(d) = duration.filter(|d| *d > 0.0 && m.hls_session.is_none()) {
                                        s.duration_ms = Some((d * 1000.0) as i64);
                                    }
                                    s.error = None;
                                });
                            }
                            "loading" if played => set(&mut |s| s.state = "buffering".into()),
                            "stopped" if reason.as_deref() == Some("ended") => end = Some(End::Finished),
                            _ => {}
                        }
                    }
                    Some(Event::Info { state, position, .. }) => {
                        if let Some(p) = position.filter(|_| played) {
                            played_at = (p * 1000.0) as i64;
                            set(&mut |s| {
                                s.position_ms = played_at;
                                if matches!(state.as_str(), "playing" | "paused") {
                                    s.state = state.clone();
                                }
                            });
                        }
                    }
                    Some(Event::Notification { name, .. }) => match name.as_str() {
                        "itemPlayedToEnd" => end = Some(End::Finished),
                        "itemFailedToPlayToEnd" => end = Some(End::Failed),
                        _ => {}
                    },
                    Some(Event::Failed(code)) => {
                        tracing::info!("cast to {}: the device can't play this (error {code})", r.name);
                        end = Some(End::Failed);
                    }
                    Some(Event::Other(_)) => {}
                    Some(Event::Closed) | None => end = Some(End::Gone),
                }
            }
            _ = beat.tick() => {
                ticks += 1;
                if idle_since.is_some_and(|t| t.elapsed() > IDLE_EXIT) {
                    return Ok(());
                }
                let (Some(m), Some((s, _))) = (current.as_ref(), live.as_mut()) else { continue };
                if ticks % 5 == 0 && let Some(id) = m.hls_session.as_deref() {
                    st.hls.keep_alive(id);
                }
                // "Still here" every two seconds, and where it is every second.
                let alive = if ticks % 2 == 0 { s.feedback().await } else { Ok(()) };
                let asked = s.ask_state(ticks).await;
                if alive.is_err() || asked.is_err() {
                    end = Some(End::Gone);
                } else if !played && loading_since.is_some_and(|t| t.elapsed() > LOAD_DEADLINE) {
                    end = Some(End::Failed);
                } else if played && last_report.elapsed() > Duration::from_secs(10) {
                    last_report = Instant::now();
                    report_progress(&st, m, played_at, m.duration_ms, false);
                }
            }
        }
        let Some(end) = end else { continue };
        let Some(m) = current.take() else { continue };
        live = None;
        loading_since = None;
        idle_since = Some(Instant::now());
        match end {
            End::Finished => {
                report_progress(&st, &m, played_at, m.duration_ms, true);
                set(&mut |s| {
                    s.state = "idle".into();
                    s.finished = true;
                });
            }
            // Gone after it had played: someone took the TV over, or switched it off.
            End::Gone if played => {
                report_progress(&st, &m, played_at, m.duration_ms, false);
                set(&mut |s| s.state = "idle".into());
            }
            End::Failed | End::Gone => {
                if played {
                    report_progress(&st, &m, played_at, m.duration_ms, false);
                }
                let why = match &hits_at_load {
                    Some((now, then)) if now.load(Ordering::Relaxed) == *then => CANT_REACH,
                    _ => CANT_PLAY,
                };
                // A file it fetched and can't play (an MP4 with its index at the end, say) may
                // play converted.
                if why == CANT_PLAY
                    && let Some(f) = m.fallback.clone()
                {
                    tokio::spawn((f.0)(played_at as f64 / 1000.0));
                    idle_since = None;
                    loading_since = Some(Instant::now());
                } else {
                    set(&mut |s| {
                        s.state = "error".into();
                        s.error = Some(why.into());
                    });
                }
            }
        }
        stop_hls(&Some(m));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_and_flags_as_receivers_write_them() {
        // An Apple TV 4K's, from the open-source notes.
        let f = parse_features("0x4A7FCA00,0xBC354BD0").unwrap();
        assert_eq!(f, 0xBC35_4BD0_4A7F_CA00);
        assert_eq!(parse_features("0x5A7FFFF7"), Some(0x5A7F_FFF7));
        assert_eq!(parse_features("0x1,0x1FFFFFFFF"), None, "each half is 32 bits");
        assert_eq!(parse_features("video"), None);
        assert_eq!(parse_flags("0x244"), Some(0x244));
        assert_eq!(parse_flags("580"), Some(580));
        let r = Receiver {
            id: "aa:bb:cc:dd:ee:ff".into(),
            name: "Den".into(),
            model: "QE85Q70B".into(),
            addr: "192.168.1.20:7000".parse().unwrap(),
            features: FEATURE_VIDEO | FEATURE_VIDEO_HLS | FEATURE_TRANSIENT_PAIRING,
            flags: FLAG_PIN_REQUIRED,
            password: false,
            version: "1".into(),
        };
        assert!(r.video() && r.hls() && r.transient_pairing() && r.needs_pin() && !r.needs_password());
        assert!(r.describe().contains("video, HLS, transient pairing, PIN required"), "{}", r.describe());
    }

    /// What is on this network (`cargo test -p corrent-server airplay_on_this_network -- --ignored
    /// --nocapture`): searches, and asks each receiver for `/info` and `/playback-info`. Plays
    /// nothing and starts no pairing.
    #[tokio::test]
    #[ignore = "looks at the real network"]
    async fn airplay_on_this_network() {
        let found = tokio::task::spawn_blocking(|| browse(Duration::from_secs(4))).await.unwrap().unwrap();
        for r in found {
            println!("{}", r.describe());
            for line in probe(&r).await {
                println!("    {line}");
            }
        }
    }
}
