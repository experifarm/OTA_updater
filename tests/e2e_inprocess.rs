//! Deterministic end-to-end tests: sender → (in-memory "broadcast") → impairment → receiver.

use ed25519_dalek::SigningKey;
use ota_updater::channel::ImpairmentConfig;
use ota_updater::receiver::{self, Event, ReceiverConfig, Report, VecSource, temp_path};
use ota_updater::sender::{self, Prepared, Schedule, SenderConfig};
use ota_updater::slicer::FecParams;
use ota_updater::wire::Packet;
use std::path::PathBuf;
use std::sync::OnceLock;

const FW_VERSION: u64 = 2;

struct Shared {
    old: Vec<u8>,
    new: Vec<u8>,
    key: SigningKey,
    prepared: Prepared,
}

fn shared() -> &'static Shared {
    static S: OnceLock<Shared> = OnceLock::new();
    S.get_or_init(|| {
        let (old, new) = ota_updater::imagegen::generate_pair(1 << 20, 21);
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let cfg = SenderConfig {
            fw_version: FW_VERSION,
            fec: FecParams {
                shard_size: 256,
                k: 16,
                m: 6,
            },
            level: 19,
            session_id: Some(0x0123_4567_89ab_cdef),
            ..Default::default()
        };
        let prepared = sender::prepare(&old, &new, &key, &cfg).unwrap();
        assert!(prepared.manifest.group_count() >= 6, "want several FEC groups");
        Shared {
            old,
            new,
            key,
            prepared,
        }
    })
}

fn broadcast(prepared: &Prepared, rounds: usize, interleave: usize) -> Vec<Vec<u8>> {
    let schedule = Schedule {
        interleave,
        manifest_every: 64,
    };
    let round = prepared.round_packets(&schedule);
    (0..rounds).flat_map(|_| round.iter().cloned()).collect()
}

struct Rx {
    _dir: tempfile::TempDir,
    old_path: PathBuf,
    out_path: PathBuf,
}

fn rx_with_base(base: &[u8]) -> Rx {
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("active.img");
    std::fs::write(&old_path, base).unwrap();
    let out_path = dir.path().join("slot_b.img");
    Rx {
        _dir: dir,
        old_path,
        out_path,
    }
}

fn config(imp: ImpairmentConfig) -> ReceiverConfig {
    let mut cfg = ReceiverConfig::new(shared().key.verifying_key());
    cfg.installed_version = FW_VERSION - 1;
    cfg.impairment = Some(imp);
    cfg
}

fn run(rx: &Rx, packets: Vec<Vec<u8>>, cfg: &ReceiverConfig) -> anyhow::Result<Report> {
    receiver::receive(
        &mut VecSource::new(packets),
        &rx.old_path,
        &rx.out_path,
        cfg,
        &mut |_e: Event<'_>| {},
    )
}

fn assert_installed(rx: &Rx, report: &Report) {
    let installed = std::fs::read(&rx.out_path).unwrap();
    assert!(installed == shared().new, "installed image differs from the new image");
    assert_eq!(report.new_sha256, ota_updater::sha256(&shared().new));
    assert!(!temp_path(&rx.out_path).exists(), "temp file left behind");
}

fn assert_not_installed(rx: &Rx) {
    assert!(!rx.out_path.exists(), "image must not be installed");
    assert!(!temp_path(&rx.out_path).exists(), "temp file left behind");
}

fn loss(loss: f64, seed: u64) -> ImpairmentConfig {
    ImpairmentConfig {
        loss,
        seed,
        ..Default::default()
    }
}

#[test]
fn clean_channel_single_round() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let report = run(&rx, broadcast(&s.prepared, 1, 1), &config(ImpairmentConfig::default())).unwrap();
    assert_installed(&rx, &report);
    assert_eq!(report.stats.groups_repaired, 0);
    assert_eq!(report.stats.shards_recovered, 0);
    assert_eq!(report.stats.groups_total, s.prepared.manifest.group_count());
    // interleave = 1 without loss: every group completes exactly when it is next in line.
    assert_eq!(report.stats.max_pending_groups, 0);
}

#[test]
fn ten_percent_loss_is_repaired_by_fec() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let report = run(&rx, broadcast(&s.prepared, 2, 8), &config(loss(0.10, 1))).unwrap();
    assert_installed(&rx, &report);
    assert!(report.stats.impairment.dropped > 0);
    assert!(report.stats.groups_repaired > 0, "{:?}", report.stats);
    assert!(report.stats.shards_recovered > 0);
}

#[test]
fn twenty_percent_loss() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let report = run(&rx, broadcast(&s.prepared, 3, 8), &config(loss(0.20, 2))).unwrap();
    assert_installed(&rx, &report);
    assert!(report.stats.groups_repaired > 0);
}

#[test]
fn burst_loss_with_interleaving() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let imp = ImpairmentConfig {
        loss: 0.15,
        burst_len: 6.0,
        seed: 3,
        ..Default::default()
    };
    let report = run(&rx, broadcast(&s.prepared, 3, 8), &config(imp)).unwrap();
    assert_installed(&rx, &report);
}

#[test]
fn corruption_duplication_reordering() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let imp = ImpairmentConfig {
        loss: 0.05,
        corrupt: 0.05,
        dup: 0.05,
        reorder: 0.10,
        seed: 4,
        ..Default::default()
    };
    let report = run(&rx, broadcast(&s.prepared, 2, 4), &config(imp)).unwrap();
    assert_installed(&rx, &report);
    let st = &report.stats;
    assert!(
        st.impairment.corrupted > 0 && st.bad_packets > 0,
        "CRC must reject corrupted packets: {st:?}"
    );
    assert!(st.redundant_shards > 0);
    assert_eq!(st.group_hash_failures, 0, "CRC32 must catch all single-bit corruption");
}

#[test]
fn heavy_loss_fails_with_one_round_and_recovers_with_more() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let err = run(&rx, broadcast(&s.prepared, 1, 8), &config(loss(0.40, 5))).unwrap_err();
    assert!(err.to_string().contains("stream ended"), "{err:#}");
    assert_not_installed(&rx);

    let rx = rx_with_base(&s.old);
    let report = run(&rx, broadcast(&s.prepared, 4, 8), &config(loss(0.40, 5))).unwrap();
    assert_installed(&rx, &report);
}

#[test]
fn late_joiner_completes_in_next_round() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let mut packets = broadcast(&s.prepared, 2, 8);
    let per_round = packets.len() / 2;
    packets.drain(..per_round * 2 / 3); // joined two thirds into round 1
    let report = run(&rx, packets, &config(ImpairmentConfig::default())).unwrap();
    assert_installed(&rx, &report);
}

#[test]
fn wrong_public_key_is_rejected() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let mut cfg = config(ImpairmentConfig::default());
    cfg.public_key = SigningKey::from_bytes(&[0x43; 32]).verifying_key();
    let err = run(&rx, broadcast(&s.prepared, 1, 8), &cfg).unwrap_err();
    assert!(err.to_string().contains("no valid manifest"), "{err:#}");
    assert_not_installed(&rx);
}

#[test]
fn wrong_base_image_is_rejected() {
    let s = shared();
    let mut other = s.old.clone();
    other[1000] ^= 1;
    let rx = rx_with_base(&other);
    let err = run(&rx, broadcast(&s.prepared, 1, 8), &config(ImpairmentConfig::default())).unwrap_err();
    assert!(err.to_string().contains("different base image"), "{err:#}");
    assert_not_installed(&rx);
}

#[test]
fn rollback_is_rejected() {
    let s = shared();
    let rx = rx_with_base(&s.old);
    let mut cfg = config(ImpairmentConfig::default());
    cfg.installed_version = FW_VERSION;
    let err = run(&rx, broadcast(&s.prepared, 1, 8), &cfg).unwrap_err();
    assert!(err.to_string().contains("rollback"), "{err:#}");
    assert_not_installed(&rx);
}

#[test]
fn forged_traffic_cannot_hijack_or_block_the_update() {
    let s = shared();
    let mut packets = Vec::new();

    // 1. An attacker broadcasts its own "update" (different session) signed with its own key.
    let attacker = SigningKey::from_bytes(&[0x66; 32]);
    let evil_new = vec![0xEE; 64 * 1024];
    let evil = sender::prepare(
        &s.old,
        &evil_new,
        &attacker,
        &SenderConfig {
            fw_version: 99,
            session_id: Some(0xbad),
            ..Default::default()
        },
    )
    .unwrap();
    packets.extend(broadcast(&evil, 1, 8));

    // 2. Forged shards with the genuine session id and valid CRCs (garbage payload) arrive before
    //    the genuine ones.
    let shard_size = s.prepared.manifest.fec.shard_size();
    for idx in 0..4u16 {
        packets.push(
            Packet::Shard {
                session_id: s.prepared.manifest.session_id,
                group: 0,
                index: idx,
                data: vec![0xA5; shard_size],
            }
            .encode(),
        );
    }

    // 3. A forged manifest fragment for the genuine session.
    packets.push(
        Packet::ManifestFrag {
            session_id: s.prepared.manifest.session_id,
            index: 0,
            count: s.prepared.container.len().div_ceil(1024) as u16,
            total_len: s.prepared.container.len() as u32,
            data: vec![0; 1024.min(s.prepared.container.len())],
        }
        .encode(),
    );

    packets.extend(broadcast(&s.prepared, 2, 8));
    let rx = rx_with_base(&s.old);
    let mut events = Vec::new();
    let report = receiver::receive(
        &mut VecSource::new(packets),
        &rx.old_path,
        &rx.out_path,
        &config(ImpairmentConfig::default()),
        &mut |e| events.push(format!("{e:?}")),
    )
    .unwrap();
    assert_installed(&rx, &report);
    assert!(report.stats.signature_failures >= 1, "{:?}", report.stats);
    assert!(
        report.stats.group_hash_failures >= 1,
        "forged shards must fail the signed group hash"
    );
    assert!(events.iter().any(|e| e.contains("GroupHashMismatch")));
}

#[test]
fn final_signed_hash_mismatch_is_not_installed() {
    // A (buggy) publisher signs a manifest whose new-image hash does not match what the patch
    // produces: the patch streams through, but the final verification must refuse to install.
    let s = shared();
    let mut manifest = s.prepared.manifest.clone();
    manifest.new_sha256[0] ^= 0xff;
    let bad = Prepared {
        container: manifest.sign(&s.key),
        manifest,
        patch: s.prepared.patch.clone(),
        shards: s.prepared.shards.clone(),
        diff_stats: s.prepared.diff_stats,
        prepare_time: s.prepared.prepare_time,
    };
    let rx = rx_with_base(&s.old);
    let err = run(&rx, broadcast(&bad, 1, 8), &config(ImpairmentConfig::default())).unwrap_err();
    assert!(err.to_string().contains("verification failed"), "{err:#}");
    assert_not_installed(&rx);
}

#[test]
fn uncompressed_patch_and_default_fec_params() {
    let (old, new) = ota_updater::imagegen::generate_pair(512 * 1024, 8);
    let key = SigningKey::from_bytes(&[0x42; 32]);
    let prepared = sender::prepare(
        &old,
        &new,
        &key,
        &SenderConfig {
            fw_version: FW_VERSION,
            compression: ota_updater::patch::Compression::None,
            ..Default::default()
        },
    )
    .unwrap();
    let rx = rx_with_base(&old);
    let mut cfg = ReceiverConfig::new(key.verifying_key());
    cfg.impairment = Some(loss(0.05, 9));
    let report = receiver::receive(
        &mut VecSource::new(broadcast(&prepared, 2, 8)),
        &rx.old_path,
        &rx.out_path,
        &cfg,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(&rx.out_path).unwrap(), new);
    assert_eq!(report.manifest.compression, ota_updater::patch::Compression::None);
}
