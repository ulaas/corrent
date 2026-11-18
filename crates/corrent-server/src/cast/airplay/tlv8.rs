//! TLV8, the framing of HomeKit pairing messages: `tag | length | value`, a value longer than
//! 255 bytes as consecutive items of the same tag.

use std::collections::BTreeMap;

pub const METHOD: u8 = 0x00;
pub const IDENTIFIER: u8 = 0x01;
pub const SALT: u8 = 0x02;
pub const PUBLIC_KEY: u8 = 0x03;
pub const PROOF: u8 = 0x04;
pub const ENCRYPTED_DATA: u8 = 0x05;
pub const STATE: u8 = 0x06;
pub const ERROR: u8 = 0x07;
pub const BACK_OFF: u8 = 0x08;
pub const SIGNATURE: u8 = 0x0A;
pub const FLAGS: u8 = 0x13;

pub fn encode(items: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, value) in items {
        if value.is_empty() {
            out.extend_from_slice(&[*tag, 0]);
        }
        for part in value.chunks(255) {
            out.push(*tag);
            out.push(part.len() as u8);
            out.extend_from_slice(part);
        }
    }
    out
}

/// The values by tag (pieces of one value joined). `None` for a message cut short.
pub fn decode(mut data: &[u8]) -> Option<BTreeMap<u8, Vec<u8>>> {
    let mut out: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    while let [tag, len, rest @ ..] = data {
        let value = rest.get(..usize::from(*len))?;
        out.entry(*tag).or_default().extend_from_slice(value);
        data = rest.get(usize::from(*len)..)?;
    }
    data.is_empty().then_some(out)
}

/// What a device's `Error` item says.
pub fn error_text(code: u8) -> &'static str {
    match code {
        0x02 => "the PIN was wrong, or this pairing isn't known to the device any more",
        0x03 => "too many attempts: the device wants a pause before the next one",
        0x04 => "the device has no room for another pairing",
        0x05 => "too many failed attempts: the device refuses pairing for now",
        0x06 => "the device isn't accepting pairing right now",
        0x07 => "the device is busy pairing with something else",
        _ => "the device refused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_are_split_and_joined() {
        let m1 = encode(&[(METHOD, &[0]), (STATE, &[1])]);
        assert_eq!(m1, [0x00, 0x01, 0x00, 0x06, 0x01, 0x01], "pair-setup M1 as on the wire");
        let key = vec![7u8; 384];
        let wire = encode(&[(STATE, &[3]), (PUBLIC_KEY, &key)]);
        assert_eq!(wire.len(), 3 + 2 + 255 + 2 + 129);
        let back = decode(&wire).unwrap();
        assert_eq!((back[&STATE].as_slice(), back[&PUBLIC_KEY].len()), (&[3u8][..], 384));
        assert_eq!(decode(&wire[..wire.len() - 1]), None, "cut short");
        assert_eq!(decode(&[]).unwrap().len(), 0);
    }
}
