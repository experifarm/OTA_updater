//! bsdiff-style binary differencing (Colin Percival's bsdiff 4.x algorithm) producing the
//! interleaved, forward-only patch stream understood by [`crate::patch`].
//!
//! 1. Suffix-sort the old image ([`crate::suffix`]).
//! 2. Scan the new image; at each position find the longest exact match in the old image by binary
//!    search on the suffix array, and accept it when it beats the current alignment by more than 8
//!    matching bytes ("approximate matching": runs with sparse byte changes, e.g. relocated pointers,
//!    are kept in a single diff region whose bytes are mostly zero).
//! 3. Extend each region forwards/backwards while at least half of the bytes match, resolve overlaps,
//!    and emit `(diff_len, extra_len, seek)` records.

use crate::patch::{Compression, PatchHeader};
use crate::suffix::{self, suffix_array};
use crate::varint;
use std::io;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub records: u64,
    pub diff_bytes: u64,
    pub extra_bytes: u64,
    /// Uncompressed patch stream length.
    pub raw_len: u64,
    /// Final (possibly compressed) patch length.
    pub patch_len: u64,
}

fn match_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Longest match of a prefix of `new` among the suffixes of `old`. Returns `(len, pos)`.
fn search(sa: &[i32], old: &[u8], new: &[u8]) -> (usize, usize) {
    search_traced(sa, old, new, |_| {})
}

/// One binary-search step over the suffix array: the window `[lo, hi]`, the probed index `mid`, and
/// whether the probed suffix sorted before the query (`go_right`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchStep {
    pub lo: usize,
    pub hi: usize,
    pub mid: usize,
    pub go_right: bool,
}

/// The diff's suffix-array search, reporting each binary-search step (for visualization). Generic
/// over the observer so the no-op version used by the diff compiles to the plain loop.
///
/// Note: bsdiff 4.3 compares only `min(suffix len, query len)` bytes, so a suffix that is a proper
/// prefix of the query compares "equal" and the search goes left, past longer matches (e.g. it
/// finds "a" instead of "ana" for "anab" in "banana"). Here such a suffix correctly sorts before
/// the query, so the window always ends next to the query's insertion point and the longest match
/// is one of its two neighbours.
pub fn search_traced<F: FnMut(SearchStep)>(sa: &[i32], old: &[u8], new: &[u8], mut on_step: F) -> (usize, usize) {
    let (mut st, mut en) = (0usize, sa.len() - 1);
    while en - st >= 2 {
        let x = st + (en - st) / 2;
        let p = sa[x] as usize;
        let m = (old.len() - p).min(new.len());
        let go_right = match old[p..p + m].cmp(&new[..m]) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Equal => old.len() - p < new.len(),
            std::cmp::Ordering::Greater => false,
        };
        on_step(SearchStep {
            lo: st,
            hi: en,
            mid: x,
            go_right,
        });
        if go_right {
            st = x;
        } else {
            en = x;
        }
    }
    let (ps, pe) = (sa[st] as usize, sa[en] as usize);
    let (x, y) = (match_len(&old[ps..], new), match_len(&old[pe..], new));
    if x > y { (x, ps) } else { (y, pe) }
}

/// One patch record, for visualization: new bytes `[new_off, new_off + diff_len)` are produced from
/// old bytes `[old_off, old_off + diff_len)` plus diff bytes, followed by `extra_len` literal bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffRecord {
    pub new_off: u64,
    pub old_off: u64,
    pub diff_len: u64,
    pub extra_len: u64,
}

/// Produce the uncompressed patch stream transforming `old` into `new`.
pub fn diff_raw(old: &[u8], new: &[u8]) -> (Vec<u8>, DiffStats) {
    diff_raw_traced(old, new, &mut |_| {})
}

/// Like [`diff_raw`], reporting every record to `on_record`.
pub fn diff_raw_traced(old: &[u8], new: &[u8], on_record: &mut dyn FnMut(DiffRecord)) -> (Vec<u8>, DiffStats) {
    assert!(old.len() <= suffix::MAX_LEN, "old image too large");
    let sa = suffix_array(old);
    let oldsize = old.len() as i64;
    let newsize = new.len() as i64;
    let at_old = |i: i64| old[i as usize];
    let at_new = |i: i64| new[i as usize];

    let mut out = Vec::with_capacity(new.len() / 4 + 64);
    out.extend_from_slice(
        &PatchHeader {
            old_size: old.len() as u64,
            new_size: new.len() as u64,
        }
        .encode(),
    );
    let mut stats = DiffStats::default();

    let (mut scan, mut len, mut pos) = (0i64, 0i64, 0i64);
    let (mut lastscan, mut lastpos, mut lastoffset) = (0i64, 0i64, 0i64);

    while scan < newsize {
        let mut oldscore = 0i64;
        scan += len;
        let mut scsc = scan;
        while scan < newsize {
            let (l, p) = search(&sa, old, &new[scan as usize..]);
            len = l as i64;
            pos = p as i64;
            while scsc < scan + len {
                let o = scsc + lastoffset;
                if o >= 0 && o < oldsize && at_old(o) == at_new(scsc) {
                    oldscore += 1;
                }
                scsc += 1;
            }
            if (len == oldscore && len != 0) || len > oldscore + 8 {
                break;
            }
            let o = scan + lastoffset;
            if o >= 0 && o < oldsize && at_old(o) == at_new(scan) {
                oldscore -= 1;
            }
            scan += 1;
        }

        if len != oldscore || scan == newsize {
            // Forward extension from the previous match.
            let (mut s, mut sf, mut lenf) = (0i64, 0i64, 0i64);
            let mut i = 0i64;
            while lastscan + i < scan && lastpos + i < oldsize {
                if at_old(lastpos + i) == at_new(lastscan + i) {
                    s += 1;
                }
                i += 1;
                if s * 2 - i > sf * 2 - lenf {
                    sf = s;
                    lenf = i;
                }
            }

            // Backward extension from the new match.
            let mut lenb = 0i64;
            if scan < newsize {
                let (mut s, mut sb) = (0i64, 0i64);
                let mut i = 1i64;
                while scan >= lastscan + i && pos >= i {
                    if at_old(pos - i) == at_new(scan - i) {
                        s += 1;
                    }
                    if s * 2 - i > sb * 2 - lenb {
                        sb = s;
                        lenb = i;
                    }
                    i += 1;
                }
            }

            // Resolve overlap between the two extensions.
            if lastscan + lenf > scan - lenb {
                let overlap = (lastscan + lenf) - (scan - lenb);
                let (mut s, mut ss, mut lens) = (0i64, 0i64, 0i64);
                for i in 0..overlap {
                    if at_new(lastscan + lenf - overlap + i) == at_old(lastpos + lenf - overlap + i) {
                        s += 1;
                    }
                    if at_new(scan - lenb + i) == at_old(pos - lenb + i) {
                        s -= 1;
                    }
                    if s > ss {
                        ss = s;
                        lens = i + 1;
                    }
                }
                lenf += lens - overlap;
                lenb -= lens;
            }

            let extra_len = (scan - lenb) - (lastscan + lenf);
            let seek = (pos - lenb) - (lastpos + lenf);
            varint::write_u64(&mut out, lenf as u64);
            varint::write_u64(&mut out, extra_len as u64);
            varint::write_u64(&mut out, varint::zigzag(seek));
            let (ns, os) = (lastscan as usize, lastpos as usize);
            out.extend(
                new[ns..ns + lenf as usize]
                    .iter()
                    .zip(&old[os..os + lenf as usize])
                    .map(|(n, o)| n.wrapping_sub(*o)),
            );
            let es = (lastscan + lenf) as usize;
            out.extend_from_slice(&new[es..es + extra_len as usize]);
            on_record(DiffRecord {
                new_off: lastscan as u64,
                old_off: lastpos as u64,
                diff_len: lenf as u64,
                extra_len: extra_len as u64,
            });

            stats.records += 1;
            stats.diff_bytes += lenf as u64;
            stats.extra_bytes += extra_len as u64;

            lastscan = scan - lenb;
            lastpos = pos - lenb;
            lastoffset = pos - scan;
        }
    }
    stats.raw_len = out.len() as u64;
    stats.patch_len = stats.raw_len;
    (out, stats)
}

/// Produce the final patch: diff + optional zstd compression of the whole stream.
pub fn make_patch(old: &[u8], new: &[u8], compression: Compression, level: i32) -> io::Result<(Vec<u8>, DiffStats)> {
    let (raw, stats) = diff_raw(old, new);
    finish_patch(raw, stats, compression, level)
}

/// Compress a raw patch stream (if requested) and record the final size in `stats`.
pub fn finish_patch(
    raw: Vec<u8>,
    stats: DiffStats,
    compression: Compression,
    level: i32,
) -> io::Result<(Vec<u8>, DiffStats)> {
    finish_patch_framed(raw, stats, compression, level, None)
}

/// Like [`finish_patch`]; with `frame_size = Some(n)` the zstd stream is a sequence of independent
/// frames of at most `n` raw bytes each, which lets every decoder stream it (see
/// [`crate::patch::PushDecompressor`]) at a small cost in compression ratio.
pub fn finish_patch_framed(
    raw: Vec<u8>,
    mut stats: DiffStats,
    compression: Compression,
    level: i32,
    frame_size: Option<usize>,
) -> io::Result<(Vec<u8>, DiffStats)> {
    let patch = match (compression, frame_size) {
        (Compression::None, _) => raw,
        (Compression::Zstd, None) => compress_zstd(&raw, level)?,
        (Compression::Zstd, Some(n)) => compress_zstd_framed(&raw, level, n)?,
    };
    stats.patch_len = patch.len() as u64;
    Ok((patch, stats))
}

/// zstd-compress `data` as concatenated independent frames of at most `frame_size` input bytes.
pub fn compress_zstd_framed(data: &[u8], level: i32, frame_size: usize) -> io::Result<Vec<u8>> {
    if data.is_empty() {
        return compress_zstd(data, level);
    }
    let mut out = Vec::new();
    for chunk in data.chunks(frame_size.max(1)) {
        out.extend(compress_zstd(chunk, level)?);
    }
    Ok(out)
}

/// zstd-compress with the C library (native builds).
#[cfg(feature = "native")]
pub fn compress_zstd(data: &[u8], level: i32) -> io::Result<Vec<u8>> {
    zstd::stream::encode_all(data, level)
}

/// zstd-compress with the pure-Rust `ruzstd` encoder (non-native builds, e.g. WebAssembly). `ruzstd`
/// only implements its "fastest" level (≈ zstd level 1), so `level` is ignored. The output is a
/// standard zstd frame (128 KiB window) readable by any zstd decoder.
#[cfg(not(feature = "native"))]
pub fn compress_zstd(data: &[u8], _level: i32) -> io::Result<Vec<u8>> {
    Ok(ruzstd::encoding::compress_to_vec(
        data,
        ruzstd::encoding::CompressionLevel::Fastest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::{self, PatchError, apply_to_vec};
    use proptest::prelude::*;

    fn roundtrip(old: &[u8], new: &[u8]) -> DiffStats {
        let (raw, stats) = diff_raw(old, new);
        assert_eq!(apply_to_vec(old, &raw, Compression::None).unwrap(), new);
        let (z, zstats) = make_patch(old, new, Compression::Zstd, 3).unwrap();
        assert_eq!(apply_to_vec(old, &z, Compression::Zstd).unwrap(), new);
        assert_eq!(zstats.patch_len, z.len() as u64);
        stats
    }

    fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
        let mut s = seed | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s as u8
            })
            .collect()
    }

    #[test]
    fn edge_cases() {
        roundtrip(b"", b"");
        roundtrip(b"", b"hello world");
        roundtrip(b"hello world", b"");
        roundtrip(b"a", b"a");
        roundtrip(b"abc", b"xyz");
        let s = roundtrip(b"same content here", b"same content here");
        assert_eq!(s.extra_bytes, 0);
    }

    #[test]
    fn structured_edits_produce_small_patches() {
        let old = pseudo_random(200_000, 42);
        let mut new = old.clone();
        // In-place byte changes.
        for i in (1000..150_000).step_by(997) {
            new[i] ^= 0x5a;
        }
        // Insertion (shifts everything after it).
        new.splice(50_000..50_000, pseudo_random(3000, 7));
        // Deletion.
        new.drain(120_000..124_000);
        // Appended section.
        new.extend(pseudo_random(5000, 9));
        let stats = roundtrip(&old, &new);
        assert!(stats.extra_bytes < 12_000, "extra bytes {}", stats.extra_bytes);
        let (z, _) = make_patch(&old, &new, Compression::Zstd, 19).unwrap();
        assert!(z.len() < new.len() / 10, "patch {} vs new {}", z.len(), new.len());
    }

    #[test]
    fn relocated_pointers_become_small_diffs() {
        // A table of 16-byte structs whose first field is a pointer that got relocated by a constant
        // (code moved), while the remaining fields are unchanged: the classic bsdiff case. The
        // approximate match keeps the table in one diff region whose bytes are mostly zero.
        let entry = |i: u32, reloc: u32| {
            let mut e = Vec::with_capacity(16);
            e.extend((0x0800_0000 + i * 52 + reloc).to_le_bytes());
            e.extend(i.to_le_bytes());
            e.extend(0xA5A5_0000u32.wrapping_add(i * 977).to_le_bytes());
            e.extend((i.wrapping_mul(2_654_435_761)).to_le_bytes());
            e
        };
        let old: Vec<u8> = (0..10_000u32).flat_map(|i| entry(i, 0)).collect();
        let new: Vec<u8> = (0..10_000u32).flat_map(|i| entry(i, 0x140)).collect();
        let stats = roundtrip(&old, &new);
        assert_eq!(stats.extra_bytes, 0, "{stats:?}");
        let (z, _) = make_patch(&old, &new, Compression::Zstd, 19).unwrap();
        // C zstd level 19 reaches > 50x on this table; the pure-Rust encoder (fastest level) ~35x.
        let min_ratio = if cfg!(feature = "native") { 50 } else { 25 };
        assert!(z.len() < old.len() / min_ratio, "patch {} bytes, {stats:?}", z.len());
    }

    #[test]
    fn patch_rejects_wrong_old_size_and_garbage() {
        let (raw, _) = diff_raw(b"old data", b"new data!");
        assert!(matches!(
            apply_to_vec(b"old dat", &raw, Compression::None),
            Err(PatchError::OldSizeMismatch { .. })
        ));
        assert!(matches!(
            apply_to_vec(b"old data", b"NOTAPATCH-garbage-garbage-garbage", Compression::None),
            Err(PatchError::BadMagic)
        ));
        assert!(matches!(
            apply_to_vec(b"old data", &raw[..raw.len() - 1], Compression::None),
            Err(PatchError::Truncated)
        ));
        let mut trailing = raw.clone();
        trailing.push(0);
        assert!(matches!(
            apply_to_vec(b"old data", &trailing, Compression::None),
            Err(PatchError::Malformed(_))
        ));
    }

    #[test]
    fn patch_rejects_out_of_range_reads() {
        let mut p = patch::PatchHeader {
            old_size: 4,
            new_size: 8,
        }
        .encode()
        .to_vec();
        // diff_len 8 from an old image of 4 bytes.
        varint::write_u64(&mut p, 8);
        varint::write_u64(&mut p, 0);
        varint::write_u64(&mut p, 0);
        p.extend([0u8; 8]);
        assert!(matches!(
            apply_to_vec(b"abcd", &p, Compression::None),
            Err(PatchError::Malformed(_))
        ));
        // Negative seek before a diff.
        let mut p = patch::PatchHeader {
            old_size: 4,
            new_size: 2,
        }
        .encode()
        .to_vec();
        varint::write_u64(&mut p, 0);
        varint::write_u64(&mut p, 1);
        varint::write_u64(&mut p, varint::zigzag(-3));
        p.push(b'x');
        varint::write_u64(&mut p, 1);
        varint::write_u64(&mut p, 0);
        varint::write_u64(&mut p, 0);
        p.push(0);
        assert!(matches!(
            apply_to_vec(b"abcd", &p, Compression::None),
            Err(PatchError::Malformed(_))
        ));
    }

    #[test]
    fn search_finds_the_longest_match_through_prefix_suffixes() {
        // bsdiff 4.3's truncated comparison returns "a" (len 1) here.
        let sa = suffix_array(b"banana");
        assert_eq!(search(&sa, b"banana", b"anab"), (3, 1));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// The suffix-array search returns a longest match (checked against brute force).
        #[test]
        fn prop_search_is_longest(old in proptest::collection::vec(0u8..3, 0..200),
                                  query in proptest::collection::vec(0u8..3, 0..40)) {
            let sa = suffix_array(&old);
            let (len, pos) = search(&sa, &old, &query);
            let best = (0..=old.len()).map(|p| match_len(&old[p..], &query)).max().unwrap();
            prop_assert_eq!(len, best);
            prop_assert_eq!(match_len(&old[pos..], &query), len);
        }

        #[test]
        fn prop_roundtrip(old in proptest::collection::vec(0u8..8, 0..3000),
                          edits in proptest::collection::vec((0usize..3000, 0u8..3, any::<u8>()), 0..40)) {
            let mut new = old.clone();
            for (at, kind, byte) in edits {
                let at = if new.is_empty() { 0 } else { at % (new.len() + 1) };
                match kind {
                    0 if at < new.len() => new[at] = byte,
                    1 => new.insert(at, byte),
                    _ if at < new.len() => { new.remove(at); }
                    _ => new.push(byte),
                }
            }
            let (raw, _) = diff_raw(&old, &new);
            prop_assert_eq!(apply_to_vec(&old, &raw, Compression::None).unwrap(), new);
        }

        /// Arbitrary bytes after a valid header must never panic the applier.
        #[test]
        fn prop_malformed_never_panics(body in proptest::collection::vec(any::<u8>(), 0..200),
                                       new_size in 0u64..1000) {
            let old = b"0123456789abcdef";
            let mut p = patch::PatchHeader { old_size: old.len() as u64, new_size }.encode().to_vec();
            p.extend(body);
            let _ = apply_to_vec(old, &p, Compression::None);
        }
    }
}
