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
            "{} · {} · {} · {} · features {:#x} · flags {:#x} · {}",
            self.name,
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
