//! The codecs behind `node:zlib`: gzip, zlib, raw deflate and brotli, each as a [`Stage`] that
//! takes input chunks and writes into a [`Sink`]. The sink fails past a byte limit, which is the
//! zip-bomb guard: one call can never produce more than `limit` bytes. Decoders run through
//! [`Inflater`] (bounded steps, end-of-stream detection) so truncated input is an error. The gzip
//! decoder and `unzip` live in `zlib_gzip`.

use std::io::{self, Write};

use brotli::{CompressorWriter, DecompressorWriter};
use flate2::{Decompress, FlushDecompress, Status};

use crate::zlib_deflate::{Deflater, dictionary_failure, window_bits};
use crate::zlib_gzip::{Gunzip, Unzip};
use crate::zlib_zstd::ZstdDecoder;

const LIMIT_MARKER: &str = "velyr-zlib-output-limit";
const BROTLI_BUFFER: usize = 16 * 1024;
const BROTLI_WINDOW: u32 = 22;
pub(crate) const INFLATE_STEP: usize = 32 * 1024;

/// Output buffer with a ceiling; passing it fails the write instead of growing.
pub(crate) struct Sink {
    pub buf: Vec<u8>,
    limit: usize,
}

impl Sink {
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn limit_error(&self) -> io::Error {
        io::Error::other(LIMIT_MARKER)
    }
}

impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.limit {
            return Err(io::Error::other(LIMIT_MARKER));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One compressor or decompressor writing into a [`Sink`].
pub(crate) trait Stage {
    fn feed(&mut self, data: &[u8]) -> io::Result<()>;
    fn sync(&mut self) -> io::Result<()>;
    fn sink(&mut self) -> &mut Sink;
    fn finish(self: Box<Self>) -> io::Result<Sink>;
    /// Changes the compression level mid-stream (`params()`); decoders and brotli ignore it.
    fn set_level(&mut self, _level: i32) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Params {
    /// zlib level `0..=9`; negative means the default.
    pub level: i32,
    /// Brotli quality `0..=11`.
    pub quality: u32,
    /// Most bytes one call may produce.
    pub limit: usize,
    /// zlib window size as log2: 9..=15, or 0 for decoders to take the maximum.
    pub window_bits: u8,
    /// Preset dictionary (zlib and raw deflate only).
    pub dictionary: Option<Vec<u8>>,
}

/// Writes all of `data`; a writer that stops taking input (stream ended) ends the loop.
fn feed_all(writer: &mut impl Write, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let taken = writer.write(data)?;
        if taken == 0 {
            break;
        }
        data = &data[taken..];
    }
    Ok(())
}

impl Stage for CompressorWriter<Sink> {
    fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        feed_all(self, data)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.flush()
    }
    fn sink(&mut self) -> &mut Sink {
        self.get_mut()
    }
    fn finish(self: Box<Self>) -> io::Result<Sink> {
        Ok(self.into_inner())
    }
}

impl Stage for DecompressorWriter<Sink> {
    fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        feed_all(self, data)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.flush()
    }
    fn sink(&mut self) -> &mut Sink {
        self.get_mut()
    }
    fn finish(mut self: Box<Self>) -> io::Result<Sink> {
        // Corrupt data already failed in `feed`; what `close` rejects is a stream cut short.
        self.close()
            .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        self.into_inner()
            .map_err(|_| io::ErrorKind::UnexpectedEof.into())
    }
}

pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Bounded inflate over `flate2::Decompress`. The write-side decoders cannot say whether the
/// stream ended, and a truncated input has to fail (Node: "unexpected end of file").
pub(crate) struct Inflater {
    raw: Decompress,
    scratch: Box<[u8]>,
    /// Handed to zlib when the stream asks for it (raw streams get it up front).
    dictionary: Option<Vec<u8>>,
    pub ended: bool,
}

const MISSING_DICTIONARY: &str = "Missing dictionary";

impl Inflater {
    pub(crate) fn new(
        zlib_header: bool,
        bits: u8,
        dictionary: Option<Vec<u8>>,
    ) -> Result<Self, String> {
        let mut raw = Decompress::new_with_window_bits(zlib_header, window_bits(bits));
        // Raw streams take the dictionary now; zlib streams ask for it after the header.
        let (upfront, pending) = if zlib_header {
            (None, dictionary)
        } else {
            (dictionary, None)
        };
        if let Some(dictionary) = upfront {
            raw.set_dictionary(&dictionary)
                .map_err(|_| dictionary_failure())?;
        }
        Ok(Self {
            raw,
            scratch: vec![0; INFLATE_STEP].into_boxed_slice(),
            dictionary: pending,
            ended: false,
        })
    }

    /// Inflates `input`, handing output to `emit` in steps; returns the bytes consumed (less
    /// than `input.len()` only when the stream ended inside it).
    pub(crate) fn run(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<usize> {
        let mut used_total = 0;
        while !self.ended {
            let (before_in, before_out) = (self.raw.total_in(), self.raw.total_out());
            let result = self.raw.decompress(
                &input[used_total..],
                &mut self.scratch,
                FlushDecompress::None,
            );
            let used = (self.raw.total_in() - before_in) as usize;
            let status = match result {
                Ok(status) => status,
                Err(error) if error.needs_dictionary().is_some() => {
                    // The header was consumed; give zlib the dictionary and carry on.
                    used_total += used;
                    let dictionary = self
                        .dictionary
                        .take()
                        .ok_or_else(|| invalid(MISSING_DICTIONARY))?;
                    self.raw
                        .set_dictionary(&dictionary)
                        .map_err(|_| invalid("Bad dictionary"))?;
                    continue;
                }
                Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
            };
            let made = (self.raw.total_out() - before_out) as usize;
            used_total += used;
            emit(&self.scratch[..made])?;
            if status == Status::StreamEnd {
                self.ended = true;
            } else if (used == 0 && made == 0)
                || (used_total == input.len() && made < self.scratch.len())
            {
                break;
            }
        }
        Ok(used_total)
    }
}

/// `inflate` (zlib header) and `inflateRaw`.
pub(crate) struct Inflate {
    inflater: Inflater,
    sink: Sink,
}

impl Inflate {
    pub(crate) fn new(zlib_header: bool, params: &Params, sink: Sink) -> Result<Self, String> {
        let dictionary = params.dictionary.clone();
        Ok(Self {
            inflater: Inflater::new(zlib_header, params.window_bits, dictionary)?,
            sink,
        })
    }
}

impl Stage for Inflate {
    fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        // Bytes after the end of the stream are ignored, as zlib does.
        let sink = &mut self.sink;
        self.inflater
            .run(data, |chunk| sink.write_all(chunk))
            .map(drop)
    }
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sink(&mut self) -> &mut Sink {
        &mut self.sink
    }
    fn finish(self: Box<Self>) -> io::Result<Sink> {
        if self.inflater.ended {
            Ok(self.sink)
        } else {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }
}

/// A codec for `kind` (`gzip`, `gunzip`, `unzip`, `deflate`, `inflate`, `deflateRaw`,
/// `inflateRaw`, `brotliCompress`, `brotliDecompress`).
pub(crate) fn stage(kind: &str, params: &Params) -> Result<Box<dyn Stage>, String> {
    let sink = Sink {
        buf: Vec::new(),
        limit: params.limit,
    };
    Ok(match kind {
        "gzip" | "deflate" | "deflateRaw" => Box::new(Deflater::new(kind, params, sink)?),
        "gunzip" => Box::new(Gunzip::new(sink, params.window_bits)?),
        "unzip" => Box::new(Unzip::new(sink, params)),
        "inflate" => Box::new(Inflate::new(true, params, sink)?),
        "inflateRaw" => Box::new(Inflate::new(false, params, sink)?),
        "brotliCompress" => Box::new(CompressorWriter::new(
            sink,
            BROTLI_BUFFER,
            params.quality,
            BROTLI_WINDOW,
        )),
        "brotliDecompress" => Box::new(DecompressorWriter::new(sink, BROTLI_BUFFER)),
        "zstdDecompress" => Box::new(ZstdDecoder::new(sink)),
        _ => return Err(format!("Unknown zlib format {kind}")),
    })
}

/// Node-style `code|message` for a codec failure.
pub(crate) fn failure(error: &io::Error, limit: usize) -> String {
    if error.to_string() == LIMIT_MARKER {
        return format!("ERR_BUFFER_TOO_LARGE|Cannot create a Buffer larger than {limit} bytes");
    }
    if error.to_string() == MISSING_DICTIONARY {
        return format!("Z_NEED_DICT|{MISSING_DICTIONARY}");
    }
    match error.kind() {
        io::ErrorKind::UnexpectedEof => "Z_BUF_ERROR|unexpected end of file".into(),
        io::ErrorKind::Unsupported => format!("Z_STREAM_ERROR|{error}"),
        _ => format!("Z_DATA_ERROR|{error}"),
    }
}

/// Whole input to whole output on one codec (the sync and callback forms).
pub(crate) fn once(kind: &str, params: &Params, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut stage = stage(kind, params)?;
    let fail = |error: io::Error| failure(&error, params.limit);
    stage.feed(data).map_err(fail)?;
    Ok(stage.finish().map_err(fail)?.buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: Params = Params {
        level: -1,
        quality: 4,
        limit: 1 << 20,
        window_bits: 15,
        dictionary: None,
    };

    /// Feeds `input` in `step`-byte chunks and drains the sink after each, like a stream does.
    fn streamed(kind: &str, input: &[u8], step: usize) -> io::Result<Vec<u8>> {
        let mut stage = stage(kind, &PARAMS).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(step) {
            stage.feed(chunk)?;
            out.append(&mut stage.sink().buf);
        }
        out.extend(stage.finish()?.buf);
        Ok(out)
    }

    #[test]
    fn chunked_input_gives_the_same_result_as_one_shot() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i * i % 251) as u8).collect();
        for (pack, unpack) in [
            ("gzip", "gunzip"),
            ("deflate", "inflate"),
            ("deflateRaw", "inflateRaw"),
            ("brotliCompress", "brotliDecompress"),
        ] {
            let packed = once(pack, &PARAMS, &data).unwrap();
            for step in [1, 7, 4096] {
                assert_eq!(
                    streamed(unpack, &packed, step).unwrap(),
                    data,
                    "{unpack} step {step}"
                );
            }
            let encoded = streamed(pack, &data, 999).unwrap();
            assert_eq!(
                streamed(unpack, &encoded, 500).unwrap(),
                data,
                "{pack} streams"
            );
        }
    }

    fn with(dictionary: Option<&[u8]>, window_bits: u8) -> Params {
        Params {
            dictionary: dictionary.map(<[u8]>::to_vec),
            window_bits,
            ..PARAMS
        }
    }

    #[test]
    fn dictionary_and_window_bits_round_trip_and_a_missing_dictionary_is_reported() {
        let dict = b"common prefix words shared by both sides";
        let data = b"common prefix words shared by both sides, then the payload";
        for (pack, unpack) in [("deflate", "inflate"), ("deflateRaw", "inflateRaw")] {
            let plain = once(pack, &PARAMS, data).unwrap();
            let packed = once(pack, &with(Some(dict), 15), data).unwrap();
            assert!(packed.len() < plain.len(), "{pack}: the dictionary helps");
            assert_eq!(once(unpack, &with(Some(dict), 15), &packed).unwrap(), data);
            assert!(once(unpack, &PARAMS, &packed).is_err(), "{unpack} needs it");
        }
        let packed = once("deflate", &with(Some(dict), 15), data).unwrap();
        let error = once("inflate", &PARAMS, &packed).unwrap_err();
        assert!(error.starts_with("Z_NEED_DICT|"), "{error}");
        let error = once("inflate", &with(Some(b"wrong"), 15), &packed).unwrap_err();
        assert!(error.starts_with("Z_DATA_ERROR|"), "{error}");
        assert!(once("gzip", &with(Some(dict), 15), data).is_err());
        let small = once("gzip", &with(None, 9), &vec![7u8; 50_000]).unwrap();
        assert_eq!(
            once("gunzip", &with(None, 0), &small).unwrap().len(),
            50_000
        );
        assert_eq!(streamed("unzip", &small, 3).unwrap().len(), 50_000);
    }

    #[test]
    fn params_changes_the_level_mid_stream_without_breaking_the_stream() {
        // Mid-stream within one algorithm family (3-6), and any level before the first byte.
        let mut stage = stage("gzip", &PARAMS).unwrap();
        stage.set_level(1).unwrap();
        stage.set_level(3).unwrap();
        stage.feed(&vec![1u8; 10_000]).unwrap();
        stage.set_level(5).unwrap();
        stage.feed(&vec![2u8; 10_000]).unwrap();
        let error = stage.set_level(9).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported, "crosses families");
        stage.feed(&[3u8; 10]).unwrap();
        let packed = stage.finish().unwrap().buf;
        let plain = once("gunzip", &PARAMS, &packed).unwrap();
        let want = [vec![1u8; 10_000], vec![2u8; 10_000], vec![3u8; 10]].concat();
        assert_eq!(plain, want);
    }

    #[test]
    fn gzip_members_concatenate_and_trailing_garbage_is_ignored() {
        let first = once("gzip", &PARAMS, b"first ").unwrap();
        let second = once("gzip", &PARAMS, b"second").unwrap();
        let joined = [first.clone(), second.clone()].concat();
        assert_eq!(streamed("gunzip", &joined, 3).unwrap(), b"first second");
        let junk = [first, vec![0, 1, 2, 3]].concat();
        assert_eq!(streamed("gunzip", &junk, 5).unwrap(), b"first ");
    }

    #[test]
    fn gzip_header_with_name_extra_and_comment_is_parsed() {
        let body = once("deflateRaw", &PARAMS, b"payload").unwrap();
        let crc = crc32fast::hash(b"payload").to_le_bytes();
        let mut member = vec![0x1f, 0x8b, 8, 4 | 8 | 16 | 2, 0, 0, 0, 0, 0, 3];
        member.extend([3, 0, 1, 2, 3]);
        member.extend(b"name\0comment\0");
        member.extend([0xaa, 0xbb]);
        member.extend(body);
        member.extend(crc);
        member.extend(7u32.to_le_bytes());
        assert_eq!(streamed("gunzip", &member, 2).unwrap(), b"payload");
        let last = member.len() - 1;
        member[last] ^= 1;
        assert!(
            streamed("gunzip", &member, 64)
                .unwrap_err()
                .to_string()
                .contains("length check")
        );
    }
}
