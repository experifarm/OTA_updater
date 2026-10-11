//! Streaming patch application.
//!
//! Patch stream layout (all integers little endian, see `docs/DESIGN.md` §3.1):
//!
//! ```text
//! header : "OTAPATCH" | version u16 | old_size u64 | new_size u64
//! record*: diff_len varint | extra_len varint | seek zigzag-varint | diff bytes | extra bytes
//! ```
//!
//! [`StreamingPatcher`] is a push-based state machine: patch bytes can be fed in arbitrary pieces as
//! they arrive (e.g. one FEC group at a time) and new-image bytes are written immediately, using only
//! `O(CHUNK)` memory. [`apply`] is the pull-based convenience wrapper around it.

use crate::varint;
use std::io::{self, Read, Seek, SeekFrom, Write};
use thiserror::Error;

pub const MAGIC: &[u8; 8] = b"OTAPATCH";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 8 + 2 + 8 + 8;
const CHUNK: usize = 64 * 1024;

/// Compression applied to the whole patch stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Zstd = 1,
}

impl Compression {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Zstd),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum PatchError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("not an OTA patch (bad magic)")]
    BadMagic,
    #[error("unsupported patch version {0}")]
    UnsupportedVersion(u16),
    #[error("patch is for an old image of {expected} bytes, but the old image has {actual} bytes")]
    OldSizeMismatch { expected: u64, actual: u64 },
    #[error("patch stream is truncated")]
    Truncated,
    #[error("malformed patch: {0}")]
    Malformed(&'static str),
    #[error("decompression failed: {0}")]
    Decompress(String),
}

fn map_eof(e: io::Error) -> PatchError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        PatchError::Truncated
    } else {
        PatchError::Io(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchHeader {
    pub old_size: u64,
    pub new_size: u64,
}

impl PatchHeader {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[..8].copy_from_slice(MAGIC);
        h[8..10].copy_from_slice(&VERSION.to_le_bytes());
        h[10..18].copy_from_slice(&self.old_size.to_le_bytes());
        h[18..26].copy_from_slice(&self.new_size.to_le_bytes());
        h
    }

    pub fn read<R: Read + ?Sized>(r: &mut R) -> Result<Self, PatchError> {
        let mut h = [0u8; HEADER_LEN];
        r.read_exact(&mut h).map_err(map_eof)?;
        if &h[..8] != MAGIC {
            return Err(PatchError::BadMagic);
        }
        let version = u16::from_le_bytes([h[8], h[9]]);
        if version != VERSION {
            return Err(PatchError::UnsupportedVersion(version));
        }
        Ok(Self {
            old_size: u64::from_le_bytes(h[10..18].try_into().unwrap()),
            new_size: u64::from_le_bytes(h[18..26].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyStats {
    pub records: u64,
    pub diff_bytes: u64,
    pub extra_bytes: u64,
    pub new_size: u64,
}

/// A piece of new image written by the patcher (for visualization).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOp {
    /// `new[new_off..+len] = old[old_off..+len] + diff`.
    Diff { new_off: u64, old_off: u64, len: u64 },
    /// `new[new_off..+len]` copied verbatim from the patch.
    Extra { new_off: u64, len: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Header,
    Record,
    Diff,
    Extra,
    Done,
}

/// Push-based patch applier.
pub struct StreamingPatcher {
    old_len: u64,
    state: State,
    /// Pending header or record-header bytes.
    buf: Vec<u8>,
    header: Option<PatchHeader>,
    produced: u64,
    /// Logical cursor in the old image.
    old_pos: i64,
    /// Actual position of the old image reader, if known.
    file_pos: Option<u64>,
    /// Bytes left in the current diff or extra section.
    left: u64,
    extra_len: u64,
    seek: i64,
    obuf: Vec<u8>,
    wbuf: Vec<u8>,
    stats: ApplyStats,
}

/// Parse `diff_len | extra_len | seek` from `buf`; `None` if more bytes are needed.
fn parse_record_header(buf: &[u8]) -> Result<Option<(u64, u64, i64)>, PatchError> {
    let mut r = buf;
    let mut fields = [0u64; 3];
    for f in &mut fields {
        *f = match varint::read_u64(&mut r) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(_) => return Err(PatchError::Malformed("invalid varint in record header")),
        };
    }
    Ok(Some((fields[0], fields[1], varint::unzigzag(fields[2]))))
}

impl StreamingPatcher {
    pub fn new(old_len: u64) -> Self {
        Self {
            old_len,
            state: State::Header,
            buf: Vec::with_capacity(HEADER_LEN),
            header: None,
            produced: 0,
            old_pos: 0,
            file_pos: None,
            left: 0,
            extra_len: 0,
            seek: 0,
            obuf: Vec::new(),
            wbuf: Vec::new(),
            stats: ApplyStats::default(),
        }
    }

    pub fn header(&self) -> Option<PatchHeader> {
        self.header
    }

    /// New-image bytes written so far.
    pub fn produced(&self) -> u64 {
        self.produced
    }

    pub fn is_complete(&self) -> bool {
        self.state == State::Done
    }

    /// Feed the next piece of the (uncompressed) patch stream. `old` is sought as needed (it may be a
    /// different reader, or one moved by the caller, on every call); new-image bytes are written to
    /// `out` immediately and reported to `on_op`.
    pub fn feed<O, W>(
        &mut self,
        mut input: &[u8],
        old: &mut O,
        out: &mut W,
        on_op: &mut dyn FnMut(PatchOp),
    ) -> Result<(), PatchError>
    where
        O: Read + Seek + ?Sized,
        W: Write + ?Sized,
    {
        // The reader's position is only trusted within one call.
        self.file_pos = None;
        while !input.is_empty() {
            match self.state {
                State::Header => {
                    let take = (HEADER_LEN - self.buf.len()).min(input.len());
                    self.buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if self.buf.len() == HEADER_LEN {
                        let header = PatchHeader::read(&mut self.buf.as_slice())?;
                        if header.old_size != self.old_len {
                            return Err(PatchError::OldSizeMismatch {
                                expected: header.old_size,
                                actual: self.old_len,
                            });
                        }
                        self.header = Some(header);
                        self.buf.clear();
                        self.state = if header.new_size == 0 {
                            State::Done
                        } else {
                            State::Record
                        };
                    }
                }
                State::Record => {
                    self.buf.push(input[0]);
                    input = &input[1..];
                    if let Some((diff_len, extra_len, seek)) = parse_record_header(&self.buf)? {
                        self.buf.clear();
                        self.begin_record(diff_len, extra_len, seek)?;
                    }
                }
                State::Diff => {
                    let n = self.left.min(input.len() as u64).min(CHUNK as u64) as usize;
                    let pos = self.old_pos as u64; // range-checked in begin_record
                    if self.file_pos != Some(pos) {
                        old.seek(SeekFrom::Start(pos))?;
                    }
                    self.obuf.resize(n, 0);
                    old.read_exact(&mut self.obuf).map_err(|e| {
                        if e.kind() == io::ErrorKind::UnexpectedEof {
                            PatchError::Malformed("old image shorter than declared")
                        } else {
                            PatchError::Io(e)
                        }
                    })?;
                    self.wbuf.clear();
                    self.wbuf
                        .extend(input[..n].iter().zip(&self.obuf).map(|(d, o)| d.wrapping_add(*o)));
                    out.write_all(&self.wbuf)?;
                    on_op(PatchOp::Diff {
                        new_off: self.produced,
                        old_off: pos,
                        len: n as u64,
                    });
                    input = &input[n..];
                    self.produced += n as u64;
                    self.old_pos += n as i64;
                    self.file_pos = Some(pos + n as u64);
                    self.left -= n as u64;
                    if self.left == 0 {
                        self.enter_extra()?;
                    }
                }
                State::Extra => {
                    let n = self.left.min(input.len() as u64) as usize;
                    out.write_all(&input[..n])?;
                    on_op(PatchOp::Extra {
                        new_off: self.produced,
                        len: n as u64,
                    });
                    input = &input[n..];
                    self.produced += n as u64;
                    self.left -= n as u64;
                    if self.left == 0 {
                        self.end_record()?;
                    }
                }
                State::Done => return Err(PatchError::Malformed("trailing data after the last record")),
            }
        }
        Ok(())
    }

    fn begin_record(&mut self, diff_len: u64, extra_len: u64, seek: i64) -> Result<(), PatchError> {
        let new_size = self.header.expect("header parsed").new_size;
        let remaining = new_size - self.produced;
        if diff_len > remaining || extra_len > remaining - diff_len {
            return Err(PatchError::Malformed("record exceeds the declared new image size"));
        }
        if diff_len > 0 {
            let start =
                u64::try_from(self.old_pos).map_err(|_| PatchError::Malformed("diff starts before the old image"))?;
            if start.checked_add(diff_len).is_none_or(|end| end > self.old_len) {
                return Err(PatchError::Malformed("diff reads past the end of the old image"));
            }
        }
        self.extra_len = extra_len;
        self.seek = seek;
        self.stats.records += 1;
        self.stats.diff_bytes += diff_len;
        self.stats.extra_bytes += extra_len;
        if diff_len > 0 {
            self.state = State::Diff;
            self.left = diff_len;
            Ok(())
        } else {
            self.enter_extra()
        }
    }

    fn enter_extra(&mut self) -> Result<(), PatchError> {
        if self.extra_len > 0 {
            self.state = State::Extra;
            self.left = self.extra_len;
            Ok(())
        } else {
            self.end_record()
        }
    }

    fn end_record(&mut self) -> Result<(), PatchError> {
        self.old_pos = self
            .old_pos
            .checked_add(self.seek)
            .ok_or(PatchError::Malformed("old cursor overflow"))?;
        let new_size = self.header.expect("header parsed").new_size;
        self.state = if self.produced == new_size {
            State::Done
        } else {
            State::Record
        };
        Ok(())
    }

    /// End of the patch stream: succeeds only if the new image is complete.
    pub fn finish(&self) -> Result<ApplyStats, PatchError> {
        if self.state != State::Done {
            return Err(PatchError::Truncated);
        }
        Ok(ApplyStats {
            new_size: self.produced,
            ..self.stats
        })
    }
}

/// Apply an uncompressed patch stream. `old` may be positioned anywhere; it is sought as needed.
pub fn apply<O, P, W>(old: &mut O, old_len: u64, mut patch: P, out: &mut W) -> Result<ApplyStats, PatchError>
where
    O: Read + Seek + ?Sized,
    P: Read,
    W: Write + ?Sized,
{
    let mut patcher = StreamingPatcher::new(old_len);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match patch.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(PatchError::Io(e)),
        };
        patcher.feed(&buf[..n], old, out, &mut |_| {})?;
    }
    out.flush()?;
    patcher.finish()
}

/// Apply a (possibly compressed) patch stream.
pub fn apply_stream<O, P, W>(
    old: &mut O,
    old_len: u64,
    patch: P,
    compression: Compression,
    out: &mut W,
) -> Result<ApplyStats, PatchError>
where
    O: Read + Seek + ?Sized,
    P: Read,
    W: Write + ?Sized,
{
    match compression {
        Compression::None => apply(old, old_len, patch, out),
        #[cfg(feature = "native")]
        Compression::Zstd => apply(old, old_len, zstd::stream::read::Decoder::new(patch)?, out),
        #[cfg(not(feature = "native"))]
        Compression::Zstd => apply_push(old, old_len, patch, compression, out),
    }
}

/// Pull loop over [`PushDecompressor`] + [`StreamingPatcher`] (handles concatenated zstd frames).
#[cfg_attr(feature = "native", allow(dead_code))]
fn apply_push<O, P, W>(
    old: &mut O,
    old_len: u64,
    mut patch: P,
    compression: Compression,
    out: &mut W,
) -> Result<ApplyStats, PatchError>
where
    O: Read + Seek + ?Sized,
    P: Read,
    W: Write + ?Sized,
{
    let mut dec = PushDecompressor::new(compression);
    let mut patcher = StreamingPatcher::new(old_len);
    let mut buf = vec![0u8; CHUNK];
    let mut sink = |chunk: &[u8]| patcher.feed(chunk, &mut *old, &mut *out, &mut |_| {});
    loop {
        let n = match patch.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(PatchError::Io(e)),
        };
        dec.feed(&buf[..n], &mut sink)?;
    }
    dec.finish(&mut sink)?;
    out.flush()?;
    patcher.finish()
}

/// Convenience: apply an in-memory patch to an in-memory old image.
pub fn apply_to_vec(old: &[u8], patch: &[u8], compression: Compression) -> Result<Vec<u8>, PatchError> {
    let mut out = Vec::new();
    apply_stream(
        &mut io::Cursor::new(old),
        old.len() as u64,
        patch,
        compression,
        &mut out,
    )?;
    Ok(out)
}

/// Push-based decompression of the patch stream (pure Rust, every build): compressed bytes go in as
/// they arrive, decompressed bytes come out as soon as the decoder can release them. A zstd stream may
/// consist of several concatenated frames.
///
/// zstd note: within a frame the decoder holds back the most recent window until the frame ends
/// (`ruzstd` frames span their whole content), so streaming needs the patch to be compressed as a
/// sequence of smaller frames (see `diff::compress_zstd_framed`); output then lags by ≤ one frame.
pub struct PushDecompressor {
    compression: Compression,
    zstd: ruzstd::decoding::FrameDecoder,
    pending: Vec<u8>,
    /// A frame has been started and not yet fully drained.
    in_frame: bool,
    frames: u64,
    scratch: Vec<u8>,
}

/// Maximum zstd frame header size; wait for this many bytes before initializing the decoder.
const ZSTD_MAX_FRAME_HEADER: usize = 18;

impl PushDecompressor {
    pub fn new(compression: Compression) -> Self {
        Self {
            compression,
            zstd: ruzstd::decoding::FrameDecoder::new(),
            pending: Vec::new(),
            in_frame: false,
            frames: 0,
            scratch: vec![0u8; CHUNK],
        }
    }

    /// Completed zstd frames so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn feed(
        &mut self,
        input: &[u8],
        sink: &mut dyn FnMut(&[u8]) -> Result<(), PatchError>,
    ) -> Result<(), PatchError> {
        match self.compression {
            Compression::None => sink(input),
            Compression::Zstd => {
                self.pending.extend_from_slice(input);
                self.pump(sink, false)
            }
        }
    }

    fn pump(
        &mut self,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), PatchError>,
        finishing: bool,
    ) -> Result<(), PatchError> {
        loop {
            if !self.in_frame {
                if self.pending.is_empty() || (self.pending.len() < ZSTD_MAX_FRAME_HEADER && !finishing) {
                    return Ok(());
                }
                self.zstd = ruzstd::decoding::FrameDecoder::new();
            }
            let (read, written) = self
                .zstd
                .decode_from_to(&self.pending, &mut self.scratch)
                .map_err(|e| PatchError::Decompress(e.to_string()))?;
            self.in_frame = true;
            self.pending.drain(..read);
            if written > 0 {
                sink(&self.scratch[..written])?;
            }
            if self.zstd.is_finished() && self.zstd.can_collect() == 0 {
                // Frame decoded and drained; the next bytes (if any) start a new frame.
                self.in_frame = false;
                self.frames += 1;
                continue;
            }
            if read == 0 && written == 0 {
                return Ok(());
            }
        }
    }

    /// End of input: release everything that is left and check the last frame is complete.
    pub fn finish(&mut self, sink: &mut dyn FnMut(&[u8]) -> Result<(), PatchError>) -> Result<(), PatchError> {
        if self.compression == Compression::None {
            return Ok(());
        }
        self.pump(sink, true)?;
        if self.in_frame || self.frames == 0 {
            return Err(PatchError::Truncated);
        }
        if !self.pending.is_empty() {
            return Err(PatchError::Malformed("trailing data after the zstd frames"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;

    fn sample() -> (Vec<u8>, Vec<u8>) {
        crate::imagegen::generate_pair(256 * 1024, 77)
    }

    /// Feeding the patch in random-sized pieces gives exactly the same output as the pull applier.
    #[test]
    fn push_equals_pull_on_random_chunks() {
        let (old, new) = sample();
        let (raw, _) = diff::diff_raw(&old, &new);
        let expected = apply_to_vec(&old, &raw, Compression::None).unwrap();
        assert_eq!(expected, new);
        let mut rng = crate::channel::SplitMix64::new(5);
        for round in 0..20 {
            let mut patcher = StreamingPatcher::new(old.len() as u64);
            let mut out = Vec::new();
            let mut cursor = io::Cursor::new(old.as_slice());
            let mut ops_bytes = 0u64;
            let mut rest = raw.as_slice();
            while !rest.is_empty() {
                let max = if round % 2 == 0 { 7 } else { 5000 };
                let n = (1 + rng.below(max) as usize).min(rest.len());
                patcher
                    .feed(&rest[..n], &mut cursor, &mut out, &mut |op| {
                        ops_bytes += match op {
                            PatchOp::Diff { len, .. } | PatchOp::Extra { len, .. } => len,
                        }
                    })
                    .unwrap();
                rest = &rest[n..];
            }
            assert!(patcher.is_complete());
            assert_eq!(patcher.finish().unwrap().new_size, new.len() as u64);
            assert_eq!(out, new, "round {round}");
            assert_eq!(ops_bytes, new.len() as u64, "ops must cover the new image exactly");
        }
    }

    #[test]
    fn ops_tile_the_new_image_in_order() {
        let (old, new) = sample();
        let (raw, _) = diff::diff_raw(&old, &new);
        let mut patcher = StreamingPatcher::new(old.len() as u64);
        let mut next = 0u64;
        patcher
            .feed(&raw, &mut io::Cursor::new(old.as_slice()), &mut io::sink(), &mut |op| {
                let (off, len) = match op {
                    PatchOp::Diff { new_off, old_off, len } => {
                        assert!(old_off + len <= old.len() as u64);
                        (new_off, len)
                    }
                    PatchOp::Extra { new_off, len } => (new_off, len),
                };
                assert_eq!(off, next);
                next += len;
            })
            .unwrap();
        assert_eq!(next, new.len() as u64);
    }

    #[test]
    fn push_patcher_reports_truncation_and_trailing_data() {
        let (raw, _) = diff::diff_raw(b"old data", b"new data!");
        let mut p = StreamingPatcher::new(8);
        p.feed(
            &raw[..raw.len() - 1],
            &mut io::Cursor::new(b"old data"),
            &mut io::sink(),
            &mut |_| {},
        )
        .unwrap();
        assert!(matches!(p.finish(), Err(PatchError::Truncated)));
        let mut p = StreamingPatcher::new(8);
        let mut extra = raw.clone();
        extra.push(1);
        assert!(matches!(
            p.feed(&extra, &mut io::Cursor::new(b"old data"), &mut io::sink(), &mut |_| {}),
            Err(PatchError::Malformed(_))
        ));
    }

    /// A fresh old-image reader on every call (as an in-memory pipeline may do) must not matter.
    #[test]
    fn push_patcher_accepts_a_new_reader_per_call() {
        let (old, new) = sample();
        let (raw, _) = diff::diff_raw(&old, &new);
        let mut patcher = StreamingPatcher::new(old.len() as u64);
        let mut out = Vec::new();
        for piece in raw.chunks(777) {
            let mut fresh = io::Cursor::new(old.as_slice());
            patcher.feed(piece, &mut fresh, &mut out, &mut |_| {}).unwrap();
        }
        patcher.finish().unwrap();
        assert!(out == new, "output differs from the new image");
    }

    fn push_all(dec: &mut PushDecompressor, z: &[u8], piece: usize) -> (Vec<u8>, Option<usize>) {
        let mut out = Vec::new();
        let mut first_output_at = None;
        let mut fed = 0usize;
        for p in z.chunks(piece) {
            fed += p.len();
            dec.feed(p, &mut |b| {
                if !b.is_empty() {
                    first_output_at.get_or_insert(fed);
                }
                out.extend_from_slice(b);
                Ok(())
            })
            .unwrap();
        }
        dec.finish(&mut |b| {
            out.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        (out, first_output_at)
    }

    /// The pure-Rust push decoder reads single-frame zstd from either encoder, in arbitrary pieces.
    #[test]
    fn push_decompressor_decodes_single_frames() {
        let (old, new) = sample();
        let (raw, stats) = diff::diff_raw(&old, &new);
        for level in [1, 19] {
            let (z, _) = diff::finish_patch(raw.clone(), stats, Compression::Zstd, level).unwrap();
            let mut dec = PushDecompressor::new(Compression::Zstd);
            let (out, _) = push_all(&mut dec, &z, 1000);
            assert!(out == raw, "level {level}");
            assert_eq!(dec.frames(), 1);
        }
        // Truncated frame is reported.
        let (z, _) = diff::finish_patch(raw.clone(), stats, Compression::Zstd, 3).unwrap();
        let mut dec = PushDecompressor::new(Compression::Zstd);
        dec.feed(&z[..z.len() / 2], &mut |_| Ok(())).unwrap();
        assert!(dec.finish(&mut |_| Ok(())).is_err());
    }

    /// With independent frames the decoder really streams: output starts long before the input ends.
    #[test]
    fn framed_zstd_streams_and_applies() {
        let (old, new) = sample();
        let (raw, stats) = diff::diff_raw(&old, &new);
        let (z, _) = diff::finish_patch_framed(raw.clone(), stats, Compression::Zstd, 3, Some(16 << 10)).unwrap();
        let mut dec = PushDecompressor::new(Compression::Zstd);
        let (out, first) = push_all(&mut dec, &z, 500);
        assert!(out == raw);
        assert_eq!(dec.frames(), raw.len().div_ceil(16 << 10) as u64);
        let first = first.expect("some output");
        assert!(first < z.len() / 4, "first output after {first} of {} bytes", z.len());
        // Concatenated frames are a valid zstd stream for the pull path too (C or pure Rust decoder).
        assert!(apply_to_vec(&old, &z, Compression::Zstd).unwrap() == new);
    }
}
