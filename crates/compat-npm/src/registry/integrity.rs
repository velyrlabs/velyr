use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sha1::Sha1;
use sha2::{Digest, Sha512};
use velyr_runtime_errors::VelyrError;

use super::packument::Dist;

/// Expected digest of a tarball. sha1 is accepted only when no sha512 is published.
pub(super) enum Expected {
    Sha512(Vec<u8>),
    Sha1(Vec<u8>),
}

pub(super) enum Hasher {
    Sha512(Sha512),
    Sha1(Sha1),
}

impl Expected {
    pub(super) fn from_dist(dist: &Dist) -> Result<Self, VelyrError> {
        let sri = dist.integrity.as_deref().unwrap_or_default();
        if let Some(digest) = sha512_digest(sri) {
            return Ok(Self::Sha512(digest));
        }
        let sha1 = dist
            .shasum
            .as_deref()
            .and_then(decode_hex)
            .filter(|d| d.len() == 20);
        let Some(digest) = sha1 else {
            return Err(VelyrError::invalid_data(
                "package has no usable sha512 or sha1 checksum",
            ));
        };
        tracing::warn!(target: "velyr.compat-npm.download", "no sha512 integrity published, falling back to sha1");
        Ok(Self::Sha1(digest))
    }

    pub(super) fn hasher(&self) -> Hasher {
        match self {
            Self::Sha512(_) => Hasher::Sha512(Sha512::new()),
            Self::Sha1(_) => Hasher::Sha1(Sha1::new()),
        }
    }

    pub(super) fn matches(&self, hasher: Hasher) -> bool {
        match (self, hasher) {
            (Self::Sha512(want), Hasher::Sha512(h)) => h.finalize().as_slice() == want.as_slice(),
            (Self::Sha1(want), Hasher::Sha1(h)) => h.finalize().as_slice() == want.as_slice(),
            _ => false,
        }
    }
}

impl Hasher {
    pub(super) fn update(&mut self, chunk: &[u8]) {
        match self {
            Self::Sha512(h) => h.update(chunk),
            Self::Sha1(h) => h.update(chunk),
        }
    }
}

/// First well-formed sha512 digest of an SRI string, if any.
pub(crate) fn sha512_digest(sri: &str) -> Option<Vec<u8>> {
    sri.split_whitespace()
        .filter_map(|entry| entry.strip_prefix("sha512-"))
        .filter_map(|b64| STANDARD.decode(b64.split('?').next()?).ok())
        .find(|digest| digest.len() == 64)
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}
