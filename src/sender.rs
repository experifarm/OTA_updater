//! Sender pipeline: diff → compress → slice → Reed-Solomon → signed manifest → broadcast schedule.

use crate::diff::{self, DiffRecord, DiffStats};
use crate::manifest::Manifest;
use crate::patch::Compression;
use crate::slicer::{self, FecParams, Layout};
use crate::wire::{self, Packet};
use anyhow::{Result, bail};
use ed25519_dalek::SigningKey;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

/// Measures elapsed wall time where a clock exists (`Instant::now` panics on wasm32-unknown-unknown).
#[cfg(feature = "native")]
fn stopwatch() -> impl FnOnce() -> Duration {
    let start = std::time::Instant::now();
    move || start.elapsed()
}

#[cfg(not(feature = "native"))]
fn stopwatch() -> impl FnOnce() -> Duration {
    || Duration::ZERO
}

/// Destination for encoded datagrams (UDP socket, in-memory collector, channel, …).
pub trait PacketSink {
    fn send(&mut self, datagram: &[u8]) -> io::Result<()>;
}

impl PacketSink for Vec<Vec<u8>> {
    fn send(&mut self, datagram: &[u8]) -> io::Result<()> {
        self.push(datagram.to_vec());
        Ok(())
    }
}

impl PacketSink for mpsc::Sender<Vec<u8>> {
    fn send(&mut self, datagram: &[u8]) -> io::Result<()> {
        mpsc::Sender::send(self, datagram.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "receiver gone"))
    }
}

#[derive(Debug, Clone)]
pub struct SenderConfig {
    pub fw_version: u64,
    pub fec: FecParams,
    pub compression: Compression,
    /// zstd compression level.
    pub level: i32,
    /// Fixed session id (random if `None`).
    pub session_id: Option<u64>,
    /// Compress as independent zstd frames of this many raw bytes, so pure-Rust decoders can stream
    /// (single frame if `None`).
    pub zstd_frame_size: Option<usize>,
}

impl Default for SenderConfig {
    fn default() -> Self {
        Self {
            fw_version: 1,
            fec: FecParams::default(),
            compression: Compression::Zstd,
            level: 19,
            session_id: None,
            zstd_frame_size: None,
        }
    }
}

/// Everything needed to broadcast one update.
pub struct Prepared {
    pub manifest: Manifest,
    /// Signed manifest container.
    pub container: Vec<u8>,
    pub patch: Vec<u8>,
    /// Per group, all k + m shards (implicit zero shards included but never sent).
    pub shards: Vec<Vec<Vec<u8>>>,
    pub diff_stats: DiffStats,
    /// The diff records of the patch (old → new mapping), e.g. for visualization.
    pub records: Vec<DiffRecord>,
    pub prepare_time: Duration,
}

/// Build patch, FEC shards and the signed manifest.
pub fn prepare(old: &[u8], new: &[u8], key: &SigningKey, cfg: &SenderConfig) -> Result<Prepared> {
    cfg.fec.validate().map_err(anyhow::Error::msg)?;
    if old.len() > crate::suffix::MAX_LEN {
        bail!("old image too large ({} bytes)", old.len());
    }
    let elapsed = stopwatch();
    let mut records = Vec::new();
    let (raw, raw_stats) = diff::diff_raw_traced(old, new, &mut |r| records.push(r));
    let (patch, diff_stats) =
        diff::finish_patch_framed(raw, raw_stats, cfg.compression, cfg.level, cfg.zstd_frame_size)?;
    let layout = Layout::new(cfg.fec, patch.len() as u64);
    if layout.group_count() > u64::from(u32::MAX) {
        bail!("patch too large for the FEC layout");
    }
    let codec = cfg.fec.codec();
    let shards = (0..layout.group_count() as u32)
        .map(|g| slicer::encode_group(&codec, &patch, &layout, g))
        .collect();
    let session_id = match cfg.session_id {
        Some(id) => id,
        #[cfg(feature = "native")]
        None => crate::keys::random_u64()?,
        #[cfg(not(feature = "native"))]
        None => bail!("a session id is required without OS randomness (feature `native`)"),
    };
    let manifest = Manifest {
        session_id,
        fw_version: cfg.fw_version,
        old_size: old.len() as u64,
        old_sha256: crate::sha256(old),
        new_size: new.len() as u64,
        new_sha256: crate::sha256(new),
        patch_size: patch.len() as u64,
        patch_sha256: crate::sha256(&patch),
        compression: cfg.compression,
        fec: cfg.fec,
        group_hashes: slicer::group_hashes(&patch, &layout),
    };
    let container = manifest.sign(key);
    Ok(Prepared {
        manifest,
        container,
        patch,
        shards,
        diff_stats,
        records,
        prepare_time: elapsed(),
    })
}

/// Packet ordering within a broadcast round.
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    /// Number of groups whose shards are interleaved (1 = group by group).
    pub interleave: usize,
    /// Re-send the manifest every this many shard packets (0 = only at round start).
    pub manifest_every: usize,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            interleave: 8,
            manifest_every: 256,
        }
    }
}

impl Prepared {
    pub fn layout(&self) -> Layout {
        self.manifest.layout()
    }

    pub fn manifest_packets(&self) -> Vec<Vec<u8>> {
        wire::manifest_fragments(self.manifest.session_id, &self.container)
            .iter()
            .map(Packet::encode)
            .collect()
    }

    /// Encoded datagrams of one carousel round.
    pub fn round_packets(&self, schedule: &Schedule) -> Vec<Vec<u8>> {
        let layout = self.layout();
        let groups = self.shards.len();
        let n = self.manifest.fec.n();
        let manifest = self.manifest_packets();
        let mut out = manifest.clone();
        let mut since_manifest = 0usize;
        let window = schedule.interleave.max(1);
        for first in (0..groups).step_by(window) {
            let last = (first + window).min(groups);
            for idx in 0..n {
                for g in first..last {
                    if layout.is_implicit(g as u32, idx) {
                        continue;
                    }
                    let pkt = Packet::Shard {
                        session_id: self.manifest.session_id,
                        group: g as u32,
                        index: idx as u16,
                        data: self.shards[g][idx].clone(),
                    };
                    out.push(pkt.encode());
                    since_manifest += 1;
                    if schedule.manifest_every > 0 && since_manifest == schedule.manifest_every {
                        out.extend(manifest.iter().cloned());
                        since_manifest = 0;
                    }
                }
            }
        }
        out
    }

    /// Human-readable summary of the prepared update.
    pub fn summary(&self) -> String {
        let m = &self.manifest;
        let layout = self.layout();
        let shards_sent: usize = (0..self.shards.len() as u32)
            .map(|g| m.fec.n() - (0..m.fec.k()).filter(|&i| layout.is_implicit(g, i)).count())
            .sum();
        format!(
            "session {:016x} fw_version {}\n\
             old image   {:>10} bytes  sha256 {}\n\
             new image   {:>10} bytes  sha256 {}\n\
             raw patch   {:>10} bytes  ({} records, {} diff bytes, {} extra bytes)\n\
             patch       {:>10} bytes  ({:?}, {:.2}% of new image)\n\
             FEC         shard {} B, k={} m={} -> {} groups, {} shards/round ({:.0}% parity overhead)\n\
             manifest    {:>10} bytes  ({} fragments), prepared in {:.2?}",
            m.session_id,
            m.fw_version,
            m.old_size,
            crate::to_hex(&m.old_sha256),
            m.new_size,
            crate::to_hex(&m.new_sha256),
            self.diff_stats.raw_len,
            self.diff_stats.records,
            self.diff_stats.diff_bytes,
            self.diff_stats.extra_bytes,
            m.patch_size,
            m.compression,
            100.0 * m.patch_size as f64 / m.new_size.max(1) as f64,
            m.fec.shard_size,
            m.fec.k,
            m.fec.m,
            m.group_count(),
            shards_sent,
            100.0 * m.fec.m() as f64 / m.fec.k() as f64,
            self.container.len(),
            self.container.len().div_ceil(wire::MANIFEST_FRAG_SIZE),
            self.prepare_time,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BroadcastConfig {
    /// `None` = repeat forever.
    pub rounds: Option<u32>,
    pub schedule: Schedule,
    /// Pause between rounds.
    pub round_gap: Duration,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            rounds: Some(3),
            schedule: Schedule::default(),
            round_gap: Duration::from_millis(50),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BroadcastStats {
    pub rounds: u32,
    pub datagrams: u64,
    pub bytes: u64,
}

/// Run the data carousel into `sink`. `on_round(round, stats)` is called after each round.
pub fn broadcast<S: PacketSink + ?Sized>(
    prepared: &Prepared,
    cfg: &BroadcastConfig,
    sink: &mut S,
    on_round: &mut dyn FnMut(u32, &BroadcastStats),
) -> io::Result<BroadcastStats> {
    let round = prepared.round_packets(&cfg.schedule);
    let mut stats = BroadcastStats::default();
    while cfg.rounds.is_none_or(|r| stats.rounds < r) {
        if stats.rounds > 0 && !cfg.round_gap.is_zero() {
            std::thread::sleep(cfg.round_gap);
        }
        for pkt in &round {
            sink.send(pkt)?;
            stats.datagrams += 1;
            stats.bytes += pkt.len() as u64;
        }
        stats.rounds += 1;
        on_round(stats.rounds, &stats);
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_contains_every_transmitted_shard_once() {
        let (old, new) = crate::imagegen::generate_pair(96 * 1024, 3);
        let key = SigningKey::from_bytes(&[4; 32]);
        let cfg = SenderConfig {
            fec: FecParams {
                shard_size: 256,
                k: 8,
                m: 3,
            },
            session_id: Some(77),
            ..Default::default()
        };
        let p = prepare(&old, &new, &key, &cfg).unwrap();
        let layout = p.layout();
        let schedule = Schedule {
            interleave: 4,
            manifest_every: 10,
        };
        let pkts = p.round_packets(&schedule);
        let mut seen = std::collections::HashSet::new();
        let mut frags = 0;
        for d in &pkts {
            match Packet::decode(d).unwrap() {
                Packet::Shard {
                    session_id,
                    group,
                    index,
                    data,
                } => {
                    assert_eq!(session_id, 77);
                    assert_eq!(data, p.shards[group as usize][index as usize]);
                    assert!(seen.insert((group, index)), "duplicate shard");
                }
                Packet::ManifestFrag { .. } => frags += 1,
            }
        }
        let expected: usize = (0..p.shards.len() as u32)
            .map(|g| (0..11).filter(|&i| !layout.is_implicit(g, i)).count())
            .sum();
        assert_eq!(seen.len(), expected);
        assert!(frags > expected / 10);
        assert!(p.summary().contains("session 000000000000004d"));
    }
}
