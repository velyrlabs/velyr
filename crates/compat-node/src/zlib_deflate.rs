//! The compressing side of `node:zlib` for gzip, zlib and raw deflate: [`Deflater`] over
//! `flate2::Compress` (not the write-side encoders, which have no dictionary, window size or
//! mid-stream level change). Output is produced in bounded steps into the stage's sink.

use std::io::{self, Write};

use flate2::{Compress, Compression, FlushCompress, Status};

use crate::zlib_codec::{INFLATE_STEP, Params, Sink, Stage};

pub(crate) fn dictionary_failure() -> String {
    "Z_STREAM_ERROR|Failed to set dictionary".into()
}

pub(crate) fn compression(level: i32) -> Compression {
    u32::try_from(level).map_or_else(
        |_| Compression::default(),
        |level| Compression::new(level.min(9)),
    )
}

/// zlib windows run 9..=15 (8 is promoted to 9, 0 means "the maximum" for decoders).
pub(crate) fn window_bits(bits: u8) -> u8 {
    if bits == 0 { 15 } else { bits.clamp(9, 15) }
}

pub(crate) struct Deflater {
    raw: Compress,
    sink: Sink,
    scratch: Box<[u8]>,
    /// Whether any data or flush went through (see `set_level`).
    touched: bool,
}

impl Deflater {
    /// `kind` is `gzip`, `deflate` (zlib header) or anything else for raw deflate.
    pub(crate) fn new(kind: &str, params: &Params, sink: Sink) -> Result<Self, String> {
        let level = compression(params.level);
        let bits = window_bits(params.window_bits);
        let mut raw = match kind {
            "gzip" => Compress::new_gzip(level, bits),
            "deflate" => Compress::new_with_window_bits(level, true, bits),
            _ => Compress::new_with_window_bits(level, false, bits),
        };
        if let Some(dictionary) = &params.dictionary {
            // zlib cannot set a dictionary under a gzip wrapper either.
            if kind == "gzip" || raw.set_dictionary(dictionary).is_err() {
                return Err(dictionary_failure());
            }
        }
        Ok(Self {
            raw,
            sink,
            scratch: vec![0; INFLATE_STEP].into_boxed_slice(),
            touched: false,
        })
    }

    /// One `compress` call; returns `(input used, output made, status)` after writing the output.
    fn step(&mut self, input: &[u8], flush: FlushCompress) -> io::Result<(usize, usize, Status)> {
        self.touched = true;
        let (before_in, before_out) = (self.raw.total_in(), self.raw.total_out());
        let status = self
            .raw
            .compress(input, &mut self.scratch, flush)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let used = (self.raw.total_in() - before_in) as usize;
        let made = (self.raw.total_out() - before_out) as usize;
        self.sink.write_all(&self.scratch[..made])?;
        Ok((used, made, status))
    }

    /// Runs `flush` with no input until zlib has nothing left to emit.
    fn drain(&mut self, flush: FlushCompress) -> io::Result<()> {
        loop {
            let (_, made, status) = self.step(&[], flush)?;
            if status == Status::StreamEnd || made < self.scratch.len() {
                return Ok(());
            }
        }
    }
}

impl Stage for Deflater {
    fn feed(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let (used, made, _) = self.step(data, FlushCompress::None)?;
            if used == 0 && made == 0 {
                break;
            }
            data = &data[used..];
        }
        Ok(())
    }
    fn sync(&mut self) -> io::Result<()> {
        self.drain(FlushCompress::Sync)
    }
    fn sink(&mut self) -> &mut Sink {
        &mut self.sink
    }
    fn finish(mut self: Box<Self>) -> io::Result<Sink> {
        self.drain(FlushCompress::Finish)?;
        Ok(self.sink)
    }
    fn set_level(&mut self, level: i32) -> io::Result<()> {
        // shortcut: flate2's zlib-rs `set_level` has no output buffer to flush into, so once data
        // has gone through it only works between levels of the same algorithm (0, 1, 2, 3-6, 7-9);
        // lift this when flate2/zlib-rs let `deflateParams` flush.
        if self.touched {
            self.drain(FlushCompress::Sync)?;
        }
        self.raw.set_level(compression(level)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "cannot switch to this level once data has been written (only within 0, 1, 2, 3-6, 7-9)",
            )
        })
    }
}
