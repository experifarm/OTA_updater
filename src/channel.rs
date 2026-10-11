//! Network impairment simulator applied per receiver (模拟广播信道).
//!
//! Deterministic (seeded SplitMix64). For every datagram: loss (Bernoulli or Gilbert-Elliott burst
//! loss), single-bit corruption, duplication and bounded reordering.

use std::collections::VecDeque;

/// SplitMix64 PRNG: tiny, fast, good enough for simulation (not for cryptography).
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [0, n). `n` must be > 0.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    pub fn chance(&mut self, p: f64) -> bool {
        p > 0.0 && self.next_f64() < p
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpairmentConfig {
    /// Long-run packet loss probability.
    pub loss: f64,
    /// Mean loss-burst length in packets; values > 1 select the Gilbert-Elliott model.
    pub burst_len: f64,
    /// Probability of flipping one random bit.
    pub corrupt: f64,
    /// Probability of delivering a packet twice.
    pub dup: f64,
    /// Probability of holding a packet back for 1..=`reorder_depth` subsequent packets.
    pub reorder: f64,
    pub reorder_depth: u32,
    pub seed: u64,
}

impl Default for ImpairmentConfig {
    fn default() -> Self {
        Self {
            loss: 0.0,
            burst_len: 1.0,
            corrupt: 0.0,
            dup: 0.0,
            reorder: 0.0,
            reorder_depth: 8,
            seed: 1,
        }
    }
}

impl ImpairmentConfig {
    pub fn is_noop(&self) -> bool {
        self.loss <= 0.0 && self.corrupt <= 0.0 && self.dup <= 0.0 && self.reorder <= 0.0
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, p) in [
            ("loss", self.loss),
            ("corrupt", self.corrupt),
            ("dup", self.dup),
            ("reorder", self.reorder),
        ] {
            if !(0.0..=1.0).contains(&p) {
                return Err(format!("{name} must be a probability in [0, 1], got {p}"));
            }
        }
        if self.loss >= 1.0 && self.burst_len > 1.0 {
            return Err("burst loss requires loss < 1".into());
        }
        if self.burst_len.is_nan() || self.burst_len < 1.0 {
            return Err(format!("burst length must be >= 1, got {}", self.burst_len));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImpairmentStats {
    pub seen: u64,
    pub dropped: u64,
    pub corrupted: u64,
    pub duplicated: u64,
    pub reordered: u64,
}

/// What the channel did to one datagram (for visualization).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fate {
    pub lost: bool,
    /// Bit index flipped in transit (caught by the CRC at the receiver).
    pub corrupted_bit: Option<u64>,
    pub duplicated: bool,
    /// Number of copies held back (reordered); they are delivered with a later datagram.
    pub delayed: u8,
}

pub struct Impairment {
    cfg: ImpairmentConfig,
    rng: SplitMix64,
    bad_state: bool,
    held: VecDeque<(u32, u64, Vec<u8>)>,
    scratch: Vec<(u64, Vec<u8>)>,
    pub stats: ImpairmentStats,
}

impl Impairment {
    pub fn new(cfg: ImpairmentConfig) -> Self {
        Self {
            rng: SplitMix64::new(cfg.seed),
            cfg,
            bad_state: false,
            held: VecDeque::new(),
            scratch: Vec::new(),
            stats: ImpairmentStats::default(),
        }
    }

    fn lose(&mut self) -> bool {
        if self.cfg.burst_len > 1.0 && self.cfg.loss > 0.0 {
            // Gilbert-Elliott: the bad state drops everything; mean sojourn in it is `burst_len`, and
            // the stationary probability of the bad state equals `loss`.
            let p_bad_to_good = 1.0 / self.cfg.burst_len;
            let p_good_to_bad = p_bad_to_good * self.cfg.loss / (1.0 - self.cfg.loss);
            let flip = if self.bad_state { p_bad_to_good } else { p_good_to_bad };
            if self.rng.chance(flip) {
                self.bad_state = !self.bad_state;
            }
            self.bad_state
        } else {
            self.rng.chance(self.cfg.loss)
        }
    }

    /// Feed one datagram; append the datagrams to deliver now to `out`.
    pub fn process(&mut self, pkt: Vec<u8>, out: &mut Vec<Vec<u8>>) {
        let mut tagged = std::mem::take(&mut self.scratch);
        self.process_tagged(0, pkt, &mut tagged);
        out.extend(tagged.drain(..).map(|(_, p)| p));
        self.scratch = tagged;
    }

    /// Like [`Impairment::process`], but every datagram carries a caller-chosen `tag` so deliveries
    /// can be matched to what was sent; returns the fate of this datagram.
    pub fn process_tagged(&mut self, tag: u64, mut pkt: Vec<u8>, out: &mut Vec<(u64, Vec<u8>)>) -> Fate {
        let mut fate = Fate::default();
        self.stats.seen += 1;
        if self.lose() {
            self.stats.dropped += 1;
            fate.lost = true;
        } else {
            if !pkt.is_empty() && self.rng.chance(self.cfg.corrupt) {
                let bit = self.rng.below(pkt.len() as u64 * 8);
                pkt[(bit / 8) as usize] ^= 1 << (bit % 8);
                self.stats.corrupted += 1;
                fate.corrupted_bit = Some(bit);
            }
            let copies = if self.rng.chance(self.cfg.dup) {
                self.stats.duplicated += 1;
                fate.duplicated = true;
                2
            } else {
                1
            };
            for c in 0..copies {
                let p = if c + 1 == copies {
                    std::mem::take(&mut pkt)
                } else {
                    pkt.clone()
                };
                if self.cfg.reorder_depth > 0 && self.rng.chance(self.cfg.reorder) {
                    let delay = 1 + self.rng.below(u64::from(self.cfg.reorder_depth)) as u32;
                    self.held.push_back((delay, tag, p));
                    self.stats.reordered += 1;
                    fate.delayed += 1;
                } else {
                    out.push((tag, p));
                }
            }
        }
        // Age held packets; the ones that are due are released *after* newer packets.
        let mut i = 0;
        while i < self.held.len() {
            self.held[i].0 -= 1;
            if self.held[i].0 == 0 {
                let (_, t, p) = self.held.remove(i).expect("index in range");
                out.push((t, p));
            } else {
                i += 1;
            }
        }
        fate
    }

    /// Release every held-back datagram (end of stream).
    pub fn flush(&mut self, out: &mut Vec<Vec<u8>>) {
        out.extend(self.held.drain(..).map(|(_, _, p)| p));
    }

    /// Release every held-back datagram with its tag (end of stream).
    pub fn flush_tagged(&mut self, out: &mut Vec<(u64, Vec<u8>)>) {
        out.extend(self.held.drain(..).map(|(_, t, p)| (t, p)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cfg: ImpairmentConfig, n: u32) -> (Impairment, Vec<Vec<u8>>) {
        let mut imp = Impairment::new(cfg);
        let mut out = Vec::new();
        for i in 0..n {
            imp.process(i.to_le_bytes().to_vec(), &mut out);
        }
        imp.flush(&mut out);
        (imp, out)
    }

    #[test]
    fn noop_passes_everything_in_order() {
        let (imp, out) = run(ImpairmentConfig::default(), 1000);
        assert_eq!(out.len(), 1000);
        assert!(out.iter().enumerate().all(|(i, p)| p == &(i as u32).to_le_bytes()));
        assert_eq!(imp.stats.dropped, 0);
    }

    #[test]
    fn bernoulli_loss_rate() {
        let cfg = ImpairmentConfig {
            loss: 0.1,
            seed: 7,
            ..Default::default()
        };
        let (imp, out) = run(cfg, 100_000);
        let rate = imp.stats.dropped as f64 / 100_000.0;
        assert!((rate - 0.1).abs() < 0.005, "loss rate {rate}");
        assert_eq!(out.len() as u64, 100_000 - imp.stats.dropped);
    }

    #[test]
    fn gilbert_elliott_burst_statistics() {
        let cfg = ImpairmentConfig {
            loss: 0.2,
            burst_len: 5.0,
            seed: 11,
            ..Default::default()
        };
        let mut imp = Impairment::new(cfg);
        let mut out = Vec::new();
        let (mut bursts, mut in_burst) = (0u64, false);
        for i in 0..200_000u32 {
            let before = imp.stats.dropped;
            imp.process(i.to_le_bytes().to_vec(), &mut out);
            let lost = imp.stats.dropped > before;
            if lost && !in_burst {
                bursts += 1;
            }
            in_burst = lost;
        }
        let rate = imp.stats.dropped as f64 / 200_000.0;
        let mean_burst = imp.stats.dropped as f64 / bursts as f64;
        assert!((rate - 0.2).abs() < 0.02, "loss rate {rate}");
        assert!((mean_burst - 5.0).abs() < 0.5, "mean burst {mean_burst}");
    }

    #[test]
    fn corruption_dup_and_reorder() {
        let cfg = ImpairmentConfig {
            corrupt: 0.05,
            dup: 0.05,
            reorder: 0.05,
            seed: 3,
            ..Default::default()
        };
        let (imp, out) = run(cfg, 20_000);
        assert_eq!(out.len() as u64, 20_000 + imp.stats.duplicated);
        assert!(imp.stats.corrupted > 0 && imp.stats.duplicated > 0 && imp.stats.reordered > 0);
        // Every delivered packet is either intact or differs by exactly one bit.
        let mut intact = std::collections::HashSet::new();
        let mut one_bit = 0;
        for p in &out {
            let v = u32::from_le_bytes(p[..4].try_into().unwrap());
            if v < 20_000 {
                intact.insert(v);
            } else {
                one_bit += 1;
            }
        }
        assert!(one_bit as u64 <= imp.stats.corrupted);
        // Out-of-order delivery actually happened.
        let values: Vec<u32> = out
            .iter()
            .map(|p| u32::from_le_bytes(p[..4].try_into().unwrap()))
            .collect();
        assert!(values.windows(2).any(|w| w[1] < w[0]));
    }

    #[test]
    fn deterministic_for_same_seed() {
        let cfg = ImpairmentConfig {
            loss: 0.3,
            burst_len: 3.0,
            corrupt: 0.1,
            dup: 0.1,
            reorder: 0.1,
            seed: 99,
            ..Default::default()
        };
        assert_eq!(run(cfg, 5000).1, run(cfg, 5000).1);
    }

    #[test]
    fn tagged_fates_match_deliveries() {
        let cfg = ImpairmentConfig {
            loss: 0.2,
            burst_len: 3.0,
            corrupt: 0.05,
            dup: 0.05,
            reorder: 0.1,
            seed: 21,
            ..Default::default()
        };
        let mut tagged = Impairment::new(cfg);
        let mut plain = Impairment::new(cfg);
        let (mut out_t, mut out_p) = (Vec::new(), Vec::new());
        let mut expected_copies = std::collections::HashMap::new();
        for i in 0..10_000u64 {
            let pkt = i.to_le_bytes().to_vec();
            let fate = tagged.process_tagged(i, pkt.clone(), &mut out_t);
            plain.process(pkt, &mut out_p);
            if !fate.lost {
                expected_copies.insert(i, if fate.duplicated { 2 } else { 1 });
            }
            if let Some(bit) = fate.corrupted_bit {
                assert!(bit < 64);
            }
        }
        tagged.flush_tagged(&mut out_t);
        plain.flush(&mut out_p);
        // Same RNG consumption as the untagged path: identical deliveries.
        assert_eq!(out_t.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(), out_p);
        assert_eq!(tagged.stats, plain.stats);
        // Every surviving packet is delivered with its own tag, the right number of times.
        let mut copies = std::collections::HashMap::new();
        for (tag, _) in &out_t {
            *copies.entry(*tag).or_insert(0) += 1;
        }
        assert_eq!(copies, expected_copies);
    }

    #[test]
    fn validation() {
        assert!(ImpairmentConfig::default().validate().is_ok());
        let bad = ImpairmentConfig {
            loss: 1.5,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let bad = ImpairmentConfig {
            burst_len: 0.5,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
    }
}
