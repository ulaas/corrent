//! Playing a link on an AirPlay 2 receiver that takes video only through its play queue (TVs
//! with AirPlay built in: no `/play`, no `/playback-info`). After pairing is verified: an RTSP
//! `SETUP` of the session, the event channel the device reports on, `RECORD`, a remote-control
//! stream, and then commands (`insertPlayQueueItem`, `setRate`, …) as property lists inside
//! `POST /command`. This follows what Apple's own senders were seen to do
//! (docs/spikes/airplay-protocol.md, section 6); closing the connection ends playback.

use std::io::Cursor;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plist::{Dictionary, Value};
use tokio::sync::mpsc;

use super::conn::Conn;
use super::pair::{self, Credentials};

/// What the device says on its event channel.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `loading`, `readyToPlay`, `playing`, `paused`, `stopped`, `idle`, with what it carried.
    State { name: String, position: Option<f64>, duration: Option<f64>, rate: Option<f64>, reason: Option<String> },
    /// `itemPlayedToEnd`, `itemFailedToPlayToEnd`, `timeJumped`, …
    Notification { name: String, detail: String },
    /// The answer to [`Session::ask_state`]: where playback is.
    Info { state: String, position: Option<f64>, duration: Option<f64>, rate: Option<f64>, ready: bool },
    /// The device couldn't play the item (its error code).
    Failed(i64),
    /// Anything else, as it came (for the log).
    Other(String),
    /// The device closed the channel.
    Closed,
}

fn bplist(v: &Value) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    v.to_writer_binary(&mut out)?;
    Ok(out)
}

fn read_plist(bytes: &[u8]) -> Option<Value> {
    Value::from_reader(Cursor::new(bytes)).ok()
}

fn dict(items: Vec<(&str, Value)>) -> Value {
    let mut d = Dictionary::new();
    for (k, v) in items {
        d.insert(k.to_string(), v);
    }
    Value::Dictionary(d)
}

fn text(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

/// A time as the device's media framework writes it.
fn cm_time(seconds: f64) -> Value {
    dict(vec![
        ("flags", Value::Integer(1.into())),
        ("value", Value::Integer(((seconds.max(0.0) * 1000.0).round() as i64).into())),
        ("epoch", Value::Integer(0.into())),
        ("timescale", Value::Integer(1000.into())),
    ])
}

/// Seconds of a time the device wrote (`{value, timescale}`), or of a plain number.
fn seconds(v: &Value) -> Option<f64> {
    if let Some(d) = v.as_dictionary() {
        let value = d.get("value").and_then(number)?;
        let scale = d.get("timescale").and_then(number).filter(|s| *s > 0.0)?;
        return Some(value / scale);
    }
    number(v)
}

fn number(v: &Value) -> Option<f64> {
    v.as_real()
        .or_else(|| v.as_signed_integer().map(|i| i as f64))
        .or_else(|| v.as_unsigned_integer().map(|i| i as f64))
}

fn upper_uuid() -> String {
    let mut b = [0u8; 16];
    let _ = aws_lc_rs::rand::fill(&mut b);
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// A command for the device's play queue, wrapped as `/command` carries it.
fn wrapped(command: &Value) -> anyhow::Result<Vec<u8>> {
    bplist(&dict(vec![("params", dict(vec![("data", Value::Data(bplist(command)?))]))]))
}

/// What a wrapped message (ours, or the device's on the event channel) carries.
pub fn unwrapped(body: &[u8]) -> Option<Value> {
    let outer = read_plist(body)?;
    let data = outer.as_dictionary()?.get("params")?.as_dictionary()?.get("data")?.as_data()?;
    read_plist(data)
}

/// The device's message as an [`Event`].
pub fn event(v: &Value) -> Event {
    let Some(d) = v.as_dictionary() else { return Event::Other(format!("{v:?}")) };
    let get = |k: &str| d.get(k).and_then(Value::as_string).map(str::to_string);
    let params = d.get("params").and_then(Value::as_dictionary);
    match get("type").as_deref() {
        Some("playbackState") => {
            let from = |k: &str| params.and_then(|p| p.get(k)).or_else(|| d.get(k));
            Event::State {
                name: get("name").unwrap_or_default(),
                position: from("position").and_then(seconds),
                duration: from("duration").and_then(seconds),
                rate: from("rate").and_then(number),
                reason: from("reason").and_then(Value::as_string).map(str::to_string),
            }
        }
        Some("playbackInfo") => match d.get("info").and_then(Value::as_dictionary) {
            Some(info) => Event::Info {
                state: info.get("playbackState").and_then(Value::as_string).unwrap_or_default().to_string(),
                position: info.get("position").and_then(seconds),
                duration: info.get("duration").and_then(seconds),
                rate: info.get("rate").and_then(number),
                ready: info.get("readyToPlay").and_then(Value::as_boolean).unwrap_or(false),
            },
            None => Event::Other(format!("{v:?}")),
        },
        Some("error") => Event::Failed(
            d.get("error")
                .and_then(Value::as_dictionary)
                .and_then(|e| e.get("code"))
                .and_then(Value::as_signed_integer)
                .unwrap_or(0),
        ),
        Some("notification") => Event::Notification {
            name: get("name").unwrap_or_default(),
            detail: params.map(|p| format!("{p:?}")).unwrap_or_default(),
        },
        _ => Event::Other(format!("{v:?}")),
    }
}

/// An NTP timestamp (seconds since 1900, 32.32 fixed point).
fn ntp_now() -> [u8; 8] {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = (now.as_secs() + 2_208_988_800) as u32;
    let frac = ((u64::from(now.subsec_nanos()) << 32) / 1_000_000_000) as u32;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&secs.to_be_bytes());
    out[4..].copy_from_slice(&frac.to_be_bytes());
    out
}

/// The answer to a timing request: its send time echoed, ours for receive and transmit.
pub fn timing_reply(request: &[u8]) -> Option<[u8; 32]> {
    if request.len() < 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out[0] = 0x80;
    out[1] = 0x53 | 0x80;
    out[2..4].copy_from_slice(request.get(2..4)?);
    out[8..16].copy_from_slice(request.get(24..32)?);
    let now = ntp_now();
    out[16..24].copy_from_slice(&now);
    out[24..32].copy_from_slice(&now);
    Some(out)
}

/// A session with a receiver, up to and including its remote-control stream.
pub struct Session {
    conn: Conn,
    cseq: u64,
    remote_id: String,
    active_remote: String,
    uri: String,
    stream_id: i64,
    pub events: mpsc::Receiver<Event>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Session {
    /// `receiver_id`: the device's `deviceid`; `local`: our address as it sees us.
    pub async fn open(
        addr: SocketAddr,
        creds: &Credentials,
        receiver_id: &str,
        local: IpAddr,
    ) -> anyhow::Result<Session> {
        let mut conn = Conn::connect(addr).await?;
        let shared = pair::verify(&mut conn, creds).await?;
        let mut r = [0u8; 16];
        aws_lc_rs::rand::fill(&mut r).map_err(|_| anyhow::anyhow!("no random numbers"))?;
        let remote_id = format!("{:X}", u64::from_be_bytes(r[..8].try_into().unwrap_or_default()));
        let active_remote = u32::from_be_bytes(r[8..12].try_into().unwrap_or_default()).to_string();
        let session_no = u32::from_be_bytes(r[12..16].try_into().unwrap_or_default());
        // A made-up address of ours (locally administered), as senders give.
        let mac = format!("02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", r[0], r[1], r[2], r[3], r[4]);
        // The timing server the session names: receivers ask it for the time.
        let timing = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await?;
        let timing_port = timing.local_addr()?.port();
        let mut tasks = vec![tokio::spawn(async move {
            let mut buf = [0u8; 128];
            while let Ok((n, from)) = timing.recv_from(&mut buf).await {
                if from.ip() == addr.ip()
                    && let Some(reply) = buf.get(..n).and_then(timing_reply)
                {
                    let _ = timing.send_to(&reply, from).await;
                }
            }
        })];
        let mut s = Session {
            conn,
            cseq: 0,
            remote_id,
            active_remote,
            uri: format!("rtsp://{local}/{session_no}"),
            stream_id: 0,
            events: mpsc::channel(1).1,
            tasks: Vec::new(),
        };
        let setup = dict(vec![
            ("deviceID", text(mac.clone())),
            ("sessionUUID", text(upper_uuid())),
            ("timingPort", Value::Integer(i64::from(timing_port).into())),
            ("timingProtocol", text("NTP")),
            ("isMultiSelectAirPlay", Value::Boolean(true)),
            ("groupContainsGroupLeader", Value::Boolean(false)),
            ("macAddress", text(mac)),
            ("model", text("iPhone14,3")),
            ("name", text("Corrent")),
            ("osBuildVersion", text("20F66")),
            ("osName", text("iPhone OS")),
            ("osVersion", text("16.5")),
            ("senderSupportsRelay", Value::Boolean(false)),
            ("sourceVersion", text("690.7.1")),
            ("statsCollectionEnabled", Value::Boolean(false)),
        ]);
        let uri = s.uri.clone();
        let answer = s.rtsp("SETUP", &uri, Some(&setup), &[]).await?;
        let event_port = read_plist(&answer)
            .and_then(|v| v.as_dictionary().and_then(|d| d.get("eventPort")).and_then(Value::as_unsigned_integer))
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| anyhow::anyhow!("the device named no event channel"))?;
        // The event channel: we connect, the device asks, we answer. Its keys are the other way
        // round than the names say, since the requests come from the device.
        let mut events = None;
        for _ in 0..5 {
            match Conn::connect(SocketAddr::new(addr.ip(), event_port)).await {
                Ok(c) => {
                    events = Some(c);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        }
        let mut events = events.ok_or_else(|| anyhow::anyhow!("the device's event channel didn't open"))?;
        events.secure(
            &pair::derive("Events-Salt", "Events-Read-Encryption-Key", &shared)?,
            &pair::derive("Events-Salt", "Events-Write-Encryption-Key", &shared)?,
        )?;
        let (tx, rx) = mpsc::channel(64);
        s.events = rx;
        tasks.push(tokio::spawn(async move {
            loop {
                let Ok((first, headers, body)) = events.incoming().await else {
                    let _ = tx.send(Event::Closed).await;
                    return;
                };
                let protocol = first.rsplit(' ').next().unwrap_or("RTSP/1.0").to_string();
                let mut reply = format!("{protocol} 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0\r\n");
                for name in ["Server", "CSeq"] {
                    if let Some((_, v)) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name))
                        && !v.contains(['\r', '\n'])
                    {
                        reply.push_str(&format!("{name}: {v}\r\n"));
                    }
                }
                reply.push_str("\r\n");
                if events.send_text(reply.as_bytes()).await.is_err() {
                    let _ = tx.send(Event::Closed).await;
                    return;
                }
                let e = match unwrapped(&body) {
                    Some(v) => event(&v),
                    None if body.is_empty() => continue,
                    None => Event::Other(format!("{first} · {} bytes", body.len())),
                };
                if tx.send(e).await.is_err() {
                    return;
                }
            }
        }));
        s.tasks = tasks;
        s.feedback().await?;
        s.rtsp("RECORD", &uri, None, &[]).await?;
        // The stream commands go over.
        let stream = dict(vec![(
            "streams",
            Value::Array(vec![dict(vec![
                ("type", Value::Integer(130.into())),
                ("controlType", Value::Integer(1.into())),
                ("channelID", text(format!("{}-RCS-2", receiver_id.to_ascii_uppercase()))),
                ("clientUUID", text(upper_uuid())),
                ("clientTypeUUID", text("A6B27562-B43A-4F2D-B75F-82391E250194")),
            ])]),
        )]);
        let answer = s.rtsp("SETUP", &uri, Some(&stream), &[]).await?;
        s.stream_id = read_plist(&answer)
            .and_then(|v| {
                v.as_dictionary()?
                    .get("streams")?
                    .as_array()?
                    .first()?
                    .as_dictionary()?
                    .get("streamID")?
                    .as_signed_integer()
            })
            .ok_or_else(|| anyhow::anyhow!("the device opened no stream for commands"))?;
        Ok(s)
    }

    /// One RTSP request; the answer's body. An answer of 400 or more is an error.
    async fn rtsp(
        &mut self,
        method: &str,
        uri: &str,
        body: Option<&Value>,
        extra: &[(&str, String)],
    ) -> anyhow::Result<Vec<u8>> {
        let mut headers = vec![
            ("User-Agent", "AirPlay/550.10".to_string()),
            ("CSeq", self.cseq.to_string()),
            ("DACP-ID", self.remote_id.clone()),
            ("Active-Remote", self.active_remote.clone()),
            ("Client-Instance", self.remote_id.clone()),
        ];
        self.cseq += 1;
        let bytes = match body {
            Some(v) => {
                headers.push(("Content-Type", "application/x-apple-binary-plist".to_string()));
                bplist(v)?
            }
            None => Vec::new(),
        };
        headers.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
        let res = self.conn.request("RTSP/1.0", method, uri, &headers, &bytes).await?;
        if res.status >= 400 {
            anyhow::bail!("the device answered {} to {method} {}", res.status, uri.rsplit('/').next().unwrap_or(uri));
        }
        Ok(res.body)
    }

    /// "Still here": every two seconds, or the device ends the session.
    pub async fn feedback(&mut self) -> anyhow::Result<()> {
        self.rtsp("POST", "/feedback", None, &[]).await.map(|_| ())
    }

    async fn command(&mut self, command: Value) -> anyhow::Result<()> {
        let body = wrapped(&command)?;
        let headers = vec![
            ("User-Agent", "AirPlay/550.10".to_string()),
            ("CSeq", self.cseq.to_string()),
            ("DACP-ID", self.remote_id.clone()),
            ("Active-Remote", self.active_remote.clone()),
            ("Client-Instance", self.remote_id.clone()),
            ("Content-Type", "application/x-apple-binary-plist".to_string()),
            ("X-Apple-StreamID", self.stream_id.to_string()),
        ];
        self.cseq += 1;
        let res = self.conn.request("RTSP/1.0", "POST", "/command", &headers, &body).await?;
        if res.status >= 400 {
            let kind = command.as_dictionary().and_then(|d| d.get("type")).and_then(Value::as_string).unwrap_or("?");
            anyhow::bail!("the device answered {} to the command {kind}", res.status);
        }
        Ok(())
    }

    /// Puts `url` on the device's queue and starts it at `start` seconds. `hls`: a playlist
    /// (`.m3u8`) rather than a file.
    pub async fn play(&mut self, url: &str, start: f64, hls: bool) -> anyhow::Result<String> {
        let zero = || {
            dict(vec![
                ("flags", Value::Integer(0.into())),
                ("value", Value::Integer(0.into())),
                ("epoch", Value::Integer(0.into())),
                ("timescale", Value::Integer(0.into())),
            ])
        };
        let uuid = upper_uuid();
        let mut item = vec![
            ("uuid", text(uuid.clone())),
            ("Content-Location", text(url)),
            ("mediaType", text(if hls { "streaming" } else { "file" })),
            ("playbackRestrictions", Value::Integer(0.into())),
            ("referenceRestrictions", Value::Integer(3.into())),
            ("supportsIntegratedTimeline", Value::Boolean(false)),
            ("allowedCommonMediaClientDataKeys", Value::Integer(0.into())),
            ("snapTimeToPausePlayback", Value::Boolean(false)),
            ("clientBundleID", text("com.apple.WebKit.GPU")),
        ];
        if hls {
            item.push(("timeToPausePlayback", zero()));
            item.push(("timeToPauseBuffering", zero()));
        }
        if !hls || start > 0.0 {
            item.push(("Start-Position", cm_time(start)));
        }
        self.command(dict(vec![("type", text("insertPlayQueueItem")), ("item", dict(item))])).await?;
        self.command(dict(vec![
            ("type", text("setProperty")),
            ("property", text("actionAtItemEnd")),
            ("value", Value::Integer(1.into())),
        ]))
        .await?;
        self.set_rate(1.0).await?;
        Ok(uuid)
    }

    /// 1: play, 0: pause.
    pub async fn set_rate(&mut self, rate: f64) -> anyhow::Result<()> {
        self.command(dict(vec![("type", text("setRate")), ("rate", Value::Real(rate))])).await
    }

    /// Jumps to `seconds` in the item [`Session::play`] named `item`. (Found by trying on a
    /// Samsung TV: the command isn't in any description of the protocol.)
    pub async fn seek(&mut self, item: &str, seconds: f64) -> anyhow::Result<()> {
        self.command(dict(vec![
            ("type", text("seek")),
            ("item", dict(vec![("uuid", text(item))])),
            ("time", cm_time(seconds)),
        ]))
        .await
    }

    /// Asks where playback is; the answer comes as an event.
    pub async fn ask_state(&mut self, id: i64) -> anyhow::Result<()> {
        self.command(dict(vec![
            ("type", text("playbackInfo")),
            ("kind", text("request")),
            ("messageID", Value::Integer(id.into())),
        ]))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_events_wrap_and_unwrap() {
        let cmd = dict(vec![("type", text("setRate")), ("rate", Value::Real(1.0))]);
        let back = unwrapped(&wrapped(&cmd).unwrap()).unwrap();
        assert_eq!(back, cmd);
        assert!(unwrapped(b"not a plist").is_none());
        let state = dict(vec![
            ("type", text("playbackState")),
            ("name", text("playing")),
            (
                "params",
                dict(vec![("position", cm_time(12.5)), ("duration", Value::Real(600.0)), ("rate", Value::Real(1.0))]),
            ),
        ]);
        assert_eq!(
            event(&state),
            Event::State {
                name: "playing".into(),
                position: Some(12.5),
                duration: Some(600.0),
                rate: Some(1.0),
                reason: None
            }
        );
        // As a Samsung TV answers a state request, and reports a file it can't play.
        let info = dict(vec![
            ("type", text("playbackInfo")),
            ("kind", text("response")),
            (
                "info",
                dict(vec![
                    ("playbackState", text("playing")),
                    ("readyToPlay", Value::Boolean(true)),
                    ("rate", Value::Integer(1.into())),
                    ("position", cm_time(6.794)),
                    ("duration", cm_time(40.0)),
                ]),
            ),
        ]);
        assert_eq!(
            event(&info),
            Event::Info {
                state: "playing".into(),
                position: Some(6.794),
                duration: Some(40.0),
                rate: Some(1.0),
                ready: true
            }
        );
        let failed =
            dict(vec![("type", text("error")), ("error", dict(vec![("code", Value::Integer((-12927).into()))]))]);
        assert_eq!(event(&failed), Event::Failed(-12927));
        let end = dict(vec![("type", text("notification")), ("name", text("itemPlayedToEnd"))]);
        assert!(matches!(event(&end), Event::Notification { name, .. } if name == "itemPlayedToEnd"));
    }

    #[test]
    fn timing_requests_are_answered_with_their_send_time() {
        let mut ask = [0u8; 32];
        ask[0] = 0x80;
        ask[1] = 0xd2;
        ask[2..4].copy_from_slice(&[0, 7]);
        ask[24..32].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let reply = timing_reply(&ask).unwrap();
        assert_eq!((reply[1], &reply[2..4], &reply[8..16]), (0xd3, &[0u8, 7][..], &[1u8, 2, 3, 4, 5, 6, 7, 8][..]));
        assert!(timing_reply(&ask[..8]).is_none());
    }

    /// Serves a folder to devices on the network for a while, with Range requests, and prints
    /// who asks for what: `SERVE_DIR=<folder> SERVE_PORT=<port> [SERVE_SECS=120] cargo test -p
    /// corrent-server serve_a_folder -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "opens a port on the network"]
    async fn serve_a_folder() {
        let dir = std::path::PathBuf::from(std::env::var("SERVE_DIR").expect("SERVE_DIR"));
        let port: u16 = std::env::var("SERVE_PORT").expect("SERVE_PORT").parse().unwrap();
        let secs: u64 = std::env::var("SERVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let dir = dir.clone();
            async move {
                println!(
                    "{} {} range {:?} agent {:?}",
                    req.method(),
                    req.uri(),
                    req.headers().get("range"),
                    req.headers().get("user-agent")
                );
                tower::ServiceExt::oneshot(tower_http::services::ServeDir::new(&dir), req).await
            }
        });
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.unwrap();
        println!("serving on port {port}");
        let _ = tokio::time::timeout(Duration::from_secs(secs), axum::serve(listener, app)).await;
    }

    /// Plays a link on a paired real device and prints what it reports for a while:
    /// `AIRPLAY_ADDR=… AIRPLAY_CREDS=<pairing file> AIRPLAY_ID=<deviceid> AIRPLAY_URL=<link the
    /// device can fetch> [AIRPLAY_SECS=40] cargo test -p corrent-server play_on_a_real_device --
    /// --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "plays on a real device"]
    async fn play_on_a_real_device() {
        let addr: SocketAddr = std::env::var("AIRPLAY_ADDR").expect("AIRPLAY_ADDR").parse().unwrap();
        let creds: Credentials =
            serde_json::from_str(&std::fs::read_to_string(std::env::var("AIRPLAY_CREDS").unwrap()).unwrap()).unwrap();
        let id = std::env::var("AIRPLAY_ID").expect("AIRPLAY_ID");
        let url = std::env::var("AIRPLAY_URL").expect("AIRPLAY_URL");
        let secs: u64 = std::env::var("AIRPLAY_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
        let local = crate::cast::local_ip_towards(addr).unwrap();
        let mut s = Session::open(addr, &creds, &id, local).await.expect("session");
        println!("session open; commands go over stream {}", s.stream_id);
        let start: f64 = std::env::var("AIRPLAY_START").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let uuid = s.play(&url, start, url.contains(".m3u8")).await.expect("queued");
        println!("queued {url}");
        let until = std::time::Instant::now() + Duration::from_secs(secs);
        let mut beat = tokio::time::interval(Duration::from_secs(2));
        let mut n = 0i64;
        while std::time::Instant::now() < until {
            tokio::select! {
                e = s.events.recv() => match e {
                    Some(Event::Closed) | None => { println!("the device closed the event channel"); break; }
                    Some(e) => println!("{:>5.1}s  {e:?}", secs as f64 - until.saturating_duration_since(std::time::Instant::now()).as_secs_f64()),
                },
                _ = beat.tick() => {
                    n += 1;
                    if let Err(e) = s.feedback().await { println!("feedback: {e}"); break; }
                    if n % 5 == 0 && let Err(e) = s.ask_state(n).await { println!("playbackInfo: {e}"); }
                    // AIRPLAY_TRY=1: ways a seek might be asked for, one every four seconds.
                    if std::env::var("AIRPLAY_TRY").is_ok() && n >= 4 && n % 2 == 0 {
                        let to = cm_time(30.0);
                        let it = || dict(vec![("uuid", text(uuid.clone()))]);
                        let tries: Vec<Value> = vec![
                            dict(vec![("type", text("seekToTime")), ("item", it()), ("time", to.clone())]),
                            dict(vec![("type", text("setProperty")), ("property", text("currentTime")), ("item", it()), ("value", to.clone())]),
                            dict(vec![("type", text("setProperty")), ("property", text("position")), ("item", it()), ("value", to.clone())]),
                            dict(vec![("type", text("seek")), ("item", it()), ("time", to.clone())]),
                            dict(vec![("type", text("seekToTime")), ("item", it()), ("time", to.clone()), ("toleranceBefore", cm_time(0.0)), ("toleranceAfter", cm_time(0.0))]),
                            dict(vec![("type", text("setProperty")), ("property", text("currentTime")), ("item", it()), ("value", Value::Real(30.0))]),
                        ];
                        if let Some(c) = tries.get((n as usize - 4) / 2) {
                            println!("trying {c:?}: {:?}", s.command(c.clone()).await.map_err(|e| e.to_string()));
                        }
                        continue;
                    }
                    if std::env::var("AIRPLAY_TRY").is_ok() { continue; }
                    if n == 10 { println!("pausing"); let _ = s.set_rate(0.0).await; }
                    if n == 13 { println!("resuming"); let _ = s.set_rate(1.0).await; }
                }
            }
        }
        println!("closing the session (that ends playback)");
    }
}
