//! HomeKit pairing with an AirPlay receiver, as the open-source senders do it
//! (docs/spikes/airplay-protocol.md): pair-setup with the PIN the device shows (once; it gives the
//! long-term keys kept in [`Credentials`]), and pair-verify at the start of every session, which
//! gives the keys of the encrypted channel.

use std::net::SocketAddr;

use aws_lc_rs::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use aws_lc_rs::{agreement, hkdf, rand};
use serde::{Deserialize, Serialize};

use super::conn::Conn;
use super::{srp, tlv8};

/// What pairing leaves behind: who the device is and who we are to it. Secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// The device's long-term Ed25519 public key, hex.
    pub device_key: String,
    /// The device's pairing identifier, hex.
    pub device_id: String,
    /// The seed of our Ed25519 key, hex.
    pub seed: String,
    /// Our pairing identifier (a UUID).
    pub client_id: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(…)")
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len()
        .is_multiple_of(2)
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())
        .flatten()
}

fn random<const N: usize>() -> anyhow::Result<[u8; N]> {
    let mut b = [0u8; N];
    rand::fill(&mut b).map_err(|_| anyhow::anyhow!("no random numbers"))?;
    Ok(b)
}

struct Len32;

impl hkdf::KeyType for Len32 {
    fn len(&self) -> usize {
        32
    }
}

/// HKDF-SHA512 (extract and expand) to 32 bytes.
pub(super) fn derive(salt: &str, info: &str, secret: &[u8]) -> anyhow::Result<[u8; 32]> {
    let mut out = [0u8; 32];
    hkdf::Salt::new(hkdf::HKDF_SHA512, salt.as_bytes())
        .extract(secret)
        .expand(&[info.as_bytes()], Len32)
        .and_then(|okm| okm.fill(&mut out))
        .map_err(|_| anyhow::anyhow!("key derivation failed"))?;
    Ok(out)
}

/// The nonce of a pairing message: four zero bytes, then its eight-letter label.
fn label(l: &[u8; 8]) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(l);
    Nonce::assume_unique_for_key(n)
}

fn cipher(key: &[u8; 32]) -> anyhow::Result<LessSafeKey> {
    Ok(LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| anyhow::anyhow!("bad key"))?))
}

pub(super) fn seal(key: &[u8; 32], l: &[u8; 8], plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut data = plain.to_vec();
    cipher(key)?
        .seal_in_place_append_tag(label(l), Aad::empty(), &mut data)
        .map_err(|_| anyhow::anyhow!("sealing failed"))?;
    Ok(data)
}

pub(super) fn open(key: &[u8; 32], l: &[u8; 8], sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut data = sealed.to_vec();
    let plain = cipher(key)?
        .open_in_place(label(l), Aad::empty(), &mut data)
        .map_err(|_| anyhow::anyhow!("the device's message doesn't open: the keys don't match"))?;
    Ok(plain.to_vec())
}

fn headers() -> Vec<(&'static str, String)> {
    vec![
        ("User-Agent", "AirPlay/320.20".to_string()),
        ("Connection", "keep-alive".to_string()),
        ("X-Apple-HKP", "3".to_string()),
        ("Content-Type", "application/octet-stream".to_string()),
    ]
}

/// One pairing message out, the device's answer in: its items, or its refusal in words.
async fn exchange(
    conn: &mut Conn,
    path: &str,
    items: &[(u8, &[u8])],
) -> anyhow::Result<std::collections::BTreeMap<u8, Vec<u8>>> {
    let res = conn.request("HTTP/1.1", "POST", path, &headers(), &tlv8::encode(items)).await?;
    if res.status != 200 {
        anyhow::bail!("the device answered {} to {path}", res.status);
    }
    let items = tlv8::decode(&res.body).ok_or_else(|| anyhow::anyhow!("the device's answer to {path} is cut short"))?;
    if let Some(code) = items.get(&tlv8::ERROR).and_then(|e| e.first()) {
        let wait = items
            .get(&tlv8::BACK_OFF)
            .map(|b| b.iter().rev().fold(0u64, |n, x| n << 8 | u64::from(*x)))
            .map(|s| format!(" (try again in {s} s)"))
            .unwrap_or_default();
        anyhow::bail!("{}{wait}", tlv8::error_text(*code));
    }
    Ok(items)
}

fn item<'a>(items: &'a std::collections::BTreeMap<u8, Vec<u8>>, tag: u8, what: &str) -> anyhow::Result<&'a [u8]> {
    items.get(&tag).map(Vec::as_slice).ok_or_else(|| anyhow::anyhow!("the device's answer lacks its {what}"))
}

/// A pairing under way: the device shows its PIN and waits for [`Setup::finish`].
pub struct Setup {
    conn: Conn,
    client: srp::Client,
    salt: Vec<u8>,
    device_public: Vec<u8>,
}

/// Asks the device to show a PIN and starts the exchange.
pub async fn start(addr: SocketAddr) -> anyhow::Result<Setup> {
    let mut conn = Conn::connect(addr).await?;
    let res = conn.request("HTTP/1.1", "POST", "/pair-pin-start", &headers(), &[]).await?;
    if res.status != 200 {
        anyhow::bail!("the device answered {} when asked to show a PIN", res.status);
    }
    let m2 = exchange(&mut conn, "/pair-setup", &[(tlv8::METHOD, &[0]), (tlv8::STATE, &[1])]).await?;
    let salt = item(&m2, tlv8::SALT, "salt")?.to_vec();
    let device_public = item(&m2, tlv8::PUBLIC_KEY, "public key")?.to_vec();
    // A secret whose public value has no leading zero byte (see `srp::Client::new`).
    let client = loop {
        if let Some(c) = srp::Client::new(&random::<32>()?) {
            break c;
        }
    };
    Ok(Setup { conn, client, salt, device_public })
}

impl Setup {
    /// The rest of the exchange with the PIN from the device's screen. `name`: what the device
    /// may show for us in its list of paired senders.
    pub async fn finish(mut self, pin: &str, name: &str) -> anyhow::Result<Credentials> {
        let pin: String = pin.chars().filter(char::is_ascii_digit).collect();
        if pin.is_empty() || pin.len() > 8 {
            anyhow::bail!("the PIN is the number on the device's screen");
        }
        let pin = format!("{pin:0>4}");
        let session = self
            .client
            .session("Pair-Setup", &pin, &self.salt, &self.device_public)
            // One exchange in a hundred or so can't be finished unambiguously: a fresh one can.
            .ok_or_else(|| anyhow::anyhow!("this attempt can't be completed: start pairing again"))?;
        let m4 = exchange(
            &mut self.conn,
            "/pair-setup",
            &[(tlv8::STATE, &[3]), (tlv8::PUBLIC_KEY, &self.client.public), (tlv8::PROOF, &session.proof)],
        )
        .await?;
        // The device knows the PIN too: only then is it the one whose screen was read.
        if item(&m4, tlv8::PROOF, "proof")? != session.expected.as_slice() {
            anyhow::bail!("the device's proof is wrong: this isn't the device that showed the PIN");
        }
        let seed: [u8; 32] = random()?;
        let ours = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| anyhow::anyhow!("no signing key"))?;
        let client_id = uuid_v4(&random::<16>()?);
        let key = derive("Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info", &session.key)?;
        let signed = [
            derive("Pair-Setup-Controller-Sign-Salt", "Pair-Setup-Controller-Sign-Info", &session.key)?.as_slice(),
            client_id.as_bytes(),
            ours.public_key().as_ref(),
        ]
        .concat();
        let signature = ours.sign(&signed);
        // {"name": name} in OPACK, as the device's list of senders shows it.
        let short: String = name.chars().take(32).collect::<String>();
        let short = if short.len() > 32 { "Corrent".to_string() } else { short };
        let mut named = vec![0xE1, 0x44, b'n', b'a', b'm', b'e', 0x40 + short.len() as u8];
        named.extend_from_slice(short.as_bytes());
        let inner = tlv8::encode(&[
            (tlv8::IDENTIFIER, client_id.as_bytes()),
            (tlv8::PUBLIC_KEY, ours.public_key().as_ref()),
            (tlv8::SIGNATURE, signature.as_ref()),
            (0x11, &named),
        ]);
        let m6 = exchange(
            &mut self.conn,
            "/pair-setup",
            &[(tlv8::STATE, &[5]), (tlv8::ENCRYPTED_DATA, &seal(&key, b"PS-Msg05", &inner)?)],
        )
        .await?;
        let theirs = open(&key, b"PS-Msg06", item(&m6, tlv8::ENCRYPTED_DATA, "keys")?)?;
        let theirs = tlv8::decode(&theirs).ok_or_else(|| anyhow::anyhow!("the device's keys are cut short"))?;
        let device_id = item(&theirs, tlv8::IDENTIFIER, "identifier")?;
        let device_key = item(&theirs, tlv8::PUBLIC_KEY, "long-term key")?;
        if device_key.len() != 32 {
            anyhow::bail!("the device's long-term key isn't one");
        }
        // The device signs its key with the session's secret. The proof above already tied this
        // session to the PIN; a signature that doesn't check is noted, not fatal (the strings
        // it is made with are HomeKit's, which not every receiver is known to follow).
        if let Ok(sig) = item(&theirs, tlv8::SIGNATURE, "signature") {
            let salt = derive("Pair-Setup-Accessory-Sign-Salt", "Pair-Setup-Accessory-Sign-Info", &session.key)?;
            let signed = [salt.as_slice(), device_id, device_key].concat();
            if UnparsedPublicKey::new(&ED25519, device_key).verify(&signed, sig).is_err() {
                tracing::info!("AirPlay pairing: the device's signature over its key doesn't check");
            }
        }
        Ok(Credentials { device_key: hex(device_key), device_id: hex(device_id), seed: hex(&seed), client_id })
    }
}

fn uuid_v4(r: &[u8; 16]) -> String {
    let mut b = *r;
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h = hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Proves to the device who we are (and it to us) and switches `conn` to the encrypted channel.
/// Returns the session's shared secret: further channels derive their keys from it.
pub async fn verify(conn: &mut Conn, c: &Credentials) -> anyhow::Result<Vec<u8>> {
    let bad = || anyhow::anyhow!("the stored pairing is damaged: pair again");
    let (device_key, device_id) = (unhex(&c.device_key).ok_or_else(bad)?, unhex(&c.device_id).ok_or_else(bad)?);
    let seed = unhex(&c.seed).ok_or_else(bad)?;
    let ours = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| bad())?;
    let rng = rand::SystemRandom::new();
    let private = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
        .map_err(|_| anyhow::anyhow!("no session key"))?;
    let public = private.compute_public_key().map_err(|_| anyhow::anyhow!("no session key"))?.as_ref().to_vec();
    let m2 = exchange(conn, "/pair-verify", &[(tlv8::STATE, &[1]), (tlv8::PUBLIC_KEY, &public)]).await?;
    let their_public = item(&m2, tlv8::PUBLIC_KEY, "session key")?.to_vec();
    let shared = agreement::agree_ephemeral(
        private,
        agreement::UnparsedPublicKey::new(&agreement::X25519, &their_public),
        anyhow::anyhow!("the device's session key isn't one"),
        |s| Ok(s.to_vec()),
    )?;
    let key = derive("Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info", &shared)?;
    let inner = open(&key, b"PV-Msg02", item(&m2, tlv8::ENCRYPTED_DATA, "proof")?)?;
    let inner = tlv8::decode(&inner).ok_or_else(|| anyhow::anyhow!("the device's proof is cut short"))?;
    if item(&inner, tlv8::IDENTIFIER, "identifier")? != device_id.as_slice() {
        anyhow::bail!("another device answers at this address: pair again");
    }
    let signed = [their_public.as_slice(), &device_id, &public].concat();
    UnparsedPublicKey::new(&ED25519, &device_key)
        .verify(&signed, item(&inner, tlv8::SIGNATURE, "signature")?)
        .map_err(|_| anyhow::anyhow!("the device isn't the one that was paired: pair again"))?;
    let signature = ours.sign(&[public.as_slice(), c.client_id.as_bytes(), &their_public].concat());
    let reply = tlv8::encode(&[(tlv8::IDENTIFIER, c.client_id.as_bytes()), (tlv8::SIGNATURE, signature.as_ref())]);
    exchange(conn, "/pair-verify", &[(tlv8::STATE, &[3]), (tlv8::ENCRYPTED_DATA, &seal(&key, b"PV-Msg03", &reply)?)])
        .await?;
    conn.secure(
        &derive("Control-Salt", "Control-Write-Encryption-Key", &shared)?,
        &derive("Control-Salt", "Control-Read-Encryption-Key", &shared)?,
    )?;
    Ok(shared)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A verification under way: both session keys, their secret and the key made from it.
    type Verifying = (Vec<u8>, Vec<u8>, Vec<u8>, [u8; 32]);
    /// The sender a test receiver was paired with: its identifier and key.
    type Paired = std::sync::Arc<std::sync::Mutex<Option<(Vec<u8>, Vec<u8>)>>>;

    /// A receiver that pairs with `pin`: the device's side of pair-setup, pair-verify and the
    /// encrypted channel, built from the same pieces. `served`: what it answers `GET /info`
    /// with once the channel is up. Returns its address.
    pub(crate) async fn fake_receiver(pin: &'static str, served: &'static [u8]) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seed: [u8; 32] = random().unwrap();
        // What a sender left behind when it paired: its id and key.
        let paired: Paired = Default::default();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let paired = paired.clone();
                tokio::spawn(async move {
                    let _ = serve(Conn::accepted(stream), pin, served, seed, paired).await;
                });
            }
        });
        addr
    }

    async fn serve(mut conn: Conn, pin: &str, served: &[u8], seed: [u8; 32], paired: Paired) -> anyhow::Result<()> {
        let me = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let my_id = b"AA:BB:CC:DD:EE:FF".to_vec();
        let mut srp_device: Option<(srp::device::Device, Vec<u8>)> = None;
        let mut session_key: Option<Vec<u8>> = None;
        let mut verifying: Option<Verifying> = None;
        loop {
            let (first, body) = conn.next_request().await?;
            let items = tlv8::decode(&body).unwrap_or_default();
            let state = items.get(&tlv8::STATE).and_then(|s| s.first().copied()).unwrap_or(0);
            let refuse = tlv8::encode(&[(tlv8::STATE, &[state + 1]), (tlv8::ERROR, &[2])]);
            match (first.split_whitespace().nth(1).unwrap_or_default(), state) {
                ("/pair-pin-start", _) => conn.respond(200, &[]).await?,
                ("/pair-setup", 1) => {
                    let salt: [u8; 16] = random()?;
                    let d = srp::device::Device::new("Pair-Setup", pin, &salt, &random::<32>()?);
                    let m2 = tlv8::encode(&[(tlv8::STATE, &[2]), (tlv8::SALT, &salt), (tlv8::PUBLIC_KEY, &d.public)]);
                    srp_device = Some((d, salt.to_vec()));
                    conn.respond(200, &m2).await?;
                }
                ("/pair-setup", 3) => {
                    let (d, salt) = srp_device.as_ref().unwrap();
                    let a = &items[&tlv8::PUBLIC_KEY];
                    let key = d.key(a);
                    // What a client that knows the PIN proves: M1 = H(H(N)^H(g) | H(I) | s | A | B | K).
                    let expected = client_proof("Pair-Setup", salt, a, &d.public, &key);
                    if items[&tlv8::PROOF] != expected {
                        conn.respond(200, &refuse).await?;
                        continue;
                    }
                    let proof = srp::device::Device::proof(a, &expected, &key);
                    session_key = Some(key);
                    conn.respond(200, &tlv8::encode(&[(tlv8::STATE, &[4]), (tlv8::PROOF, &proof)])).await?;
                }
                ("/pair-setup", 5) => {
                    let k = session_key.as_ref().unwrap();
                    let key = derive("Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info", k)?;
                    let inner = tlv8::decode(&open(&key, b"PS-Msg05", &items[&tlv8::ENCRYPTED_DATA])?).unwrap();
                    let (id, their_key) = (inner[&tlv8::IDENTIFIER].clone(), inner[&tlv8::PUBLIC_KEY].clone());
                    let signed = [
                        derive("Pair-Setup-Controller-Sign-Salt", "Pair-Setup-Controller-Sign-Info", k)?.as_slice(),
                        &id,
                        &their_key,
                    ]
                    .concat();
                    UnparsedPublicKey::new(&ED25519, &their_key).verify(&signed, &inner[&tlv8::SIGNATURE]).unwrap();
                    assert!(inner.contains_key(&0x11), "the sender names itself");
                    *paired.lock().unwrap() = Some((id, their_key));
                    let salt = derive("Pair-Setup-Accessory-Sign-Salt", "Pair-Setup-Accessory-Sign-Info", k)?;
                    let sig = me.sign(&[salt.as_slice(), &my_id, me.public_key().as_ref()].concat());
                    let mine = tlv8::encode(&[
                        (tlv8::IDENTIFIER, &my_id),
                        (tlv8::PUBLIC_KEY, me.public_key().as_ref()),
                        (tlv8::SIGNATURE, sig.as_ref()),
                    ]);
                    let m6 =
                        tlv8::encode(&[(tlv8::STATE, &[6]), (tlv8::ENCRYPTED_DATA, &seal(&key, b"PS-Msg06", &mine)?)]);
                    conn.respond(200, &m6).await?;
                }
                ("/pair-verify", 1) => {
                    let rng = rand::SystemRandom::new();
                    let private = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).unwrap();
                    let public = private.compute_public_key().unwrap().as_ref().to_vec();
                    let theirs = items[&tlv8::PUBLIC_KEY].clone();
                    let shared = agreement::agree_ephemeral(
                        private,
                        agreement::UnparsedPublicKey::new(&agreement::X25519, &theirs),
                        anyhow::anyhow!("x"),
                        |s| Ok(s.to_vec()),
                    )?;
                    let key = derive("Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info", &shared)?;
                    let sig = me.sign(&[public.as_slice(), &my_id, &theirs].concat());
                    let inner = tlv8::encode(&[(tlv8::IDENTIFIER, &my_id), (tlv8::SIGNATURE, sig.as_ref())]);
                    let m2 = tlv8::encode(&[
                        (tlv8::STATE, &[2]),
                        (tlv8::PUBLIC_KEY, &public),
                        (tlv8::ENCRYPTED_DATA, &seal(&key, b"PV-Msg02", &inner)?),
                    ]);
                    verifying = Some((public, theirs, shared, key));
                    conn.respond(200, &m2).await?;
                }
                ("/pair-verify", 3) => {
                    let (public, theirs, shared, key) = verifying.take().unwrap();
                    let inner = tlv8::decode(&open(&key, b"PV-Msg03", &items[&tlv8::ENCRYPTED_DATA])?).unwrap();
                    let known = paired.lock().unwrap().clone();
                    let ok = known.is_some_and(|(id, their_key)| {
                        inner[&tlv8::IDENTIFIER] == id
                            && UnparsedPublicKey::new(&ED25519, &their_key)
                                .verify(&[theirs.as_slice(), &id, &public].concat(), &inner[&tlv8::SIGNATURE])
                                .is_ok()
                    });
                    if !ok {
                        conn.respond(200, &refuse).await?;
                        continue;
                    }
                    conn.respond(200, &tlv8::encode(&[(tlv8::STATE, &[4])])).await?;
                    // The device reads with the sender's write key.
                    conn.secure(
                        &derive("Control-Salt", "Control-Read-Encryption-Key", &shared)?,
                        &derive("Control-Salt", "Control-Write-Encryption-Key", &shared)?,
                    )?;
                }
                ("/info", _) => conn.respond(200, served).await?,
                _ => conn.respond(404, &[]).await?,
            }
        }
    }

    /// The client's proof as the device recomputes it.
    fn client_proof(user: &str, salt: &[u8], a: &[u8], b: &[u8], key: &[u8]) -> Vec<u8> {
        use aws_lc_rs::digest::{SHA512, digest};
        let h = |parts: &[&[u8]]| digest(&SHA512, &parts.concat()).as_ref().to_vec();
        let n = srp::modulus();
        let mixed: Vec<u8> = h(&[&n]).iter().zip(h(&[&[5u8]])).map(|(x, y)| x ^ y).collect();
        h(&[&mixed, &h(&[user.as_bytes()]), salt, a, b, key])
    }

    /// Pairs with a real device (it shows a PIN on its screen) and keeps the result:
    /// `AIRPLAY_ADDR=192.168.1.20:7000 AIRPLAY_PIN_FILE=<file the PIN is then written to>
    /// AIRPLAY_CREDS=<file for the pairing> cargo test -p corrent-server pair_with_a_real_device
    /// -- --ignored --nocapture`. With the pairing already in `AIRPLAY_CREDS`, only verifies it
    /// and asks the device a few harmless things inside the channel.
    #[tokio::test]
    #[ignore = "pairs with a real device"]
    async fn pair_with_a_real_device() {
        let addr: SocketAddr = std::env::var("AIRPLAY_ADDR").expect("AIRPLAY_ADDR").parse().unwrap();
        let file = std::path::PathBuf::from(std::env::var("AIRPLAY_CREDS").expect("AIRPLAY_CREDS"));
        let creds: Credentials = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap(),
            Err(_) => {
                let pin_file = std::path::PathBuf::from(std::env::var("AIRPLAY_PIN_FILE").expect("AIRPLAY_PIN_FILE"));
                let _ = std::fs::remove_file(&pin_file);
                let setup = start(addr).await.expect("the device shows a PIN");
                println!("the device shows a PIN now; waiting for it in {}", pin_file.display());
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
                let pin = loop {
                    if let Ok(pin) = std::fs::read_to_string(&pin_file) {
                        break pin;
                    }
                    assert!(std::time::Instant::now() < deadline, "no PIN came");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                };
                let creds = setup.finish(&pin, "Corrent").await.expect("paired");
                std::fs::write(&file, serde_json::to_string(&creds).unwrap()).unwrap();
                println!(
                    "paired; the device calls itself {:?}",
                    String::from_utf8_lossy(&unhex(&creds.device_id).unwrap())
                );
                creds
            }
        };
        let mut conn = Conn::connect(addr).await.unwrap();
        verify(&mut conn, &creds).await.expect("verified");
        println!("verified: the channel is encrypted");
        for path in ["/info", "/playback-info", "/server-info"] {
            let headers = [("User-Agent", "AirPlay/550.10".to_string())];
            match conn.request("HTTP/1.1", "GET", path, &headers, &[]).await {
                Ok(r) => println!("GET {path}: {} · {} bytes · {:?}", r.status, r.body.len(), r.header("Content-Type")),
                Err(e) => println!("GET {path}: {e}"),
            }
        }
    }

    #[tokio::test]
    async fn pairs_with_the_pin_and_talks_inside_the_channel() {
        // More than one frame's worth, to cross the 1024-byte framing both ways.
        static INFO: [u8; 3000] = [0x5a; 3000];
        let addr = fake_receiver("0412", &INFO).await;
        // A wrong PIN is refused, in words.
        let e = start(addr).await.unwrap().finish("9999", "Corrent").await.unwrap_err();
        assert!(e.to_string().contains("PIN was wrong"), "{e}");
        // The right one, typed the way people type it.
        let creds = start(addr).await.unwrap().finish(" 412 ", "Corrent on the test bench").await.unwrap();
        assert_eq!(unhex(&creds.device_id).unwrap(), b"AA:BB:CC:DD:EE:FF");
        assert_eq!(creds.client_id.len(), 36);
        // Before verifying, nothing is sealed; after, the channel carries requests of any size.
        let mut conn = Conn::connect(addr).await.unwrap();
        verify(&mut conn, &creds).await.unwrap();
        let long = "x".repeat(2500);
        let res = conn.request("HTTP/1.1", "GET", "/info", &[("X-Long", long)], &[]).await.unwrap();
        assert_eq!((res.status, res.body.len()), (200, 3000));
        let res = conn.request("RTSP/1.0", "GET", "/nothing", &[], &[]).await.unwrap();
        assert_eq!(res.status, 404);
        // Someone else's pairing doesn't verify.
        let mut other = creds.clone();
        other.seed = hex(&[7u8; 32]);
        let mut conn = Conn::connect(addr).await.unwrap();
        let e = verify(&mut conn, &other).await.unwrap_err();
        assert!(e.to_string().contains("isn't known to the device"), "{e}");
    }
}
