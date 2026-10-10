//! Arithmetic in GF(2^8) with the primitive polynomial x⁸+x⁴+x³+x²+1 (`0x11D`), generator 2.
//!
//! Addition is XOR; multiplication/division use exp/log tables built at compile time.

/// Reduction polynomial x⁸+x⁴+x³+x²+1.
pub const POLY: u16 = 0x11D;

struct Tables {
    /// `exp[i] = 2^i`, doubled in length so `exp[log a + log b]` needs no modulo.
    exp: [u8; 512],
    /// `log[a]` for a ≠ 0 (`log[0]` is unused).
    log: [u8; 256],
}

const fn build_tables() -> Tables {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    let mut i = 0;
    while i < 255 {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= POLY;
        }
        i += 1;
    }
    while i < 512 {
        exp[i] = exp[i - 255];
        i += 1;
    }
    Tables { exp, log }
}

static TABLES: Tables = build_tables();

/// a + b (= a − b) in GF(2^8).
#[inline]
pub fn add(a: u8, b: u8) -> u8 {
    a ^ b
}

/// a · b in GF(2^8).
#[inline]
pub fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        TABLES.exp[TABLES.log[a as usize] as usize + TABLES.log[b as usize] as usize]
    }
}

/// Multiplicative inverse. Panics on 0 (which has no inverse).
#[inline]
pub fn inv(a: u8) -> u8 {
    assert!(a != 0, "0 has no multiplicative inverse in GF(2^8)");
    TABLES.exp[255 - TABLES.log[a as usize] as usize]
}

/// a / b in GF(2^8). Panics if b == 0.
#[inline]
pub fn div(a: u8, b: u8) -> u8 {
    assert!(b != 0, "division by zero in GF(2^8)");
    if a == 0 {
        0
    } else {
        TABLES.exp[TABLES.log[a as usize] as usize + 255 - TABLES.log[b as usize] as usize]
    }
}

/// 2^e (the generator raised to `e`).
#[inline]
pub fn exp(e: usize) -> u8 {
    TABLES.exp[e % 255]
}

/// The 256-entry row `c · x` for every x, used for bulk slice operations.
#[inline]
pub fn mul_row(c: u8) -> [u8; 256] {
    let mut row = [0u8; 256];
    if c != 0 {
        let lc = TABLES.log[c as usize] as usize;
        for (x, slot) in row.iter_mut().enumerate().skip(1) {
            *slot = TABLES.exp[lc + TABLES.log[x] as usize];
        }
    }
    row
}

/// `dst[i] ^= c · src[i]` for all i (multiply-accumulate). Slices must have equal length.
pub fn mul_slice_xor(c: u8, src: &[u8], dst: &mut [u8]) {
    assert_eq!(src.len(), dst.len(), "mul_slice_xor length mismatch");
    match c {
        0 => {}
        1 => dst.iter_mut().zip(src).for_each(|(d, s)| *d ^= s),
        _ => {
            let row = mul_row(c);
            dst.iter_mut().zip(src).for_each(|(d, s)| *d ^= row[*s as usize]);
        }
    }
}

/// `dst[i] = c · src[i]` for all i. Slices must have equal length.
pub fn mul_slice(c: u8, src: &[u8], dst: &mut [u8]) {
    assert_eq!(src.len(), dst.len(), "mul_slice length mismatch");
    match c {
        0 => dst.fill(0),
        1 => dst.copy_from_slice(src),
        _ => {
            let row = mul_row(c);
            dst.iter_mut().zip(src).for_each(|(d, s)| *d = row[*s as usize]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carry-less "Russian peasant" multiply, independent of the tables.
    fn slow_mul(mut a: u8, mut b: u8) -> u8 {
        let mut p = 0u8;
        while b != 0 {
            if b & 1 != 0 {
                p ^= a;
            }
            let carry = a & 0x80 != 0;
            a <<= 1;
            if carry {
                a ^= (POLY & 0xff) as u8;
            }
            b >>= 1;
        }
        p
    }

    #[test]
    fn generator_is_primitive() {
        let mut seen = [false; 256];
        for e in 0..255 {
            let v = exp(e);
            assert!(!seen[v as usize], "2^{e} repeats");
            seen[v as usize] = true;
        }
        assert!(!seen[0]);
        assert_eq!(exp(255), 1);
    }

    #[test]
    fn mul_matches_reference_exhaustively() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                assert_eq!(mul(a, b), slow_mul(a, b), "{a} * {b}");
            }
        }
    }

    #[test]
    fn field_axioms() {
        for a in 0..=255u8 {
            assert_eq!(mul(a, 1), a);
            assert_eq!(mul(a, 0), 0);
            assert_eq!(add(a, a), 0);
            if a != 0 {
                assert_eq!(mul(a, inv(a)), 1, "inverse of {a}");
                for b in 1..=255u8 {
                    assert_eq!(mul(div(a, b), b), a);
                }
            }
        }
        // Distributivity and associativity on a sample.
        for a in (0..=255u8).step_by(7) {
            for b in (0..=255u8).step_by(11) {
                for c in (0..=255u8).step_by(13) {
                    assert_eq!(mul(a, add(b, c)), add(mul(a, b), mul(a, c)));
                    assert_eq!(mul(mul(a, b), c), mul(a, mul(b, c)));
                }
            }
        }
    }

    #[test]
    fn slice_ops() {
        let src: Vec<u8> = (0..=255).collect();
        for c in [0u8, 1, 2, 0x53, 0xff] {
            let mut dst = vec![0x5au8; 256];
            mul_slice_xor(c, &src, &mut dst);
            for (i, &d) in dst.iter().enumerate() {
                assert_eq!(d, 0x5a ^ mul(c, i as u8));
            }
            let mut out = vec![0u8; 256];
            mul_slice(c, &src, &mut out);
            for (i, &d) in out.iter().enumerate() {
                assert_eq!(d, mul(c, i as u8));
            }
        }
    }
}
