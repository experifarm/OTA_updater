//! Streaming patch application.
//!
//! Patch stream layout (all integers little endian, see `docs/DESIGN.md` §3.1):
//!
//! ```text
//! header : "OTAPATCH" | version u16 | old_size u64 | new_size u64
//! record*: diff_len varint | extra_len varint | seek zigzag-varint | diff bytes | extra bytes
//! ```
//!
//! The applier reads the patch strictly forwards and only needs `O(CHUNK)` memory, so it can consume a
//! patch while it is still being received.

use crate::varint;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use thiserror::Error;

pub const MAGIC: &[u8; 8] = b"OTAPATCH";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 8 + 2 + 8 + 8;
const CHUNK: usize = 64 * 1024;

/// Compression applied to the whole patch stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Zstd = 1,
}

impl Compression {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Zstd),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum PatchError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("not an OTA patch (bad magic)")]
    BadMagic,
    #[error("unsupported patch version {0}")]
    UnsupportedVersion(u16),
    #[error("patch is for an old image of {expected} bytes, but the old image has {actual} bytes")]
    OldSizeMismatch { expected: u64, actual: u64 },
    #[error("patch stream is truncated")]
    Truncated,
    #[error("malformed patch: {0}")]
    Malformed(&'static str),
}

fn map_eof(e: io::Error) -> PatchError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        PatchError::Truncated
    } else {
        PatchError::Io(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchHeader {
    pub old_size: u64,
    pub new_size: u64,
}

impl PatchHeader {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[..8].copy_from_slice(MAGIC);
        h[8..10].copy_from_slice(&VERSION.to_le_bytes());
        h[10..18].copy_from_slice(&self.old_size.to_le_bytes());
        h[18..26].copy_from_slice(&self.new_size.to_le_bytes());
        h
    }

    pub fn read<R: Read + ?Sized>(r: &mut R) -> Result<Self, PatchError> {
        let mut h = [0u8; HEADER_LEN];
        r.read_exact(&mut h).map_err(map_eof)?;
        if &h[..8] != MAGIC {
            return Err(PatchError::BadMagic);
        }
        let version = u16::from_le_bytes([h[8], h[9]]);
        if version != VERSION {
            return Err(PatchError::UnsupportedVersion(version));
        }
        Ok(Self {
            old_size: u64::from_le_bytes(h[10..18].try_into().unwrap()),
            new_size: u64::from_le_bytes(h[18..26].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyStats {
    pub records: u64,
    pub diff_bytes: u64,
    pub extra_bytes: u64,
    pub new_size: u64,
}

/// Apply an uncompressed patch stream. `old` must be positioned anywhere; it is sought as needed.
pub fn apply<O, P, W>(old: &mut O, old_len: u64, patch: P, out: &mut W) -> Result<ApplyStats, PatchError>
where
    O: Read + Seek + ?Sized,
    P: Read,
    W: Write + ?Sized,
{
    let mut patch = BufReader::with_capacity(CHUNK, patch);
    let header = PatchHeader::read(&mut patch)?;
    if header.old_size != old_len {
        return Err(PatchError::OldSizeMismatch {
            expected: header.old_size,
            actual: old_len,
        });
    }

    let mut stats = ApplyStats::default();
    let mut produced = 0u64;
    let mut old_pos = 0i64; // logical cursor in the old image
    let mut file_pos: Option<u64> = None; // actual position of `old`, if known
    let mut pbuf = vec![0u8; CHUNK];
    let mut obuf = vec![0u8; CHUNK];

    while produced < header.new_size {
        let diff_len = varint::read_u64(&mut patch).map_err(map_eof)?;
        let extra_len = varint::read_u64(&mut patch).map_err(map_eof)?;
        let seek = varint::unzigzag(varint::read_u64(&mut patch).map_err(map_eof)?);

        let remaining = header.new_size - produced;
        if diff_len > remaining || extra_len > remaining - diff_len {
            return Err(PatchError::Malformed("record exceeds the declared new image size"));
        }

        if diff_len > 0 {
            let start =
                u64::try_from(old_pos).map_err(|_| PatchError::Malformed("diff starts before the old image"))?;
            if start.checked_add(diff_len).is_none_or(|end| end > old_len) {
                return Err(PatchError::Malformed("diff reads past the end of the old image"));
            }
            if file_pos != Some(start) {
                old.seek(SeekFrom::Start(start))?;
            }
            let mut left = diff_len;
            while left > 0 {
                let n = left.min(CHUNK as u64) as usize;
                patch.read_exact(&mut pbuf[..n]).map_err(map_eof)?;
                old.read_exact(&mut obuf[..n]).map_err(|e| {
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        PatchError::Malformed("old image shorter than declared")
                    } else {
                        PatchError::Io(e)
                    }
                })?;
                for (p, o) in pbuf[..n].iter_mut().zip(&obuf[..n]) {
                    *p = p.wrapping_add(*o);
                }
                out.write_all(&pbuf[..n])?;
                left -= n as u64;
            }
            file_pos = Some(start + diff_len);
            old_pos += diff_len as i64;
        }

        let mut left = extra_len;
        while left > 0 {
            let n = left.min(CHUNK as u64) as usize;
            patch.read_exact(&mut pbuf[..n]).map_err(map_eof)?;
            out.write_all(&pbuf[..n])?;
            left -= n as u64;
        }

        produced += diff_len + extra_len;
        old_pos = old_pos
            .checked_add(seek)
            .ok_or(PatchError::Malformed("old cursor overflow"))?;
        stats.records += 1;
        stats.diff_bytes += diff_len;
        stats.extra_bytes += extra_len;
    }

    let mut probe = [0u8; 1];
    loop {
        match patch.read(&mut probe) {
            Ok(0) => break,
            Ok(_) => return Err(PatchError::Malformed("trailing data after the last record")),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(PatchError::Io(e)),
        }
    }
    out.flush()?;
    stats.new_size = produced;
    Ok(stats)
}

/// Apply a (possibly compressed) patch stream.
pub fn apply_stream<O, P, W>(
    old: &mut O,
    old_len: u64,
    patch: P,
    compression: Compression,
    out: &mut W,
) -> Result<ApplyStats, PatchError>
where
    O: Read + Seek + ?Sized,
    P: Read,
    W: Write + ?Sized,
{
    match compression {
        Compression::None => apply(old, old_len, patch, out),
        Compression::Zstd => apply(old, old_len, zstd::stream::read::Decoder::new(patch)?, out),
    }
}

/// Convenience: apply an in-memory patch to an in-memory old image.
pub fn apply_to_vec(old: &[u8], patch: &[u8], compression: Compression) -> Result<Vec<u8>, PatchError> {
    let mut out = Vec::new();
    apply_stream(
        &mut io::Cursor::new(old),
        old.len() as u64,
        patch,
        compression,
        &mut out,
    )?;
    Ok(out)
}
