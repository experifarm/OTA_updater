//! Synthetic firmware image pairs for demos and tests.
//!
//! The old image mimics a firmware layout (header, low-entropy "machine code", a relocation-sensitive
//! pointer table, a string table, calibration data, 0xFF erase padding). The new image applies the
//! kinds of changes a real release makes: patched instructions, an inserted function (which shifts all
//! later code and therefore relocates pointers), a removed function, edited strings and data.

use crate::channel::SplitMix64;

const CODE_BASE: u32 = 0x0800_0000;
const HEADER_LEN: usize = 256;
const OPCODES: [u8; 24] = [
    0x10, 0x11, 0x12, 0x18, 0x20, 0x21, 0x28, 0x2a, 0x30, 0x31, 0x38, 0x40, 0x41, 0x48, 0x50, 0x58, 0x60, 0x68, 0x70,
    0x78, 0x80, 0x90, 0xa0, 0xb0,
];
const WORDS: [&str; 24] = [
    "sensor",
    "error",
    "timeout",
    "config",
    "battery",
    "voltage",
    "motor",
    "calibration",
    "update",
    "network",
    "invalid",
    "state",
    "packet",
    "retry",
    "buffer",
    "overflow",
    "firmware",
    "watchdog",
    "reset",
    "flash",
    "checksum",
    "init",
    "ready",
    "failed",
];

#[derive(Clone)]
struct Firmware {
    version: (u8, u8, u8),
    code: Vec<u32>,
    /// Pointer table: word indices into `code` (rendered as absolute addresses).
    targets: Vec<usize>,
    strings: Vec<String>,
    data: Vec<u8>,
}

fn instruction(rng: &mut SplitMix64) -> u32 {
    let op = OPCODES[rng.below(OPCODES.len() as u64) as usize] as u32;
    let rd = rng.below(8) as u32;
    let rs = rng.below(8) as u32;
    // Immediates are small most of the time, as in real code.
    let imm = if rng.chance(0.8) {
        rng.below(64)
    } else {
        rng.below(1 << 16)
    } as u32;
    op << 24 | rd << 20 | rs << 16 | imm
}

fn sentence(rng: &mut SplitMix64) -> String {
    let n = 2 + rng.below(5) as usize;
    let words: Vec<&str> = (0..n).map(|_| WORDS[rng.below(WORDS.len() as u64) as usize]).collect();
    format!("{}: {}", rng.below(1000), words.join(" "))
}

impl Firmware {
    fn generate(size: usize, rng: &mut SplitMix64) -> Self {
        let code = (0..size * 55 / 100 / 4).map(|_| instruction(rng)).collect::<Vec<_>>();
        let targets = (0..size * 10 / 100 / 16)
            .map(|_| rng.below(code.len().max(1) as u64) as usize)
            .collect();
        let mut strings = vec!["firmware v1.0.0 (build 1000)".to_string()];
        let mut len = 0;
        while len < size * 12 / 100 {
            let s = sentence(rng);
            len += s.len() + 1;
            strings.push(s);
        }
        let data = (0..size * 8 / 100)
            .map(|i| (i as u8).wrapping_mul(3) ^ rng.below(4) as u8)
            .collect();
        Self {
            version: (1, 0, 0),
            code,
            targets,
            strings,
            data,
        }
    }

    fn render(&self, min_size: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(min_size);
        out.extend_from_slice(b"FWIMG\0");
        out.extend([self.version.0, self.version.1, self.version.2, 0]);
        out.extend((self.code.len() as u32).to_le_bytes());
        out.extend((self.targets.len() as u32).to_le_bytes());
        out.extend((self.strings.len() as u32).to_le_bytes());
        out.resize(HEADER_LEN, 0);
        for w in &self.code {
            out.extend(w.to_le_bytes());
        }
        for (i, &t) in self.targets.iter().enumerate() {
            out.extend((CODE_BASE + (HEADER_LEN + 4 * t) as u32).to_le_bytes());
            out.extend((i as u32).to_le_bytes());
            out.extend((0x0001_0000u32 | (i as u32 % 7)).to_le_bytes());
            out.extend(((t as u32).wrapping_mul(2_654_435_761) >> 8).to_le_bytes());
        }
        for s in &self.strings {
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        out.extend_from_slice(&self.data);
        let size = out.len().max(min_size).next_multiple_of(4096);
        out.resize(size, 0xff);
        out
    }

    fn next_release(&self, rng: &mut SplitMix64) -> Self {
        let mut fw = self.clone();
        fw.version = (1, 1, 0);
        // Bug fixes: ~1% of instructions change.
        for _ in 0..fw.code.len() / 100 {
            let i = rng.below(fw.code.len() as u64) as usize;
            fw.code[i] = instruction(rng);
        }
        // A new function inserted mid-image: everything after it moves, pointers are relocated.
        let at = fw.code.len() * 2 / 5;
        let inserted: Vec<u32> = (0..fw.code.len() / 50).map(|_| instruction(rng)).collect();
        let n_ins = inserted.len();
        fw.code.splice(at..at, inserted);
        for t in &mut fw.targets {
            if *t >= at {
                *t += n_ins;
            }
        }
        // A function removed later in the image.
        let del_at = fw.code.len() * 4 / 5;
        let n_del = (fw.code.len() / 200).min(fw.code.len() - del_at);
        fw.code.drain(del_at..del_at + n_del);
        for t in &mut fw.targets {
            if *t >= del_at + n_del {
                *t -= n_del;
            } else if *t >= del_at {
                *t = del_at;
            }
        }
        // New pointer-table entries.
        for _ in 0..fw.targets.len() / 50 {
            fw.targets.push(rng.below(fw.code.len() as u64) as usize);
        }
        // Strings: version bump, a few edits, some new messages.
        fw.strings[0] = "firmware v1.1.0 (build 1042)".to_string();
        for _ in 0..fw.strings.len() / 30 {
            let i = 1 + rng.below(fw.strings.len() as u64 - 1) as usize;
            fw.strings[i] = sentence(rng);
        }
        for _ in 0..20 {
            fw.strings.push(sentence(rng));
        }
        // Calibration data: ~5% of bytes tweaked.
        for _ in 0..fw.data.len() / 20 {
            let i = rng.below(fw.data.len() as u64) as usize;
            fw.data[i] = fw.data[i].wrapping_add(1 + rng.below(3) as u8);
        }
        fw
    }
}

/// Generate an `(old, new)` firmware image pair of roughly `size` bytes.
pub fn generate_pair(size: usize, seed: u64) -> (Vec<u8>, Vec<u8>) {
    let size = size.max(4096);
    let mut rng = SplitMix64::new(seed);
    let old_fw = Firmware::generate(size - HEADER_LEN, &mut rng);
    let new_fw = old_fw.next_release(&mut rng);
    (old_fw.render(size), new_fw.render(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_different() {
        let (a_old, a_new) = generate_pair(64 * 1024, 5);
        let (b_old, b_new) = generate_pair(64 * 1024, 5);
        assert_eq!(a_old, b_old);
        assert_eq!(a_new, b_new);
        assert_ne!(a_old, a_new);
        assert!(a_old.len() >= 64 * 1024 && a_new.len() >= 64 * 1024);
        assert_eq!(&a_old[..6], b"FWIMG\0");
        let (c_old, _) = generate_pair(64 * 1024, 6);
        assert_ne!(a_old, c_old);
    }
}
