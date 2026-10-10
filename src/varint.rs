//! LEB128 unsigned varints and zigzag encoding for signed values.

use std::io::{self, Read};

/// Append `v` as an LEB128 varint (1..=10 bytes).
pub fn write_u64(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Read an LEB128 varint. Rejects encodings longer than 10 bytes or overflowing 64 bits.
pub fn read_u64<R: Read + ?Sized>(r: &mut R) -> io::Result<u64> {
    let mut result = 0u64;
    for i in 0..10 {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        let byte = b[0];
        let payload = u64::from(byte & 0x7f);
        if i == 9 && payload > 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "varint overflows u64"));
        }
        result |= payload << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "varint longer than 10 bytes",
    ))
}

/// Map signed to unsigned so small magnitudes stay small: 0,-1,1,-2,… → 0,1,2,3,…
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for v in [
            0u64,
            1,
            127,
            128,
            255,
            300,
            16_383,
            16_384,
            u32::MAX as u64,
            u64::MAX - 1,
            u64::MAX,
        ] {
            let mut buf = Vec::new();
            write_u64(&mut buf, v);
            assert!(buf.len() <= 10);
            assert_eq!(read_u64(&mut buf.as_slice()).unwrap(), v);
        }
        for v in [0i64, -1, 1, -2, 2, i64::MIN, i64::MAX, -123_456_789] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
    }

    #[test]
    fn rejects_bad_encodings() {
        let overlong = [0x80u8; 11];
        assert!(read_u64(&mut overlong.as_slice()).is_err());
        let overflow = [0xffu8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert!(read_u64(&mut overflow.as_slice()).is_err());
        let truncated = [0x80u8];
        assert_eq!(
            read_u64(&mut truncated.as_slice()).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
