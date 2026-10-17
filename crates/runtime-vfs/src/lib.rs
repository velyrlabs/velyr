//! Read-only virtual file system for single-file executables (`velyr compile`).
//!
//! Layout appended to a copy of the velyr binary:
//! `[payload][sha256(payload): 32][payload_len: u64 LE][MAGIC: 8]`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

mod loader;
pub use loader::VfsLoader;

pub const MAGIC: &[u8; 8] = b"VELYRVFS";
const HASH_LEN: usize = 32;
const TRAILER_LEN: u64 = (HASH_LEN + 8 + MAGIC.len()) as u64;

#[derive(Debug, PartialEq, Eq)]
pub enum VfsError {
    /// The trailer or the payload layout is malformed.
    Corrupt(&'static str),
    /// The payload does not match the hash recorded in the trailer.
    HashMismatch,
    Io(String),
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corrupt(what) => write!(f, "corrupt embedded payload: {what}"),
            Self::HashMismatch => write!(f, "embedded payload hash mismatch"),
            Self::Io(e) => write!(f, "io error reading embedded payload: {e}"),
        }
    }
}

impl std::error::Error for VfsError {}

impl From<io::Error> for VfsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Everything a compiled executable carries.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Payload {
    /// Virtual path of the entry module.
    pub entry: String,
    /// Permission flags fixed at compile time (for example `--allow-read=assets`).
    pub permission_args: Vec<String>,
    /// Virtual path -> bytes: transformed modules, node_modules files and assets.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Payload {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, self.entry.as_bytes());
        put_len(&mut out, self.permission_args.len());
        for arg in &self.permission_args {
            put_bytes(&mut out, arg.as_bytes());
        }
        put_len(&mut out, self.files.len());
        for (path, data) in &self.files {
            put_bytes(&mut out, path.as_bytes());
            put_bytes(&mut out, data);
        }
        out
    }

    fn decode(mut buf: &[u8]) -> Result<Self, VfsError> {
        let entry = take_string(&mut buf)?;
        let permission_args = (0..take_len(&mut buf)?)
            .map(|_| take_string(&mut buf))
            .collect::<Result<_, _>>()?;
        let mut files = BTreeMap::new();
        for _ in 0..take_len(&mut buf)? {
            let path = take_string(&mut buf)?;
            files.insert(path, take_bytes(&mut buf)?.to_vec());
        }
        if !buf.is_empty() {
            return Err(VfsError::Corrupt("trailing bytes"));
        }
        Ok(Self { entry, permission_args, files })
    }

    /// Returns `host_exe` followed by this payload and its trailer.
    pub fn append_to(&self, mut host_exe: Vec<u8>) -> Vec<u8> {
        let body = self.encode();
        host_exe.extend_from_slice(&body);
        host_exe.extend_from_slice(&Sha256::digest(&body));
        host_exe.extend_from_slice(&(body.len() as u64).to_le_bytes());
        host_exe.extend_from_slice(MAGIC);
        host_exe
    }

    /// Reads only the tail of `path`. `Ok(None)` when the file carries no payload.
    pub fn read_from(path: &Path) -> Result<Option<Self>, VfsError> {
        let mut file = File::open(path)?;
        let total = file.metadata()?.len();
        if total < TRAILER_LEN {
            return Ok(None);
        }
        file.seek(SeekFrom::End(-(TRAILER_LEN as i64)))?;
        let mut trailer = [0u8; TRAILER_LEN as usize];
        file.read_exact(&mut trailer)?;
        let (hash, rest) = trailer.split_at(HASH_LEN);
        let (len, magic) = rest.split_at(8);
        if magic != MAGIC {
            return Ok(None);
        }
        let len = u64::from_le_bytes(len.try_into().expect("8 bytes"));
        if len > total - TRAILER_LEN {
            return Err(VfsError::Corrupt("payload length exceeds file"));
        }
        file.seek(SeekFrom::End(-((TRAILER_LEN + len) as i64)))?;
        let mut body = vec![0u8; len as usize];
        file.read_exact(&mut body)?;
        if Sha256::digest(&body).as_slice() != hash {
            return Err(VfsError::HashMismatch);
        }
        Self::decode(&body).map(Some)
    }
}

/// Read-only view over the payload files.
#[derive(Debug, Clone)]
pub struct Vfs {
    files: BTreeMap<String, Vec<u8>>,
}

impl Vfs {
    pub fn new(files: BTreeMap<String, Vec<u8>>) -> Self {
        Self { files }
    }

    pub fn read(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u64).to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_len(out, bytes.len());
    out.extend_from_slice(bytes);
}

fn take_len(buf: &mut &[u8]) -> Result<usize, VfsError> {
    let (head, rest) = buf.split_at_checked(8).ok_or(VfsError::Corrupt("truncated length"))?;
    *buf = rest;
    usize::try_from(u64::from_le_bytes(head.try_into().expect("8 bytes")))
        .map_err(|_| VfsError::Corrupt("length overflow"))
}

fn take_bytes<'a>(buf: &mut &'a [u8]) -> Result<&'a [u8], VfsError> {
    let len = take_len(buf)?;
    let (head, rest) = buf.split_at_checked(len).ok_or(VfsError::Corrupt("truncated data"))?;
    *buf = rest;
    Ok(head)
}

fn take_string(buf: &mut &[u8]) -> Result<String, VfsError> {
    String::from_utf8(take_bytes(buf)?.to_vec()).map_err(|_| VfsError::Corrupt("invalid utf-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Payload {
        Payload {
            entry: "/app/main.js".into(),
            permission_args: vec!["--allow-read=assets".into()],
            files: BTreeMap::from([
                ("/app/main.js".into(), b"console.log(1)".to_vec()),
                ("/app/assets/a.txt".into(), vec![0, 255, 1]),
            ]),
        }
    }

    fn write_exe(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn round_trips_through_a_file() {
        let (_d, path) = write_exe(&sample().append_to(b"host binary".to_vec()));
        assert_eq!(Payload::read_from(&path).unwrap(), Some(sample()));
    }

    #[test]
    fn plain_binary_has_no_payload() {
        let (_d, path) = write_exe(&[7u8; 200]);
        assert_eq!(Payload::read_from(&path).unwrap(), None);
        let (_d, tiny) = write_exe(b"x");
        assert_eq!(Payload::read_from(&tiny).unwrap(), None);
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let mut bytes = sample().append_to(b"host".to_vec());
        bytes[6] ^= 0xff;
        let (_d, path) = write_exe(&bytes);
        assert_eq!(Payload::read_from(&path), Err(VfsError::HashMismatch));
    }

    #[test]
    fn oversized_length_is_corrupt_not_a_panic() {
        let mut bytes = sample().append_to(Vec::new());
        let at = bytes.len() - MAGIC.len() - 8;
        bytes[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let (_d, path) = write_exe(&bytes);
        assert!(matches!(Payload::read_from(&path), Err(VfsError::Corrupt(_))));
    }

    #[test]
    fn vfs_is_read_only_lookup() {
        let vfs = Vfs::new(sample().files);
        assert_eq!(vfs.read("/app/main.js"), Some(&b"console.log(1)"[..]));
        assert!(!vfs.exists("/nope"));
    }
}
