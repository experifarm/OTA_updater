//! End-to-end over real UDP multicast on the loopback interface: one sender, two concurrent receivers
//! with different simulated impairments.

use ed25519_dalek::SigningKey;
use ota_updater::channel::ImpairmentConfig;
use ota_updater::net::{MulticastConfig, MulticastReceiver, MulticastSender};
use ota_updater::receiver::{self, ReceiverConfig};
use ota_updater::sender::{self, BroadcastConfig, Schedule, SenderConfig};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

#[test]
fn multicast_broadcast_to_two_lossy_receivers() {
    let (old, new) = ota_updater::imagegen::generate_pair(1 << 20, 33);
    let key = SigningKey::from_bytes(&[7; 32]);
    let prepared = sender::prepare(
        &old,
        &new,
        &key,
        &SenderConfig {
            fw_version: 5,
            ..Default::default()
        },
    )
    .unwrap();

    let mcast = MulticastConfig {
        group: Ipv4Addr::new(239, 255, 77, 9),
        port: 40_000 + (std::process::id() % 20_000) as u16,
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("active.img");
    std::fs::write(&old_path, &old).unwrap();

    let impairments = [
        ImpairmentConfig {
            loss: 0.05,
            seed: 1,
            ..Default::default()
        },
        ImpairmentConfig {
            loss: 0.15,
            burst_len: 4.0,
            corrupt: 0.01,
            dup: 0.01,
            reorder: 0.02,
            seed: 2,
            ..Default::default()
        },
    ];
    let done = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for (i, imp) in impairments.into_iter().enumerate() {
        // Join the group before the sender starts.
        let mut source = MulticastReceiver::new(&mcast).expect("join multicast group on loopback");
        let mut cfg = ReceiverConfig::new(key.verifying_key());
        cfg.installed_version = 4;
        cfg.timeout = Some(Duration::from_secs(60));
        cfg.impairment = Some(imp);
        let old_path = old_path.clone();
        let out_path = dir.path().join(format!("rx{i}.img"));
        let done = Arc::clone(&done);
        handles.push(thread::spawn(move || {
            let result = receiver::receive(&mut source, &old_path, &out_path, &cfg, &mut |_| {});
            done.fetch_add(1, Ordering::SeqCst);
            result.map(|r| (out_path, r))
        }));
    }

    let mut sink = MulticastSender::new(&mcast, 50_000).unwrap();
    let bcfg = BroadcastConfig {
        rounds: Some(1),
        schedule: Schedule::default(),
        round_gap: Duration::ZERO,
    };
    let mut rounds = 0;
    while done.load(Ordering::SeqCst) < handles.len() && rounds < 30 {
        sender::broadcast(&prepared, &bcfg, &mut sink, &mut |_, _| {}).unwrap();
        rounds += 1;
        thread::sleep(Duration::from_millis(20));
    }

    for h in handles {
        let (out_path, report) = h.join().unwrap().expect("receiver completed");
        assert_eq!(std::fs::read(&out_path).unwrap(), new);
        assert_eq!(report.manifest.session_id, prepared.manifest.session_id);
        assert!(report.stats.datagrams > 0);
    }
}
