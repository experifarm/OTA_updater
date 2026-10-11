//! OTA sender: differencing, slicing, Reed-Solomon encoding, Ed25519 signing and UDP multicast broadcast.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ota_updater::net::{self, MulticastConfig, MulticastSender};
use ota_updater::patch::{self, Compression};
use ota_updater::sender::{self, BroadcastConfig, Schedule, SenderConfig};
use ota_updater::slicer::FecParams;
use ota_updater::{diff, imagegen, keys};
use std::fs;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "ota-sender",
    version,
    about = "OTA update sender: diff, slice, FEC-encode, sign and broadcast"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate an Ed25519 signing key pair (ota_signing.key / ota_signing.pub).
    Keygen {
        #[arg(long, default_value = "keys")]
        out_dir: PathBuf,
        /// Overwrite existing key files.
        #[arg(long)]
        force: bool,
    },
    /// Generate a synthetic old/new firmware image pair (old.img, new.img).
    GenImages {
        #[arg(long, default_value = "images")]
        out_dir: PathBuf,
        /// Approximate image size in bytes.
        #[arg(long, default_value_t = 4 << 20)]
        size: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Create a patch file offline (and verify it round-trips).
    Diff {
        #[arg(long)]
        old: PathBuf,
        #[arg(long)]
        new: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// zstd compression level.
        #[arg(long, default_value_t = 19)]
        level: i32,
        /// Do not compress the patch stream.
        #[arg(long)]
        no_compress: bool,
    },
    /// Prepare an update and broadcast it over UDP multicast.
    Send(SendArgs),
}

#[derive(Args)]
struct SendArgs {
    /// Base image the receivers currently run.
    #[arg(long)]
    old: PathBuf,
    /// New image to deliver.
    #[arg(long)]
    new: PathBuf,
    /// Ed25519 signing key (hex), e.g. keys/ota_signing.key.
    #[arg(long)]
    key: PathBuf,
    /// Firmware version of the new image (receivers refuse versions <= installed).
    #[arg(long)]
    fw_version: u64,
    #[arg(long, default_value_t = net::DEFAULT_GROUP)]
    group: Ipv4Addr,
    #[arg(long, default_value_t = net::DEFAULT_PORT)]
    port: u16,
    /// Interface address to send from (127.0.0.1 = loopback simulation).
    #[arg(long, default_value_t = net::DEFAULT_IFACE)]
    iface: Ipv4Addr,
    #[arg(long, default_value_t = 1)]
    ttl: u32,
    /// Shard payload size in bytes.
    #[arg(long, default_value_t = 1024)]
    shard_size: u16,
    /// Data shards per FEC group.
    #[arg(short = 'k', long = "data-shards", default_value_t = 32)]
    k: u16,
    /// Parity shards per FEC group.
    #[arg(short = 'm', long = "parity-shards", default_value_t = 8)]
    m: u16,
    /// Number of groups whose shards are interleaved.
    #[arg(long, default_value_t = 8)]
    interleave: usize,
    /// Repeat the manifest every N shard packets.
    #[arg(long, default_value_t = 256)]
    manifest_every: usize,
    /// Carousel rounds to broadcast.
    #[arg(long, default_value_t = 3)]
    rounds: u32,
    /// Broadcast until interrupted.
    #[arg(long)]
    forever: bool,
    /// Send rate limit in kbit/s (0 = unlimited).
    #[arg(long, default_value_t = 20_000)]
    rate_kbps: u64,
    /// Pause between rounds in milliseconds.
    #[arg(long, default_value_t = 50)]
    round_gap_ms: u64,
    /// Wait before the first packet (gives receivers time to join).
    #[arg(long, default_value_t = 0)]
    start_delay_ms: u64,
    /// zstd compression level.
    #[arg(long, default_value_t = 19)]
    level: i32,
    #[arg(long)]
    no_compress: bool,
    /// Fixed session id (random by default).
    #[arg(long)]
    session_id: Option<u64>,
}

fn compression(no_compress: bool) -> Compression {
    if no_compress {
        Compression::None
    } else {
        Compression::Zstd
    }
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Keygen { out_dir, force } => {
            let key = keys::generate()?;
            let (secret, public) = keys::save_keypair(&out_dir, &key, force)?;
            println!("signing key : {}", secret.display());
            println!(
                "public key  : {} ({})",
                public.display(),
                ota_updater::to_hex(key.verifying_key().as_bytes())
            );
        }
        Cmd::GenImages { out_dir, size, seed } => {
            let (old, new) = imagegen::generate_pair(size, seed);
            fs::create_dir_all(&out_dir)?;
            let (op, np) = (out_dir.join("old.img"), out_dir.join("new.img"));
            fs::write(&op, &old)?;
            fs::write(&np, &new)?;
            println!(
                "{} ({} bytes, sha256 {})",
                op.display(),
                old.len(),
                ota_updater::to_hex(&ota_updater::sha256(&old))
            );
            println!(
                "{} ({} bytes, sha256 {})",
                np.display(),
                new.len(),
                ota_updater::to_hex(&ota_updater::sha256(&new))
            );
        }
        Cmd::Diff {
            old,
            new,
            out,
            level,
            no_compress,
        } => {
            let old_data = fs::read(&old).with_context(|| format!("reading {}", old.display()))?;
            let new_data = fs::read(&new).with_context(|| format!("reading {}", new.display()))?;
            let start = Instant::now();
            let comp = compression(no_compress);
            let (p, stats) = diff::make_patch(&old_data, &new_data, comp, level)?;
            let took = start.elapsed();
            let check = patch::apply_to_vec(&old_data, &p, comp)?;
            anyhow::ensure!(check == new_data, "patch verification failed");
            fs::write(&out, &p)?;
            println!(
                "{}: {} bytes ({:.2}% of new image, raw {} bytes, {} records, {} extra bytes) in {:.2?}; verified",
                out.display(),
                p.len(),
                100.0 * p.len() as f64 / new_data.len().max(1) as f64,
                stats.raw_len,
                stats.records,
                stats.extra_bytes,
                took
            );
        }
        Cmd::Send(a) => send(a)?,
    }
    Ok(())
}

fn send(a: SendArgs) -> Result<()> {
    let old = fs::read(&a.old).with_context(|| format!("reading {}", a.old.display()))?;
    let new = fs::read(&a.new).with_context(|| format!("reading {}", a.new.display()))?;
    let key = keys::load_signing_key(&a.key)?;
    let cfg = SenderConfig {
        fw_version: a.fw_version,
        fec: FecParams {
            shard_size: a.shard_size,
            k: a.k,
            m: a.m,
        },
        compression: compression(a.no_compress),
        level: a.level,
        session_id: a.session_id,
        zstd_frame_size: None,
    };
    println!("preparing update (diff + compress + FEC + sign)...");
    let prepared = sender::prepare(&old, &new, &key, &cfg)?;
    println!("{}", prepared.summary());

    let mcast = MulticastConfig {
        group: a.group,
        port: a.port,
        iface: a.iface,
        ttl: a.ttl,
    };
    let mut sink = MulticastSender::new(&mcast, a.rate_kbps)
        .with_context(|| format!("opening multicast sender on {} via {}", a.group, a.iface))?;
    let bcfg = BroadcastConfig {
        rounds: (!a.forever).then_some(a.rounds),
        schedule: Schedule {
            interleave: a.interleave,
            manifest_every: a.manifest_every,
        },
        round_gap: Duration::from_millis(a.round_gap_ms),
    };
    if a.start_delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(a.start_delay_ms));
    }
    println!(
        "broadcasting to {}:{} via {} at {} kbit/s ({})",
        a.group,
        a.port,
        a.iface,
        a.rate_kbps,
        if a.forever {
            "forever".to_string()
        } else {
            format!("{} rounds", a.rounds)
        }
    );
    let start = Instant::now();
    let stats = sender::broadcast(&prepared, &bcfg, &mut sink, &mut |round, s| {
        println!(
            "  round {round}: {} datagrams, {:.2} MiB total, {:.2?}",
            s.datagrams,
            s.bytes as f64 / (1 << 20) as f64,
            start.elapsed()
        );
    })?;
    println!(
        "done: {} rounds, {} datagrams, {:.2} MiB in {:.2?}",
        stats.rounds,
        stats.datagrams,
        stats.bytes as f64 / (1 << 20) as f64,
        start.elapsed()
    );
    Ok(())
}
