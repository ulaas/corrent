//! The Cast v2 wire format: 4-byte big-endian length, then a protobuf `CastMessage`
//! (`cast_channel.proto`). Only the few fields senders use, encoded by hand.

#[derive(Clone, Debug, PartialEq)]
pub struct CastMessage {
    pub source: String,
    pub destination: String,
    pub namespace: String,
    /// Payload (`payload_type` STRING); binary payloads are dropped.
    pub payload: String,
}

/// Larger frames are refused (the protocol caps them at 64 KiB).
pub const MAX_FRAME: usize = 64 * 1024;

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn string(out: &mut Vec<u8>, field: u64, s: &str) {
    varint(out, field << 3 | 2);
    varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

impl CastMessage {
    /// The framed message.
    pub fn encode(&self) -> Vec<u8> {
        let mut m = Vec::with_capacity(64 + self.payload.len());
        varint(&mut m, 1 << 3); // protocol_version = CASTV2_1_0
        varint(&mut m, 0);
        string(&mut m, 2, &self.source);
        string(&mut m, 3, &self.destination);
        string(&mut m, 4, &self.namespace);
        varint(&mut m, 5 << 3); // payload_type = STRING
        varint(&mut m, 0);
        string(&mut m, 6, &self.payload);
        let mut out = (m.len() as u32).to_be_bytes().to_vec();
        out.extend(m);
        out
    }

    /// Decodes one message body (without the length prefix).
    pub fn decode(mut b: &[u8]) -> Option<CastMessage> {
        fn read_varint(b: &mut &[u8]) -> Option<u64> {
            let mut v = 0u64;
            for shift in (0..64).step_by(7) {
                let (&x, rest) = b.split_first()?;
                *b = rest;
                v |= u64::from(x & 0x7f) << shift;
                if x & 0x80 == 0 {
                    return Some(v);
                }
            }
            None
        }
        let mut m = CastMessage {
            source: String::new(),
            destination: String::new(),
            namespace: String::new(),
            payload: String::new(),
        };
        while !b.is_empty() {
            let key = read_varint(&mut b)?;
            match key & 7 {
                0 => {
                    read_varint(&mut b)?;
                }
                2 => {
                    let len = usize::try_from(read_varint(&mut b)?).ok()?;
                    if len > b.len() {
                        return None;
                    }
                    let (v, rest) = b.split_at(len);
                    b = rest;
                    let s = || String::from_utf8_lossy(v).into_owned();
                    match key >> 3 {
                        2 => m.source = s(),
                        3 => m.destination = s(),
                        4 => m.namespace = s(),
                        6 => m.payload = s(),
                        _ => {}
                    }
                }
                1 => b = b.get(8..)?,
                5 => b = b.get(4..)?,
                _ => return None,
            }
        }
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = CastMessage {
            source: "sender-0".into(),
            destination: "receiver-0".into(),
            namespace: "urn:x-cast:com.google.cast.tp.connection".into(),
            payload: r#"{"type":"CONNECT"}"#.repeat(20),
        };
        let framed = m.encode();
        let len = u32::from_be_bytes(framed[..4].try_into().unwrap()) as usize;
        assert_eq!(len, framed.len() - 4);
        assert_eq!(CastMessage::decode(&framed[4..]), Some(m));
    }

    #[test]
    fn garbage_is_refused_without_panicking() {
        let mut rng = corrent_fuzz::Rng::new(7);
        for _ in 0..5000 {
            let n = (rng.next_u64() % 64) as usize;
            let b: Vec<u8> = (0..n).map(|_| rng.next_u64() as u8).collect();
            let _ = CastMessage::decode(&b);
        }
        assert_eq!(CastMessage::decode(&[0x12, 0x05, b'a']), None);
    }
}
