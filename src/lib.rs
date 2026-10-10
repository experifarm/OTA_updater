//! Broadcast OTA (over-the-air) firmware updater.
//!
//! Sender pipeline: [`diff`] (bsdiff-style, [`suffix`] array) → zstd → [`slicer`] →
//! [`rs`] Reed-Solomon FEC → [`manifest`] (Ed25519-signed) → [`wire`] packets → [`net`] UDP multicast.
//!
//! Receiver pipeline: [`net`] → [`channel`] impairment simulation → [`wire`] decode (CRC32) →
//! [`rs`] reconstruct → per-group hash check → streaming [`patch`] apply → signed hash verification.
//!
//! See `docs/DESIGN.md` for the full design.

pub mod channel;
pub mod diff;
pub mod gf256;
pub mod hashio;
pub mod imagegen;
pub mod keys;
pub mod manifest;
pub mod net;
pub mod patch;
pub mod receiver;
pub mod rs;
pub mod sender;
pub mod slicer;
pub mod suffix;
pub mod varint;
pub mod wire;

/// SHA-256 digest of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

/// Lower-case hex encoding (used for keys and log output).
pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Decode hex (whitespace around the string is ignored).
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    let (pairs, rest) = s.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    pairs
        .iter()
        .map(|&[hi, lo]| Some(nibble(hi)? << 4 | nibble(lo)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let data = [0u8, 1, 0x7f, 0x80, 0xff, 0xab];
        assert_eq!(to_hex(&data), "00017f80ffab");
        assert_eq!(from_hex(" 00017F80ffab\n").unwrap(), data);
        assert!(from_hex("abc").is_none());
        assert!(from_hex("zz").is_none());
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            to_hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
