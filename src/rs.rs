//! Systematic Reed-Solomon erasure code over GF(2^8).
//!
//! The generator matrix is `[I_k ; C]` where `C` is an m×k Cauchy matrix
//! `C[i][j] = 1 / (x_i + y_j)` with `x_i = k + i`, `y_j = j`. All `x_i`, `y_j` are distinct, so every
//! square sub-matrix of `C` is non-singular, which makes every k×k sub-matrix of `[I; C]` invertible:
//! **any k of the k+m shards reconstruct the original k data shards** (MDS property).

use crate::gf256;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RsError {
    #[error("invalid Reed-Solomon parameters k={k}, m={m} (need k >= 1 and k + m <= 256)")]
    InvalidParams { k: usize, m: usize },
    #[error("expected {expected} shards, got {got}")]
    ShardCount { expected: usize, got: usize },
    #[error("shards have different lengths")]
    ShardLength,
    #[error("not enough shards to reconstruct: have {have}, need {need}")]
    NotEnoughShards { have: usize, need: usize },
}

/// Reed-Solomon codec with `k` data shards and `m` parity shards.
#[derive(Debug, Clone)]
pub struct ReedSolomon {
    k: usize,
    m: usize,
    /// Cauchy parity matrix, m rows × k columns, row-major.
    parity: Vec<u8>,
}

impl ReedSolomon {
    pub fn new(k: usize, m: usize) -> Result<Self, RsError> {
        if k == 0 || k + m > 256 {
            return Err(RsError::InvalidParams { k, m });
        }
        let mut parity = vec![0u8; m * k];
        for i in 0..m {
            let x = (k + i) as u8;
            for j in 0..k {
                let y = j as u8;
                parity[i * k + j] = gf256::inv(gf256::add(x, y));
            }
        }
        Ok(Self { k, m, parity })
    }

    pub fn data_shards(&self) -> usize {
        self.k
    }

    pub fn parity_shards(&self) -> usize {
        self.m
    }

    pub fn total_shards(&self) -> usize {
        self.k + self.m
    }

    /// Row `idx` of the systematic generator matrix (length k).
    fn generator_row(&self, idx: usize) -> Vec<u8> {
        if idx < self.k {
            let mut row = vec![0u8; self.k];
            row[idx] = 1;
            row
        } else {
            let i = idx - self.k;
            self.parity[i * self.k..(i + 1) * self.k].to_vec()
        }
    }

    /// Compute the m parity shards for `data` (exactly k equally sized shards).
    pub fn encode<T: AsRef<[u8]>>(&self, data: &[T]) -> Result<Vec<Vec<u8>>, RsError> {
        if data.len() != self.k {
            return Err(RsError::ShardCount {
                expected: self.k,
                got: data.len(),
            });
        }
        let len = data[0].as_ref().len();
        if data.iter().any(|d| d.as_ref().len() != len) {
            return Err(RsError::ShardLength);
        }
        let mut out = vec![vec![0u8; len]; self.m];
        for (i, shard) in out.iter_mut().enumerate() {
            for (j, d) in data.iter().enumerate() {
                gf256::mul_slice_xor(self.parity[i * self.k + j], d.as_ref(), shard);
            }
        }
        Ok(out)
    }

    /// Reconstruct missing **data** shards in place. `shards` has k+m entries (`None` = erased).
    ///
    /// Returns the number of data shards that had to be recovered. Parity shards are not
    /// regenerated (use [`ReedSolomon::reconstruct_all`] for that).
    pub fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<usize, RsError> {
        let n = self.total_shards();
        if shards.len() != n {
            return Err(RsError::ShardCount {
                expected: n,
                got: shards.len(),
            });
        }
        let present: Vec<usize> = (0..n).filter(|&i| shards[i].is_some()).collect();
        let Some(&first) = present.first() else {
            return Err(RsError::NotEnoughShards { have: 0, need: self.k });
        };
        let len = shards[first].as_ref().map_or(0, Vec::len);
        if present.iter().any(|&i| shards[i].as_ref().map_or(0, Vec::len) != len) {
            return Err(RsError::ShardLength);
        }
        if present.len() < self.k {
            return Err(RsError::NotEnoughShards {
                have: present.len(),
                need: self.k,
            });
        }
        let missing: Vec<usize> = (0..self.k).filter(|&i| shards[i].is_none()).collect();
        if missing.is_empty() {
            return Ok(0);
        }

        // `present` is ascending, so surviving data shards are preferred (their rows are unit
        // vectors, which keeps the elimination cheap).
        let chosen = &present[..self.k];
        let mut matrix = Vec::with_capacity(self.k * self.k);
        for &idx in chosen {
            matrix.extend(self.generator_row(idx));
        }
        let inverse = invert(&matrix, self.k).expect("every k×k sub-matrix of [I; Cauchy] is invertible");

        let mut recovered = Vec::with_capacity(missing.len());
        for &d in &missing {
            let mut out = vec![0u8; len];
            for (j, &src_idx) in chosen.iter().enumerate() {
                let src = shards[src_idx].as_deref().expect("chosen shard is present");
                gf256::mul_slice_xor(inverse[d * self.k + j], src, &mut out);
            }
            recovered.push((d, out));
        }
        let count = recovered.len();
        for (d, out) in recovered {
            shards[d] = Some(out);
        }
        Ok(count)
    }

    /// Reconstruct every missing shard (data and parity) in place.
    pub fn reconstruct_all(&self, shards: &mut [Option<Vec<u8>>]) -> Result<(), RsError> {
        self.reconstruct(shards)?;
        if shards[self.k..].iter().any(Option::is_none) {
            let parity = self.encode(
                &shards[..self.k]
                    .iter()
                    .map(|s| s.as_deref().expect("data reconstructed"))
                    .collect::<Vec<_>>(),
            )?;
            for (slot, p) in shards[self.k..].iter_mut().zip(parity) {
                if slot.is_none() {
                    *slot = Some(p);
                }
            }
        }
        Ok(())
    }
}

/// Invert an n×n matrix over GF(2^8) by Gauss-Jordan elimination. `None` if singular.
pub fn invert(matrix: &[u8], n: usize) -> Option<Vec<u8>> {
    assert_eq!(matrix.len(), n * n);
    let mut a = matrix.to_vec();
    let mut inv = vec![0u8; n * n];
    for i in 0..n {
        inv[i * n + i] = 1;
    }
    for col in 0..n {
        let pivot = (col..n).find(|&r| a[r * n + col] != 0)?;
        if pivot != col {
            for c in 0..n {
                a.swap(pivot * n + c, col * n + c);
                inv.swap(pivot * n + c, col * n + c);
            }
        }
        let scale = gf256::inv(a[col * n + col]);
        for c in 0..n {
            a[col * n + c] = gf256::mul(a[col * n + c], scale);
            inv[col * n + c] = gf256::mul(inv[col * n + c], scale);
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let f = a[row * n + col];
            if f == 0 {
                continue;
            }
            for c in 0..n {
                a[row * n + c] ^= gf256::mul(f, a[col * n + c]);
                inv[row * n + c] ^= gf256::mul(f, inv[col * n + c]);
            }
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic test data.
    fn shards(k: usize, len: usize, seed: u64) -> Vec<Vec<u8>> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..k)
            .map(|_| {
                (0..len)
                    .map(|_| {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        s as u8
                    })
                    .collect()
            })
            .collect()
    }

    fn codeword(rs: &ReedSolomon, data: &[Vec<u8>]) -> Vec<Vec<u8>> {
        let mut all = data.to_vec();
        all.extend(rs.encode(data).unwrap());
        all
    }

    #[test]
    fn rejects_invalid_params() {
        assert!(ReedSolomon::new(0, 4).is_err());
        assert!(ReedSolomon::new(200, 57).is_err());
        assert!(ReedSolomon::new(200, 56).is_ok());
        assert!(ReedSolomon::new(1, 0).is_ok());
    }

    #[test]
    fn matrix_inverse_roundtrip() {
        let rs = ReedSolomon::new(5, 3).unwrap();
        let mut m = Vec::new();
        for idx in [0, 2, 5, 6, 7] {
            m.extend(rs.generator_row(idx));
        }
        let inv = invert(&m, 5).unwrap();
        // inv · m == I
        for r in 0..5 {
            for c in 0..5 {
                let mut acc = 0u8;
                for t in 0..5 {
                    acc ^= gf256::mul(inv[r * 5 + t], m[t * 5 + c]);
                }
                assert_eq!(acc, u8::from(r == c));
            }
        }
        assert!(invert(&[1, 1, 1, 1], 2).is_none());
    }

    /// Every erasure pattern of size <= m is recoverable (exhaustive for small codes).
    #[test]
    fn exhaustive_erasure_patterns() {
        for (k, m) in [(1, 1), (2, 2), (3, 2), (4, 4), (5, 3), (6, 6), (8, 4)] {
            let rs = ReedSolomon::new(k, m).unwrap();
            let data = shards(k, 17, (k * 31 + m) as u64);
            let full = codeword(&rs, &data);
            let n = k + m;
            for mask in 0u32..(1 << n) {
                let erased = mask.count_ones() as usize;
                let mut work: Vec<Option<Vec<u8>>> = full
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (mask & (1 << i) == 0).then(|| s.clone()))
                    .collect();
                let res = rs.reconstruct_all(&mut work);
                if erased <= m {
                    res.unwrap_or_else(|e| panic!("k={k} m={m} mask={mask:b}: {e}"));
                    for (i, s) in work.iter().enumerate() {
                        assert_eq!(s.as_ref().unwrap(), &full[i], "k={k} m={m} mask={mask:b} shard {i}");
                    }
                } else {
                    assert_eq!(
                        res,
                        Err(RsError::NotEnoughShards {
                            have: n - erased,
                            need: k
                        })
                    );
                }
            }
        }
    }

    #[test]
    fn random_erasures_large_codes() {
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut rnd = |bound: usize| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % bound as u64) as usize
        };
        for (k, m) in [(32, 8), (200, 56), (10, 20)] {
            let rs = ReedSolomon::new(k, m).unwrap();
            let data = shards(k, 64, k as u64);
            let full = codeword(&rs, &data);
            for _ in 0..40 {
                let mut work: Vec<Option<Vec<u8>>> = full.iter().cloned().map(Some).collect();
                let erasures = rnd(m + 1);
                let mut erased = 0;
                while erased < erasures {
                    let i = rnd(k + m);
                    if work[i].take().is_some() {
                        erased += 1;
                    }
                }
                let recovered = rs.reconstruct(&mut work).unwrap();
                assert!(recovered <= erasures);
                for i in 0..k {
                    assert_eq!(work[i].as_ref().unwrap(), &full[i]);
                }
            }
        }
    }

    #[test]
    fn errors_are_reported() {
        let rs = ReedSolomon::new(3, 2).unwrap();
        assert_eq!(
            rs.encode(&[vec![1u8; 4], vec![2u8; 4]]),
            Err(RsError::ShardCount { expected: 3, got: 2 })
        );
        assert_eq!(
            rs.encode(&[vec![1u8; 4], vec![2u8; 4], vec![3u8; 5]]),
            Err(RsError::ShardLength)
        );
        let mut bad = vec![Some(vec![0u8; 4]), None, Some(vec![0u8; 3]), Some(vec![0u8; 4]), None];
        assert_eq!(rs.reconstruct(&mut bad), Err(RsError::ShardLength));
        let mut none: Vec<Option<Vec<u8>>> = vec![None; 5];
        assert!(matches!(
            rs.reconstruct(&mut none),
            Err(RsError::NotEnoughShards { have: 0, need: 3 })
        ));
    }
}
