//! Deterministic OTA scenario: real sender + per-receiver impairment + real `ReceiverCore` + real
//! streaming patcher, recorded as a complete event log on a simulated clock.

use crate::toy;
use ed25519_dalek::SigningKey;
use ota_updater::channel::{Fate, Impairment, ImpairmentConfig, ImpairmentStats};
use ota_updater::diff::DiffRecord;
use ota_updater::manifest::Manifest;
use ota_updater::patch::{Compression, PatchError, PatchOp, PushDecompressor, StreamingPatcher};
use ota_updater::receiver::core::{BadPacketReason, CoreConfig, ReceiverCore, RxEvent};
use ota_updater::sender::{self, Prepared, Schedule, SenderConfig};
use ota_updater::slicer::FecParams;
use ota_updater::wire::{self, Packet};
use ota_updater::{sha256, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;

// ---------------------------------------------------------------- configuration

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    /// A few hundred bytes of readable text firmware.
    #[default]
    Toy,
    /// Synthetic binary firmware (`imageSize` bytes).
    Real,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScenarioConfig {
    pub scale: Scale,
    /// Image size for the real scale.
    pub image_size: usize,
    pub seed: u64,
    pub shard_size: u16,
    pub k: u16,
    pub m: u16,
    pub interleave: usize,
    pub manifest_every: usize,
    pub max_rounds: u32,
    /// Packets per simulated second.
    pub rate_pps: f64,
    /// Flight time from the antenna to a receiver.
    pub latency_ms: f64,
    pub round_gap_ms: f64,
    pub compress: bool,
    pub fw_version: u64,
    /// Inject forged traffic (fake update + forged shards + forged manifest fragment).
    pub attacker: bool,
    pub receivers: Vec<ReceiverSpec>,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            scale: Scale::Toy,
            image_size: 1 << 20,
            seed: 1,
            shard_size: 16,
            k: 4,
            m: 2,
            interleave: 2,
            manifest_every: 24,
            max_rounds: 6,
            rate_pps: 12.0,
            latency_ms: 900.0,
            round_gap_ms: 600.0,
            compress: false,
            fw_version: 2,
            attacker: false,
            receivers: vec![ReceiverSpec::default()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReceiverSpec {
    pub name: String,
    pub loss: f64,
    pub burst_len: f64,
    pub corrupt: f64,
    pub dup: f64,
    pub reorder: f64,
    pub seed: u64,
    /// The receiver starts listening at this time.
    pub join_ms: f64,
    pub installed_version: u64,
    /// The receiver runs a different base image than the update was built for.
    pub wrong_base: bool,
    /// The receiver pins a different publisher key.
    pub wrong_key: bool,
}

impl Default for ReceiverSpec {
    fn default() -> Self {
        Self {
            name: "A".into(),
            loss: 0.0,
            burst_len: 1.0,
            corrupt: 0.0,
            dup: 0.0,
            reorder: 0.0,
            seed: 1,
            join_ms: 0.0,
            installed_version: 1,
            wrong_base: false,
            wrong_key: false,
        }
    }
}

// ---------------------------------------------------------------- event log

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioLog {
    pub summary: Summary,
    pub manifest: ManifestCard,
    /// Toy scale only: the images as text, plus a class per new byte (0 same, 1 changed, 2 extra).
    pub toy: Option<ToyImages>,
    pub heat: Heat,
    /// `[new_off, old_off, diff_len, extra_len]` per diff record.
    pub records: Vec<[u64; 4]>,
    pub groups: Vec<GroupInfo>,
    pub packets: Vec<PacketRec>,
    /// Start time of every broadcast round.
    pub rounds: Vec<f64>,
    pub end_ms: f64,
    pub receivers: Vec<RxLog>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub scale: Scale,
    pub old_size: usize,
    pub new_size: usize,
    pub raw_patch: u64,
    pub patch_size: u64,
    pub compressed: bool,
    pub records: u64,
    pub diff_bytes: u64,
    pub extra_bytes: u64,
    pub groups: u32,
    pub shard_size: u16,
    pub k: u16,
    pub m: u16,
    pub packets_per_round: usize,
    pub interval_ms: f64,
    pub latency_ms: f64,
    pub manifest_len: usize,
    pub manifest_frags: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestCard {
    pub session: String,
    pub fw_version: u64,
    pub old_sha: String,
    pub new_sha: String,
    pub patch_sha: String,
    pub public_key: String,
    pub signature: String,
    /// First 8 hex digits of each group's SHA-256.
    pub group_hashes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ToyImages {
    pub old: String,
    pub new: String,
    pub class: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct Heat {
    pub buckets: usize,
    /// Fraction of bytes per bucket of the new image that differ from their old source.
    pub changed: Vec<f32>,
    /// Fraction of bytes per bucket that are literal (extra) bytes.
    pub extra: Vec<f32>,
}

#[derive(Debug, Serialize)]
pub struct GroupInfo {
    pub len: usize,
    /// Data shards that carry real bytes (the rest are implicit zero padding).
    pub real: usize,
}

/// `kind`: 0 manifest fragment, 1 shard, 2 attacker manifest, 3 attacker shard.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PacketRec {
    pub t: f64,
    pub kind: u8,
    pub g: u32,
    pub i: u16,
    pub r: u32,
    pub src: u8,
}

#[derive(Debug, Serialize)]
pub struct Delivery {
    /// Packet index.
    pub p: u32,
    pub t: f64,
    /// ok | lost | corrupt | dup | late | off (not listening yet)
    pub f: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ev {
    CrcBad {
        t: f64,
        p: u32,
    },
    Bad {
        t: f64,
        p: u32,
        reason: &'static str,
    },
    Ignored {
        t: f64,
        p: u32,
    },
    ManifestFrag {
        t: f64,
        p: u32,
        index: u16,
        count: u16,
    },
    ManifestBadSig {
        t: f64,
        p: u32,
    },
    ManifestOk {
        t: f64,
        p: u32,
    },
    Rejected {
        t: f64,
        p: u32,
        reason: String,
    },
    Buffered {
        t: f64,
        p: u32,
        g: u32,
        i: u16,
    },
    Stored {
        t: f64,
        p: u32,
        g: u32,
        i: u16,
    },
    Redundant {
        t: f64,
        p: u32,
        g: u32,
        i: u16,
    },
    Decoded {
        t: f64,
        g: u32,
        used: Vec<u16>,
        missing: Vec<u16>,
        ok: bool,
    },
    Released {
        t: f64,
        g: u32,
        len: usize,
    },
    Write {
        t: f64,
        off: u64,
        len: u64,
        kind: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        old: Option<u64>,
    },
    Verified {
        t: f64,
        ok: bool,
        sha: String,
    },
    Installed {
        t: f64,
    },
    Failed {
        t: f64,
        reason: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RxLog {
    pub spec: ReceiverSpec,
    pub deliveries: Vec<Delivery>,
    pub events: Vec<Ev>,
    /// For every event, the index of the delivery whose processing produced it (`u32::MAX` if none).
    pub event_delivery: Vec<u32>,
    pub result: RxResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RxResult {
    /// installed | rejected | failed
    pub status: &'static str,
    pub reason: Option<String>,
    pub t: f64,
    pub stats: RxStats,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RxStats {
    pub seen: u64,
    pub dropped: u64,
    pub corrupted: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub bad_packets: u64,
    pub redundant_shards: u64,
    pub shards_recovered: u64,
    pub groups_repaired: u32,
    pub hash_failures: u32,
    pub signature_failures: u32,
    pub max_pending_groups: usize,
    pub written: u64,
}

// ---------------------------------------------------------------- simulation

/// Deterministic demo keys (clearly not secret): the publisher and attacker keys of the simulation.
pub fn demo_key(label: &str, seed: u64) -> SigningKey {
    let mut material = label.as_bytes().to_vec();
    material.extend(seed.to_le_bytes());
    SigningKey::from_bytes(&sha256(&material))
}

/// Forged traffic: a fake update signed with the attacker's key, garbage shards for the genuine
/// session (valid CRCs), and a forged manifest fragment for the genuine session.
fn attacker_packets(p: &Prepared, seed: u64) -> Vec<(u8, Vec<u8>)> {
    let attacker = demo_key("ota-viz attacker key", seed);
    let fec = p.manifest.fec;
    let evil = Manifest {
        session_id: 0x0bad_0bad,
        fw_version: 99,
        new_sha256: sha256(b"evil"),
        patch_sha256: sha256(b"evil patch"),
        ..p.manifest.clone()
    };
    let mut out = Vec::new();
    for frag in wire::manifest_fragments(evil.session_id, &evil.sign(&attacker)) {
        out.push((2, frag.encode()));
    }
    let layout = p.layout();
    // Forged shard 0 is always among the k shards used for decoding, so the group must fail its hash.
    let forged = layout.real_data_shards(0).min(2);
    for idx in 0..forged as u16 {
        out.push((
            3,
            Packet::Shard {
                session_id: p.manifest.session_id,
                group: 0,
                index: idx,
                data: vec![0xA5; fec.shard_size()],
            }
            .encode(),
        ));
    }
    let len = p.container.len();
    let first = len.min(wire::MANIFEST_FRAG_SIZE);
    let mut fake = vec![0u8; first];
    fake[..4].copy_from_slice(&(len as u32 - 4 - 64).to_le_bytes());
    out.push((
        2,
        Packet::ManifestFrag {
            session_id: p.manifest.session_id,
            index: 0,
            count: len.div_ceil(wire::MANIFEST_FRAG_SIZE) as u16,
            total_len: len as u32,
            data: fake,
        }
        .encode(),
    ));
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Listening,
    Installed,
    Rejected,
    Failed,
}

struct RxSim {
    spec: ReceiverSpec,
    core: ReceiverCore,
    imp: Impairment,
    old: Vec<u8>,
    expected_sha: [u8; 32],
    dec: Option<PushDecompressor>,
    patcher: Option<StreamingPatcher>,
    new_img: Vec<u8>,
    deliveries: Vec<Delivery>,
    events: Vec<Ev>,
    event_delivery: Vec<u32>,
    /// Delivery being processed (`u32::MAX` outside packet handling).
    cur_delivery: u32,
    /// Fates of packets with copies still in flight (duplicated or held back).
    pending: HashMap<u64, (Fate, u8)>,
    status: Status,
    reason: Option<String>,
    done_at: f64,
    give_up_at: f64,
    latency: f64,
    rx_events: Vec<RxEvent>,
    out: Vec<(u64, Vec<u8>)>,
}

impl RxSim {
    fn done(&self) -> bool {
        self.status != Status::Listening
    }

    fn emit(&mut self, ev: Ev) {
        self.events.push(ev);
        self.event_delivery.push(self.cur_delivery);
    }

    fn finish_with(&mut self, status: Status, t: f64, reason: Option<String>) {
        if let Some(r) = &reason
            && status == Status::Failed
        {
            self.emit(Ev::Failed { t, reason: r.clone() });
        }
        self.status = status;
        self.reason = reason;
        self.done_at = t;
    }

    fn on_packet(&mut self, p: u32, t_send: f64, datagram: &[u8]) {
        if self.done() {
            return;
        }
        if t_send < self.spec.join_ms {
            self.deliveries.push(Delivery {
                p,
                t: t_send + self.latency,
                f: "off",
            });
            return;
        }
        if self.core.manifest().is_none() && t_send > self.give_up_at {
            let reason = format!("timed out: {}", self.core.progress_message());
            self.finish_with(Status::Failed, t_send, Some(reason));
            return;
        }
        let fate = self.imp.process_tagged(u64::from(p), datagram.to_vec(), &mut self.out);
        if fate.lost {
            self.deliveries.push(Delivery {
                p,
                t: t_send + self.latency * 0.55,
                f: "lost",
            });
        } else if fate.delayed > 0 || fate.duplicated {
            let copies = if fate.duplicated { 2 } else { 1 };
            self.pending.insert(u64::from(p), (fate, copies));
        }
        let arrive = t_send + self.latency;
        let mut out = std::mem::take(&mut self.out);
        for (tag, data) in out.drain(..) {
            let (f, copy) = match self.pending.get_mut(&tag) {
                Some((f, left)) => {
                    let f = *f;
                    let copy = if f.duplicated { 2 - *left } else { 0 };
                    *left -= 1;
                    if *left == 0 {
                        self.pending.remove(&tag);
                    }
                    (f, copy)
                }
                None => (fate, 0),
            };
            let kind = if f.corrupted_bit.is_some() {
                "corrupt"
            } else if tag != u64::from(p) {
                "late"
            } else if copy == 1 {
                "dup"
            } else {
                "ok"
            };
            self.deliveries.push(Delivery {
                p: tag as u32,
                t: arrive,
                f: kind,
            });
            if !self.done() {
                self.handle(tag as u32, arrive, &data);
            }
        }
        self.out = out;
    }

    fn handle(&mut self, p: u32, t: f64, datagram: &[u8]) {
        // The delivery of this datagram was pushed just before.
        self.cur_delivery = self.deliveries.len() as u32 - 1;
        let mut evs = std::mem::take(&mut self.rx_events);
        self.core.handle(datagram, &mut evs);
        for ev in evs.drain(..) {
            if self.done() {
                break;
            }
            self.dispatch(p, t, ev);
        }
        self.rx_events = evs;
        self.cur_delivery = u32::MAX;
    }

    fn dispatch(&mut self, p: u32, t: f64, ev: RxEvent) {
        match ev {
            RxEvent::BadPacket { reason } => self.emit(match reason {
                BadPacketReason::Crc => Ev::CrcBad { t, p },
                BadPacketReason::Format => Ev::Bad { t, p, reason: "format" },
                BadPacketReason::Fragment => Ev::Bad {
                    t,
                    p,
                    reason: "fragment",
                },
                BadPacketReason::Shard => Ev::Bad { t, p, reason: "shard" },
            }),
            RxEvent::Ignored { .. } => self.emit(Ev::Ignored { t, p }),
            RxEvent::ManifestFragment { index, count, .. } => self.emit(Ev::ManifestFrag { t, p, index, count }),
            RxEvent::ManifestRepeat => {}
            RxEvent::ManifestSignatureInvalid { .. } => self.emit(Ev::ManifestBadSig { t, p }),
            RxEvent::ManifestAccepted { .. } => {
                let m = self.core.manifest().expect("accepted");
                self.dec = Some(PushDecompressor::new(m.compression));
                self.patcher = Some(StreamingPatcher::new(self.old.len() as u64));
                self.emit(Ev::ManifestOk { t, p });
            }
            RxEvent::SessionRejected { reason, .. } => {
                self.emit(Ev::Rejected {
                    t,
                    p,
                    reason: reason.clone(),
                });
                self.finish_with(Status::Rejected, t, Some(reason));
            }
            RxEvent::ShardBuffered { group, index } => self.emit(Ev::Buffered {
                t,
                p,
                g: group,
                i: index,
            }),
            RxEvent::ShardStored { group, index } => self.emit(Ev::Stored {
                t,
                p,
                g: group,
                i: index,
            }),
            RxEvent::ShardRedundant { group, index } => self.emit(Ev::Redundant {
                t,
                p,
                g: group,
                i: index,
            }),
            RxEvent::GroupDecoded {
                group,
                used,
                missing,
                hash_ok,
            } => self.emit(Ev::Decoded {
                t,
                g: group,
                used,
                missing,
                ok: hash_ok,
            }),
            RxEvent::GroupReleased { group, bytes } => {
                self.emit(Ev::Released {
                    t,
                    g: group,
                    len: bytes.len(),
                });
                if let Err(e) = self.feed(t, &bytes, false) {
                    self.finish_with(Status::Failed, t, Some(format!("patch failed: {e}")));
                }
            }
            RxEvent::Complete => {
                if let Err(e) = self.feed(t, &[], true) {
                    self.finish_with(Status::Failed, t, Some(format!("patch failed: {e}")));
                    return;
                }
                self.verify(t);
            }
        }
    }

    /// Push patch bytes through decompression into the streaming patcher, logging the writes.
    fn feed(&mut self, t: f64, bytes: &[u8], last: bool) -> Result<(), PatchError> {
        let (Some(dec), Some(patcher)) = (self.dec.as_mut(), self.patcher.as_mut()) else {
            return Ok(());
        };
        let mut old = Cursor::new(self.old.as_slice());
        let new_img = &mut self.new_img;
        let mut ops: Vec<PatchOp> = Vec::new();
        let mut sink = |chunk: &[u8]| patcher.feed(chunk, &mut old, &mut *new_img, &mut |op| ops.push(op));
        if last {
            dec.finish(&mut sink)?;
        } else {
            dec.feed(bytes, &mut sink)?;
        }
        // Merge contiguous pieces of the same kind.
        let mut merged: Vec<PatchOp> = Vec::new();
        for op in ops {
            if let Some(last) = merged.last_mut()
                && merge_into(last, op)
            {
                continue;
            }
            merged.push(op);
        }
        for op in merged {
            self.emit(match op {
                PatchOp::Diff { new_off, old_off, len } => Ev::Write {
                    t,
                    off: new_off,
                    len,
                    kind: "diff",
                    old: Some(old_off),
                },
                PatchOp::Extra { new_off, len } => Ev::Write {
                    t,
                    off: new_off,
                    len,
                    kind: "extra",
                    old: None,
                },
            });
        }
        Ok(())
    }

    fn verify(&mut self, t: f64) {
        let complete = self.patcher.as_ref().and_then(|p| p.finish().ok()).is_some();
        let sha = sha256(&self.new_img);
        let ok = complete && sha == self.expected_sha;
        self.emit(Ev::Verified {
            t,
            ok,
            sha: to_hex(&sha),
        });
        if ok {
            self.emit(Ev::Installed { t });
            self.finish_with(Status::Installed, t, None);
        } else {
            self.finish_with(
                Status::Failed,
                t,
                Some("new image does not match the signed hash — not installed".into()),
            );
        }
    }

    fn into_log(self) -> RxLog {
        let s = self.core.stats();
        let imp: ImpairmentStats = self.imp.stats;
        let stats = RxStats {
            seen: imp.seen,
            dropped: imp.dropped,
            corrupted: imp.corrupted,
            duplicated: imp.duplicated,
            reordered: imp.reordered,
            bad_packets: s.bad_packets,
            redundant_shards: s.redundant_shards,
            shards_recovered: s.shards_recovered,
            groups_repaired: s.groups_repaired,
            hash_failures: s.group_hash_failures,
            signature_failures: s.signature_failures,
            max_pending_groups: s.max_pending_groups,
            written: self.new_img.len() as u64,
        };
        RxLog {
            spec: self.spec,
            deliveries: self.deliveries,
            events: self.events,
            event_delivery: self.event_delivery,
            result: RxResult {
                status: match self.status {
                    Status::Installed => "installed",
                    Status::Rejected => "rejected",
                    Status::Failed | Status::Listening => "failed",
                },
                reason: self.reason,
                t: self.done_at,
                stats,
            },
        }
    }
}

fn merge_into(last: &mut PatchOp, op: PatchOp) -> bool {
    match (last, op) {
        (
            PatchOp::Diff { new_off, old_off, len },
            PatchOp::Diff {
                new_off: n2,
                old_off: o2,
                len: l2,
            },
        ) if *new_off + *len == n2 && *old_off + *len == o2 => {
            *len += l2;
            true
        }
        (PatchOp::Extra { new_off, len }, PatchOp::Extra { new_off: n2, len: l2 }) if *new_off + *len == n2 => {
            *len += l2;
            true
        }
        _ => false,
    }
}

fn heat(old: &[u8], new: &[u8], records: &[DiffRecord], buckets: usize) -> (Heat, Vec<u8>) {
    let buckets = buckets.min(new.len()).max(1);
    let bucket_of = |i: u64| (i as usize * buckets / new.len().max(1)).min(buckets - 1);
    let mut changed = vec![0u32; buckets];
    let mut extra = vec![0u32; buckets];
    let mut class = vec![0u8; new.len()];
    for r in records {
        for j in 0..r.diff_len {
            let ni = r.new_off + j;
            if new[ni as usize] != old[(r.old_off + j) as usize] {
                changed[bucket_of(ni)] += 1;
                class[ni as usize] = 1;
            }
        }
        for j in 0..r.extra_len {
            let ni = r.new_off + r.diff_len + j;
            extra[bucket_of(ni)] += 1;
            class[ni as usize] = 2;
        }
    }
    let mut size = vec![0u32; buckets];
    for i in 0..new.len() {
        size[bucket_of(i as u64)] += 1;
    }
    let frac = |v: Vec<u32>| -> Vec<f32> {
        v.iter()
            .zip(&size)
            .map(|(&a, &s)| if s == 0 { 0.0 } else { a as f32 / s as f32 })
            .collect()
    };
    (
        Heat {
            buckets,
            changed: frac(changed),
            extra: frac(extra),
        },
        class,
    )
}

/// Run a whole scenario.
pub fn run(cfg: &ScenarioConfig) -> Result<ScenarioLog, String> {
    if cfg.receivers.is_empty() || cfg.receivers.len() > 8 {
        return Err("between 1 and 8 receivers are supported".into());
    }
    // `is_finite` also rejects NaN.
    if !cfg.rate_pps.is_finite() || cfg.rate_pps <= 0.0 || cfg.max_rounds == 0 {
        return Err("rate and max rounds must be positive".into());
    }
    let (old, new) = match cfg.scale {
        Scale::Toy => toy::images(),
        Scale::Real => ota_updater::imagegen::generate_pair(cfg.image_size.clamp(16 << 10, 8 << 20), cfg.seed),
    };
    let key = demo_key("ota-viz publisher key", cfg.seed);
    let fec = FecParams {
        shard_size: cfg.shard_size,
        k: cfg.k,
        m: cfg.m,
    };
    let prepared = sender::prepare(
        &old,
        &new,
        &key,
        &SenderConfig {
            fw_version: cfg.fw_version,
            fec,
            compression: if cfg.compress {
                Compression::Zstd
            } else {
                Compression::None
            },
            level: 1,
            session_id: Some(0x5e55_0000 + cfg.seed),
            // Independent 16 KiB frames: the pure-Rust decoder can then stream group by group.
            zstd_frame_size: Some(16 << 10),
        },
    )
    .map_err(|e| e.to_string())?;
    let m = &prepared.manifest;
    let layout = prepared.layout();

    // Broadcast schedule.
    let schedule = Schedule {
        interleave: cfg.interleave,
        manifest_every: cfg.manifest_every,
    };
    let round = prepared.round_packets(&schedule);
    let interval = 1000.0 / cfg.rate_pps;
    let round_ms = round.len() as f64 * interval;
    let attack = if cfg.attacker {
        attacker_packets(&prepared, cfg.seed)
    } else {
        Vec::new()
    };

    let mut rxs: Vec<RxSim> = cfg
        .receivers
        .iter()
        .map(|spec| {
            let mut base = old.clone();
            if spec.wrong_base {
                base[old.len() / 2] ^= 0x01;
            }
            let mut core_cfg = CoreConfig::new(if spec.wrong_key {
                demo_key("ota-viz other key", cfg.seed).verifying_key()
            } else {
                key.verifying_key()
            });
            core_cfg.installed_version = spec.installed_version;
            RxSim {
                core: ReceiverCore::new(core_cfg, sha256(&base), base.len() as u64),
                imp: Impairment::new(ImpairmentConfig {
                    loss: spec.loss.clamp(0.0, 0.95),
                    burst_len: spec.burst_len.max(1.0),
                    corrupt: spec.corrupt.clamp(0.0, 1.0),
                    dup: spec.dup.clamp(0.0, 1.0),
                    reorder: spec.reorder.clamp(0.0, 1.0),
                    reorder_depth: 4,
                    seed: spec.seed,
                }),
                old: base,
                expected_sha: m.new_sha256,
                dec: None,
                patcher: None,
                new_img: Vec::with_capacity(new.len()),
                deliveries: Vec::new(),
                events: Vec::new(),
                event_delivery: Vec::new(),
                cur_delivery: u32::MAX,
                pending: HashMap::new(),
                status: Status::Listening,
                reason: None,
                done_at: 0.0,
                give_up_at: spec.join_ms + 1.5 * (round_ms + cfg.round_gap_ms),
                latency: cfg.latency_ms,
                rx_events: Vec::new(),
                out: Vec::new(),
                spec: spec.clone(),
            }
        })
        .collect();

    let mut packets = Vec::new();
    let mut rounds = Vec::new();
    let mut t = 0.0;
    for r in 0..cfg.max_rounds {
        rounds.push(t);
        let mut slot = |kind: u8, src: u8, datagram: &[u8], t: f64, packets: &mut Vec<PacketRec>| {
            let (g, i) = match Packet::decode(datagram) {
                Ok(Packet::Shard { group, index, .. }) => (group, index),
                _ => (0, 0),
            };
            let p = packets.len() as u32;
            packets.push(PacketRec { t, kind, g, i, r, src });
            for rx in &mut rxs {
                rx.on_packet(p, t, datagram);
            }
        };
        if r == 0 {
            // The attacker strikes first, so forged shards are buffered before the genuine ones.
            for (kind, d) in &attack {
                slot(*kind, 1, d, t, &mut packets);
                t += interval;
            }
        }
        for d in &round {
            let kind = match Packet::decode(d) {
                Ok(Packet::Shard { .. }) => 1,
                _ => 0,
            };
            slot(kind, 0, d, t, &mut packets);
            t += interval;
        }
        if rxs.iter().all(RxSim::done) {
            break;
        }
        t += cfg.round_gap_ms;
    }
    let end = t + cfg.latency_ms;
    for rx in &mut rxs {
        if rx.done() {
            continue;
        }
        // End of broadcast: held-back packets arrive, then the receiver gives up.
        let mut held = Vec::new();
        rx.imp.flush_tagged(&mut held);
        for (tag, data) in held {
            rx.deliveries.push(Delivery {
                p: tag as u32,
                t: end,
                f: "late",
            });
            if !rx.done() {
                rx.handle(tag as u32, end, &data);
            }
        }
        if !rx.done() {
            let reason = format!("broadcast ended: {}", rx.core.progress_message());
            rx.finish_with(Status::Failed, end, Some(reason));
        }
    }

    let (heat, class) = heat(&old, &new, &prepared.records, 480);
    let signature = &prepared.container[prepared.container.len() - 64..];
    let log = ScenarioLog {
        summary: Summary {
            scale: cfg.scale,
            old_size: old.len(),
            new_size: new.len(),
            raw_patch: prepared.diff_stats.raw_len,
            patch_size: m.patch_size,
            compressed: m.compression == Compression::Zstd,
            records: prepared.diff_stats.records,
            diff_bytes: prepared.diff_stats.diff_bytes,
            extra_bytes: prepared.diff_stats.extra_bytes,
            groups: m.group_count(),
            shard_size: m.fec.shard_size,
            k: m.fec.k,
            m: m.fec.m,
            packets_per_round: round.len(),
            interval_ms: interval,
            latency_ms: cfg.latency_ms,
            manifest_len: prepared.container.len(),
            manifest_frags: prepared.container.len().div_ceil(wire::MANIFEST_FRAG_SIZE),
        },
        manifest: ManifestCard {
            session: format!("{:016x}", m.session_id),
            fw_version: m.fw_version,
            old_sha: to_hex(&m.old_sha256),
            new_sha: to_hex(&m.new_sha256),
            patch_sha: to_hex(&m.patch_sha256),
            public_key: to_hex(key.verifying_key().as_bytes()),
            signature: to_hex(signature),
            group_hashes: m.group_hashes.iter().map(|h| to_hex(&h[..4])).collect(),
        },
        toy: (cfg.scale == Scale::Toy).then(|| ToyImages {
            old: String::from_utf8_lossy(&old).into_owned(),
            new: String::from_utf8_lossy(&new).into_owned(),
            class,
        }),
        heat,
        records: prepared
            .records
            .iter()
            .map(|r| [r.new_off, r.old_off, r.diff_len, r.extra_len])
            .collect(),
        groups: (0..m.group_count())
            .map(|g| GroupInfo {
                len: layout.group_len(g),
                real: layout.real_data_shards(g),
            })
            .collect(),
        packets,
        rounds,
        end_ms: end,
        receivers: rxs.into_iter().map(RxSim::into_log).collect(),
    };
    Ok(log)
}
