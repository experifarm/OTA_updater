//! UDP packet format with a CRC32 trailer.
//!
//! ```text
//! offset 0  magic "OTA1" | version u8 | kind u8 | reserved u16 | session_id u64
//! kind 1 ManifestFrag: frag_index u16 | frag_count u16 | total_len u32 | data_len u16 | data
//! kind 2 Shard       : group u32 | shard_index u16 | data_len u16 | data
//! trailer            : crc32 u32 over all preceding bytes
//! ```
//!
//! A packet failing the CRC is dropped by the receiver, turning corruption into an erasure that the
//! Reed-Solomon code can repair.

use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"OTA1";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 16;
pub const CRC_LEN: usize = 4;
pub const KIND_MANIFEST_FRAG: u8 = 1;
pub const KIND_SHARD: u8 = 2;
/// Manifest container bytes carried per fragment.
pub const MANIFEST_FRAG_SIZE: usize = 1024;
/// Receive buffer size; comfortably larger than any valid packet.
pub const MAX_DATAGRAM: usize = 2048;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    #[error("packet too short")]
    TooShort,
    #[error("CRC mismatch")]
    BadCrc,
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported wire version {0}")]
    UnsupportedVersion(u8),
    #[error("unknown packet kind {0}")]
    UnknownKind(u8),
    #[error("length field does not match packet size")]
    BadLength,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    ManifestFrag {
        session_id: u64,
        index: u16,
        count: u16,
        total_len: u32,
        data: Vec<u8>,
    },
    Shard {
        session_id: u64,
        group: u32,
        index: u16,
        data: Vec<u8>,
    },
}

impl Packet {
    pub fn session_id(&self) -> u64 {
        match self {
            Packet::ManifestFrag { session_id, .. } | Packet::Shard { session_id, .. } => *session_id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 12 + MANIFEST_FRAG_SIZE + CRC_LEN);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        match self {
            Packet::ManifestFrag {
                session_id,
                index,
                count,
                total_len,
                data,
            } => {
                out.push(KIND_MANIFEST_FRAG);
                out.extend(0u16.to_le_bytes());
                out.extend(session_id.to_le_bytes());
                out.extend(index.to_le_bytes());
                out.extend(count.to_le_bytes());
                out.extend(total_len.to_le_bytes());
                out.extend((data.len() as u16).to_le_bytes());
                out.extend_from_slice(data);
            }
            Packet::Shard {
                session_id,
                group,
                index,
                data,
            } => {
                out.push(KIND_SHARD);
                out.extend(0u16.to_le_bytes());
                out.extend(session_id.to_le_bytes());
                out.extend(group.to_le_bytes());
                out.extend(index.to_le_bytes());
                out.extend((data.len() as u16).to_le_bytes());
                out.extend_from_slice(data);
            }
        }
        let crc = crc32fast::hash(&out);
        out.extend(crc.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, WireError> {
        if buf.len() < HEADER_LEN + CRC_LEN {
            return Err(WireError::TooShort);
        }
        let (body, crc) = buf.split_at(buf.len() - CRC_LEN);
        if crc32fast::hash(body) != u32::from_le_bytes(crc.try_into().unwrap()) {
            return Err(WireError::BadCrc);
        }
        if &body[..4] != MAGIC {
            return Err(WireError::BadMagic);
        }
        if body[4] != VERSION {
            return Err(WireError::UnsupportedVersion(body[4]));
        }
        let kind = body[5];
        let session_id = u64::from_le_bytes(body[8..16].try_into().unwrap());
        let p = &body[HEADER_LEN..];
        let u16_at = |o: usize| u16::from_le_bytes([p[o], p[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        match kind {
            KIND_MANIFEST_FRAG => {
                if p.len() < 10 {
                    return Err(WireError::TooShort);
                }
                let data_len = usize::from(u16_at(8));
                if p.len() != 10 + data_len {
                    return Err(WireError::BadLength);
                }
                Ok(Packet::ManifestFrag {
                    session_id,
                    index: u16_at(0),
                    count: u16_at(2),
                    total_len: u32_at(4),
                    data: p[10..].to_vec(),
                })
            }
            KIND_SHARD => {
                if p.len() < 8 {
                    return Err(WireError::TooShort);
                }
                let data_len = usize::from(u16_at(6));
                if p.len() != 8 + data_len {
                    return Err(WireError::BadLength);
                }
                Ok(Packet::Shard {
                    session_id,
                    group: u32_at(0),
                    index: u16_at(4),
                    data: p[8..].to_vec(),
                })
            }
            other => Err(WireError::UnknownKind(other)),
        }
    }
}

/// Split a signed manifest container into fragment packets.
pub fn manifest_fragments(session_id: u64, container: &[u8]) -> Vec<Packet> {
    let chunks: Vec<&[u8]> = container.chunks(MANIFEST_FRAG_SIZE).collect();
    let count = chunks.len() as u16;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, c)| Packet::ManifestFrag {
            session_id,
            index: i as u16,
            count,
            total_len: container.len() as u32,
            data: c.to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_crc() {
        let pkts = [
            Packet::Shard {
                session_id: 42,
                group: 7,
                index: 39,
                data: vec![0xab; 1024],
            },
            Packet::ManifestFrag {
                session_id: u64::MAX,
                index: 1,
                count: 3,
                total_len: 2500,
                data: vec![1, 2, 3],
            },
        ];
        for p in pkts {
            let enc = p.encode();
            assert_eq!(Packet::decode(&enc).unwrap(), p);
            for i in 0..enc.len() {
                let mut bad = enc.clone();
                bad[i] ^= 0x10;
                assert_eq!(
                    Packet::decode(&bad),
                    Err(WireError::BadCrc),
                    "bit flip at {i} not detected"
                );
            }
            assert_eq!(Packet::decode(&enc[..enc.len() - 1]), Err(WireError::BadCrc));
        }
        let shard = Packet::Shard {
            session_id: 1,
            group: 0,
            index: 0,
            data: vec![0; 1024],
        };
        assert_eq!(shard.encode().len(), HEADER_LEN + 8 + 1024 + CRC_LEN);
        assert_eq!(Packet::decode(&[0u8; 5]), Err(WireError::TooShort));
    }

    #[test]
    fn fragments_cover_container() {
        let container: Vec<u8> = (0..2500u32).map(|i| i as u8).collect();
        let frags = manifest_fragments(9, &container);
        assert_eq!(frags.len(), 3);
        let mut joined: Vec<u8> = Vec::new();
        for f in &frags {
            if let Packet::ManifestFrag {
                count, total_len, data, ..
            } = f
            {
                assert_eq!((*count, *total_len), (3, 2500));
                joined.extend(data);
            }
        }
        assert_eq!(joined, container);
    }
}
