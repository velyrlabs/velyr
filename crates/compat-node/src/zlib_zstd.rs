//! zstd decompression for `node:zlib` (`ruzstd`, pure Rust). A [`Stage`] like the other decoders,
//! with the same output ceiling: the decoder writes through the [`Sink`], so a bomb fails at the
//! limit instead of growing.
//!
//! shortcut: the input is buffered and decoded when the stream finishes, so output does not appear
//! chunk by chunk; upgrade when a caller needs incremental zstd output (ruzstd reads from a `Read`,
//! it cannot be fed partial frames). Compression is not offered. Dictionaries are not supported.
//! Like Node, only the first frame is decoded. Unlike Node, empty or truncated input is an error
//! (Node returns what it had), so a cut-off HTTP body does not pass as a complete one.

use std::io::{self, Read, Write};

use ruzstd::decoding::StreamingDecoder;

use crate::zlib_codec::{Sink, Stage, invalid};

/// Largest window a frame may ask for: zstd's own default ceiling (windowLog 27).
const MAX_WINDOW: u64 = 128 << 20;
const SKIPPABLE_MAGIC: u32 = 0x184D_2A50;
const STEP: usize = 32 * 1024;
/// Compressed input may exceed the output ceiling by a little (incompressible data, headers).
const INPUT_SLACK: usize = 64 * 1024;

pub(crate) struct ZstdDecoder {
    input: Vec<u8>,
    sink: Sink,
}

impl ZstdDecoder {
    pub(crate) fn new(sink: Sink) -> Self {
        Self {
            input: Vec::new(),
            sink,
        }
    }
}

/// Bytes a skippable frame occupies (magic `0x184D2A5?`, 4-byte size, payload), if one starts here.
fn skippable_len(input: &[u8]) -> io::Result<Option<usize>> {
    let Some(magic) = input.first_chunk::<4>().map(|m| u32::from_le_bytes(*m)) else {
        return Ok(None);
    };
    if magic & 0xFFFF_FFF0 != SKIPPABLE_MAGIC {
        return Ok(None);
    }
    let size = input
        .get(4..8)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(io::ErrorKind::UnexpectedEof)?;
    Ok(Some(8 + size as usize))
}

impl Stage for ZstdDecoder {
    fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        if self.input.len().saturating_add(data.len())
            > self.sink.limit().saturating_add(INPUT_SLACK)
        {
            return Err(self.sink.limit_error());
        }
        self.input.extend_from_slice(data);
        Ok(())
    }

    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn sink(&mut self) -> &mut Sink {
        &mut self.sink
    }

    fn finish(mut self: Box<Self>) -> io::Result<Sink> {
        let input = std::mem::take(&mut self.input);
        let mut rest = input.as_slice();
        if rest.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let mut chunk = vec![0u8; STEP];
        // Like Node, only the first frame is decoded; whatever follows it is ignored.
        {
            if let Some(skip) = skippable_len(rest)? {
                if rest.len() < skip {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                return Ok(self.sink);
            }
            let mut frame = StreamingDecoder::new_with_max_window_size(&mut rest, MAX_WINDOW)
                .map_err(|error| invalid(&error.to_string()))?;
            loop {
                let made = frame.read(&mut chunk).map_err(|error| {
                    if error.kind() == io::ErrorKind::UnexpectedEof {
                        error
                    } else {
                        invalid(&error.to_string())
                    }
                })?;
                if made == 0 {
                    break;
                }
                self.sink.write_all(&chunk[..made])?;
            }
            // A frame cut short (even at a block boundary) must not read as a complete one.
            let done = frame.into_frame_decoder();
            if !done.is_finished() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            // ruzstd computes the checksum but leaves comparing it to the caller.
            if let (Some(stored), Some(computed)) = (
                done.get_checksum_from_data(),
                done.get_calculated_checksum(),
            ) && stored != computed
            {
                return Err(invalid("Data corruption detected: checksum mismatch"));
            }
        }
        Ok(self.sink)
    }
}
