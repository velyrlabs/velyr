//! `Content-Encoding` decoding for response bodies: push-style, so a chunk goes in and whatever
//! it decodes to comes out. Codings are undone in reverse order of the header; one this module
//! does not know leaves the body as it came.

use std::io::Write;

use brotli_decompressor::DecompressorWriter;
use flate2::write::{DeflateDecoder, GzDecoder, ZlibDecoder};
use velyr_runtime_errors::{ErrorCode, VelyrError};

/// Decoded bytes one response may produce; a zip bomb ends in `QuotaExceeded`.
const MAX_DECODED: u64 = 256 * 1024 * 1024;
const BROTLI_BUFFER: usize = 8192;

enum Stage {
    Gzip(GzDecoder<Vec<u8>>),
    Zlib(ZlibDecoder<Vec<u8>>),
    RawDeflate(DeflateDecoder<Vec<u8>>),
    /// `deflate` is zlib-wrapped by the book but raw in the wild; the first two bytes tell.
    Deflate(Vec<u8>),
    Brotli(Box<DecompressorWriter<Vec<u8>>>),
}

pub(crate) struct Decoder {
    stages: Vec<Stage>,
    produced: u64,
}

fn corrupt(err: impl std::fmt::Display) -> VelyrError {
    VelyrError::new(
        ErrorCode::InvalidData,
        format!("invalid compressed body: {err}"),
    )
}

fn looks_like_zlib(head: &[u8]) -> bool {
    head[0] & 0x0f == 8 && (u16::from(head[0]) << 8 | u16::from(head[1])) % 31 == 0
}

impl Stage {
    fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, VelyrError> {
        match self {
            Stage::Gzip(w) => drain(w, input),
            Stage::Zlib(w) => drain(w, input),
            Stage::RawDeflate(w) => drain(w, input),
            Stage::Brotli(w) => drain(w.as_mut(), input),
            Stage::Deflate(head) => {
                head.extend_from_slice(input);
                if head.len() < 2 {
                    return Ok(Vec::new());
                }
                let head = std::mem::take(head);
                *self = if looks_like_zlib(&head) {
                    Stage::Zlib(ZlibDecoder::new(Vec::new()))
                } else {
                    Stage::RawDeflate(DeflateDecoder::new(Vec::new()))
                };
                self.push(&head)
            }
        }
    }

    /// Ends the stream; an incomplete one is an error.
    fn finish(self) -> Result<Vec<u8>, VelyrError> {
        match self {
            Stage::Gzip(w) => w.finish().map_err(corrupt),
            Stage::Zlib(w) => w.finish().map_err(corrupt),
            Stage::RawDeflate(w) => w.finish().map_err(corrupt),
            Stage::Brotli(mut w) => {
                w.flush().map_err(corrupt)?;
                w.into_inner()
                    .map_err(|_| corrupt("truncated brotli stream"))
            }
            Stage::Deflate(head) if head.is_empty() => Ok(Vec::new()),
            Stage::Deflate(_) => Err(corrupt("truncated deflate stream")),
        }
    }
}

fn drain<W: Write + Sink>(writer: &mut W, input: &[u8]) -> Result<Vec<u8>, VelyrError> {
    writer.write_all(input).map_err(corrupt)?;
    Ok(std::mem::take(writer.sink()))
}

/// The `Vec` a decoder writes into.
trait Sink {
    fn sink(&mut self) -> &mut Vec<u8>;
}

macro_rules! sink {
    ($($ty:ty),+) => {$(impl Sink for $ty { fn sink(&mut self) -> &mut Vec<u8> { self.get_mut() } })+};
}
sink!(
    GzDecoder<Vec<u8>>,
    ZlibDecoder<Vec<u8>>,
    DeflateDecoder<Vec<u8>>
);

impl Sink for DecompressorWriter<Vec<u8>> {
    fn sink(&mut self) -> &mut Vec<u8> {
        self.get_mut()
    }
}

impl Decoder {
    /// `None` when `content_encoding` asks for nothing this module undoes.
    pub(crate) fn for_encoding(content_encoding: &str) -> Option<Self> {
        let stages: Vec<Stage> = content_encoding
            .split(',')
            .rev()
            .map(|coding| coding.trim().to_ascii_lowercase())
            .filter_map(|coding| match coding.as_str() {
                "gzip" | "x-gzip" => Some(Stage::Gzip(GzDecoder::new(Vec::new()))),
                "deflate" => Some(Stage::Deflate(Vec::new())),
                "br" => Some(Stage::Brotli(Box::new(DecompressorWriter::new(
                    Vec::new(),
                    BROTLI_BUFFER,
                )))),
                _ => None,
            })
            .collect();
        (!stages.is_empty()).then_some(Self {
            stages,
            produced: 0,
        })
    }

    fn account(&mut self, out: &[u8]) -> Result<(), VelyrError> {
        self.produced += out.len() as u64;
        if self.produced > MAX_DECODED {
            return Err(VelyrError::quota_exceeded("body_bytes"));
        }
        Ok(())
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<u8>, VelyrError> {
        let mut data = chunk.to_vec();
        for stage in &mut self.stages {
            data = stage.push(&data)?;
        }
        self.account(&data)?;
        Ok(data)
    }

    /// What is still buffered, once the body ended; an error if the stream was cut short.
    pub(crate) fn finish(&mut self) -> Result<Vec<u8>, VelyrError> {
        // What one stage still held goes through the next before that one ends.
        let mut carry = Vec::new();
        for mut stage in std::mem::take(&mut self.stages) {
            let mut out = if carry.is_empty() {
                Vec::new()
            } else {
                stage.push(&carry)?
            };
            out.extend(stage.finish()?);
            carry = out;
        }
        self.account(&carry)?;
        Ok(carry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn decode(coding: &str, input: &[u8], step: usize) -> Result<Vec<u8>, VelyrError> {
        let mut decoder = Decoder::for_encoding(coding).expect("known coding");
        let mut out = Vec::new();
        for piece in input.chunks(step) {
            out.extend(decoder.push(piece)?);
        }
        out.extend(decoder.finish()?);
        Ok(out)
    }

    #[test]
    fn gzip_survives_any_chunking_and_truncation_is_an_error() {
        let packed = gzip(&b"abc".repeat(1000));
        assert_eq!(decode("gzip", &packed, 7).unwrap(), b"abc".repeat(1000));
        assert!(decode("x-gzip", &packed[..packed.len() - 6], 7).is_err());
    }

    #[test]
    fn deflate_accepts_zlib_and_raw_streams() {
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(b"zlib").unwrap();
        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(b"raw").unwrap();
        assert_eq!(
            decode("deflate", &zlib.finish().unwrap(), 1).unwrap(),
            b"zlib"
        );
        assert_eq!(
            decode("deflate", &raw.finish().unwrap(), 1).unwrap(),
            b"raw"
        );
    }

    #[test]
    fn brotli_and_stacked_codings_decode_and_a_cut_brotli_stream_fails() {
        let br = [0x0b, 0x02, 0x80, b'h', b'e', b'l', b'l', b'o', 0x03];
        assert_eq!(decode("br", &br, 3).unwrap(), b"hello");
        assert!(decode("br", &br[..6], 3).is_err());
        // Brotli applied first, gzip second; undone in reverse.
        assert_eq!(decode("br, gzip", &gzip(&br), 5).unwrap(), b"hello");
    }

    #[test]
    fn unknown_or_identity_codings_are_left_alone() {
        assert!(Decoder::for_encoding("identity").is_none());
        assert!(Decoder::for_encoding("zstd").is_none());
    }
}
