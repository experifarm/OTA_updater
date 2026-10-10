//! Slicing a patch into FEC groups and shards (切片), and reassembling it.
//!
//! Group `g` covers patch bytes `[g·k·S, min((g+1)·k·S, L))`. Within a group the last real shard is
//! zero-padded. Data shards lying entirely past the end of the patch (only possible in the last group)
//! are *implicit zero shards*: they belong to the RS codeword but are never transmitted.

use crate::rs::ReedSolomon;
use std::ops::Range;

/// FEC parameters: shard payload size and data/parity shards per group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FecParams {
    pub shard_size: u16,
    pub k: u16,
    pub m: u16,
}

impl Default for FecParams {
    fn default() -> Self {
        Self {
            shard_size: 1024,
            k: 32,
            m: 8,
        }
    }
}

/// Largest shard payload accepted (keeps packets under a 1500-byte Ethernet MTU).
pub const MAX_SHARD_SIZE: u16 = 1400;

impl FecParams {
    pub fn validate(&self) -> Result<(), String> {
        if self.shard_size == 0 || self.shard_size > MAX_SHARD_SIZE {
            return Err(format!(
                "shard size must be in 1..={MAX_SHARD_SIZE}, got {}",
                self.shard_size
            ));
        }
        if self.k == 0 || usize::from(self.k) + usize::from(self.m) > 256 {
            return Err(format!("need k >= 1 and k + m <= 256, got k={} m={}", self.k, self.m));
        }
        Ok(())
    }

    pub fn shard_size(&self) -> usize {
        usize::from(self.shard_size)
    }

    pub fn k(&self) -> usize {
        usize::from(self.k)
    }

    pub fn m(&self) -> usize {
        usize::from(self.m)
    }

    /// Shards per group (k + m).
    pub fn n(&self) -> usize {
        self.k() + self.m()
    }

    /// Patch bytes carried by one group.
    pub fn group_bytes(&self) -> u64 {
        self.shard_size() as u64 * self.k() as u64
    }

    pub fn codec(&self) -> ReedSolomon {
        ReedSolomon::new(self.k(), self.m()).expect("validated FEC params")
    }
}

/// Mapping of a patch of `patch_len` bytes onto groups and shards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub fec: FecParams,
    pub patch_len: u64,
}

impl Layout {
    pub fn new(fec: FecParams, patch_len: u64) -> Self {
        Self { fec, patch_len }
    }

    pub fn group_count(&self) -> u64 {
        self.patch_len.div_ceil(self.fec.group_bytes())
    }

    pub fn group_range(&self, g: u32) -> Range<u64> {
        let start = u64::from(g) * self.fec.group_bytes();
        let end = (start + self.fec.group_bytes()).min(self.patch_len);
        start.min(end)..end
    }

    pub fn group_len(&self, g: u32) -> usize {
        let r = self.group_range(g);
        (r.end - r.start) as usize
    }

    /// Number of data shards that carry real patch bytes in group `g`.
    pub fn real_data_shards(&self, g: u32) -> usize {
        self.group_len(g).div_ceil(self.fec.shard_size())
    }

    /// Whether shard `idx` of group `g` is an implicit all-zero shard (never transmitted).
    pub fn is_implicit(&self, g: u32, idx: usize) -> bool {
        idx < self.fec.k() && idx >= self.real_data_shards(g)
    }
}

/// The k (padded) data shards of group `g`.
pub fn data_shards(patch: &[u8], layout: &Layout, g: u32) -> Vec<Vec<u8>> {
    let r = layout.group_range(g);
    let bytes = &patch[r.start as usize..r.end as usize];
    let s = layout.fec.shard_size();
    (0..layout.fec.k())
        .map(|i| {
            let mut shard = vec![0u8; s];
            let start = (i * s).min(bytes.len());
            let end = ((i + 1) * s).min(bytes.len());
            shard[..end - start].copy_from_slice(&bytes[start..end]);
            shard
        })
        .collect()
}

/// All k + m shards of group `g` (data followed by parity).
pub fn encode_group(codec: &ReedSolomon, patch: &[u8], layout: &Layout, g: u32) -> Vec<Vec<u8>> {
    let mut shards = data_shards(patch, layout, g);
    let parity = codec.encode(&shards).expect("equal-size data shards");
    shards.extend(parity);
    shards
}

/// SHA-256 of every group's (unpadded) bytes.
pub fn group_hashes(patch: &[u8], layout: &Layout) -> Vec<[u8; 32]> {
    (0..layout.group_count() as u32)
        .map(|g| {
            let r = layout.group_range(g);
            crate::sha256(&patch[r.start as usize..r.end as usize])
        })
        .collect()
}

/// Concatenate the (reconstructed) data shards of group `g` and strip padding.
pub fn assemble_group(data: &[Option<Vec<u8>>], layout: &Layout, g: u32) -> Vec<u8> {
    let len = layout.group_len(g);
    let mut out = Vec::with_capacity(len);
    for shard in &data[..layout.fec.k()] {
        if out.len() >= len {
            break;
        }
        out.extend_from_slice(shard.as_deref().expect("data shard present"));
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_math() {
        let fec = FecParams {
            shard_size: 100,
            k: 4,
            m: 2,
        };
        let l = Layout::new(fec, 1050);
        assert_eq!(l.group_count(), 3);
        assert_eq!(l.group_range(0), 0..400);
        assert_eq!(l.group_range(2), 800..1050);
        assert_eq!(l.group_len(2), 250);
        assert_eq!(l.real_data_shards(2), 3);
        assert!(l.is_implicit(2, 3));
        assert!(!l.is_implicit(2, 2));
        assert!(!l.is_implicit(2, 4), "parity shards are never implicit");
        assert_eq!(Layout::new(fec, 0).group_count(), 0);
        assert_eq!(Layout::new(fec, 400).group_count(), 1);
    }

    #[test]
    fn slice_reconstruct_assemble_roundtrip() {
        let fec = FecParams {
            shard_size: 64,
            k: 5,
            m: 3,
        };
        let patch: Vec<u8> = (0..1000u32).map(|i| (i * 31 % 251) as u8).collect();
        let layout = Layout::new(fec, patch.len() as u64);
        let codec = fec.codec();
        let mut rebuilt = Vec::new();
        for g in 0..layout.group_count() as u32 {
            let shards = encode_group(&codec, &patch, &layout, g);
            assert_eq!(shards.len(), 8);
            // Drop up to m transmitted shards, keep implicit shards as zeros.
            let mut rx: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
            let mut dropped = 0;
            for (idx, slot) in rx.iter_mut().enumerate() {
                if dropped < 3 && !layout.is_implicit(g, idx) && idx % 2 == 0 {
                    *slot = None;
                    dropped += 1;
                }
            }
            codec.reconstruct(&mut rx).unwrap();
            let bytes = assemble_group(&rx, &layout, g);
            assert_eq!(crate::sha256(&bytes), group_hashes(&patch, &layout)[g as usize]);
            rebuilt.extend(bytes);
        }
        assert_eq!(rebuilt, patch);
    }

    #[test]
    fn validates_params() {
        assert!(FecParams::default().validate().is_ok());
        let p = |shard_size, k, m| FecParams { shard_size, k, m };
        assert!(p(0, 1, 0).validate().is_err());
        assert!(p(2000, 1, 0).validate().is_err());
        assert!(p(64, 0, 1).validate().is_err());
        assert!(p(64, 250, 7).validate().is_err());
        assert!(p(64, 250, 6).validate().is_ok());
    }
}
