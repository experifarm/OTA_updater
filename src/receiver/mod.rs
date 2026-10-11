//! Receiver pipeline (监听分片 → 纠错编码 → 流式差分还原 → Ed25519签名校验).
//!
//! The protocol logic lives in the sans-IO [`core::ReceiverCore`]; this module is the driver that
//! connects it to a datagram source, the impairment simulator, a patch-apply thread and the file
//! system:
//!
//! ```text
//! DatagramSource ─► Impairment ─► ReceiverCore ─► GroupReleased ─► mpsc ─► [apply thread]
//! [apply thread]: ChannelReader ─► zstd ─► patch::apply(old image) ─► HashingWriter ─► <out>.part
//! finish: size + SHA-256 == signed manifest ─► fsync ─► atomic rename <out>.part → <out>
//! ```
//!
//! Every byte handed to the patcher is authenticated (it matches a signed per-group hash), and the new
//! image is only installed once its full SHA-256 matches the signed manifest. The driver needs threads
//! and a clock (any `std` host); the browser simulation drives `ReceiverCore` directly instead.

pub mod core;

use self::core::{CoreConfig, ReceiverCore, RxEvent};
use crate::channel::{Impairment, ImpairmentConfig, ImpairmentStats};
use crate::hashio::{HashingReader, HashingWriter, hash_reader};
use crate::manifest::Manifest;
use crate::patch::{self, ApplyStats, Compression};
use anyhow::{Context, Result, anyhow, bail};
use ed25519_dalek::VerifyingKey;
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

    pub fn core(&self) -> CoreConfig {
        CoreConfig {
            public_key: self.public_key,
            installed_version: self.installed_version,
            max_image_size: self.max_image_size,
            max_prebuffer_bytes: self.max_prebuffer_bytes,
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

/// The patch-apply thread of an accepted session.
struct ApplyWorker {
    tx: Option<mpsc::Sender<Vec<u8>>>,
    handle: Option<JoinHandle<Result<ApplyOutcome>>>,
    gone: bool,
}

impl ApplyWorker {
    fn send(&mut self, bytes: Vec<u8>) {
        if let Some(tx) = &self.tx
            && tx.send(bytes).is_err()
        {
            // The apply thread stopped early; its error is reported by `finish`.
            self.tx = None;
            self.gone = true;
        }
    }

    fn join(&mut self) -> Result<ApplyOutcome> {
        self.tx = None;
        let handle = self
            .handle
            .take()
            .ok_or_else(|| anyhow!("apply thread already joined"))?;
        handle.join().map_err(|_| anyhow!("apply thread panicked"))?
    }
}

impl Drop for ApplyWorker {
    fn drop(&mut self) {
        // Close the stream so the apply thread terminates, then wait for it (it owns the temp file).
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Driver<'a> {
    core: ReceiverCore,
    old_path: PathBuf,
    tmp_path: PathBuf,
    start: Instant,
    stats: ReceiveStats,
    worker: Option<ApplyWorker>,
    events: &'a mut dyn FnMut(Event<'_>),
    scratch: Vec<RxEvent>,
}

impl Driver<'_> {
    fn done(&self) -> bool {
        self.core.is_complete() || self.worker.as_ref().is_some_and(|w| w.gone)
    }

    fn handle(&mut self, datagram: &[u8]) -> Result<()> {
        let mut evs = std::mem::take(&mut self.scratch);
        self.core.handle(datagram, &mut evs);
        for ev in evs.drain(..) {
            self.dispatch(ev)?;
        }
        self.scratch = evs;
        Ok(())
    }

    fn dispatch(&mut self, ev: RxEvent) -> Result<()> {
        let now = self.start.elapsed();
        match ev {
            RxEvent::ManifestAccepted { .. } => {
                let m = self.core.manifest().expect("accepted manifest");
                let (tx, rx) = mpsc::channel();
                let handle = spawn_apply(self.old_path.clone(), self.tmp_path.clone(), m.compression, rx)?;
                self.worker = Some(ApplyWorker {
                    tx: Some(tx),
                    handle: Some(handle),
                    gone: false,
                });
                self.stats.manifest_at = Some(now);
                (self.events)(Event::ManifestAccepted(m));
            }
            RxEvent::ManifestSignatureInvalid { session_id } => {
                (self.events)(Event::ManifestSignatureInvalid { session_id });
            }
            RxEvent::SessionRejected { session_id, reason } => {
                (self.events)(Event::SessionRejected {
                    session_id,
                    reason: &reason,
                });
            }
            RxEvent::GroupDecoded {
                group,
                missing,
                hash_ok,
                ..
            } => {
                if hash_ok {
                    let total = self.core.manifest().map_or(0, Manifest::group_count);
                    let completed = self.core.completed_groups();
                    if completed == total {
                        self.stats.last_group_at = Some(now);
                    }
                    (self.events)(Event::GroupComplete {
                        group,
                        completed,
                        total,
                        recovered_shards: missing.len(),
                    });
                } else {
                    (self.events)(Event::GroupHashMismatch { group });
                }
            }
            RxEvent::GroupReleased { bytes, .. } => {
                self.stats.first_group_at.get_or_insert(now);
                if let Some(w) = &mut self.worker {
                    w.send(bytes);
                }
            }
            RxEvent::Complete => {
                if let Some(w) = &mut self.worker {
                    w.tx = None; // EOF for the patcher
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self, out_path: &Path) -> Result<Report> {
        let m = self
            .core
            .manifest()
            .ok_or_else(|| anyhow!("no accepted manifest"))?
            .clone();
        let outcome = self.worker.as_mut().ok_or_else(|| anyhow!("no apply worker"))?.join()?;
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
        let mut stats = self.core.stats().clone();
        stats.manifest_at = self.stats.manifest_at;
        stats.first_group_at = self.stats.first_group_at;
        stats.last_group_at = self.stats.last_group_at;
        stats.impairment = self.stats.impairment;
        Ok(Report {
            manifest: m,
            stats,
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
    if let Some(imp) = &cfg.impairment {
        imp.validate().map_err(anyhow::Error::msg)?;
    }
    let old_file = File::open(old_path).with_context(|| format!("opening base image {}", old_path.display()))?;
    let (old_sha256, old_size) = hash_reader(BufReader::new(old_file))?;
    let tmp_path = temp_path(out_path);
    // Declared before the driver so it is dropped after it (the driver joins the apply thread).
    let mut guard = TempGuard {
        path: tmp_path.clone(),
        armed: true,
    };
    let mut driver = Driver {
        core: ReceiverCore::new(cfg.core(), old_sha256, old_size),
        old_path: old_path.to_path_buf(),
        tmp_path,
        start,
        stats: ReceiveStats::default(),
        worker: None,
        events,
        scratch: Vec::new(),
    };
    let mut impairment = cfg.impairment.filter(|c| !c.is_noop()).map(Impairment::new);
    let mut deliver = Vec::new();

    while !driver.done() {
        if let Some(t) = cfg.timeout
            && start.elapsed() > t
        {
            bail!("timed out after {:.1?}: {}", t, driver.core.progress_message());
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
            driver.handle(&d)?;
            if driver.done() {
                break;
            }
        }
        if closed && !driver.done() {
            bail!(
                "stream ended before the update completed: {}",
                driver.core.progress_message()
            );
        }
    }
    if let Some(imp) = &impairment {
        driver.stats.impairment = imp.stats;
    }
    let report = driver.finish(out_path)?;
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
