//! Receiver pipeline (监听分片 → 纠错编码 → 流式差分还原 → Ed25519签名校验).
//!
//! ```text
//! DatagramSource ─► Impairment ─► wire::decode (CRC32)
//!   ├─ ManifestFrag ─► reassemble ─► verify signature (pinned key) ─► acceptance checks ─► session
//!   └─ Shard ───────► group buffer ─► (≥ k shards) RS reconstruct ─► SHA-256 == signed group hash
//!                        └─► in-order release ─► mpsc ─► [apply thread]
//! [apply thread]: ChannelReader ─► zstd ─► patch::apply(old image) ─► HashingWriter ─► <out>.part
//! finish: size + SHA-256 == signed manifest ─► fsync ─► atomic rename <out>.part → <out>
//! ```
//!
//! Every byte handed to the patcher is authenticated (it matches a signed per-group hash), and the new
//! image is only installed once its full SHA-256 matches the signed manifest.

use crate::channel::{Impairment, ImpairmentConfig, ImpairmentStats};
use crate::hashio::{HashingReader, HashingWriter, hash_reader};
use crate::manifest::{self, Manifest};
use crate::patch::{self, ApplyStats, Compression};
use crate::rs::ReedSolomon;
use crate::slicer::{self, Layout};
use crate::wire::{self, Packet};
use anyhow::{Context, Result, anyhow, bail};
use ed25519_dalek::VerifyingKey;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Result of polling a datagram source.
#[derive(Debug)]
pub enum Recv {
    Datagram(Vec<u8>),
    /// Nothing arrived within the source's poll interval.
    Timeout,
    /// The source will never deliver more datagrams.
    Closed,
}

pub trait DatagramSource {
    fn recv(&mut self) -> io::Result<Recv>;
}

/// In-memory source (deterministic tests): yields the datagrams, then `Closed`.
pub struct VecSource(std::vec::IntoIter<Vec<u8>>);

impl VecSource {
    pub fn new(datagrams: Vec<Vec<u8>>) -> Self {
        Self(datagrams.into_iter())
    }
}

impl DatagramSource for VecSource {
    fn recv(&mut self) -> io::Result<Recv> {
        Ok(self.0.next().map_or(Recv::Closed, Recv::Datagram))
    }
}

impl DatagramSource for mpsc::Receiver<Vec<u8>> {
    fn recv(&mut self) -> io::Result<Recv> {
        match self.recv_timeout(Duration::from_millis(100)) {
            Ok(d) => Ok(Recv::Datagram(d)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(Recv::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(Recv::Closed),
        }
    }
}

/// `Read` over a channel of byte chunks; EOF when the sending side is dropped.
pub struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl ChannelReader {
    pub fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            chunk: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.chunk.len() {
            match self.rx.recv() {
                Ok(c) => {
                    self.chunk = c;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.pos);
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    /// Pinned Ed25519 public key of the update publisher.
    pub public_key: VerifyingKey,
    /// Firmware version currently installed; only strictly newer versions are accepted.
    pub installed_version: u64,
    pub max_image_size: u64,
    /// Bound on shards buffered before a manifest has been verified.
    pub max_prebuffer_bytes: usize,
    pub timeout: Option<Duration>,
    pub impairment: Option<ImpairmentConfig>,
}

impl ReceiverConfig {
    pub fn new(public_key: VerifyingKey) -> Self {
        Self {
            public_key,
            installed_version: 0,
            max_image_size: 1 << 30,
            max_prebuffer_bytes: 32 << 20,
            timeout: None,
            impairment: None,
        }
    }
}

/// Progress notifications.
#[derive(Debug)]
pub enum Event<'a> {
    ManifestAccepted(&'a Manifest),
    ManifestSignatureInvalid {
        session_id: u64,
    },
    SessionRejected {
        session_id: u64,
        reason: &'a str,
    },
    GroupComplete {
        group: u32,
        completed: u32,
        total: u32,
        recovered_shards: usize,
    },
    GroupHashMismatch {
        group: u32,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ReceiveStats {
    /// Datagrams delivered to the decoder (after impairment).
    pub datagrams: u64,
    pub impairment: ImpairmentStats,
    /// CRC, format or validation failures.
    pub bad_packets: u64,
    /// Packets of other or rejected sessions.
    pub ignored_packets: u64,
    pub manifest_repeats: u64,
    /// Shards not needed: their group was already complete, or the shard was already held.
    pub redundant_shards: u64,
    pub shards_accepted: u64,
    /// Data shards rebuilt by Reed-Solomon decoding.
    pub shards_recovered: u64,
    pub groups_total: u32,
    /// Groups that needed FEC repair (at least one data shard missing).
    pub groups_repaired: u32,
    pub group_hash_failures: u32,
    pub signature_failures: u32,
    /// Largest number of completed groups waiting for an earlier group (in-order release).
    pub max_pending_groups: usize,
    pub manifest_at: Option<Duration>,
    /// When the first authenticated patch bytes reached the patcher.
    pub first_group_at: Option<Duration>,
    /// When the last group was reconstructed.
    pub last_group_at: Option<Duration>,
}

#[derive(Debug)]
pub struct Report {
    pub manifest: Manifest,
    pub stats: ReceiveStats,
    pub apply: ApplyStats,
    pub new_sha256: [u8; 32],
    pub output: PathBuf,
    pub elapsed: Duration,
}

/// Path of the in-progress image written next to `out`.
pub fn temp_path(out: &Path) -> PathBuf {
    let mut name = out
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "image".into());
    name.push(".part");
    out.with_file_name(name)
}

struct TempGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct ApplyOutcome {
    stats: ApplyStats,
    new_sha256: [u8; 32],
    new_len: u64,
    patch_sha256: [u8; 32],
    patch_len: u64,
}

fn spawn_apply(
    old_path: PathBuf,
    tmp_path: PathBuf,
    compression: Compression,
    rx: mpsc::Receiver<Vec<u8>>,
) -> io::Result<JoinHandle<Result<ApplyOutcome>>> {
    thread::Builder::new().name("ota-apply".into()).spawn(move || {
        let old_file = File::open(&old_path).with_context(|| format!("opening {}", old_path.display()))?;
        let old_len = old_file.metadata()?.len();
        let mut old = BufReader::with_capacity(64 * 1024, old_file);
        let mut reader = HashingReader::new(ChannelReader::new(rx));
        let file = File::create(&tmp_path).with_context(|| format!("creating {}", tmp_path.display()))?;
        let mut writer = HashingWriter::new(BufWriter::with_capacity(256 * 1024, file));
        let stats = patch::apply_stream(&mut old, old_len, &mut reader, compression, &mut writer)
            .context("applying patch stream")?;
        io::copy(&mut reader, &mut io::sink())?;
        let (patch_sha256, patch_len) = reader.digest();
        let (buffered, new_sha256, new_len) = writer.finish();
        let file = buffered.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(ApplyOutcome {
            stats,
            new_sha256,
            new_len,
            patch_sha256,
            patch_len,
        })
    })
}

struct Assembly {
    count: u16,
    total_len: u32,
    frags: Vec<Option<Vec<u8>>>,
    have: u16,
}

#[derive(Default)]
struct GroupState {
    /// Empty until the first shard arrives (lazy allocation).
    shards: Vec<Option<Vec<u8>>>,
    present: usize,
    done: bool,
}

struct Session {
    manifest: Manifest,
    layout: Layout,
    codec: ReedSolomon,
    groups: Vec<GroupState>,
    completed: u32,
    next_group: u32,
    ready: BTreeMap<u32, Vec<u8>>,
    tx: Option<mpsc::Sender<Vec<u8>>>,
    worker: Option<JoinHandle<Result<ApplyOutcome>>>,
    worker_gone: bool,
}

impl Session {
    fn group_count(&self) -> u32 {
        self.manifest.group_count()
    }

    fn all_delivered(&self) -> bool {
        self.next_group == self.group_count()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Close the stream so the apply thread terminates, then wait for it (it owns the temp file).
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

const MAX_ASSEMBLIES: usize = 16;

struct Engine<'c> {
    cfg: &'c ReceiverConfig,
    old_path: PathBuf,
    tmp_path: PathBuf,
    old_sha256: [u8; 32],
    old_size: u64,
    assemblies: HashMap<u64, Assembly>,
    rejected: HashMap<u64, String>,
    prebuffer: HashMap<u64, Vec<(u32, u16, Vec<u8>)>>,
    prebuffer_bytes: usize,
    session: Option<Session>,
    stats: ReceiveStats,
    start: Instant,
}

impl<'c> Engine<'c> {
    fn is_complete(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.all_delivered() || s.worker_gone)
    }

    fn handle(&mut self, datagram: &[u8], events: &mut dyn FnMut(Event<'_>)) -> Result<()> {
        self.stats.datagrams += 1;
        let Ok(pkt) = Packet::decode(datagram) else {
            self.stats.bad_packets += 1;
            return Ok(());
        };
        let sid = pkt.session_id();
        if self.rejected.contains_key(&sid) || self.session.as_ref().is_some_and(|s| s.manifest.session_id != sid) {
            self.stats.ignored_packets += 1;
            return Ok(());
        }
        match pkt {
            Packet::ManifestFrag {
                index,
                count,
                total_len,
                data,
                ..
            } => {
                if self.session.is_some() {
                    self.stats.manifest_repeats += 1;
                } else if let Some(container) = self.add_fragment(sid, index, count, total_len, data) {
                    self.try_accept(sid, &container, events)?;
                }
            }
            Packet::Shard { group, index, data, .. } => {
                if self.session.is_some() {
                    self.add_shard(group, index, data, events);
                } else {
                    self.prebuffer_shard(sid, group, index, data);
                }
            }
        }
        Ok(())
    }

    fn add_fragment(&mut self, sid: u64, index: u16, count: u16, total_len: u32, data: Vec<u8>) -> Option<Vec<u8>> {
        let total = total_len as usize;
        let frag = wire::MANIFEST_FRAG_SIZE;
        let valid = total > 0
            && total <= manifest::MAX_CONTAINER_LEN
            && usize::from(count) == total.div_ceil(frag)
            && index < count
            && data.len()
                == if index + 1 == count {
                    total - frag * (usize::from(count) - 1)
                } else {
                    frag
                };
        if !valid {
            self.stats.bad_packets += 1;
            return None;
        }
        if !self.assemblies.contains_key(&sid) && self.assemblies.len() >= MAX_ASSEMBLIES {
            self.assemblies.clear();
        }
        let asm = self.assemblies.entry(sid).or_insert_with(|| Assembly {
            count,
            total_len,
            frags: vec![None; usize::from(count)],
            have: 0,
        });
        if asm.count != count || asm.total_len != total_len {
            *asm = Assembly {
                count,
                total_len,
                frags: vec![None; usize::from(count)],
                have: 0,
            };
        }
        let slot = &mut asm.frags[usize::from(index)];
        if slot.is_none() {
            *slot = Some(data);
            asm.have += 1;
        }
        if asm.have < asm.count {
            return None;
        }
        let asm = self.assemblies.remove(&sid)?;
        Some(asm.frags.into_iter().flatten().flatten().collect())
    }

    fn reject(&mut self, sid: u64, reason: String, events: &mut dyn FnMut(Event<'_>)) {
        events(Event::SessionRejected {
            session_id: sid,
            reason: &reason,
        });
        self.prebuffer_bytes -= self
            .prebuffer
            .remove(&sid)
            .map_or(0, |v| v.iter().map(|(_, _, d)| d.len()).sum());
        self.rejected.insert(sid, reason);
    }

    fn try_accept(&mut self, sid: u64, container: &[u8], events: &mut dyn FnMut(Event<'_>)) -> Result<()> {
        // Unauthenticated failures only discard the reassembly: a forged fragment must not be able to
        // blacklist a genuine session.
        let body = match manifest::verify(container, &self.cfg.public_key) {
            Ok(body) => body,
            Err(_) => {
                self.stats.signature_failures += 1;
                events(Event::ManifestSignatureInvalid { session_id: sid });
                return Ok(());
            }
        };
        let m = match Manifest::decode(body) {
            Ok(m) => m,
            Err(e) => {
                self.reject(sid, format!("signed manifest is invalid: {e}"), events);
                return Ok(());
            }
        };
        let reason = if m.session_id != sid {
            Some("signed session id does not match the packets".to_string())
        } else if m.old_size != self.old_size || m.old_sha256 != self.old_sha256 {
            Some(format!(
                "update is for a different base image (sha256 {}…, local {}…)",
                &crate::to_hex(&m.old_sha256)[..16],
                &crate::to_hex(&self.old_sha256)[..16]
            ))
        } else if m.fw_version <= self.cfg.installed_version {
            Some(format!(
                "rollback refused: offered version {} <= installed version {}",
                m.fw_version, self.cfg.installed_version
            ))
        } else if m.new_size > self.cfg.max_image_size {
            Some(format!("new image of {} bytes exceeds the limit", m.new_size))
        } else if m.patch_size > self.cfg.max_image_size.saturating_mul(2).saturating_add(1 << 20) {
            Some(format!("patch of {} bytes exceeds the limit", m.patch_size))
        } else {
            None
        };
        if let Some(reason) = reason {
            self.reject(sid, reason, events);
            return Ok(());
        }
        self.start_session(m, events)
    }

    fn start_session(&mut self, m: Manifest, events: &mut dyn FnMut(Event<'_>)) -> Result<()> {
        let sid = m.session_id;
        let (tx, rx) = mpsc::channel();
        let worker = spawn_apply(self.old_path.clone(), self.tmp_path.clone(), m.compression, rx)?;
        let groups = (0..m.group_count()).map(|_| GroupState::default()).collect();
        self.stats.groups_total = m.group_count();
        self.stats.manifest_at = Some(self.start.elapsed());
        let session = Session {
            layout: m.layout(),
            codec: m.fec.codec(),
            manifest: m,
            groups,
            completed: 0,
            next_group: 0,
            ready: BTreeMap::new(),
            tx: Some(tx),
            worker: Some(worker),
            worker_gone: false,
        };
        events(Event::ManifestAccepted(&session.manifest));
        self.session = Some(session);
        self.assemblies.clear();
        let pre = self.prebuffer.remove(&sid).unwrap_or_default();
        self.prebuffer.clear();
        self.prebuffer_bytes = 0;
        for (g, i, d) in pre {
            self.add_shard(g, i, d, events);
        }
        self.release_ready();
        Ok(())
    }

    fn prebuffer_shard(&mut self, sid: u64, group: u32, index: u16, data: Vec<u8>) {
        if self.prebuffer_bytes + data.len() > self.cfg.max_prebuffer_bytes {
            self.stats.ignored_packets += 1;
            return;
        }
        self.prebuffer_bytes += data.len();
        self.prebuffer.entry(sid).or_default().push((group, index, data));
    }

    fn add_shard(&mut self, group: u32, index: u16, data: Vec<u8>, events: &mut dyn FnMut(Event<'_>)) {
        let elapsed = self.start.elapsed();
        let stats = &mut self.stats;
        let s = self.session.as_mut().expect("active session");
        let idx = usize::from(index);
        let (k, n, shard_size) = (s.manifest.fec.k(), s.manifest.fec.n(), s.manifest.fec.shard_size());
        if group >= s.group_count() || idx >= n || s.layout.is_implicit(group, idx) || data.len() != shard_size {
            stats.bad_packets += 1;
            return;
        }
        let layout = s.layout;
        let gs = &mut s.groups[group as usize];
        if gs.done {
            stats.redundant_shards += 1;
            return;
        }
        if gs.shards.is_empty() {
            gs.shards = vec![None; n];
            gs.present = 0;
            for i in 0..k {
                if layout.is_implicit(group, i) {
                    gs.shards[i] = Some(vec![0u8; shard_size]);
                    gs.present += 1;
                }
            }
        }
        if gs.shards[idx].is_some() {
            stats.redundant_shards += 1;
            return;
        }
        gs.shards[idx] = Some(data);
        gs.present += 1;
        stats.shards_accepted += 1;
        if gs.present < k {
            return;
        }

        let missing = (0..k).filter(|&i| gs.shards[i].is_none()).count();
        if s.codec.reconstruct(&mut gs.shards).is_err() {
            // Cannot happen with k equal-sized shards; treat as a bad group defensively.
            gs.shards.clear();
            stats.group_hash_failures += 1;
            return;
        }
        let bytes = slicer::assemble_group(&gs.shards, &layout, group);
        if crate::sha256(&bytes) != s.manifest.group_hashes[group as usize] {
            // A shard was forged or corrupted undetected: discard and collect fresh shards next round.
            gs.shards.clear();
            gs.present = 0;
            stats.group_hash_failures += 1;
            events(Event::GroupHashMismatch { group });
            return;
        }
        gs.done = true;
        gs.shards = Vec::new();
        s.completed += 1;
        stats.shards_recovered += missing as u64;
        if missing > 0 {
            stats.groups_repaired += 1;
        }
        if s.completed == s.group_count() {
            stats.last_group_at = Some(elapsed);
        }
        s.ready.insert(group, bytes);
        events(Event::GroupComplete {
            group,
            completed: s.completed,
            total: s.group_count(),
            recovered_shards: missing,
        });
        self.release_ready();
    }

    /// Hand completed groups to the patcher strictly in order.
    fn release_ready(&mut self) {
        let elapsed = self.start.elapsed();
        let Some(s) = self.session.as_mut() else { return };
        while let Some(bytes) = s.ready.remove(&s.next_group) {
            self.stats.first_group_at.get_or_insert(elapsed);
            if let Some(tx) = &s.tx
                && tx.send(bytes).is_err()
            {
                // The apply thread stopped early; its error is reported by `finish`.
                s.tx = None;
                s.worker_gone = true;
            }
            s.next_group += 1;
        }
        self.stats.max_pending_groups = self.stats.max_pending_groups.max(s.ready.len());
        if s.all_delivered() {
            s.tx = None; // EOF for the patcher
        }
    }

    fn progress_message(&self) -> String {
        match &self.session {
            Some(s) => {
                let missing: Vec<String> = (0..s.group_count())
                    .filter(|&g| !s.groups[g as usize].done)
                    .take(10)
                    .map(|g| g.to_string())
                    .collect();
                format!(
                    "{}/{} groups complete; missing groups {}{}",
                    s.completed,
                    s.group_count(),
                    missing.join(","),
                    if (s.group_count() - s.completed) as usize > missing.len() {
                        ",…"
                    } else {
                        ""
                    }
                )
            }
            None if !self.rejected.is_empty() => {
                let reasons: Vec<String> = self
                    .rejected
                    .iter()
                    .map(|(sid, r)| format!("session {sid:016x}: {r}"))
                    .collect();
                format!("no acceptable update; rejected: {}", reasons.join("; "))
            }
            None => format!(
                "no valid manifest received ({} datagrams, {} signature failures)",
                self.stats.datagrams, self.stats.signature_failures
            ),
        }
    }

    fn finish(&mut self, out_path: &Path) -> Result<Report> {
        let mut s = self.session.take().ok_or_else(|| anyhow!("no active session"))?;
        s.tx = None;
        let worker = s.worker.take().expect("worker present");
        let outcome = worker.join().map_err(|_| anyhow!("apply thread panicked"))??;
        let m = s.manifest.clone();
        if outcome.patch_len != m.patch_size || outcome.patch_sha256 != m.patch_sha256 {
            bail!("reassembled patch does not match the signed manifest");
        }
        if outcome.new_len != m.new_size || outcome.new_sha256 != m.new_sha256 {
            bail!(
                "new image verification failed: got {} bytes sha256 {}, signed manifest says {} bytes sha256 {} — not installing",
                outcome.new_len,
                crate::to_hex(&outcome.new_sha256),
                m.new_size,
                crate::to_hex(&m.new_sha256)
            );
        }
        fs::rename(&self.tmp_path, out_path)
            .with_context(|| format!("installing {} -> {}", self.tmp_path.display(), out_path.display()))?;
        if let Some(dir) = out_path.parent().filter(|d| !d.as_os_str().is_empty())
            && let Ok(d) = File::open(dir)
        {
            let _ = d.sync_all();
        }
        Ok(Report {
            manifest: m,
            stats: self.stats.clone(),
            apply: outcome.stats,
            new_sha256: outcome.new_sha256,
            output: out_path.to_path_buf(),
            elapsed: self.start.elapsed(),
        })
    }
}

/// Receive one update from `source`, reconstruct it from `old_path` and install it at `out_path`.
pub fn receive<S: DatagramSource + ?Sized>(
    source: &mut S,
    old_path: &Path,
    out_path: &Path,
    cfg: &ReceiverConfig,
    events: &mut dyn FnMut(Event<'_>),
) -> Result<Report> {
    let start = Instant::now();
    let old_file = File::open(old_path).with_context(|| format!("opening base image {}", old_path.display()))?;
    let (old_sha256, old_size) = hash_reader(BufReader::new(old_file))?;
    let tmp_path = temp_path(out_path);
    // Declared before the engine so it is dropped after it (the engine joins the apply thread).
    let mut guard = TempGuard {
        path: tmp_path.clone(),
        armed: true,
    };
    let mut engine = Engine {
        cfg,
        old_path: old_path.to_path_buf(),
        tmp_path,
        old_sha256,
        old_size,
        assemblies: HashMap::new(),
        rejected: HashMap::new(),
        prebuffer: HashMap::new(),
        prebuffer_bytes: 0,
        session: None,
        stats: ReceiveStats::default(),
        start,
    };
    if let Some(imp) = &cfg.impairment {
        imp.validate().map_err(anyhow::Error::msg)?;
    }
    let mut impairment = cfg.impairment.filter(|c| !c.is_noop()).map(Impairment::new);
    let mut deliver = Vec::new();

    while !engine.is_complete() {
        if let Some(t) = cfg.timeout
            && start.elapsed() > t
        {
            bail!("timed out after {:.1?}: {}", t, engine.progress_message());
        }
        let closed = match source.recv()? {
            Recv::Datagram(d) => {
                match &mut impairment {
                    Some(imp) => imp.process(d, &mut deliver),
                    None => deliver.push(d),
                }
                false
            }
            Recv::Timeout => false,
            Recv::Closed => {
                if let Some(imp) = &mut impairment {
                    imp.flush(&mut deliver);
                }
                true
            }
        };
        for d in deliver.drain(..) {
            engine.handle(&d, events)?;
            if engine.is_complete() {
                break;
            }
        }
        if closed && !engine.is_complete() {
            bail!(
                "stream ended before the update completed: {}",
                engine.progress_message()
            );
        }
    }
    if let Some(imp) = &impairment {
        engine.stats.impairment = imp.stats;
    }
    let report = engine.finish(out_path)?;
    guard.armed = false;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_reader_concatenates_chunks() {
        let (tx, rx) = mpsc::channel();
        for c in [
            b"hello ".to_vec(),
            Vec::new(),
            b"streaming ".to_vec(),
            b"world".to_vec(),
        ] {
            tx.send(c).unwrap();
        }
        drop(tx);
        let mut s = String::new();
        ChannelReader::new(rx).read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello streaming world");
    }

    #[test]
    fn temp_path_is_sibling() {
        assert_eq!(temp_path(Path::new("/a/b/new.img")), PathBuf::from("/a/b/new.img.part"));
        assert_eq!(temp_path(Path::new("new.img")), PathBuf::from("new.img.part"));
    }
}
