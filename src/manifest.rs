//! The signed update manifest (Ed25519签名).
//!
//! ```text
//! "OTAMANIF" | format_version u16
//! session_id u64 | fw_version u64
//! old_size u64 | old_sha256 [32]
//! new_size u64 | new_sha256 [32]
//! patch_size u64 | patch_sha256 [32] | compression u8
//! shard_size u16 | k u16 | m u16 | group_count u32
//! group_sha256 [32] × group_count
//! ```
//!
//! Signed container: `manifest_len u32 | manifest_bytes | signature [64]`, the signature being Ed25519
//! over `SIGNING_DOMAIN ‖ manifest_bytes`. Receivers verify the signature (`verify_strict`, pinned key)
//! *before* parsing the manifest body.

use crate::patch::Compression;
use crate::slicer::{FecParams, Layout};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use thiserror::Error;

pub const MAGIC: &[u8; 8] = b"OTAMANIF";
pub const FORMAT_VERSION: u16 = 1;
/// Domain separation prefix for manifest signatures.
pub const SIGNING_DOMAIN: &[u8] = b"OTA-UPDATER-MANIFEST-v1\0";
pub const SIGNATURE_LEN: usize = 64;
/// Upper bound on a signed container (≈ 32k groups ≈ 1 GiB of patch with default FEC params).
pub const MAX_CONTAINER_LEN: usize = 1 << 20;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest is truncated")]
    Truncated,
    #[error("not a manifest (bad magic)")]
    BadMagic,
    #[error("unsupported manifest format version {0}")]
    UnsupportedVersion(u16),
    #[error("manifest signature is invalid")]
    BadSignature,
    #[error("invalid manifest: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub session_id: u64,
    pub fw_version: u64,
    pub old_size: u64,
    pub old_sha256: [u8; 32],
    pub new_size: u64,
    pub new_sha256: [u8; 32],
    pub patch_size: u64,
    pub patch_sha256: [u8; 32],
    pub compression: Compression,
    pub fec: FecParams,
    pub group_hashes: Vec<[u8; 32]>,
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ManifestError> {
        if self.buf.len() < n {
            return Err(ManifestError::Truncated);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ManifestError> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    fn u8(&mut self) -> Result<u8, ManifestError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ManifestError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, ManifestError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ManifestError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

impl Manifest {
    pub fn layout(&self) -> Layout {
        Layout::new(self.fec, self.patch_size)
    }

    pub fn group_count(&self) -> u32 {
        self.group_hashes.len() as u32
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(160 + 32 * self.group_hashes.len());
        out.extend_from_slice(MAGIC);
        out.extend(FORMAT_VERSION.to_le_bytes());
        out.extend(self.session_id.to_le_bytes());
        out.extend(self.fw_version.to_le_bytes());
        out.extend(self.old_size.to_le_bytes());
        out.extend(self.old_sha256);
        out.extend(self.new_size.to_le_bytes());
        out.extend(self.new_sha256);
        out.extend(self.patch_size.to_le_bytes());
        out.extend(self.patch_sha256);
        out.push(self.compression as u8);
        out.extend(self.fec.shard_size.to_le_bytes());
        out.extend(self.fec.k.to_le_bytes());
        out.extend(self.fec.m.to_le_bytes());
        out.extend((self.group_hashes.len() as u32).to_le_bytes());
        for h in &self.group_hashes {
            out.extend(h);
        }
        out
    }

    /// Parse and validate internal consistency. Only call on authenticated bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        let mut r = Reader { buf: bytes };
        if &r.array::<8>()? != MAGIC {
            return Err(ManifestError::BadMagic);
        }
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(ManifestError::UnsupportedVersion(version));
        }
        let session_id = r.u64()?;
        let fw_version = r.u64()?;
        let old_size = r.u64()?;
        let old_sha256 = r.array()?;
        let new_size = r.u64()?;
        let new_sha256 = r.array()?;
        let patch_size = r.u64()?;
        let patch_sha256 = r.array()?;
        let compression =
            Compression::from_u8(r.u8()?).ok_or_else(|| ManifestError::Invalid("unknown compression".into()))?;
        let fec = FecParams {
            shard_size: r.u16()?,
            k: r.u16()?,
            m: r.u16()?,
        };
        fec.validate().map_err(ManifestError::Invalid)?;
        let group_count = r.u32()?;
        let expected_groups = Layout::new(fec, patch_size).group_count();
        if u64::from(group_count) != expected_groups {
            return Err(ManifestError::Invalid(format!(
                "group count {group_count} does not match patch size {patch_size} (expected {expected_groups})"
            )));
        }
        if r.buf.len() != group_count as usize * 32 {
            return Err(if r.buf.len() < group_count as usize * 32 {
                ManifestError::Truncated
            } else {
                ManifestError::Invalid("trailing bytes".into())
            });
        }
        let group_hashes = (0..group_count).map(|_| r.array()).collect::<Result<_, _>>()?;
        Ok(Self {
            session_id,
            fw_version,
            old_size,
            old_sha256,
            new_size,
            new_sha256,
            patch_size,
            patch_sha256,
            compression,
            fec,
            group_hashes,
        })
    }

    /// Serialize and sign: returns the signed container.
    pub fn sign(&self, key: &SigningKey) -> Vec<u8> {
        let body = self.encode();
        let signature = key.sign(&signing_message(&body));
        let mut out = Vec::with_capacity(4 + body.len() + SIGNATURE_LEN);
        out.extend((body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out.extend(signature.to_bytes());
        out
    }
}

fn signing_message(body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(SIGNING_DOMAIN.len() + body.len());
    msg.extend_from_slice(SIGNING_DOMAIN);
    msg.extend_from_slice(body);
    msg
}

/// Verify a signed container with the pinned public key; returns the authenticated manifest bytes.
///
/// Any error from this function means the container is not authentic (or not intact) — it must not
/// be blamed on the claimed sender.
pub fn verify<'a>(container: &'a [u8], key: &VerifyingKey) -> Result<&'a [u8], ManifestError> {
    if container.len() > MAX_CONTAINER_LEN {
        return Err(ManifestError::Invalid("container too large".into()));
    }
    if container.len() < 4 + SIGNATURE_LEN {
        return Err(ManifestError::Truncated);
    }
    let body_len = u32::from_le_bytes(container[..4].try_into().unwrap()) as usize;
    if container.len() != 4 + body_len + SIGNATURE_LEN {
        return Err(ManifestError::Truncated);
    }
    let body = &container[4..4 + body_len];
    let sig_bytes: [u8; SIGNATURE_LEN] = container[4 + body_len..].try_into().unwrap();
    let signature = Signature::from_bytes(&sig_bytes);
    key.verify_strict(&signing_message(body), &signature)
        .map_err(|_| ManifestError::BadSignature)?;
    Ok(body)
}

/// Verify a signed container with the pinned public key, then parse the manifest.
pub fn verify_and_decode(container: &[u8], key: &VerifyingKey) -> Result<Manifest, ManifestError> {
    Manifest::decode(verify(container, key)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let fec = FecParams {
            shard_size: 512,
            k: 4,
            m: 2,
        };
        let patch_size = 5000;
        let groups = Layout::new(fec, patch_size).group_count() as usize;
        Manifest {
            session_id: 0xdead_beef,
            fw_version: 7,
            old_size: 1234,
            old_sha256: [1; 32],
            new_size: 5678,
            new_sha256: [2; 32],
            patch_size,
            patch_sha256: [3; 32],
            compression: Compression::Zstd,
            fec,
            group_hashes: (0..groups).map(|i| [i as u8; 32]).collect(),
        }
    }

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn encode_decode_roundtrip() {
        let m = sample();
        assert_eq!(Manifest::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn sign_verify() {
        let m = sample();
        let k = key(9);
        let container = m.sign(&k);
        assert_eq!(verify_and_decode(&container, &k.verifying_key()).unwrap(), m);
    }

    #[test]
    fn any_flipped_byte_is_rejected() {
        let m = sample();
        let k = key(9);
        let container = m.sign(&k);
        for i in 0..container.len() {
            let mut bad = container.clone();
            bad[i] ^= 0x01;
            assert!(
                verify_and_decode(&bad, &k.verifying_key()).is_err(),
                "flip at byte {i} accepted"
            );
        }
    }

    #[test]
    fn wrong_key_is_rejected() {
        let container = sample().sign(&key(9));
        assert_eq!(
            verify_and_decode(&container, &key(10).verifying_key()),
            Err(ManifestError::BadSignature)
        );
    }

    #[test]
    fn inconsistent_manifest_is_rejected() {
        let mut m = sample();
        m.group_hashes.pop();
        assert!(matches!(Manifest::decode(&m.encode()), Err(ManifestError::Invalid(_))));
        let mut m = sample();
        m.fec.k = 0;
        assert!(matches!(Manifest::decode(&m.encode()), Err(ManifestError::Invalid(_))));
        let enc = sample().encode();
        assert_eq!(Manifest::decode(&enc[..enc.len() - 1]), Err(ManifestError::Truncated));
        let mut long = enc.clone();
        long.push(0);
        assert!(matches!(Manifest::decode(&long), Err(ManifestError::Invalid(_))));
    }
}
