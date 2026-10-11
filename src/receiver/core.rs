//! Sans-IO receiver state machine.
//!
//! [`ReceiverCore`] consumes raw datagrams and emits [`RxEvent`]s. It performs no I/O, spawns no threads
//! and reads no clock, so the same logic drives the native receiver (`receiver::receive`) and the
//! deterministic in-browser simulation used by the visualization.
//!
//! ```text
//! datagram ─► wire::decode (CRC32)
//!   ├─ ManifestFrag ─► reassemble ─► verify signature (pinned key) ─► acceptance checks ─► session
//!   └─ Shard ───────► group buffer ─► (≥ k shards) RS reconstruct ─► SHA-256 == signed group hash
//!                        └─► in-order release (GroupReleased)
//! ```

use super::ReceiveStats;
use crate::manifest::{self, Manifest};
use crate::rs::ReedSolomon;
use crate::slicer::{self, Layout};
use crate::wire::{self, Packet, WireError};
use ed25519_dalek::VerifyingKey;
use std::collections::{BTreeMap, HashMap};

/// Acceptance policy of a receiver.
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Pinned Ed25519 public key of the update publisher.
    pub public_key: VerifyingKey,
    /// Firmware version currently installed; only strictly newer versions are accepted.
    pub installed_version: u64,
    pub max_image_size: u64,
    /// Bound on shards buffered before a manifest has been verified.
    pub max_prebuffer_bytes: usize,
}

impl CoreConfig {
    pub fn new(public_key: VerifyingKey) -> Self {
        Self {
            public_key,
            installed_version: 0,
            max_image_size: 1 << 30,
            max_prebuffer_bytes: 32 << 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadPacketReason {
    /// CRC32 mismatch (corruption in transit).
    Crc,
    /// Not a well-formed packet.
    Format,
    /// Manifest fragment with inconsistent fields.
    Fragment,
    /// Shard outside the session's layout.
    Shard,
}

/// Everything observable that happens inside the receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RxEvent {
    BadPacket {
        reason: BadPacketReason,
    },
    /// Packet of another (or a rejected) session.
    Ignored {
        session_id: u64,
    },
    ManifestFragment {
        session_id: u64,
        index: u16,
        count: u16,
    },
    /// Manifest fragment received after the manifest was already accepted.
    ManifestRepeat,
    ManifestSignatureInvalid {
        session_id: u64,
    },
    /// The manifest is authentic and acceptable; see [`ReceiverCore::manifest`].
    ManifestAccepted {
        session_id: u64,
    },
    SessionRejected {
        session_id: u64,
        reason: String,
    },
    /// Shard kept until a manifest is verified.
    ShardBuffered {
        group: u32,
        index: u16,
    },
    ShardStored {
        group: u32,
        index: u16,
    },
    /// Shard not needed: its group is complete, or the shard is already held.
    ShardRedundant {
        group: u32,
        index: u16,
    },
    /// k shards were present: the group was RS-decoded from shards `used`, rebuilding the data shards
    /// `missing`; `hash_ok` tells whether the result matched the signed group hash.
    GroupDecoded {
        group: u32,
        used: Vec<u16>,
        missing: Vec<u16>,
        hash_ok: bool,
    },
    /// Authenticated patch bytes of `group`, released in order for the patcher.
    GroupReleased {
        group: u32,
        bytes: Vec<u8>,
    },
    /// Every group has been released.
    Complete,
}

struct Assembly {
    count: u16,
    total_len: u32,
    frags: Vec<Option<Vec<u8>>>,
    have: u16,
}

impl Assembly {
    fn new(count: u16, total_len: u32) -> Self {
        Self {
            count,
            total_len,
            frags: vec![None; usize::from(count)],
            have: 0,
        }
    }
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
}

const MAX_ASSEMBLIES: usize = 16;

pub struct ReceiverCore {
    cfg: CoreConfig,
    old_sha256: [u8; 32],
    old_size: u64,
    assemblies: HashMap<u64, Assembly>,
    rejected: HashMap<u64, String>,
    prebuffer: HashMap<u64, Vec<(u32, u16, Vec<u8>)>>,
    prebuffer_bytes: usize,
    session: Option<Session>,
    complete: bool,
    stats: ReceiveStats,
}

impl ReceiverCore {
    /// `old_sha256` / `old_size` describe the locally installed base image.
    pub fn new(cfg: CoreConfig, old_sha256: [u8; 32], old_size: u64) -> Self {
        Self {
            cfg,
            old_sha256,
            old_size,
            assemblies: HashMap::new(),
            rejected: HashMap::new(),
            prebuffer: HashMap::new(),
            prebuffer_bytes: 0,
            session: None,
            complete: false,
            stats: ReceiveStats::default(),
        }
    }

    /// The accepted (verified) manifest, once there is one.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.session.as_ref().map(|s| &s.manifest)
    }

    /// All groups have been released to the patcher.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn completed_groups(&self) -> u32 {
        self.session.as_ref().map_or(0, |s| s.completed)
    }

    pub fn stats(&self) -> &ReceiveStats {
        &self.stats
    }

    /// Sessions rejected so far, with reasons.
    pub fn rejections(&self) -> &HashMap<u64, String> {
        &self.rejected
    }

    /// Process one datagram, appending what happened to `out`.
    pub fn handle(&mut self, datagram: &[u8], out: &mut Vec<RxEvent>) {
        self.stats.datagrams += 1;
        let pkt = match Packet::decode(datagram) {
            Ok(p) => p,
            Err(e) => {
                self.stats.bad_packets += 1;
                let reason = if e == WireError::BadCrc {
                    BadPacketReason::Crc
                } else {
                    BadPacketReason::Format
                };
                out.push(RxEvent::BadPacket { reason });
                return;
            }
        };
        let sid = pkt.session_id();
        if self.rejected.contains_key(&sid) || self.session.as_ref().is_some_and(|s| s.manifest.session_id != sid) {
            self.stats.ignored_packets += 1;
            out.push(RxEvent::Ignored { session_id: sid });
            return;
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
                    out.push(RxEvent::ManifestRepeat);
                } else if let Some(container) = self.add_fragment(sid, index, count, total_len, data, out) {
                    self.try_accept(sid, &container, out);
                }
            }
            Packet::Shard { group, index, data, .. } => {
                if self.session.is_some() {
                    self.add_shard(group, index, data, out);
                } else {
                    self.prebuffer_shard(sid, group, index, data, out);
                }
            }
        }
    }

    fn add_fragment(
        &mut self,
        sid: u64,
        index: u16,
        count: u16,
        total_len: u32,
        data: Vec<u8>,
        out: &mut Vec<RxEvent>,
    ) -> Option<Vec<u8>> {
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
            out.push(RxEvent::BadPacket {
                reason: BadPacketReason::Fragment,
            });
            return None;
        }
        out.push(RxEvent::ManifestFragment {
            session_id: sid,
            index,
            count,
        });
        if !self.assemblies.contains_key(&sid) && self.assemblies.len() >= MAX_ASSEMBLIES {
            self.assemblies.clear();
        }
        let asm = self
            .assemblies
            .entry(sid)
            .or_insert_with(|| Assembly::new(count, total_len));
        if asm.count != count || asm.total_len != total_len {
            *asm = Assembly::new(count, total_len);
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

    fn reject(&mut self, sid: u64, reason: String, out: &mut Vec<RxEvent>) {
        self.prebuffer_bytes -= self
            .prebuffer
            .remove(&sid)
            .map_or(0, |v| v.iter().map(|(_, _, d)| d.len()).sum());
        out.push(RxEvent::SessionRejected {
            session_id: sid,
            reason: reason.clone(),
        });
        self.rejected.insert(sid, reason);
    }

    fn try_accept(&mut self, sid: u64, container: &[u8], out: &mut Vec<RxEvent>) {
        // Unauthenticated failures only discard the reassembly: a forged fragment must not be able to
        // blacklist a genuine session.
        let Ok(body) = manifest::verify(container, &self.cfg.public_key) else {
            self.stats.signature_failures += 1;
            out.push(RxEvent::ManifestSignatureInvalid { session_id: sid });
            return;
        };
        let m = match Manifest::decode(body) {
            Ok(m) => m,
            Err(e) => return self.reject(sid, format!("signed manifest is invalid: {e}"), out),
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
        match reason {
            Some(reason) => self.reject(sid, reason, out),
            None => self.start_session(m, out),
        }
    }

    fn start_session(&mut self, m: Manifest, out: &mut Vec<RxEvent>) {
        let sid = m.session_id;
        self.stats.groups_total = m.group_count();
        self.session = Some(Session {
            layout: m.layout(),
            codec: m.fec.codec(),
            groups: (0..m.group_count()).map(|_| GroupState::default()).collect(),
            manifest: m,
            completed: 0,
            next_group: 0,
            ready: BTreeMap::new(),
        });
        out.push(RxEvent::ManifestAccepted { session_id: sid });
        self.assemblies.clear();
        let pre = self.prebuffer.remove(&sid).unwrap_or_default();
        self.prebuffer.clear();
        self.prebuffer_bytes = 0;
        for (g, i, d) in pre {
            self.add_shard(g, i, d, out);
        }
        self.release_ready(out);
    }

    fn prebuffer_shard(&mut self, sid: u64, group: u32, index: u16, data: Vec<u8>, out: &mut Vec<RxEvent>) {
        if self.prebuffer_bytes + data.len() > self.cfg.max_prebuffer_bytes {
            self.stats.ignored_packets += 1;
            out.push(RxEvent::Ignored { session_id: sid });
            return;
        }
        self.prebuffer_bytes += data.len();
        self.prebuffer.entry(sid).or_default().push((group, index, data));
        out.push(RxEvent::ShardBuffered { group, index });
    }

    fn add_shard(&mut self, group: u32, index: u16, data: Vec<u8>, out: &mut Vec<RxEvent>) {
        let stats = &mut self.stats;
        let s = self.session.as_mut().expect("active session");
        let idx = usize::from(index);
        let (k, n, shard_size) = (s.manifest.fec.k(), s.manifest.fec.n(), s.manifest.fec.shard_size());
        if group >= s.manifest.group_count() || idx >= n || s.layout.is_implicit(group, idx) || data.len() != shard_size
        {
            stats.bad_packets += 1;
            out.push(RxEvent::BadPacket {
                reason: BadPacketReason::Shard,
            });
            return;
        }
        let layout = s.layout;
        let gs = &mut s.groups[group as usize];
        if gs.done {
            stats.redundant_shards += 1;
            out.push(RxEvent::ShardRedundant { group, index });
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
            out.push(RxEvent::ShardRedundant { group, index });
            return;
        }
        gs.shards[idx] = Some(data);
        gs.present += 1;
        stats.shards_accepted += 1;
        out.push(RxEvent::ShardStored { group, index });
        if gs.present < k {
            return;
        }

        // The codec decodes from the first k present shards (ascending index).
        let used: Vec<u16> = (0..n)
            .filter(|&i| gs.shards[i].is_some())
            .take(k)
            .map(|i| i as u16)
            .collect();
        let missing: Vec<u16> = (0..k).filter(|&i| gs.shards[i].is_none()).map(|i| i as u16).collect();
        let hash_ok = s.codec.reconstruct(&mut gs.shards).is_ok() && {
            let bytes = slicer::assemble_group(&gs.shards, &layout, group);
            let ok = crate::sha256(&bytes) == s.manifest.group_hashes[group as usize];
            if ok {
                s.ready.insert(group, bytes);
            }
            ok
        };
        out.push(RxEvent::GroupDecoded {
            group,
            used,
            missing: missing.clone(),
            hash_ok,
        });
        if !hash_ok {
            // A shard was forged or corrupted undetected: discard and collect fresh shards next round.
            gs.shards.clear();
            gs.present = 0;
            stats.group_hash_failures += 1;
            return;
        }
        gs.done = true;
        gs.shards = Vec::new();
        s.completed += 1;
        stats.shards_recovered += missing.len() as u64;
        if !missing.is_empty() {
            stats.groups_repaired += 1;
        }
        self.release_ready(out);
    }

    /// Release completed groups to the patcher strictly in order.
    fn release_ready(&mut self, out: &mut Vec<RxEvent>) {
        let Some(s) = self.session.as_mut() else { return };
        while let Some(bytes) = s.ready.remove(&s.next_group) {
            out.push(RxEvent::GroupReleased {
                group: s.next_group,
                bytes,
            });
            s.next_group += 1;
        }
        self.stats.max_pending_groups = self.stats.max_pending_groups.max(s.ready.len());
        if s.next_group == s.manifest.group_count() && !self.complete {
            self.complete = true;
            out.push(RxEvent::Complete);
        }
    }

    /// Human-readable progress / failure summary.
    pub fn progress_message(&self) -> String {
        match &self.session {
            Some(s) => {
                let total = s.manifest.group_count();
                let missing: Vec<String> = (0..total)
                    .filter(|&g| !s.groups[g as usize].done)
                    .take(10)
                    .map(|g| g.to_string())
                    .collect();
                format!(
                    "{}/{} groups complete; missing groups {}{}",
                    s.completed,
                    total,
                    missing.join(","),
                    if (total - s.completed) as usize > missing.len() {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sender::{self, Schedule, SenderConfig};
    use crate::slicer::FecParams;
    use ed25519_dalek::SigningKey;

    fn setup() -> (Vec<u8>, sender::Prepared, SigningKey) {
        let (old, new) = crate::imagegen::generate_pair(64 * 1024, 2);
        let key = SigningKey::from_bytes(&[1; 32]);
        let cfg = SenderConfig {
            fw_version: 2,
            fec: FecParams {
                shard_size: 128,
                k: 4,
                m: 2,
            },
            session_id: Some(9),
            ..Default::default()
        };
        let p = sender::prepare(&old, &new, &key, &cfg).unwrap();
        (old, p, key)
    }

    #[test]
    fn event_sequence_for_a_clean_round() {
        let (old, p, key) = setup();
        let mut core = ReceiverCore::new(
            CoreConfig::new(key.verifying_key()),
            crate::sha256(&old),
            old.len() as u64,
        );
        let mut events = Vec::new();
        let schedule = Schedule {
            interleave: 1,
            manifest_every: 0,
        };
        for d in p.round_packets(&schedule) {
            core.handle(&d, &mut events);
        }
        assert!(core.is_complete());
        // Manifest fragments first (one or more), then acceptance.
        let accepted = events
            .iter()
            .position(|e| matches!(e, RxEvent::ManifestAccepted { session_id: 9 }))
            .expect("manifest accepted");
        assert!(accepted >= 1);
        assert!(
            events[..accepted]
                .iter()
                .all(|e| matches!(e, RxEvent::ManifestFragment { .. }))
        );
        // `Complete` comes exactly once, right after the last release; later parity is redundant.
        let complete_at: Vec<usize> = (0..events.len()).filter(|&i| events[i] == RxEvent::Complete).collect();
        assert_eq!(complete_at.len(), 1);
        assert!(matches!(events[complete_at[0] - 1], RxEvent::GroupReleased { .. }));
        assert!(
            events[complete_at[0] + 1..]
                .iter()
                .all(|e| matches!(e, RxEvent::ShardRedundant { .. }))
        );
        let released: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                RxEvent::GroupReleased { group, .. } => Some(*group),
                _ => None,
            })
            .collect();
        assert_eq!(released, (0..p.manifest.group_count()).collect::<Vec<_>>());
        let patch: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                RxEvent::GroupReleased { bytes, .. } => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(patch, p.patch, "released bytes must equal the signed patch");
        // Without loss every group decodes from its k data shards and nothing is rebuilt.
        for e in &events {
            if let RxEvent::GroupDecoded { missing, hash_ok, .. } = e {
                assert!(missing.is_empty() && *hash_ok);
            }
        }
    }

    #[test]
    fn decode_reports_used_and_missing_shards() {
        let (old, p, key) = setup();
        let mut core = ReceiverCore::new(
            CoreConfig::new(key.verifying_key()),
            crate::sha256(&old),
            old.len() as u64,
        );
        let mut events = Vec::new();
        let schedule = Schedule {
            interleave: 1,
            manifest_every: 0,
        };
        // Drop data shards 0 and 2 of group 0.
        for d in p.round_packets(&schedule) {
            if let Ok(Packet::Shard { group: 0, index, .. }) = Packet::decode(&d)
                && (index == 0 || index == 2)
            {
                continue;
            }
            core.handle(&d, &mut events);
        }
        let decoded = events
            .iter()
            .find_map(|e| match e {
                RxEvent::GroupDecoded {
                    group: 0,
                    used,
                    missing,
                    hash_ok,
                } => Some((used.clone(), missing.clone(), *hash_ok)),
                _ => None,
            })
            .unwrap();
        assert_eq!(decoded, (vec![1, 3, 4, 5], vec![0, 2], true));
        assert!(core.is_complete());
        assert_eq!(core.stats().shards_recovered, 2);
    }
}
