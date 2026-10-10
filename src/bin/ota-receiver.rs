//! OTA receiver: listens for shards, repairs losses with Reed-Solomon, streams the patch into a new
//! image and installs it only after Ed25519-signed hash verification.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ota_updater::channel::ImpairmentConfig;
use ota_updater::net::{self, MulticastConfig, MulticastReceiver};
use ota_updater::patch::{self, Compression};
use ota_updater::receiver::{self, Event, ReceiverConfig, Report};
use ota_updater::{keys, to_hex};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ota-receiver",
    version,
    about = "OTA update receiver: listen, FEC-repair, stream-patch, verify, install"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Join the multicast group and receive one update.
    Listen(ListenArgs),
    /// Apply a patch file offline.
    Apply {
        #[arg(long)]
        old: PathBuf,
        #[arg(long)]
        patch: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// The patch stream is not compressed.
        #[arg(long)]
        no_compress: bool,
    },
}

#[derive(Args)]
struct ListenArgs {
    /// Currently installed (base) image.
    #[arg(long)]
    old: PathBuf,
    /// Where to install the new image.
    #[arg(long)]
    out: PathBuf,
    /// Pinned Ed25519 public key of the publisher (hex), e.g. keys/ota_signing.pub.
    #[arg(long)]
    pubkey: PathBuf,
    /// Installed firmware version; only newer updates are accepted (anti-rollback).
    #[arg(long, default_value_t = 0)]
    installed_version: u64,
    #[arg(long, default_value_t = net::DEFAULT_GROUP)]
    group: Ipv4Addr,
    #[arg(long, default_value_t = net::DEFAULT_PORT)]
    port: u16,
    /// Interface address to join on (127.0.0.1 = loopback simulation).
    #[arg(long, default_value_t = net::DEFAULT_IFACE)]
    iface: Ipv4Addr,
    /// Give up after this many seconds (0 = wait forever).
    #[arg(long, default_value_t = 120)]
    timeout: u64,
    /// Simulated packet loss probability.
    #[arg(long, default_value_t = 0.0)]
    loss: f64,
    /// Mean loss-burst length in packets (> 1 enables Gilbert-Elliott burst loss).
    #[arg(long, default_value_t = 1.0)]
    burst_len: f64,
    /// Simulated single-bit corruption probability (caught by CRC32).
    #[arg(long, default_value_t = 0.0)]
    corrupt: f64,
    /// Simulated duplication probability.
    #[arg(long, default_value_t = 0.0)]
    dup: f64,
    /// Simulated reordering probability.
    #[arg(long, default_value_t = 0.0)]
    reorder: f64,
    /// Seed of the impairment simulator.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Label used in log lines.
    #[arg(long, default_value = "rx")]
    name: String,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Listen(a) => listen(a),
        Cmd::Apply {
            old,
            patch: patch_path,
            out,
            no_compress,
        } => {
            let mut old_file = BufReader::new(File::open(&old).with_context(|| format!("opening {}", old.display()))?);
            let old_len = old_file.get_ref().metadata()?.len();
            let patch_file = File::open(&patch_path).with_context(|| format!("opening {}", patch_path.display()))?;
            let comp = if no_compress {
                Compression::None
            } else {
                Compression::Zstd
            };
            let mut w = ota_updater::hashio::HashingWriter::new(BufWriter::new(File::create(&out)?));
            let stats = patch::apply_stream(&mut old_file, old_len, patch_file, comp, &mut w)?;
            let (_, sha, len) = w.finish();
            println!(
                "{}: {len} bytes, sha256 {} ({} records, {} diff bytes, {} extra bytes)",
                out.display(),
                to_hex(&sha),
                stats.records,
                stats.diff_bytes,
                stats.extra_bytes
            );
            Ok(())
        }
    }
}

fn listen(a: ListenArgs) -> Result<()> {
    let public_key = keys::load_verifying_key(&a.pubkey)?;
    let mcast = MulticastConfig {
        group: a.group,
        port: a.port,
        iface: a.iface,
        ttl: 1,
    };
    let mut source =
        MulticastReceiver::new(&mcast).with_context(|| format!("joining {}:{} on {}", a.group, a.port, a.iface))?;
    let impairment = ImpairmentConfig {
        loss: a.loss,
        burst_len: a.burst_len,
        corrupt: a.corrupt,
        dup: a.dup,
        reorder: a.reorder,
        seed: a.seed,
        ..Default::default()
    };
    impairment.validate().map_err(anyhow::Error::msg)?;
    let mut cfg = ReceiverConfig::new(public_key);
    cfg.installed_version = a.installed_version;
    cfg.timeout = (a.timeout > 0).then(|| Duration::from_secs(a.timeout));
    cfg.impairment = Some(impairment);

    let name = a.name.clone();
    println!(
        "[{name}] listening on {}:{} via {} (rcvbuf {} KiB); base {} ; impairment loss={} burst={} corrupt={} dup={} reorder={}",
        a.group,
        a.port,
        a.iface,
        source.recv_buffer_size().unwrap_or(0) / 1024,
        a.old.display(),
        a.loss,
        a.burst_len,
        a.corrupt,
        a.dup,
        a.reorder
    );
    let mut last_decile = 0u32;
    let mut on_event = |e: Event<'_>| match e {
        Event::ManifestAccepted(m) => println!(
            "[{name}] manifest verified (Ed25519): session {:016x}, fw v{}, patch {} bytes in {} groups (k={} m={}), new image {} bytes",
            m.session_id,
            m.fw_version,
            m.patch_size,
            m.group_count(),
            m.fec.k,
            m.fec.m,
            m.new_size
        ),
        Event::ManifestSignatureInvalid { session_id } => {
            println!("[{name}] manifest for session {session_id:016x} failed signature verification; ignored")
        }
        Event::SessionRejected { session_id, reason } => {
            println!("[{name}] rejected session {session_id:016x}: {reason}")
        }
        Event::GroupHashMismatch { group } => {
            println!("[{name}] group {group} failed its signed hash; collecting it again")
        }
        Event::GroupComplete { completed, total, .. } => {
            let decile = completed * 10 / total.max(1);
            if decile > last_decile {
                last_decile = decile;
                println!("[{name}] {completed}/{total} groups reconstructed ({}%)", decile * 10);
            }
        }
    };
    let report = receiver::receive(&mut source, &a.old, &a.out, &cfg, &mut on_event)?;
    print_report(&name, &report);
    Ok(())
}

fn print_report(name: &str, r: &Report) {
    let s = &r.stats;
    let ms = |d: Option<Duration>| d.map_or("-".to_string(), |d| format!("{:.0} ms", d.as_secs_f64() * 1000.0));
    println!("[{name}] installed {} ({} bytes)", r.output.display(), r.apply.new_size);
    println!("[{name}]   sha256 {} == signed manifest", to_hex(&r.new_sha256));
    println!(
        "[{name}]   datagrams {} | simulated: dropped {} corrupted {} duplicated {} reordered {}",
        s.impairment.seen.max(s.datagrams),
        s.impairment.dropped,
        s.impairment.corrupted,
        s.impairment.duplicated,
        s.impairment.reordered
    );
    println!(
        "[{name}]   bad packets {} | redundant shards {} | FEC: {} groups repaired, {} data shards rebuilt | hash failures {}",
        s.bad_packets, s.redundant_shards, s.groups_repaired, s.shards_recovered, s.group_hash_failures
    );
    println!(
        "[{name}]   timeline: manifest {} | first patch bytes to patcher {} | last group {} | done {:.0} ms (max {} groups waiting for in-order release)",
        ms(s.manifest_at),
        ms(s.first_group_at),
        ms(s.last_group_at),
        r.elapsed.as_secs_f64() * 1000.0,
        s.max_pending_groups
    );
}
