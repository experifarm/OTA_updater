//! Native tests of the chapter lessons (real engine results the chapters display).

use ota_viz::lessons;
use serde_json::{Value, json};

fn run(name: &str, args: Value) -> Value {
    lessons::run(name, &args).unwrap_or_else(|e| panic!("lesson {name}: {e}"))
}

fn bytes(v: &Value) -> Vec<u8> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as u8)
        .collect()
}

#[test]
fn diff_segments_tile_the_patch_and_provenance_is_consistent() {
    let d = run("diff", json!({}));
    let raw = bytes(&d["raw"]);
    let mut next = 0;
    for s in d["segments"].as_array().unwrap() {
        assert_eq!(s["start"].as_u64().unwrap(), next);
        next = s["end"].as_u64().unwrap();
    }
    assert_eq!(next as usize, raw.len());
    let (old, new) = (bytes(&d["old"]), bytes(&d["new"]));
    let kind = bytes(&d["kind"]);
    for (i, src) in d["src"].as_array().unwrap().iter().enumerate() {
        let src = src.as_i64().unwrap();
        match kind[i] {
            0 => assert_eq!(new[i], old[src as usize]),
            1 => assert_ne!(new[i], old[src as usize]),
            _ => assert_eq!(src, -1),
        }
    }
    // Custom texts work too.
    let d = run("diff", json!({ "old": "hello world", "new": "hello brave new world" }));
    assert!(d["stats"]["extraBytes"].as_u64().unwrap() >= 10);
}

#[test]
fn suffix_search_finds_the_longest_match() {
    let s = run("suffix", json!({ "word": "banana", "query": "anab" }));
    let texts: Vec<&str> = s["suffixes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["", "a", "ana", "anana", "banana", "na", "nana"]);
    assert_eq!(s["match"]["len"], 3);
    assert_eq!(s["match"]["text"], "ana");
    assert!(!s["steps"].as_array().unwrap().is_empty());
}

#[test]
fn compress_shows_mostly_zero_diff_bytes() {
    let c = run("compress", json!({ "scale": "real" }));
    let raw = c["rawLen"].as_u64().unwrap();
    assert!(
        c["zeros"].as_u64().unwrap() * 2 > raw,
        "diff stream should be mostly zero bytes"
    );
    assert!(c["framed"].as_u64().unwrap() < raw / 5);
    assert!(c["single"].as_u64().unwrap() < raw / 5);
}

#[test]
fn slice_layout_matches_parameters() {
    let s = run("slice", json!({ "shardSize": 16, "k": 4, "m": 2 }));
    let len = s["patch"].as_array().unwrap().len();
    let groups = s["groups"].as_array().unwrap();
    assert_eq!(groups.len(), len.div_ceil(64));
    let total: u64 = groups.iter().map(|g| g["len"].as_u64().unwrap()).sum();
    assert_eq!(total as usize, len);
    assert!(lessons::run("slice", &json!({ "shardSize": 0 })).is_err());
}

#[test]
fn reed_solomon_recovers_up_to_m_losses_only() {
    let ok = run(
        "rs",
        json!({ "text": "OTA rocks! v1.1", "k": 4, "m": 2, "lost": [0, 5] }),
    );
    assert_eq!(ok["decode"]["ok"], true);
    assert_eq!(ok["decode"]["matches"], true);
    assert_eq!(ok["decode"]["text"], "OTA rocks! v1.1");
    assert_eq!(ok["decode"]["chosen"], json!([1, 2, 3, 4]));
    // Each worked parity byte equals the XOR of its products.
    for term in ok["terms"].as_array().unwrap() {
        let x = term["parts"]
            .as_array()
            .unwrap()
            .iter()
            .fold(0u64, |acc, p| acc ^ p["p"].as_u64().unwrap());
        assert_eq!(x, term["result"].as_u64().unwrap());
    }
    let fail = run("rs", json!({ "k": 4, "m": 2, "lost": [0, 1, 2] }));
    assert_eq!(fail["decode"]["ok"], false);
    assert_eq!(fail["decode"]["have"], 3);
}

#[test]
fn signature_detects_every_tampering() {
    assert_eq!(run("sign", json!({ "action": "none" }))["verified"], true);
    for action in ["fw", "hash", "key", "resign"] {
        let s = run("sign", json!({ "action": action }));
        assert_eq!(s["verified"], false, "{action} must not verify");
    }
    let fw = run("sign", json!({ "action": "fw" }));
    let changed: Vec<&str> = fw["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["changed"] == true)
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(changed, ["fwVersion"]);
}

#[test]
fn one_flipped_bit_changes_about_half_the_hash() {
    let h = run("hash", json!({ "text": "firmware v1.1.0", "bit": 3 }));
    let bits = h["differingBits"].as_u64().unwrap();
    assert!((80..=176).contains(&bits), "{bits}");
    assert_eq!(run("hash", json!({ "bit": -1 }))["differingBits"], 0);
}

#[test]
fn interleaving_spreads_a_burst_across_groups() {
    let args = |w: u64| json!({ "shardSize": 16, "k": 4, "m": 2, "interleave": w, "burstStart": 0, "burstLen": 6 });
    let ok_groups = |v: &Value| {
        v["perGroup"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|g| g["ok"] == true)
            .count()
    };
    let one = run("interleave", args(1));
    let six = run("interleave", args(6));
    let groups = one["groups"].as_u64().unwrap() as usize;
    assert_eq!(
        ok_groups(&one),
        groups - 1,
        "a 6-packet burst kills one group when sent group by group"
    );
    assert_eq!(
        ok_groups(&six),
        groups,
        "interleaving spreads the burst so every group survives"
    );
}

#[test]
fn crc_rejects_every_single_bit_flip() {
    assert_eq!(run("crc", json!({ "bit": -1 }))["accepted"], true);
    let len = run("crc", json!({}))["bytes"].as_array().unwrap().len();
    for bit in (0..len * 8).step_by(37) {
        let c = run("crc", json!({ "bit": bit }));
        assert_eq!(c["accepted"], false, "bit {bit}");
    }
}

#[test]
fn install_only_with_matching_hash() {
    assert_eq!(run("install", json!({}))["ok"], true);
    assert_eq!(run("install", json!({ "corrupt": 10 }))["ok"], false);
    assert!(lessons::run("nope", &json!({})).is_err());
}
