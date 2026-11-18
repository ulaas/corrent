# AirPlay 2 sender: HomeKit pairing and playing a URL — implementation note

Read on 2026-11-19 from primary sources. Every fact names its source file.
"pyatv master" = github.com/postlund/pyatv at commit `b277a4c8222ecdcbaab8a24e3e713ca44765adb4` (2026-06-19),
fetched from `raw.githubusercontent.com/postlund/pyatv/master/...`.
Markers: **[CODE]** read in merged source; **[PR]** read in an unmerged pull request (less authority);
**[REPORT]** a user's observation in an issue; **[COMPUTED]** computed locally by me from fetched data;
**UNVERIFIED** not confirmed by anything I fetched.

Sources that could not be fetched: `emanuelecozzi.net/docs/airplay2` (TLS certificate expired; not read).
`openairplay.github.io/airplay-spec` was read for cross-checks only; its `post_pair_setup`, `post_pair_verify`
and `post_command` pages are empty stubs.

## What a Samsung TV did (QE85Q70B, 2022 Tizen, AirPlay 377.40.00; 2026-11-19)

Tried with the code in `crates/corrent-server/src/cast/airplay/`. Everything below this section is
the research it was written from.

- PIN pair-setup (Method 0, `X-Apple-HKP: 3`, Name TLV sent, A as 384 bytes) and pair-verify work
  as sections 2 and 4 say. The identifier the TV gives in M6 is not its mDNS `deviceid`.
- Inside the channel `GET /info` answers 200, `/playback-info` and `/server-info` 404.
- Section 6.1 (SETUP, event channel, feedback, RECORD) and the #2941 variant of 6B work: commands
  as `POST /command RTSP/1.0` with `X-Apple-StreamID`; `channelID` with either the mDNS id or the
  pairing identifier.
- An MP4 with `moov` at the end: fetched (`bytes=0-`, agent `samsung-agent/1.1`), then
  `{type: error, error: {code: -12927}}` and `itemRemoved`. A faststart MP4 and HLS (MPEG-TS
  segments, fetched with agent `AirPlay/2.0 … MFi_AirPlay_Device`) play. Plain-HTTP LAN links are fine.
- `Start-Position` as a CMTime (value in ms, timescale 1000) works for HLS (it starts at the
  segment before).
- State arrives as `playbackState` events (`loading`, `playing` with position/duration/rate,
  `paused`, `stopped` with `reason: ended`) and notifications (`itemPlayedToEnd`, `timeJumped`,
  `loadedTimeRangesChanged`, `playbackLikelyToKeepUp`, `currentItemChanged`). A `playbackInfo`
  request is answered on the event channel with `info.position`/`duration` as CMTimes,
  `playbackState`, `readyToPlay`, `rate`.
- **Seek** (in no source): `{type: "seek", item: {uuid: <the queued item's>}, time: <CMTime>}`.
  `seekToTime`, `setProperty currentTime/position`, and `seek` without the item are answered 200
  and do nothing.
- Closing the control connection ends playback.

## 0. The main finding for a Samsung TV (read this first)

The target announces features `0x038bcf46007f8ad0` = bits
`4, 6, 7, 9, 11, 15-22, 33, 34, 38, 40-43, 46, 47, 48, 49, 51, 55, 56, 57` [COMPUTED]; status `0x244` = bits 2, 6, 9.
Bit 0 (video v1) is absent, bit 49 (pyatv: `SupportsAirPlayVideoV2`) is present.

- pyatv **master** has one URL player only: `POST /play` then polling `GET /playback-info` (section 6A). There is no
  "v1 player / v2 player" choice by feature bit in master; the only choice is the *protocol class*
  (`AirPlayV1` vs `AirPlayV2`, section 6.0), and both end in `POST /play`.
- On receivers with bit 49 but not bit 0, `/play` and `/playback-info` do not exist (LG webOS answers 404) [PR #2941, REPORT].
  On current tvOS (26/27) `/play` answers 200 but `/playback-info` answers 403 or 500 and nothing plays [REPORT #2846, #2906].
- What works there is the **play-queue flow** (section 6B): a remote-control stream (`SETUP` with a type 130 stream,
  `controlType: 1`), then `POST /command` carrying `insertPlayQueueItem`, `setProperty actionAtItemEnd`, `setRate`;
  state comes back as `POST /command` requests on the event channel. This exists only in open PRs
  (#2941 tested on an LG; #2846 lists "Samsung Frame 2025" in an **unchecked** test plan), not in master.
- So for this Samsung: implement PIN pair-setup + pair-verify + encrypted control channel + SETUP/event channel/RECORD/feedback
  exactly as in sections 2, 4, 5, 6.1, then try 6B first and 6A as fallback. Nobody has reported a Samsung result
  in the pyatv tracker in detail (section 7).

## 1. TLV8 — `pyatv/auth/hap_tlv8.py` [CODE]

```python
class TlvValue(IntEnum):
    Method = 0x00
    Identifier = 0x01
    Salt = 0x02
    PublicKey = 0x03
    Proof = 0x04
    EncryptedData = 0x05
    SeqNo = 0x06
    Error = 0x07
    BackOff = 0x08
    Certificate = 0x09
    Signature = 0x0A
    Permissions = 0x0B
    FragmentData = 0x0C
    FragmentLast = 0x0D
    # Apple internal(?)
    Name = 0x11
    Flags = 0x13

class Flags(IntEnum):
    TransientPairing = 0x10

class ErrorCode(IntEnum):
    Unknown = 0x01
    Authentication = 0x02
    BackOff = 0x03
    MaxPeers = 0x04
    MaxTries = 0x05
    Unavailable = 0x06
    Busy = 0x07

class Method(IntEnum):
    PairSetup = 0x00
    PairSetupWithAuth = 0x01
    PairVerify = 0x02
    AddPairing = 0x03
    RemovePairing = 0x04
    ListPairing = 0x05

class State(IntEnum):   # values of SeqNo (0x06)
    M1 = 0x01 ... M6 = 0x06
```

- Encoding: `tag (1 byte) | length (1 byte) | value`. A value longer than 255 bytes is written as consecutive
  items with the same tag, each at most 255 bytes (`size = min(length, 255)`); the reader concatenates
  repeated tags (`result[tag] += value`). A 384-byte SRP public key is 255 + 129.
- pyatv's writer emits **nothing** for an empty value (the `while pos < len(value)` loop never runs).
- pyatv's reader concatenates *any* repeated tag, adjacent or not; fine for these messages.
- Items are written in dict insertion order (e.g. M1: Method, then SeqNo).
- `BackOff` value is an integer of seconds, little-endian (`stringify`). pyatv never inspects `Error` in pairing
  replies: a reply with `Error` simply raises `KeyError` on the missing Salt/PublicKey/EncryptedData. A real
  implementation should check tag 0x07 in every reply.

## 2. Pair-setup with a PIN (persistent pairing)

Sources: `pyatv/protocols/airplay/auth/hap.py`, `pyatv/auth/hap_srp.py`, `pyatv/protocols/airplay/pairing.py`,
`pyatv/support/http.py` [CODE].

### 2.1 HTTP requests

All are plain `HTTP/1.1` POSTs on a fresh TCP connection to the AirPlay port (the mDNS `_airplay._tcp` port,
7000 on most receivers), not encrypted. Headers, verbatim:

```python
_AIRPLAY_HEADERS = {
    "User-Agent": "AirPlay/320.20",
    "Connection": "keep-alive",
    "X-Apple-HKP": 3,
    "Content-Type": "application/octet-stream",
}
```

`_format_message` (`support/http.py`) writes: request line, then `Content-Length: <n>` **only if there is a body**,
then the given headers in order. No `CSeq`, no `X-Apple-ProtocolVersion`, no `Host`, no session/device headers
are sent during pairing. Wire form of M1:

```
POST /pair-setup HTTP/1.1\r\n
Content-Length: 6\r\n
User-Agent: AirPlay/320.20\r\n
Connection: keep-alive\r\n
X-Apple-HKP: 3\r\n
Content-Type: application/octet-stream\r\n
\r\n
<00 01 00 06 01 01>
```

Sequence (`AirPlayHapPairSetupProcedure`):

1. `POST /pair-pin-start` — same headers, **no body** (so no Content-Length line). Makes the TV show the PIN.
2. `POST /pair-setup` M1: `{Method(0x00): 00, SeqNo(0x06): 01}`.
   Reply M2: `Salt(0x02)` (16 bytes per HAP; pyatv takes whatever arrives), `PublicKey(0x03)` = SRP B (384 bytes).
3. User types the PIN. `pairing.py`: `self.pin_code = str(pin).zfill(4)` — the SRP password is the PIN as a
   4-character ASCII string, zero-padded on the left, no dashes.
4. `POST /pair-setup` M3: `{SeqNo: 03, PublicKey: A, Proof: M1}`.
   Reply M4 carries `Proof` = server M2 (and SeqNo 04). **pyatv does not read this reply at all** (it neither checks
   the status TLV nor the server proof; its `verify_proof(self._session.key_proof_hash)` call compares its own
   value with itself). Verify M2 yourself: `H(A | M1 | K)`.
5. `POST /pair-setup` M5: `{SeqNo: 05, EncryptedData: <ciphertext+16-byte tag>}`.
   Reply M6: `EncryptedData`.

Note on pyatv's M1: it sends Method `0x00` (PairSetup), not `0x01` (PairSetupWithAuth), and no Flags TLV.
openairplay `pairing/hkp.html` says a receiver with `SupportsUnifiedPairSetupAndMFi` (bit 51, set on this TV) *can*
do "Pair Setup with Auth" and then adds an encrypted MFi blob (tag 0x05) to M4; with Method 0x00 that is not requested.

### 2.2 SRP — exact computation

`hap_srp.py`:

```python
context = SRPContext(
    "Pair-Setup",
    str(pin),
    prime=constants.PRIME_3072,
    generator=constants.PRIME_3072_GEN,
    hash_func=hashlib.sha512,
)
self._session = SRPClientSession(
    context, binascii.hexlify(self._auth_private).decode()
)
```

pyatv pins `srptools==1.0.1` (`requirements/requirements.txt`; minimum `0.2.0` in `base_versions.txt`).
The second positional argument is the client private value `a`: **pyatv uses the 32-byte Ed25519 seed (its long-term
secret key) as SRP `a`**, read as a big-endian integer. That is a pyatv peculiarity, not a protocol requirement —
use a fresh random `a` (>= 256 bits). (srptools master/2.0.0 made `private` keyword-only, so this call only works
with 1.x.)

Group (`srptools/constants.py`, identical to RFC 5054 3072-bit, and to `homebridge/fast-srp` `hap_test_data.ts`):
`g = 5`, N =

```
FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA6
3B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245
E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F2411
7C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F
83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08
CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9
DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D
04507A33A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7
ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864D8760273
3EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E208E24FA074E5AB31
43DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF
```

Formulas as implemented in srptools 1.0.1 `context.py` / `common.py` / `client.py` (H = SHA-512, `|` = concatenation).
In srptools, an **integer** argument to `hash()` is converted with `int_to_bytes` = minimal big-endian bytes
(leading zero bytes stripped); `pad()` left-pads to len(N) = 384 bytes; bytes are hashed as they are.

| Value | Formula | Byte conventions |
|---|---|---|
| I | `"Pair-Setup"` | ASCII |
| P | PIN string, e.g. `"0412"` | ASCII |
| k | `H(N \| PAD(g))` | N minimal (=384 bytes), g padded to 384 bytes |
| x | `H(s \| H(I \| ":" \| P))` | s = salt bytes exactly as received (1.0.1 keeps salt as bytes: `unhexlify`), inner hash as 64 raw bytes |
| A | `g^a mod N` | sent as minimal bytes (`unhexlify(session.public)`) — see caveat |
| u | `H(PAD(A) \| PAD(B))` | both padded to 384 |
| S | `(B - k*g^x) ^ (a + u*x) mod N` | |
| K | `H(S)` | S as **minimal** bytes (not padded); K is 64 bytes |
| M1 | `H( (H(N) xor H(g)) \| H(I) \| s \| A \| B \| K )` | H(N): N minimal; H(g): g minimal (the single byte `05`, **not padded**); A and B **minimal** (not padded); s raw; K 64 bytes |
| M2 | `H(A \| M1 \| K)` | A minimal |

Constants for this group [COMPUTED, and `k` matches `fast-srp/src/test/test_hap_vectors.ts`]:

```
k         = a9c2e2559bf0ebb53f0cbbf62282906bede7f2182f00678211fbd5bde5b285033a4993503b87397f9be5ec02080fedbc0835587ad039060879b8621e8c3659e0
H(N)^H(g) = b3d63ef6aafb2e9796dae006fe60f20e5326fe1e1c52a5021eb21747ca33dfea8ff203bd45a2544395d73d19899c174a683c9e783296dd1693ebc71cf5a53da3
H("Pair-Setup") = cd3fa890ac891aa9a9d23346f1e4f698b01240ad810cf029d5d57fc79f1b1fc4e2c40862dfb3828ed2c58aa80f678a45a7f4d78b9c4ac33fdc5546f2a10430b3
```

**Padding caveat (the one real ambiguity).** srptools hashes A, B and S with leading zero bytes stripped;
`homebridge/fast-srp` (the HAP accessory side used by Homebridge/HAP-NodeJS, `src/srp.ts`) hashes A, B (in M1/M2)
and S (in K) as fixed 384-byte buffers (`toBuffer(params.N_length_bits / 8)`). The two agree unless A, B or S has a
leading zero byte (about 1/256 each). Neither source tells which convention Apple's/Samsung's stack uses when a
leading zero occurs — **UNVERIFIED**. Practical handling: regenerate `a` until A's top byte is non-zero (then A is
unambiguous and is sent as 384 bytes); if B or S turns out to have a leading zero byte and the receiver rejects M1
(Error 0x02), restart pairing. pyatv itself sends A with zeros stripped (so occasionally 383 bytes).
The intermediate ints `H(N) xor H(g)` and `H(I)` are also "minimal" in srptools, but for this group and username
neither starts with a zero byte (values above), so they are always 64 bytes.

Session key handed on: `binascii.unhexlify(self._session.key)` = K (64 bytes, SHA-512 of S). Everything after
SRP uses **K**, not S.

### 2.3 M5 / M6

`hkdf_expand` (used everywhere below):

```python
def hkdf_expand(salt: str, info: str, shared_secret: bytes) -> bytes:
    hkdf = HKDF(algorithm=hashes.SHA512(), length=32, salt=salt.encode(), info=info.encode(), ...)
    return hkdf.derive(shared_secret)      # full HKDF (extract + expand), 32 bytes out
```

`step3` (M5):

```python
ios_device_x = hkdf_expand("Pair-Setup-Controller-Sign-Salt", "Pair-Setup-Controller-Sign-Info", K)
self._session_key = hkdf_expand("Pair-Setup-Encrypt-Salt", "Pair-Setup-Encrypt-Info", K)
device_info = ios_device_x + self.pairing_id + self._auth_public
device_signature = self._signing_key.sign(device_info)
tlv = {
    TlvValue.Identifier: self.pairing_id,
    TlvValue.PublicKey: self._auth_public,
    TlvValue.Signature: device_signature,
}
if name:
    tlv[TlvValue.Name] = opack.pack({"name": name})
chacha = chacha20.Chacha20Cipher8byteNonce(self._session_key, self._session_key)
encrypted_data = chacha.encrypt(write_tlv(tlv), nonce="PS-Msg05".encode())
```

- `pairing_id` = `str(uuid.uuid4()).encode()` — 36 ASCII bytes, lowercase UUID with dashes (the controller identifier).
- `_auth_public` = raw 32-byte Ed25519 public key of a key made from 32 random bytes (`Ed25519PrivateKey.from_private_bytes(os.urandom(32))`).
- Signature input: `HKDF(K, Controller-Sign) (32) | pairing_id (36) | Ed25519 public key (32)`; Ed25519, 64 bytes.
- Sub-TLV order: Identifier(0x01), PublicKey(0x03), Signature(0x0A), then Name(0x11).
- Name TLV 0x11: pyatv always passes a name for AirPlay pairing (`core.settings.info.name`), so it is always present;
  value is OPACK of `{"name": <str>}` = `E1 44 6E 61 6D 65 (40+len) <utf8>` for names up to 32 bytes
  (`support/opack.py`: dict of n<15 entries = `0xE0+n`; string of len <= 0x20 = `0x40+len`; longer: `0x61 len8`).
  Whether a third-party receiver requires or ignores this TLV is UNVERIFIED (it is "Apple internal(?)" in pyatv).
- Encryption: ChaCha20-Poly1305 (IETF, 96-bit nonce), key = the Pair-Setup-Encrypt key, **no AAD**,
  nonce = 4 zero bytes followed by the 8 ASCII bytes: `00 00 00 00 | "PS-Msg05"`
  (`_pad_nonce`: `b"\x00" * (NONCE_LENGTH - len(nonce)) + nonce`). Output = ciphertext | 16-byte tag, placed in
  `EncryptedData` (fragmented at 255).

`step4` (M6): decrypt `EncryptedData` with the same key, nonce `00 00 00 00 | "PS-Msg06"`, no AAD. Inner TLV:
`Identifier` (accessory pairing id), `PublicKey` (accessory Ed25519 LTPK, 32 bytes), `Signature`.
pyatv has `# TODO: verify signature here` — it does not verify. Per the HAP convention the accessory signs
`HKDF(K, salt "Pair-Setup-Accessory-Sign-Salt", info "Pair-Setup-Accessory-Sign-Info") | accessory id | accessory LTPK`
— those two strings do **not** appear anywhere in pyatv: **UNVERIFIED** from the sources read here.

### 2.4 Stored credentials — `pyatv/auth/hap_pairing.py`

```python
return HapCredentials(atv_pub_key, self._auth_private, atv_identifier, self.pairing_id)
# __str__:
":".join([hexlify(ltpk), hexlify(ltsk), hexlify(atv_id), hexlify(client_id)])
```

`ltpk` = accessory Ed25519 public key (32 B); `ltsk` = our Ed25519 **seed** (32 B, raw private bytes);
`atv_id` = accessory identifier bytes from M6; `client_id` = our 36-byte pairing id. Four hex strings joined by `:`.
Other forms: 2 fields `client_id:ltsk` = legacy AirPlay 1 credentials; `HapCredentials(b"transient")` = transient;
all empty = none. Example of a real LG credential string in issue #1518 shows `atv_id` = the ASCII of the TV's
device id (`35383a32383a...` = "58:28:...").

## 3. Transient pair-setup — `pyatv/protocols/airplay/auth/hap_transient.py` [CODE]

```python
_AIRPLAY_HEADERS = {
    "User-Agent": "AirPlay/320.20",
    "Connection": "keep-alive",
    "X-Apple-HKP": 4,
    "Content-Type": "application/octet-stream",
}
TRANSIENT_PIN = 3939
```

- `POST /pair-pin-start` (no body) — pyatv sends it here too.
- `POST /pair-setup` M1: `{Method: 00, SeqNo: 01, Flags(0x13): 10}` (Flags is one byte, `TransientPairing = 0x10`).
- M2 as usual; SRP exactly as section 2.2 with password `"3939"` (`str(3939)`).
- `POST /pair-setup` M3: `{SeqNo: 03, PublicKey: A, Proof: M1}`. Reply M4 is not inspected.
- **M5/M6 are skipped, and no pair-verify is done.** Keys come straight from the SRP session key K:

```python
shared = binascii.unhexlify(self.srp.shared_key)     # K = H(S), 64 bytes
output_key = hkdf_expand(salt, output_info, shared)
input_key = hkdf_expand(salt, input_info, shared)
```

with the same strings as section 4.3 (`"Control-Salt"`, `"Control-Write-Encryption-Key"`, `"Control-Read-Encryption-Key"`).
Encryption starts right after M4. (openairplay `pairing/hkp.html` agrees: "Default code is 3939", "EncryptionKey is SRP shared secret".)
- When pyatv picks transient: no stored credentials and features contain bit 43 (`SupportsSystemPairing`) or bit 48
  (pyatv name `SupportsCoreUtilsPairingAndEncryption`) — `auth/__init__.py: extract_credentials`. This TV has both
  bits, so pyatv would *try* transient. [REPORT, PR #2941]: an LG OLED (webOS, AirPlay SDK 3.72) "does not accept
  transient pairing"; PIN pairing was needed. Expect the same on a TV that shows a PIN.

Name discrepancy to be aware of: openairplay `features.html` calls bit 38 `SupportsCoreUtilsPairingAndEncryption`
and bit 48 `SupportsTransientPairing`; pyatv `utils.py` calls bit 38 `SupportsUnifiedMediaControl` and bit 48
`SupportsCoreUtilsPairingAndEncryption`. Bit 49 is unnamed in openairplay; pyatv: `SupportsAirPlayVideoV2`.

## 4. Pair-verify — `auth/hap.py` (`AirPlayHapPairVerifyProcedure`), `hap_srp.py` (`verify1`, `verify2`) [CODE]

### 4.1 Requests

`POST /pair-verify HTTP/1.1`, headers = the section 2.1 set (`X-Apple-HKP: 3`, `Content-Type: application/octet-stream`,
`User-Agent: AirPlay/320.20`, `Connection: keep-alive`) + Content-Length. Done on the connection that will then
carry the session (a new TCP connection for every session; pairing and verifying are separate connections in pyatv).

- M1: `{SeqNo: 01, PublicKey: <our X25519 public key, 32 bytes>}` — ephemeral X25519 key from 32 random bytes.
  No Method TLV.
- M2 reply: `PublicKey` = accessory X25519 public (32), `EncryptedData`.
- M3: `{SeqNo: 03, EncryptedData: ...}`.
- M4 reply: `# TODO: check status code` — pyatv ignores it. Check for `Error` (0x07) yourself.

### 4.2 Crypto (`verify1`)

```python
self._shared = self._verify_private.exchange(X25519PublicKey.from_public_bytes(session_pub_key))
session_key = hkdf_expand("Pair-Verify-Encrypt-Salt", "Pair-Verify-Encrypt-Info", self._shared)
chacha = chacha20.Chacha20Cipher8byteNonce(session_key, session_key)
decrypted_tlv = read_tlv(chacha.decrypt(encrypted, nonce="PV-Msg02".encode()))
identifier = decrypted_tlv[TlvValue.Identifier]
signature = decrypted_tlv[TlvValue.Signature]
if identifier != credentials.atv_id: raise AuthenticationError("incorrect device response")
info = session_pub_key + bytes(identifier) + self._public_bytes
ltpk.verify(bytes(signature), bytes(info))            # Ed25519, stored accessory LTPK
device_info = self._public_bytes + credentials.client_id + session_pub_key
device_signature = Ed25519PrivateKey.from_private_bytes(credentials.ltsk).sign(device_info)
tlv = write_tlv({TlvValue.Identifier: credentials.client_id, TlvValue.Signature: device_signature})
return chacha.encrypt(tlv, nonce="PV-Msg03".encode())
```

- Shared secret = raw X25519 output (32 bytes).
- M2 decrypt: key = HKDF-SHA512(salt `Pair-Verify-Encrypt-Salt`, info `Pair-Verify-Encrypt-Info`, shared), 
  nonce `00 00 00 00 | "PV-Msg02"`, no AAD.
- Accessory signature covers `accessory X25519 pub | accessory identifier | our X25519 pub`.
- Our signature covers `our X25519 pub | client_id (36 bytes) | accessory X25519 pub`.
- M3 encrypt: same key, nonce `00 00 00 00 | "PV-Msg03"`, no AAD; inner TLV Identifier(0x01), Signature(0x0A).

### 4.3 Session keys for the control connection — `auth/__init__.py`

```python
CONTROL_SALT = "Control-Salt"
CONTROL_OUTPUT_INFO = "Control-Write-Encryption-Key"
CONTROL_INPUT_INFO = "Control-Read-Encryption-Key"
...
output_key, input_key = verifier.encryption_keys(CONTROL_SALT, CONTROL_OUTPUT_INFO, CONTROL_INPUT_INFO)
session = HAPSession(); session.enable(output_key, input_key)
connection.receive_processor = session.decrypt
connection.send_processor = session.encrypt
```

`verify2`: `hkdf_expand(salt, info, self._shared)` — the IKM is the **raw X25519 shared secret** (not the
Pair-Verify-Encrypt key). Sender→receiver key: info `Control-Write-Encryption-Key`; receiver→sender key:
`Control-Read-Encryption-Key`; both HKDF-SHA512, salt `Control-Salt`, 32 bytes. (Transient: same strings, IKM = K.)
openairplay `pairing/hkp.html` states the same strings and directions.

## 5. Encrypted channel framing — `pyatv/auth/hap_session.py`, `pyatv/support/chacha20.py` [CODE]

```python
FRAME_LENGTH = 1024  # As specified by HAP, section 5.2.2 (Release R1)
AUTH_TAG_LENGTH = 16
# encrypt:
frame = data[0:1024]; length = int.to_bytes(len(frame), 2, byteorder="little")
frame = self.chacha20.encrypt(frame, aad=length); output += length + frame
# decrypt:
length = buf[0:2]; block_length = int.from_bytes(length, "little") + 16
block = buf[2 : 2 + block_length]; output += self.chacha20.decrypt(block, aad=length)
```

- Wire frame: `len (2 bytes LE, plaintext length) | ciphertext (len bytes) | tag (16 bytes)`.
- Plaintext is cut into chunks of at most 1024 bytes; AAD = the 2 length bytes.
- `HAPSession.enable` uses `Chacha20Cipher(out_key, in_key)` with default `nonce_length=8`: nonce = 
  `00 00 00 00 | counter as 8 bytes little-endian`; one counter per direction, both start at 0, +1 per frame.
- The **whole HTTP/RTSP text** (request line, headers, body) is inside: `send_processor` is applied to the fully
  formatted message and `receive_processor` to raw socket bytes before HTTP parsing. A message larger than 1024
  bytes spans several frames; a reply may arrive split across frames/TCP segments (buffer until
  `2 + len + 16` bytes are available; pyatv returns what it has decoded so far).
- Encryption switches on immediately after the pair-verify M4 reply (transient: after M4 of pair-setup) has been
  read in the clear; the next request is the first encrypted frame (counter 0).

## 6. Playing a URL

### 6.0 How pyatv picks the code path [CODE: `protocols/airplay/__init__.py`, `utils.py`]

- `PlayUrl` is reported available if features contain bit 0 (`SupportsAirPlayVideoV1`) **or** bit 49 (`SupportsAirPlayVideoV2`).
- Protocol class: `get_protocol_version` → `AirPlayV2` if features contain bit 38 (`SupportsUnifiedMediaControl`) or
  bit 48 (`SupportsCoreUtilsPairingAndEncryption`), else `AirPlayV1` (a user setting can force either).
  This TV has 38 and 48 → `AirPlayV2`.
- `AirPlayV1.play_url` (`raop/protocols/airplayv1.py`): pair-verify, then `POST /play` with body
  `{"Content-Location": url, "Start-Position": position, "X-Apple-Session-ID": str(uuid4())}`. No SETUP.
- `AirPlayV2.play_url`: section 6A.
- [PR #2941] adds: if `AirPlayV2` and bit 49 set and bit 0 **not** set → `AirPlayVideoV2Player` (section 6B).
- [PR #2942] `parse_features` bug: for `features=0x7F8AD0,0xB8BCF46` (LG prints the low word without zero padding)
  pyatv concatenates the hex strings and mis-shifts the upper word. Pad the low word to 8 hex digits when parsing
  the mDNS `features`/`ft` value `low,high`.
- Pairing requirement (`utils.py`): status flag `0x200` (`LEGACY_PAIRING_BIT`; openairplay: bit 9 `OneTimePairingRequired`)
  or `0x8` (`PINRequired`) → pairing mandatory. `0x244` has 0x200 set. `0x80` = password required. `acl=1` → pairing disabled;
  `act=2` → unsupported ("Current User").

### 6.1 Common session setup (used by 6A and 6B) [CODE: `raop/protocols/airplayv2.py`, `support/rtsp.py`]

On one new TCP connection:

1. Pair-verify (section 4) → control encryption on (section 5).
2. A local UDP "timing server" is bound first (`player.py: timing_server`), its port goes into SETUP as `timingPort`.
   It answers NTP-style timing requests (`TimingServer.datagram_received`: reply type `0x53 | 0x80`, echoes send time,
   receive/transmit = now).
3. `SETUP rtsp://<local_ip>/<session_id> RTSP/1.0`, body = binary plist:

```python
{
    "deviceID": "AA:BB:CC:DD:EE:FF",
    "sessionUUID": str(uuid4()).upper(),
    "timingPort": timing_server_port,
    "timingProtocol": "NTP",
    "isMultiSelectAirPlay": True,
    "groupContainsGroupLeader": False,
    "macAddress": "AA:BB:CC:DD:EE:FF",
    "model": "iPhone14,3",
    "name": "pyatv",
    "osBuildVersion": "20F66",
    "osName": "iPhone OS",
    "osVersion": "16.5",
    "senderSupportsRelay": False,
    "sourceVersion": "690.7.1",
    "statsCollectionEnabled": False,
}
```

   Reply (binary plist) has `eventPort` (and `timingPort`). [PR #2846 and its rework add `"sessionCorrelationUUID": <UUID upper>`
   and use a random locally-administered MAC for `deviceID`/`macAddress`.]
   (`ap2_session.py`, the remote-control-only session, instead sends `"isRemoteControlOnly": True, "timingProtocol": "None"`
   plus osName/sourceVersion "550.10"/model/deviceID/osVersion/osBuildVersion/macAddress/sessionUUID/name. A report in #2846
   says `timingProtocol: "None"` in the *stream* session SETUP is rejected with 500 by tvOS 27.2.)
4. Event channel: TCP connect **from the sender to** `<receiver ip>:eventPort` (retry up to 5 times, 1 s apart, on
   connection refused). It is HAP-framed from the first byte (section 5) with keys

```python
EVENTS_SALT = "Events-Salt"
EVENTS_WRITE_INFO = "Events-Write-Encryption-Key"
EVENTS_READ_INFO = "Events-Read-Encryption-Key"
# setup_channel(EventChannel, verifier, ip, event_port, EVENTS_SALT, EVENTS_READ_INFO, EVENTS_WRITE_INFO)
#                                                         salt        output_info       input_info
```

   i.e. **reversed** relative to the control channel: the sender *encrypts* with HKDF(salt `Events-Salt`,
   info `Events-Read-Encryption-Key`) and *decrypts* with info `Events-Write-Encryption-Key` (IKM = the same
   pair-verify shared secret; code comment: "Read/Write info reversed here as connection originates from receiver!").
   Counters start at 0. On this channel the **receiver sends requests** and the sender must answer each
   (`channels.py: EventChannel.handle_received`): `"<protocol>/<version> 200 OK"` with headers
   `Content-Length: 0`, `Audio-Latency: 0`, and `Server` and `CSeq` echoed from the request if present.
   "Event channel ... must be set up though" (comment in `ap2_session.py`).
5. Feedback: `POST /feedback RTSP/1.0` with no body, every `FEEDBACK_INTERVAL = 2.0` seconds ("This is what iOS uses"),
   errors ignored. Started **before** RECORD in `play_url`.
6. `RECORD rtsp://<local_ip>/<session_id> RTSP/1.0`, no body, no extra headers.

Headers on every RTSP-style request (`RtspSession.exchange` + `_format_message`), in wire order:

```
<METHOD> <uri> RTSP/1.0
User-Agent: AirPlay/550.10
Content-Type: <only when passed as content_type, e.g. text/parameters>
Content-Length: <n>            (only when there is a body)
CSeq: <counter, starts at 0, +1 per request>
DACP-ID: <random 64-bit, uppercase hex, no padding>      f"{randrange(2 ** 64):X}"
Active-Remote: <random 32-bit, decimal>
Client-Instance: <same value as DACP-ID>
Content-Type: application/x-apple-binary-plist           (when the body is a dict → binary plist)
<extra headers>
```

`uri` defaults to `rtsp://<local ip>/<session_id>` with `session_id = randrange(2**32)` (decimal); `/feedback`,
`/command`, `/rate?...`, `/info` are sent with their path as the URI. Replies are matched by `CSeq`; timeout 4 s.
pyatv master never sends `X-Apple-Device-ID`, and sends `X-Apple-Session-ID` / `X-Apple-ProtocolVersion` only on `/play`
(6A) — not on SETUP/RECORD/feedback. HTTP status handling (`http.py`): 403 → "not authenticated";
401 → password needed; in the remote-control session 470 is treated as "invalid or missing credentials".

### 6A. Legacy `/play` flow (pyatv master) [CODE: `airplayv2.py: play_url`, `player.py`]

After 6.1 steps 1–6:

```python
HEADERS = {
    "User-Agent": "AirPlay/550.10",
    "Content-Type": "application/x-apple-binary-plist",
    "X-Apple-ProtocolVersion": "1",
    "X-Apple-Session-ID": str(uuid4()).lower(),
    "X-Apple-Stream-ID": "1",
}
body = {
    "Content-Location": url,
    "Start-Position-Seconds": position,
    "uuid": self.uuid,                       # str(uuid4()), lowercase
    "streamType": 1,
    "mediaType": "file",
    "mightSupportStorePastisKeyRequests": True,
    "playbackRestrictions": 0,
    "secureConnectionMs": 22,
    "volume": 1.0,
    "infoMs": 122,
    "connectMs": 18,
    "authMs": 0,
    "bonjourMs": 0,
    "referenceRestrictions": 3,
    "SenderMACAddress": "AA:BB:CC:DD:EE:FF",
    "model": "iPhone14,3",
    "postAuthMs": 0,
    "clientBundleID": "dev.pyatv.GPU",
    "clientProcName": "dev.pyatv.GPU",
    "osBuildVersion": "20G1116",
    "rate": 1.0,
}
```

- `POST /play HTTP/1.1` (via `connection.post`: **HTTP/1.1, no CSeq/DACP-ID/Active-Remote**), `Content-Length`, the
  HEADERS above, body = binary plist. ("Most fields are not needed here, but keeping them for reference".)
  `position` is an `int` of seconds (`int(kwargs.get("position", 0))`).
- Then, as `RTSP/1.0` requests with the 6.1 header set and binary-plist bodies:
  - `PUT /setProperty?isInterestedInDateRange` body `{"value": True}`
  - `PUT /setProperty?actionAtItemEnd` body `{"value": 0}`
  - `POST /rate?value=1.000000` (no body) — "Most important command ... (will start paused otherwise)"
  - `PUT /setProperty?forwardEndTime` body `{"value": {"flags": 0, "value": 0, "epoch": 0, "timescale": 0}}`
  - `PUT /setProperty?reverseEndTime` same body
- `/play` reply 500 → retry up to 3 times, 1 s apart; any 4xx/5xx → error.
- Position/status: `GET /playback-info HTTP/1.1` once per second on the same encrypted connection (only header:
  `User-Agent: pyatv/<version>`). Reply is a plist; pyatv uses only `error` (`{code, domain}`) and the presence of
  `duration` ("only available if something is playing"); 5 polls without `duration` = not started/ended.
  openairplay `video/http_requests.html` lists the keys: `duration`, `position`, `rate`, `readyToPlay`,
  `playbackBufferEmpty`, `playbackBufferFull`, ... (AirPlay 1 era).
- **Not in pyatv at all**: `POST /scrub?position=<seconds float>`, `GET /scrub`, `POST /stop`, `POST /rate?value=0.000000`
  for pause. They are documented only in openairplay's AirPlay 1 video page (plain HTTP, `Content-Length: 0`).
  pyatv's `stop` just closes the TCP connection ("The Apple TV requires the request to stay open during the entire
  play duration"). Whether an AirPlay 2 TV accepts `/scrub`/`/stop` is UNVERIFIED.

### 6B. Play-queue flow for "video v2" receivers [PR #2941 (LG-tested), PR #2846 + rework `robkochman/pyatv@8144c77c` (tvOS-tested)]

Not merged; three variants exist. Common core, after 6.1 steps 1–6 (#2941 order: SETUP, event channel, start feedback,
RECORD; #2846/rework additionally send `GET /info` over the encrypted RTSP channel before RECORD):

1. `SETUP` (RTSP, same URI) with a remote-control stream, body binary plist:

```python
# PR #2941 (captured from macOS/Safari casting to an LG):
{"streams": [{"type": 130, "controlType": 1,
              "channelID": f"{receiver_id}-RCS-2",        # receiver_id = the receiver's mDNS "deviceid"
              "clientUUID": str(uuid4()).upper(),
              "clientTypeUUID": "A6B27562-B43A-4F2D-B75F-82391E250194"}]}
# rework 8144c77c: same, but "channelID": f"{sender_device_id}-RCS-1"
# PR #2846 original: hard-coded 'channelID': '36:CB:3F:E1:93:B0-RCS-1'
```

   Reply: `{'streams': [{'streamID': 6, 'type': 130}]}` — keep `streamID`.
   (For comparison, pyatv master's MRP data channel uses `controlType: 2`, `clientTypeUUID 1910A70F-DBC0-4242-AF95-115DB30604E1`,
   `wantsDedicatedSocket: True`, a `seed`, and a separate socket with salt `"DataStream-Salt" + str(seed)` — not used here.)

2. Commands: `POST /command` with body = binary plist `{"params": {"data": <binary plist of the command, as data>}}`.
   - #2941: sent as `RTSP/1.0` with the 6.1 header set + `X-Apple-StreamID: <streamID>`.
   - rework: sent as `POST /command HTTP/1.1` through the RTSP exchange (so still CSeq, DACP-ID, Active-Remote,
     Client-Instance) with `User-Agent: AirPlay/870.14.1`, `X-Apple-ProtocolVersion: 1`,
     `X-Apple-Session-ID: <the SETUP sessionUUID, uppercase>`, `X-Apple-StreamID: <streamID>`; inner plist written with
     `sort_keys=False`.
   - #2846 original: `HTTP/1.1` via `connection.post`, headers `User-Agent: AirPlay/870.14.1`, Content-Type bplist,
     `X-Apple-ProtocolVersion: 1`, `X-Apple-Session-ID: <sessionUUID>`, `X-Apple-StreamID`, and a constant `CSeq: 1`.
   - Note the header name `X-Apple-StreamID` (master's `/play` code has `X-Apple-Stream-ID`, which #2846 changes).

3. Command sequence. #2941 (what macOS sends, per its capture, in `docs/documentation/protocols.md` of the PR):

```javascript
{'type': 'insertPlayQueueItem',
 'item': {'uuid': 'BBF9E22B-7369-4F13-AF5E-C997F559968A',
          'Content-Location': 'http://10.0.0.2:8765/video.mp4',
          'Start-Position': {'flags': 1, 'value': 0, 'epoch': 0, 'timescale': 1000000000},
          'mediaType': 'file',
          'playbackRestrictions': 0,
          'referenceRestrictions': 3,
          'supportsIntegratedTimeline': False,
          'allowedCommonMediaClientDataKeys': 0,
          'snapTimeToPausePlayback': False,
          'clientBundleID': 'com.apple.WebKit.GPU'}}
{'type': 'setProperty', 'property': 'actionAtItemEnd', 'value': 1}
{'type': 'setRate', 'rate': 1.0}  # 0.0 pauses
{'type': 'playbackInfo', 'kind': 'request', 'messageID': 1}
```

   - HLS (`.m3u8`/`.m3u`): `'mediaType': 'streaming'`, plus `timeToPausePlayback` and `timeToPauseBuffering` =
     `{"flags": 0, "value": 0, "epoch": 0, "timescale": 0}`; `Start-Position` only when > 0; for live, macOS sends
     `Start-Date` instead (not needed in the test). "A live playlist queued as `file` stays in `loading` forever."
   - `Start-Position` is a CMTime dict: `value = int(position * 1e9)`, `timescale = 10**9`, `flags = 1`, `epoch = 0`.
   - rework/#2846 use a minimal item `{uuid, mediaType: "file", Content-Location, Start-Position-Seconds}` and add
     `{"type": "setProperty", "value": True, "property": "isInterestedInDateRange", "item": {"uuid": <item uuid>}}`.
     [REPORT] `Start-Position-Seconds` is ignored on tvOS 27; `Start-Position` as CMTime
     (`{"value": int(round(position*1000)), "timescale": 1000, "flags": 1, "epoch": 0}`) works.
   - A command reply >= 400 is a failure.

4. Events: the receiver sends `POST /command` requests on the **event channel** with the same
   `{"params": {"data": <bplist>}}` wrapping (answer each with 200 as in 6.1 step 4). Decoded examples:
   `{'type': 'playbackState', 'name': 'playing', 'params': {...}}` (position, duration, seekable ranges; the rework
   also reads `params.playbackState`), names seen: `loading`, `readyToPlay`, `playing`, `paused`, `stopped`
   (with `'reason': 'ended'`), `idle`; `{'type': 'notification', 'name': 'itemPlayedToEnd'}`,
   `itemFailedToPlayToEnd` (error in `params`), `timeJumped` (with a `position` CMTime).
   Replies to `playbackInfo` requests come back as `{'kind': 'response', 'messageID': n, ...}` on the event channel (#2846).
5. Position: [REPORT, tvOS 27] no periodic updates; `playing` carries `position`, `duration`, `rate` once; track
   position as last report + rate × elapsed, or poll with `playbackInfo` requests. Pause/resume: `setRate` 0.0 / 1.0.
   A seek command is **not** shown in any of the three sources — UNVERIFIED (PR #2941: "Seeking and position reporting:
   `play_url` has no API for them").
6. Stop: close the control connection ("Closing the connection stops playback"; the TV leaves the player).
   Also watch for the receiver closing the RTSP connection/event channel without a `stopped` event.
7. The media server must support HTTP Range (tvOS asks `bytes=0-1` first, then ranges; needs 206) [REPORT].

## 7. Samsung / LG reports in the pyatv tracker

Searched issues for "Samsung play_url", "LG play_url", "Samsung AirPlay TV", "LG TV airplay", "playback-info 403".

- **#1518** (open, 2021) LG UM7300PUA, features `0x7F8AD0,0x38BCB46`, paired with HAP credentials: `POST /play` →
  `HTTP/1.1 404 Not Found` (`Server: AirTunes/377.25.06`). Maintainer: "My guess is that your TV does not support the `/play` command".
- **#2393** (open, 2024) LG webOS (OLED55CX6LA and others): `play_url` fails with
  `RTSP/1.0 method PUT failed with code 501: Not Implemented` (the `PUT /setProperty` after the failed `/play`). 
  Comment 2026-11-08: LG OLED42C24LA (AirPlay SDK 3.72) announces bit 49 not bit 0, 404 on `/play` and `/playback-info`;
  `/info` describes a web-based player (`playbackCapabilities` with `hlsJSVersion`, `webEngineName: Apple App Engine`);
  the macOS capture uses the 6B flow and replaying it works.
- **PR #2941** (open, 2026-11-08): implements 6B; tested on that LG **paired with PIN (it does not accept transient
  pairing)**: MP4 (also Dolby Vision) and HLS (TS, fMP4, HEVC, EVENT, live) play; "Other brands (Samsung, Vizio, Sony
  and so on) are not tested yet."
- **PR #2846** (open, 2026-04-25) "Fix compatibility with latest AirPlay": test plan line
  "`[ ]` Manually tested `play_url` against AppleTV and Samsung Frame 2025" — unchecked, no Samsung detail given.
  Later comments are all Apple TV (tvOS 27.0–27.2.1): stock 0.18.0 gets 200 on pair-verify/SETUP/`/play`, then
  **403 on `GET /playback-info`** and the TV never fetches the file; with the `/command` flow it plays. One report of
  the TV closing the stream ~30 s after `playing` on tvOS 27.2 with the rework (not with the PR head on 27.2.1).
- **#2906** (open) tvOS 26.6: `/play` 200, `/playback-info` 500, nothing plays.
- **PR #2701** (closed): "Handle RTSP errors when setting volume on Samsung Q70 TVs" — audio streaming (RAOP) only;
  shows Samsung TVs are used with pyatv's AirPlay 2 audio path (pairing + SETUP work there).
- No issue describes `play_url` succeeding or failing on a Samsung TV with a log. The `403` on unauthenticated
  `GET /playback-info` from this TV says nothing by itself (pyatv only calls it inside the encrypted session); the
  feature profile (49 without 0) matches the LG case, so expect `/play` to be absent and 6B to be the path.

## 8. Test vectors for SRP-6a, 3072-bit, SHA-512 (HAP)

Source: `https://raw.githubusercontent.com/homebridge/fast-srp/latest/src/test/hap_test_data.ts` (values a, A, b, B, s,
v, u, S, K; N and g as in 2.2) and `.../src/test/test_hap_vectors.ts` (k, x, M1). These are the HomeKit Accessory
Protocol specification's SRP test vectors as reproduced by that project. I recomputed all of them locally
with the srptools formulas of section 2.2 and they match [COMPUTED]; A, B and S are all full 384 bytes here, so the
vector does **not** exercise the leading-zero ambiguity.

Conventions: I and P ASCII; `x = H(s | H(I ":" P))`; `k = H(N | PAD(g))`; `u = H(PAD(A) | PAD(B))`; `K = H(S)`;
`M1 = H(H(N) xor H(g) | H(I) | s | A | B | K)` with `H(g)` over the single byte 05.

```
I = "alice"
P = "password123"
s = BEB25379 D1A8581E B5A72767 3A2441EE

a = 60975527 035CF2AD 1989806F 0407210B C81EDC04 E2762A56 AFD529DD DA2D4393
b = E487CB59 D31AC550 471E81F0 0F6928E0 1DDA08E9 74A004F4 9E61F5D1 05284D20

A = FAB6F5D2 615D1E32 3512E799 1CC37443 F487DA60 4CA8C923 0FCB04E5 41DCE628
    0B27CA46 80B0374F 179DC3BD C7553FE6 2459798C 701AD864 A91390A2 8C93B644
    ADBF9C00 745B942B 79F9012A 21B9B787 82319D83 A1F83628 66FBD6F4 6BFC0DDB
    2E1AB6E4 B45A9906 B82E37F0 5D6F97F6 A3EB6E18 2079759C 4F684783 7B62321A
    C1B4FA68 641FCB4B B98DD697 A0C73641 385F4BAB 25B79358 4CC39FC8 D48D4BD8
    67A9A3C1 0F8EA121 70268E34 FE3BBE6F F89998D6 0DA2F3E4 283CBEC1 393D52AF
    724A5723 0C604E9F BCE583D7 613E6BFF D67596AD 121A8707 EEC46944 95703368
    6A155F64 4D5C5863 B48F61BD BF19A53E AB6DAD0A 186B8C15 2E5F5D8C AD4B0EF8
    AA4EA500 8834C3CD 342E5E0F 167AD045 92CD8BD2 79639398 EF9E114D FAAAB919
    E14E8509 89224DDD 98576D79 385D2210 902E9F9B 1F2D86CF A47EE244 635465F7
    1058421A 0184BE51 DD10CC9D 079E6F16 04E7AA9B 7CF7883C 7D4CE12B 06EBE160
    81E23F27 A231D184 32D7D1BB 55C28AE2 1FFCF005 F57528D1 5A88881B B3BBB7FE

B = 40F57088 A482D4C7 733384FE 0D301FDD CA9080AD 7D4F6FDF 09A01006 C3CB6D56
    2E41639A E8FA21DE 3B5DBA75 85B27558 9BDB2798 63C56280 7B2B9908 3CD1429C
    DBE89E25 BFBD7E3C AD3173B2 E3C5A0B1 74DA6D53 91E6A06E 465F037A 40062548
    39A56BF7 6DA84B1C 94E0AE20 8576156F E5C140A4 BA4FFC9E 38C3B07B 88845FC6
    F7DDDA93 381FE0CA 6084C4CD 2D336E54 51C464CC B6EC65E7 D16E548A 273E8262
    84AF2559 B6264274 215960FF F47BDD63 D3AFF064 D6137AF7 69661C9D 4FEE4738
    2603C88E AA098058 1D077584 61B777E4 356DDA58 35198B51 FEEA308D 70F75450
    B71675C0 8C7D8302 FD7539DD 1FF2A11C B4258AA7 0D234436 AA42B6A0 615F3F91
    5D55CC3B 966B2716 B36E4D1A 06CE5E5D 2EA3BEE5 A1270E87 51DA45B6 0B997B0F
    FDB0F996 2FEE4F03 BEE780BA 0A845B1D 92714217 83AE6601 A61EA2E3 42E4F2E8
    BC935A40 9EAD19F2 21BD1B74 E2964DD1 9FC845F6 0EFC0933 8B60B6B2 56D8CAC8
    89CCA306 CC370A0B 18C8B886 E95DA0AF 5235FEF4 393020D2 B7F30569 04759042

v = 9B5E0617 01EA7AEB 39CF6E35 19655A85 3CF94C75 CAF2555E F1FAF759 BB79CB47
    7014E04A 88D68FFC 05323891 D4C205B8 DE81C2F2 03D8FAD1 B24D2C10 9737F1BE
    BBD71F91 2447C4A0 3C26B9FA D8EDB3E7 80778E30 2529ED1E E138CCFC 36D4BA31
    3CC48B14 EA8C22A0 186B222E 655F2DF5 603FD75D F76B3B08 FF895006 9ADD03A7
    54EE4AE8 8587CCE1 BFDE3679 4DBAE459 2B7B904F 442B041C B17AEBAD 1E3AEBE3
    CBE99DE6 5F4BB1FA 00B0E7AF 06863DB5 3B02254E C66E781E 3B62A821 2C86BEB0
    D50B5BA6 D0B478D8 C4E9BBCE C2176532 6FBD1405 8D2BBDE2 C33045F0 3873E539
    48D78B79 4F0790E4 8C36AED6 E880F557 427B2FC0 6DB5E1E2 E1D7E661 AC482D18
    E528D729 5EF74372 95FF1A72 D4027717 13F16876 DD050AE5 B7AD53CC B90855C9
    39566483 58ADFD96 6422F524 98732D68 D1D7FBEF 10D78034 AB8DCB6F 0FCF885C
    C2B2EA2C 3E6AC866 09EA058A 9DA8CC63 531DC915 414DF568 B09482DD AC1954DE
    C7EB714F 6FF7D44C D5B86F6B D1158109 30637C01 D0F6013B C9740FA2 C633BA89

k = a9c2e2559bf0ebb53f0cbbf62282906bede7f2182f00678211fbd5bde5b285033a4993503b87397f9be5ec02080fedbc0835587ad039060879b8621e8c3659e0
x = b149ecb0946b0b206d77e73d95deb7c41bd12e86a5e2eea3893d5416591a002ff94bfea384dc0e1c550f7ed4d5a9d2ad1f1526f01c56b5c10577730cc4a4d709

u = 03AE5F3C 3FA9EFF1 A50D7DBB 8D2F60A1 EA66EA71 2D50AE97 6EE34641 A1CD0E51
    C4683DA3 83E8595D 6CB56A15 D5FBC754 3E07FBDD D316217E 01A391A1 8EF06DFF

S = F1036FEC D017C823 9C0D5AF7 E0FCF0D4 08B009E3 6411618A 60B23AAB BFC38339
    72682312 14BAACDC 94CA1C53 F442FB51 C1B027C3 18AE238E 16414D60 D1881B66
    486ADE10 ED02BA33 D098F6CE 9BCF1BB0 C46CA2C4 7F2F174C 59A9C61E 2560899B
    83EF6113 1E6FB30B 714F4E43 B735C9FE 6080477C 1B83E409 3E4D456B 9BCA492C
    F9339D45 BC42E67C E6C02C24 3E49F5DA 42A869EC 855780E8 4207B8A1 EA6501C4
    78AAC0DF D3D22614 F531A00D 826B7954 AE8B14A9 85A42931 5E6DD366 4CF47181
    496A9432 9CDE8005 CAE63C2F 9CA4969B FE840019 24037C44 6559BDBB 9DB9D4DD
    142FBCD7 5EEF2E16 2C843065 D99E8F05 762C4DB7 ABD9DB20 3D41AC85 A58C05BD
    4E2DBF82 2A934523 D54E0653 D376CE8B 56DCB452 7DDDC1B9 94DC7509 463A7468
    D7F02B1B EB168571 4CE1DD1E 71808A13 7F788847 B7C6B7BF A1364474 B3B7E894
    78954F6A 8E68D45B 85A88E4E BFEC1336 8EC0891C 3BC86CF5 00978801 78D86135
    E7287234 58538858 D715B7B2 47406222 C1019F53 603F0169 52D49710 0858824C

K = 5CBC219D B052138E E1148C71 CD449896 3D682549 CE91CA24 F098468F 06015BEB
    6AF245C2 093F98C3 651BCA83 AB8CAB2B 580BBF02 184FEFDF 26142F73 DF95AC50

M1 = 5f7c14ab57ed0e94fd1d78c6b4dd09ed7e340b7e05d419a9fd760f6b35e523d1310777a1ae1d2826f596f3a85116cc457c7c964d4f44ded5559da818c88b617f

M2 = 2fa0e81f5cb73b88fa0964270f321dd641f2227a5d805c40f1bfe96aaf6a19ffce8e23287965a39eab9d5a02215f89e128177ed2c4f103e655a045531bcbf7ad
     [COMPUTED by me as H(A | M1 | K); not present in the fast-srp test file]
```

Other vector sets (different parameters — useful only for the generic SRP core):
- `https://raw.githubusercontent.com/idlesign/srptools/master/tests/test_context.py` — RFC 5054 Appendix B
  (1024-bit, SHA-1, I="alice", P="password123", same s/a/b as above) and a second SHA-1 set (I="bouke", P="test").
  Includes a proof `M = 3F3BC671...` and proof hash for srptools' own M1/M2 definition.
- `https://raw.githubusercontent.com/homebridge/fast-srp/latest/src/test/test_rfc_5054.ts` and `test_picl_vectors.ts`
  (listed in the repo tree; not read).

## 9. Open uncertainties (summary)

1. Leading-zero handling of A/B/S in M1, M2 and K on the receiver (2.2) — unverified; avoid for A, retry otherwise.
2. Accessory signature check in M6 (`Pair-Setup-Accessory-Sign-Salt/-Info`) — strings not present in pyatv; pyatv skips the check.
3. Whether the TV needs/accepts Name TLV 0x11 in M5, and whether it wants Method 0x00 or 0x01 in M1 (it sets bit 51).
4. `User-Agent: AirPlay/320.20` and `X-Apple-HKP: 3` are what pyatv sends; no source read says which headers a Samsung requires.
5. The play-queue flow (6B) is from unmerged PRs, verified on an LG and Apple TVs only. Differences between the
   variants (`channelID` suffix `-RCS-2` with the receiver id vs `-RCS-1` with a sender id; RTSP/1.0 vs HTTP/1.1 for
   `/command`; extra headers; `GET /info` before RECORD; `sessionCorrelationUUID`) are untested on Samsung.
6. Seek in the play-queue flow, and `/scrub`, `/stop` on AirPlay 2 receivers — not in any source read.
7. emanuelecozzi.net was not readable (expired certificate), so nothing here is cross-checked against it.
