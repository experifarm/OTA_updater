//! Native tests of the in-browser simulation: the scenarios the sandbox presets rely on.

use ota_viz::sim::{self, ReceiverSpec, Scale, ScenarioConfig};

fn rx(name: &str) -> ReceiverSpec {
    ReceiverSpec {
        name: name.into(),
        ..Default::default()
    }
}

fn events_of<'a>(log: &'a serde_json::Value, rx: usize, ty: &str) -> Vec<&'a serde_json::Value> {
    log["receivers"][rx]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == ty)
        .collect()
}

fn run_json(cfg: &ScenarioConfig) -> serde_json::Value {
    serde_json::to_value(sim::run(cfg).unwrap()).unwrap()
}

#[test]
fn toy_clean_installs_and_writes_the_whole_image() {
    let cfg = ScenarioConfig::default();
    let log = run_json(&cfg);
    let r = &log["receivers"][0];
    assert_eq!(r["result"]["status"], "installed", "{}", r["result"]);
    let new_size = log["summary"]["newSize"].as_u64().unwrap();
    // Write events tile the new image exactly, in order.
    let mut next = 0;
    for w in events_of(&log, 0, "write") {
        assert_eq!(w["off"].as_u64().unwrap(), next);
        next += w["len"].as_u64().unwrap();
    }
    assert_eq!(next, new_size);
    assert!(log["toy"]["old"].as_str().unwrap().starts_with("TOY-FW v1.0.0"));
    assert_eq!(
        log["groups"].as_array().unwrap().len() as u64,
        log["summary"]["groups"].as_u64().unwrap()
    );
    // Sender stops after the round in which everyone finished.
    assert_eq!(log["rounds"].as_array().unwrap().len(), 1);
}

#[test]
fn lossy_receivers_need_fec_and_rounds() {
    let cfg = ScenarioConfig {
        receivers: vec![
            ReceiverSpec {
                loss: 0.15,
                seed: 3,
                ..rx("A")
            },
            ReceiverSpec {
                loss: 0.2,
                burst_len: 4.0,
                corrupt: 0.05,
                dup: 0.05,
                reorder: 0.05,
                seed: 4,
                ..rx("B")
            },
        ],
        ..Default::default()
    };
    let log = run_json(&cfg);
    for i in 0..2 {
        let r = &log["receivers"][i];
        assert_eq!(r["result"]["status"], "installed", "receiver {i}: {}", r["result"]);
        assert!(r["result"]["stats"]["dropped"].as_u64().unwrap() > 0);
    }
    let decoded = events_of(&log, 0, "decoded");
    assert!(
        decoded.iter().any(|d| !d["missing"].as_array().unwrap().is_empty()),
        "FEC repair expected"
    );
    assert!(
        !events_of(&log, 1, "crc_bad").is_empty(),
        "corruption must be caught by the CRC"
    );
    let fates: Vec<&str> = log["receivers"][1]["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["f"].as_str().unwrap())
        .collect();
    for f in ["ok", "lost", "corrupt"] {
        assert!(fates.contains(&f), "missing fate {f}");
    }
}

#[test]
fn real_scale_with_zstd_streams_and_installs() {
    let cfg = ScenarioConfig {
        scale: Scale::Real,
        image_size: 256 << 10,
        shard_size: 256,
        k: 8,
        m: 4,
        interleave: 4,
        manifest_every: 64,
        compress: true,
        receivers: vec![ReceiverSpec {
            loss: 0.1,
            seed: 5,
            ..rx("A")
        }],
        ..Default::default()
    };
    let log = run_json(&cfg);
    let r = &log["receivers"][0];
    assert_eq!(r["result"]["status"], "installed", "{}", r["result"]);
    assert!(log["summary"]["compressed"].as_bool().unwrap());
    assert!(log["summary"]["patchSize"].as_u64().unwrap() < log["summary"]["newSize"].as_u64().unwrap() / 5);
    // Streaming: patch bytes are written while groups are still arriving.
    let first_write = events_of(&log, 0, "write")[0]["t"].as_f64().unwrap();
    let last_release = events_of(&log, 0, "released").last().unwrap()["t"].as_f64().unwrap();
    assert!(
        first_write < last_release,
        "first write {first_write} vs last release {last_release}"
    );
}

#[test]
fn attacker_cannot_hijack_and_bad_receivers_are_refused() {
    let cfg = ScenarioConfig {
        attacker: true,
        receivers: vec![
            rx("genuine"),
            ReceiverSpec {
                installed_version: 2,
                ..rx("rollback")
            },
            ReceiverSpec {
                wrong_base: true,
                ..rx("other-base")
            },
            ReceiverSpec {
                wrong_key: true,
                ..rx("wrong-key")
            },
        ],
        ..Default::default()
    };
    let log = run_json(&cfg);
    let status = |i: usize| log["receivers"][i]["result"]["status"].as_str().unwrap().to_string();
    let reason = |i: usize| {
        log["receivers"][i]["result"]["reason"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(status(0), "installed", "{}", log["receivers"][0]["result"]);
    assert!(
        !events_of(&log, 0, "manifest_bad_sig").is_empty(),
        "forged manifests must fail"
    );
    assert!(
        events_of(&log, 0, "decoded").iter().any(|d| d["ok"] == false),
        "forged shards must fail the signed group hash"
    );
    assert_eq!(status(1), "rejected");
    assert!(reason(1).contains("rollback"), "{}", reason(1));
    assert_eq!(status(2), "rejected");
    assert!(reason(2).contains("different base image"), "{}", reason(2));
    assert_eq!(status(3), "failed");
    assert!(reason(3).contains("no valid manifest"), "{}", reason(3));
    let kinds: Vec<u64> = log["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_u64().unwrap())
        .collect();
    assert!(kinds.contains(&2) && kinds.contains(&3), "attacker packets present");
}

#[test]
fn late_joiner_and_too_few_rounds() {
    let cfg = ScenarioConfig {
        receivers: vec![ReceiverSpec {
            join_ms: 4000.0,
            ..rx("late")
        }],
        ..Default::default()
    };
    let log = run_json(&cfg);
    assert_eq!(log["receivers"][0]["result"]["status"], "installed");
    assert!(
        log["rounds"].as_array().unwrap().len() >= 2,
        "late joiner needs the next round"
    );
    let off = log["receivers"][0]["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["f"] == "off")
        .count();
    assert!(off > 0);

    let cfg = ScenarioConfig {
        max_rounds: 1,
        receivers: vec![ReceiverSpec {
            loss: 0.6,
            seed: 9,
            ..rx("storm")
        }],
        ..Default::default()
    };
    let log = run_json(&cfg);
    assert_eq!(log["receivers"][0]["result"]["status"], "failed");
}

#[test]
fn deterministic_and_json_entry_point() {
    let cfg = ScenarioConfig {
        receivers: vec![ReceiverSpec {
            loss: 0.2,
            reorder: 0.1,
            seed: 11,
            ..rx("A")
        }],
        ..Default::default()
    };
    let a = serde_json::to_string(&sim::run(&cfg).unwrap()).unwrap();
    let b = serde_json::to_string(&sim::run(&cfg).unwrap()).unwrap();
    assert_eq!(a, b);
    let json = ota_viz::run_scenario(r#"{"scale":"toy","receivers":[{"name":"X","loss":0.1}]}"#).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["receivers"][0]["spec"]["name"], "X");
}

#[test]
fn every_event_points_at_the_delivery_that_caused_it() {
    let cfg = ScenarioConfig {
        attacker: true,
        receivers: vec![ReceiverSpec {
            loss: 0.25,
            corrupt: 0.06,
            dup: 0.05,
            reorder: 0.05,
            seed: 21,
            ..rx("Car")
        }],
        ..Default::default()
    };
    let log = run_json(&cfg);
    let r = &log["receivers"][0];
    let events = r["events"].as_array().unwrap();
    let map = r["eventDelivery"].as_array().unwrap();
    let deliveries = r["deliveries"].as_array().unwrap();
    assert_eq!(events.len(), map.len());
    for (e, d) in events.iter().zip(map) {
        let d = d.as_u64().unwrap();
        if d == u64::from(u32::MAX) {
            assert_eq!(e["type"], "failed", "only end-of-broadcast failures have no delivery");
            continue;
        }
        let del = &deliveries[d as usize];
        assert_ne!(del["f"], "lost", "lost packets produce no events");
        assert_eq!(del["t"], e["t"], "an event happens when its packet arrives");
        if e["type"] != "stored" || e.get("p").is_none() {
            continue;
        }
        // Replayed buffered shards are stored when the manifest arrives, so `p` may differ there.
        if e["p"] == del["p"] {
            continue;
        }
        let replay = events
            .iter()
            .zip(map)
            .any(|(x, m)| x["type"] == "manifest_ok" && m.as_u64().unwrap() == d);
        assert!(replay, "stored event attributed to another packet: {e}");
    }
}
