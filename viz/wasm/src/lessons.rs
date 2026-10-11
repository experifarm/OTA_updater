//! Interactive lessons for the guided chapters. Every lesson runs real `ota_updater` code on small
//! inputs and returns JSON for the page to draw; nothing here re-implements an algorithm.

use crate::sim::demo_key;
use crate::toy;
use ota_updater::diff;
use ota_updater::manifest::{self, Manifest};
use ota_updater::patch::{Compression, HEADER_LEN};
use ota_updater::rs::ReedSolomon;
use ota_updater::sender::{self, Prepared, Schedule, SenderConfig};
use ota_updater::slicer::{FecParams, Layout};
use ota_updater::suffix::suffix_array;
use ota_updater::varint;
use ota_updater::wire::Packet;
use ota_updater::{gf256, imagegen, sha256, to_hex};
use serde_json::{Value, json};

type Lesson = Result<Value, String>;

fn arg_str<'a>(a: &'a Value, key: &str, default: &'a str) -> &'a str {
    a.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn arg_u64(a: &Value, key: &str, default: u64) -> u64 {
    a.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn arg_i64(a: &Value, key: &str, default: i64) -> i64 {
    a.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Dispatch a lesson by name.
pub fn run(name: &str, args: &Value) -> Lesson {
    match name {
        "diff" => lesson_diff(args),
        "suffix" => lesson_suffix(args),
        "compress" => lesson_compress(args),
        "slice" => lesson_slice(args),
        "rs" => lesson_rs(args),
        "sign" => lesson_sign(args),
        "hash" => lesson_hash(args),
        "interleave" => lesson_interleave(args),
        "crc" => lesson_crc(args),
        "install" => lesson_install(args),
        other => Err(format!("unknown lesson `{other}`")),
    }
}

const TOY_FEC: FecParams = FecParams {
    shard_size: 16,
    k: 4,
    m: 2,
};

/// The toy update as the sandbox's toy scale prepares it (seed 1, uncompressed patch).
fn toy_prepared(fec: FecParams) -> Result<Prepared, String> {
    let (old, new) = toy::images();
    sender::prepare(
        &old,
        &new,
        &demo_key("ota-viz publisher key", 1),
        &SenderConfig {
            fw_version: 2,
            fec,
            compression: Compression::None,
            level: 1,
            session_id: Some(0x5e55_0001),
            zstd_frame_size: None,
        },
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- chapter 2: diff

fn lesson_diff(a: &Value) -> Lesson {
    let (toy_old, toy_new) = toy::images();
    let old = a
        .get("old")
        .and_then(Value::as_str)
        .map_or(toy_old, |s| s.as_bytes().to_vec());
    let new = a
        .get("new")
        .and_then(Value::as_str)
        .map_or(toy_new, |s| s.as_bytes().to_vec());
    if old.len() > 8192 || new.len() > 8192 {
        return Err("texts are limited to 8 KiB".into());
    }
    let mut records = Vec::new();
    let (raw, stats) = diff::diff_raw_traced(&old, &new, &mut |r| records.push(r));

    // Annotate the real patch stream by parsing it: header, then per record control varints,
    // diff bytes and extra bytes.
    let mut segments = vec![json!({ "kind": "header", "start": 0, "end": HEADER_LEN })];
    let mut pos = HEADER_LEN;
    while pos < raw.len() {
        let mut cur = &raw[pos..];
        let before = cur.len();
        let diff_len = varint::read_u64(&mut cur).map_err(|e| e.to_string())? as usize;
        let extra_len = varint::read_u64(&mut cur).map_err(|e| e.to_string())? as usize;
        let seek = varint::unzigzag(varint::read_u64(&mut cur).map_err(|e| e.to_string())?);
        let ctrl = before - cur.len();
        segments.push(json!({
            "kind": "ctrl", "start": pos, "end": pos + ctrl,
            "diffLen": diff_len, "extraLen": extra_len, "seek": seek,
        }));
        pos += ctrl;
        if diff_len > 0 {
            segments.push(json!({ "kind": "diff", "start": pos, "end": pos + diff_len }));
            pos += diff_len;
        }
        if extra_len > 0 {
            segments.push(json!({ "kind": "extra", "start": pos, "end": pos + extra_len }));
            pos += extra_len;
        }
    }

    // Provenance of every new byte: 0 copied unchanged, 1 copied and adjusted, 2 brand new.
    let mut kind = vec![2u8; new.len()];
    let mut src = vec![-1i64; new.len()];
    for r in &records {
        for j in 0..r.diff_len {
            let ni = (r.new_off + j) as usize;
            let oi = (r.old_off + j) as usize;
            src[ni] = oi as i64;
            kind[ni] = u8::from(new[ni] != old[oi]);
        }
    }
    Ok(json!({
        "old": old,
        "new": new,
        "raw": raw,
        "segments": segments,
        "records": records.iter().map(|r| json!({
            "newOff": r.new_off, "oldOff": r.old_off, "diffLen": r.diff_len, "extraLen": r.extra_len,
        })).collect::<Vec<_>>(),
        "kind": kind,
        "src": src,
        "stats": {
            "records": stats.records, "diffBytes": stats.diff_bytes,
            "extraBytes": stats.extra_bytes, "rawLen": stats.raw_len,
        },
    }))
}

fn lesson_suffix(a: &Value) -> Lesson {
    let word = arg_str(a, "word", "banana").as_bytes();
    let query = arg_str(a, "query", "ana").as_bytes();
    if word.len() > 40 || query.len() > 40 {
        return Err("word and query are limited to 40 bytes".into());
    }
    let sa = suffix_array(word);
    let mut steps = Vec::new();
    let (len, pos) = diff::search_traced(&sa, word, query, |s| {
        steps.push(json!({ "lo": s.lo, "hi": s.hi, "mid": s.mid, "goRight": s.go_right }));
    });
    Ok(json!({
        "suffixes": sa.iter().map(|&p| json!({ "pos": p, "text": text(&word[p as usize..]) })).collect::<Vec<_>>(),
        "steps": steps,
        "match": { "len": len, "pos": pos, "text": text(&word[pos..pos + len]) },
    }))
}

// ---------------------------------------------------------------- chapter 3: compress

fn lesson_compress(a: &Value) -> Lesson {
    let real = arg_str(a, "scale", "real") == "real";
    let (old, new) = if real {
        imagegen::generate_pair(1 << 20, 1)
    } else {
        toy::images()
    };
    let (raw, stats) = diff::diff_raw(&old, &new);
    let single = diff::compress_zstd(&raw, 1).map_err(|e| e.to_string())?.len();
    let framed = diff::compress_zstd_framed(&raw, 1, 16 << 10)
        .map_err(|e| e.to_string())?
        .len();
    let zeros = raw.iter().filter(|&&b| b == 0).count();
    let mut hist = vec![0u64; 256];
    for &b in &raw {
        hist[b as usize] += 1;
    }
    // Byte map: share of non-zero bytes per cell (0..=255).
    let cells = raw.len().clamp(1, 4096);
    let per = raw.len().div_ceil(cells);
    let map: Vec<u8> = raw
        .chunks(per.max(1))
        .map(|c| ((c.iter().filter(|&&b| b != 0).count() * 255) / c.len()) as u8)
        .collect();
    // A window of diff bytes (typical content of the stream).
    let sample_at = (HEADER_LEN + 4).min(raw.len());
    let sample = raw[sample_at..(sample_at + 64).min(raw.len())].to_vec();
    Ok(json!({
        "newSize": new.len(),
        "rawLen": raw.len(),
        "zeros": zeros,
        "single": single,
        "framed": framed,
        "extraBytes": stats.extra_bytes,
        "map": map,
        "cellBytes": per,
        "hist": hist,
        "sample": sample,
        "sampleAt": sample_at,
    }))
}

// ---------------------------------------------------------------- chapter 4: slice

fn fec_from(a: &Value) -> Result<FecParams, String> {
    let fec = FecParams {
        shard_size: arg_u64(a, "shardSize", 16) as u16,
        k: arg_u64(a, "k", 4) as u16,
        m: arg_u64(a, "m", 2) as u16,
    };
    fec.validate()?;
    Ok(fec)
}

fn lesson_slice(a: &Value) -> Lesson {
    let (old, new) = toy::images();
    let (raw, _) = diff::diff_raw(&old, &new);
    let fec = fec_from(a)?;
    let layout = Layout::new(fec, raw.len() as u64);
    let mut packets = 0;
    let groups: Vec<Value> = (0..layout.group_count() as u32)
        .map(|g| {
            let r = layout.group_range(g);
            let real = layout.real_data_shards(g);
            packets += real + fec.m();
            json!({
                "start": r.start, "len": r.end - r.start, "real": real,
                "pad": real * fec.shard_size() - (r.end - r.start) as usize,
                "implicit": fec.k() - real,
            })
        })
        .collect();
    Ok(json!({
        "patch": raw,
        "shardSize": fec.shard_size, "k": fec.k, "m": fec.m,
        "groups": groups,
        "packets": packets,
    }))
}

// ---------------------------------------------------------------- chapter 5: Reed-Solomon

fn lesson_rs(a: &Value) -> Lesson {
    let msg = arg_str(a, "text", "OTA rocks! v1.1").as_bytes();
    if msg.is_empty() || msg.len() > 64 {
        return Err("text must be 1..=64 bytes".into());
    }
    let k = arg_u64(a, "k", 4).clamp(1, 8) as usize;
    let m = arg_u64(a, "m", 2).min(6) as usize;
    let rs = ReedSolomon::new(k, m).map_err(|e| e.to_string())?;
    let n = k + m;
    let len = msg.len().div_ceil(k);
    let data: Vec<Vec<u8>> = (0..k)
        .map(|i| {
            let mut s = vec![0u8; len];
            let start = (i * len).min(msg.len());
            let end = ((i + 1) * len).min(msg.len());
            s[..end - start].copy_from_slice(&msg[start..end]);
            s
        })
        .collect();
    let parity = rs.encode(&data).map_err(|e| e.to_string())?;
    let generator: Vec<Vec<u8>> = (0..n).map(|i| rs.generator_row(i)).collect();

    // One byte column worked out: parity_i[col] = XOR_j C[i][j] · data_j[col] in GF(2^8).
    let col = (arg_u64(a, "col", 0) as usize).min(len - 1);
    let terms: Vec<Value> = (0..m)
        .map(|i| {
            let row = &generator[k + i];
            let parts: Vec<Value> = (0..k)
                .map(|j| json!({ "c": row[j], "d": data[j][col], "p": gf256::mul(row[j], data[j][col]) }))
                .collect();
            json!({ "row": k + i, "parts": parts, "result": parity[i][col] })
        })
        .collect();

    // Erasures chosen by the user, then the real decoder.
    let lost: Vec<usize> = a
        .get("lost")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(Value::as_u64)
                .map(|x| x as usize)
                .filter(|&x| x < n)
                .collect()
        })
        .unwrap_or_default();
    let all: Vec<Vec<u8>> = data.iter().chain(&parity).cloned().collect();
    let mut shards: Vec<Option<Vec<u8>>> = (0..n).map(|i| (!lost.contains(&i)).then(|| all[i].clone())).collect();
    let present: Vec<usize> = (0..n).filter(|i| !lost.contains(i)).collect();
    let decode = if present.len() >= k {
        let chosen = present[..k].to_vec();
        let (matrix, inverse) = rs.decode_matrix(&chosen).ok_or("decode matrix")?;
        let missing: Vec<usize> = (0..k).filter(|i| lost.contains(i)).collect();
        rs.reconstruct(&mut shards).map_err(|e| e.to_string())?;
        let rebuilt: Vec<Vec<u8>> = shards[..k].iter().map(|s| s.clone().unwrap_or_default()).collect();
        let joined: Vec<u8> = rebuilt.concat();
        json!({
            "ok": true, "chosen": chosen, "matrix": matrix, "inverse": inverse, "missing": missing,
            "data": rebuilt, "text": text(&joined[..msg.len()]), "matches": joined[..msg.len()] == *msg,
        })
    } else {
        json!({ "ok": false, "have": present.len(), "need": k })
    };
    Ok(json!({
        "k": k, "m": m, "len": len, "data": data, "parity": parity, "generator": generator,
        "col": col, "terms": terms, "lost": lost, "decode": decode,
    }))
}

// ---------------------------------------------------------------- chapter 6: sign

fn manifest_fields(m: &Manifest) -> Vec<(String, String)> {
    vec![
        ("session".into(), format!("{:016x}", m.session_id)),
        ("fwVersion".into(), m.fw_version.to_string()),
        ("oldSize".into(), m.old_size.to_string()),
        ("oldSha".into(), to_hex(&m.old_sha256)),
        ("newSize".into(), m.new_size.to_string()),
        ("newSha".into(), to_hex(&m.new_sha256)),
        ("patchSize".into(), m.patch_size.to_string()),
        ("patchSha".into(), to_hex(&m.patch_sha256)),
        (
            "fec".into(),
            format!("{} B shards, k={}, m={}", m.fec.shard_size, m.fec.k, m.fec.m),
        ),
        (
            "groups".into(),
            m.group_hashes
                .iter()
                .map(|h| to_hex(&h[..4]))
                .collect::<Vec<_>>()
                .join(" "),
        ),
    ]
}

fn lesson_sign(a: &Value) -> Lesson {
    let action = arg_str(a, "action", "none");
    let publisher = demo_key("ota-viz publisher key", 1);
    let attacker = demo_key("ota-viz attacker key", 1);
    let p = toy_prepared(TOY_FEC)?;
    let original = p.manifest.clone();
    let orig_body = original.encode();
    let orig_sig = p.container[p.container.len() - 64..].to_vec();
    let split = |c: Vec<u8>| c[c.len() - 64..].to_vec();
    let (shown, sig) = match action {
        "none" => (original.clone(), orig_sig.clone()),
        // Edit a field but keep the publisher's signature.
        "fw" => (
            Manifest {
                fw_version: 99,
                ..original.clone()
            },
            orig_sig.clone(),
        ),
        "hash" => {
            let mut m = original.clone();
            m.new_sha256[0] ^= 0x01;
            (m, orig_sig.clone())
        }
        // The attacker signs the genuine manifest, or a tampered one, with their own key.
        "key" => (original.clone(), split(original.sign(&attacker))),
        "resign" => {
            let m = Manifest {
                fw_version: 99,
                ..original.clone()
            };
            let sig = split(m.sign(&attacker));
            (m, sig)
        }
        other => return Err(format!("unknown action `{other}`")),
    };
    let body = shown.encode();
    let mut container = (body.len() as u32).to_le_bytes().to_vec();
    container.extend_from_slice(&body);
    container.extend_from_slice(&sig);
    let verdict = manifest::verify_and_decode(&container, &publisher.verifying_key());
    let changed: Vec<usize> = (0..body.len().min(orig_body.len()))
        .filter(|&i| body[i] != orig_body[i])
        .collect();
    let orig_fields = manifest_fields(&original);
    let fields: Vec<Value> = manifest_fields(&shown)
        .into_iter()
        .zip(orig_fields)
        .map(|((name, value), (_, was))| json!({ "name": name, "value": value, "changed": value != was }))
        .collect();
    Ok(json!({
        "action": action,
        "fields": fields,
        "body": to_hex(&body),
        "changed": changed,
        "signature": to_hex(&sig),
        "sigChanged": sig != orig_sig,
        "publicKey": to_hex(publisher.verifying_key().as_bytes()),
        "verified": verdict.is_ok(),
        "error": verdict.err().map(|e| e.to_string()),
    }))
}

fn lesson_hash(a: &Value) -> Lesson {
    let msg = arg_str(a, "text", "firmware v1.1.0").as_bytes().to_vec();
    if msg.len() > 256 {
        return Err("text is limited to 256 bytes".into());
    }
    let bit = arg_i64(a, "bit", -1);
    let mut flipped = msg.clone();
    if bit >= 0 && (bit as usize) < msg.len() * 8 {
        flipped[bit as usize / 8] ^= 1 << (bit as usize % 8);
    }
    let (h1, h2) = (sha256(&msg), sha256(&flipped));
    let differing: u32 = h1.iter().zip(&h2).map(|(x, y)| (x ^ y).count_ones()).sum();
    Ok(json!({
        "text": text(&msg), "flipped": text(&flipped), "bytes": msg, "flippedBytes": flipped,
        "h1": to_hex(&h1), "h2": to_hex(&h2), "differingBits": differing,
    }))
}

// ---------------------------------------------------------------- chapter 7: broadcast

fn lesson_interleave(a: &Value) -> Lesson {
    let fec = fec_from(a)?;
    let p = toy_prepared(fec)?;
    let layout = p.layout();
    let schedule = Schedule {
        interleave: arg_u64(a, "interleave", 1).max(1) as usize,
        manifest_every: 0,
    };
    let order: Vec<(u32, u16)> = p
        .round_packets(&schedule)
        .iter()
        .filter_map(|d| match Packet::decode(d) {
            Ok(Packet::Shard { group, index, .. }) => Some((group, index)),
            _ => None,
        })
        .collect();
    let start = arg_u64(a, "burstStart", 0) as usize;
    let len = arg_u64(a, "burstLen", 0) as usize;
    let lost: Vec<bool> = (0..order.len()).map(|i| i >= start && i < start + len).collect();
    let groups = layout.group_count() as usize;
    let per_group: Vec<Value> = (0..groups)
        .map(|g| {
            let sent = order.iter().filter(|(x, _)| *x as usize == g).count();
            let lost_n = order
                .iter()
                .zip(&lost)
                .filter(|((x, _), l)| *x as usize == g && **l)
                .count();
            let implicit = fec.k() - layout.real_data_shards(g as u32);
            json!({ "sent": sent, "lost": lost_n, "ok": sent - lost_n + implicit >= fec.k() })
        })
        .collect();
    Ok(json!({
        "groups": groups, "k": fec.k, "m": fec.m,
        "order": order.iter().map(|(g, i)| [*g as u64, u64::from(*i)]).collect::<Vec<_>>(),
        "lost": lost, "perGroup": per_group,
    }))
}

// ---------------------------------------------------------------- chapter 8: receive

fn lesson_crc(a: &Value) -> Lesson {
    let p = toy_prepared(TOY_FEC)?;
    let original = Packet::Shard {
        session_id: p.manifest.session_id,
        group: 0,
        index: 0,
        data: p.shards[0][0].clone(),
    }
    .encode();
    let mut bytes = original.clone();
    let bit = arg_i64(a, "bit", -1);
    if bit >= 0 && (bit as usize) < bytes.len() * 8 {
        bytes[bit as usize / 8] ^= 1 << (bit as usize % 8);
    }
    let body_end = bytes.len() - 4;
    let stored = u32::from_le_bytes(bytes[body_end..].try_into().expect("4 bytes"));
    let computed = crc32fast::hash(&bytes[..body_end]);
    let decoded = Packet::decode(&bytes);
    let fields = [
        ("magic", 0, 4),
        ("version", 4, 5),
        ("kind", 5, 6),
        ("reserved", 6, 8),
        ("session", 8, 16),
        ("group", 16, 20),
        ("index", 20, 22),
        ("length", 22, 24),
        ("data", 24, body_end),
        ("crc32", body_end, bytes.len()),
    ];
    Ok(json!({
        "bytes": bytes,
        "original": original,
        "fields": fields.iter().map(|(n, s, e)| json!({ "name": n, "start": s, "end": e })).collect::<Vec<_>>(),
        "stored": stored,
        "computed": computed,
        "accepted": decoded.is_ok(),
        "error": decoded.err().map(|e| e.to_string()),
    }))
}

// ---------------------------------------------------------------- chapter 9: install

fn lesson_install(a: &Value) -> Lesson {
    let p = toy_prepared(TOY_FEC)?;
    let (old, new) = toy::images();
    let mut slot_b = new.clone();
    let corrupt = arg_i64(a, "corrupt", -1);
    if corrupt >= 0 && (corrupt as usize) < slot_b.len() {
        slot_b[corrupt as usize] ^= 0x20;
    }
    let actual = sha256(&slot_b);
    Ok(json!({
        "slotA": text(&old),
        "slotB": text(&slot_b),
        "expected": to_hex(&p.manifest.new_sha256),
        "actual": to_hex(&actual),
        "ok": actual == p.manifest.new_sha256,
        "fwVersion": p.manifest.fw_version,
    }))
}
