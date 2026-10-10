//! I/O adapters that compute SHA-256 over everything passing through them.

use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

/// Writer that hashes and counts all bytes written to the inner writer.
pub struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    len: u64,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `(inner, sha256, length)`.
    pub fn finish(self) -> (W, [u8; 32], u64) {
        (self.inner, self.hasher.finalize().into(), self.len)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Reader that hashes and counts all bytes read from the inner reader.
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    len: u64,
}

impl<R: Read> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            len: 0,
        }
    }

    /// Returns `(sha256, length)` of everything read so far.
    pub fn digest(&self) -> ([u8; 32], u64) {
        (self.hasher.clone().finalize().into(), self.len)
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }
}

/// SHA-256 and length of a whole stream.
pub fn hash_reader<R: Read>(mut r: R) -> io::Result<([u8; 32], u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((hasher.finalize().into(), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_and_reader_agree() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 7) as u8).collect();
        let mut w = HashingWriter::new(Vec::new());
        w.write_all(&data).unwrap();
        let (inner, digest, len) = w.finish();
        assert_eq!(inner, data);
        assert_eq!(len, data.len() as u64);
        assert_eq!(digest, crate::sha256(&data));

        let mut r = HashingReader::new(data.as_slice());
        let mut sink = Vec::new();
        r.read_to_end(&mut sink).unwrap();
        assert_eq!(r.digest(), (digest, len));
        assert_eq!(hash_reader(data.as_slice()).unwrap(), (digest, len));
    }
}
