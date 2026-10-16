//! The gzip side of `node:zlib`'s decoders: [`Gunzip`] (header, body, CRC-32 and size trailer,
//! concatenated members) and [`Unzip`], which picks gzip or zlib from the first two bytes.

use std::io::{self, Write};

use crate::zlib_codec::{Inflate, Inflater, Params, Sink, Stage, invalid};

const GZIP_FIXED_HEADER: usize = 10;
const GZIP_TRAILER: usize = 8;
/// Longest gzip header (extra field, name, comment) accepted before the body starts.
const GZIP_MAX_HEADER: usize = 1 << 20;

enum GzipState {
    Header,
    Body,
    Trailer,
    /// Past the last member: anything after it is ignored.
    Done,
}

/// `gunzip`: header and trailer handled here (checking CRC-32 and size), the body by [`Inflater`].
/// Concatenated members decode as one stream, like Node.
pub(crate) struct Gunzip {
    inflater: Inflater,
    window_bits: u8,
    sink: Sink,
    state: GzipState,
    /// Header or trailer bytes still incomplete across chunks.
    pending: Vec<u8>,
    crc: crc32fast::Hasher,
    size: u32,
    members: usize,
}

/// Length of the gzip header at the start of `buf`, `None` while it is incomplete.
fn gzip_header_len(buf: &[u8]) -> io::Result<Option<usize>> {
    if buf.first().is_some_and(|&b| b != 0x1f) || buf.get(1).is_some_and(|&b| b != 0x8b) {
        return Err(invalid("incorrect header check"));
    }
    if buf.len() < GZIP_FIXED_HEADER {
        return Ok(None);
    }
    if buf[2] != 8 {
        return Err(invalid("unknown compression method"));
    }
    let flags = buf[3];
    let mut at = GZIP_FIXED_HEADER;
    if flags & 4 != 0 {
        let Some(len) = buf.get(at..at + 2) else {
            return Ok(None);
        };
        at += 2 + usize::from(u16::from_le_bytes([len[0], len[1]]));
    }
    for terminated in [8u8, 16] {
        if flags & terminated != 0 {
            let Some(end) = buf
                .get(at..)
                .and_then(|rest| rest.iter().position(|&b| b == 0))
            else {
                return Ok(None);
            };
            at += end + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    Ok((buf.len() >= at).then_some(at))
}

impl Gunzip {
    pub(crate) fn new(sink: Sink, window_bits: u8) -> Result<Self, String> {
        Ok(Self {
            inflater: Inflater::new(false, window_bits, None)?,
            window_bits,
            sink,
            state: GzipState::Header,
            pending: Vec::new(),
            crc: crc32fast::Hasher::new(),
            size: 0,
            members: 0,
        })
    }

    fn header(&mut self, data: &[u8]) -> io::Result<()> {
        if self.members > 0 && self.pending.is_empty() && data.first().is_some_and(|&b| b != 0x1f) {
            self.state = GzipState::Done;
            return Ok(());
        }
        self.pending.extend_from_slice(data);
        if self.pending.len() > GZIP_MAX_HEADER {
            return Err(invalid("gzip header too long"));
        }
        if let Some(len) = gzip_header_len(&self.pending)? {
            let body = self.pending.split_off(len);
            self.pending.clear();
            self.state = GzipState::Body;
            return self.feed(&body);
        }
        Ok(())
    }

    fn trailer(&mut self, data: &[u8]) -> io::Result<()> {
        let take = data.len().min(GZIP_TRAILER - self.pending.len());
        self.pending.extend_from_slice(&data[..take]);
        if self.pending.len() < GZIP_TRAILER {
            return Ok(());
        }
        let crc = u32::from_le_bytes(self.pending[..4].try_into().expect("4 bytes"));
        let size = u32::from_le_bytes(self.pending[4..].try_into().expect("4 bytes"));
        if crc != std::mem::replace(&mut self.crc, crc32fast::Hasher::new()).finalize() {
            return Err(invalid("incorrect data check"));
        }
        if size != self.size {
            return Err(invalid("incorrect length check"));
        }
        self.pending.clear();
        self.size = 0;
        self.members += 1;
        self.inflater = Inflater::new(false, self.window_bits, None).map_err(io::Error::other)?;
        self.state = GzipState::Header;
        self.feed(&data[take..])
    }
}

impl Stage for Gunzip {
    fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        match self.state {
            GzipState::Done => Ok(()),
            GzipState::Header => self.header(data),
            GzipState::Trailer => self.trailer(data),
            GzipState::Body => {
                let (sink, crc, size) = (&mut self.sink, &mut self.crc, &mut self.size);
                let used = self.inflater.run(data, |chunk| {
                    crc.update(chunk);
                    *size = size.wrapping_add(chunk.len() as u32);
                    sink.write_all(chunk)
                })?;
                if self.inflater.ended {
                    self.state = GzipState::Trailer;
                    return self.trailer(&data[used..]);
                }
                Ok(())
            }
        }
    }
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sink(&mut self) -> &mut Sink {
        &mut self.sink
    }
    fn finish(self: Box<Self>) -> io::Result<Sink> {
        let complete = match self.state {
            GzipState::Done => true,
            GzipState::Header => self.members > 0 && self.pending.is_empty(),
            _ => false,
        };
        if complete {
            Ok(self.sink)
        } else {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }
}

enum UnzipState {
    /// Fewer than two bytes seen: `sink` is still ours.
    Sniffing {
        sink: Sink,
        seen: Vec<u8>,
    },
    Decoding(Box<dyn Stage>),
}

/// `unzip`: gzip when the input starts with the gzip magic, zlib otherwise.
pub(crate) struct Unzip(Option<UnzipState>, Params);

impl Unzip {
    pub(crate) fn new(sink: Sink, params: &Params) -> Self {
        let sniffing = UnzipState::Sniffing {
            sink,
            seen: Vec::new(),
        };
        Self(Some(sniffing), params.clone())
    }
}

impl Stage for Unzip {
    fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(UnzipState::Sniffing { seen, .. }) = &mut self.0 {
            seen.extend_from_slice(data);
            // Wait while gzip is still possible; a first byte other than 0x1f rules it out.
            if seen.len() < 2 && seen.first().is_none_or(|&b| b == 0x1f) {
                return Ok(());
            }
            let Some(UnzipState::Sniffing { sink, seen }) = self.0.take() else {
                unreachable!("just matched");
            };
            // Neither constructor can fail here: no dictionary reaches the gzip branch, and the
            // zlib branch defers its dictionary until the stream asks for it.
            let mut decoder: Box<dyn Stage> = if seen.starts_with(&[0x1f, 0x8b]) {
                Box::new(Gunzip::new(sink, self.1.window_bits).map_err(io::Error::other)?)
            } else {
                Box::new(Inflate::new(true, &self.1, sink).map_err(io::Error::other)?)
            };
            let result = decoder.feed(&seen);
            self.0 = Some(UnzipState::Decoding(decoder));
            return result;
        }
        match &mut self.0 {
            Some(UnzipState::Decoding(decoder)) => decoder.feed(data),
            _ => Ok(()),
        }
    }
    fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn sink(&mut self) -> &mut Sink {
        match self.0.as_mut().expect("unzip state is always set") {
            UnzipState::Sniffing { sink, .. } => sink,
            UnzipState::Decoding(decoder) => decoder.sink(),
        }
    }
    fn finish(mut self: Box<Self>) -> io::Result<Sink> {
        match self.0.take() {
            Some(UnzipState::Decoding(decoder)) => decoder.finish(),
            // Empty input, or a lone 0x1f: nothing a decoder could accept.
            _ => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::zlib_codec::{Params, once};

    const PARAMS: Params = Params {
        level: -1,
        quality: 4,
        limit: 1 << 20,
        window_bits: 15,
        dictionary: None,
    };

    #[test]
    fn unzip_detects_gzip_and_zlib_even_when_fed_byte_by_byte() {
        for pack in ["gzip", "deflate"] {
            let packed = once(pack, &PARAMS, b"unzip me").unwrap();
            assert_eq!(
                once("unzip", &PARAMS, &packed).unwrap(),
                b"unzip me",
                "{pack}"
            );
            let mut stage = crate::zlib_codec::stage("unzip", &PARAMS).unwrap();
            for byte in &packed {
                stage.feed(&[*byte]).unwrap();
            }
            assert_eq!(stage.finish().unwrap().buf, b"unzip me", "{pack} bytewise");
        }
        assert!(once("unzip", &PARAMS, b"").is_err());
        assert!(once("unzip", &PARAMS, &[0x1f]).is_err());
    }
}
