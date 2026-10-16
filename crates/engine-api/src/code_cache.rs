//! Engine-agnostic code cache contract (VELYR-60, experimental).
//!
//! An engine may serialize the compiled form of a module so another isolate of the same process
//! skips parsing. Engines validate cached bytes by length at best, so every blob travels in an
//! envelope that binds it to the exact source text and to its own checksum; bytes that fail the
//! check never reach the engine.

use std::hash::{DefaultHasher, Hasher};

const MAGIC: &[u8; 4] = b"VCC1";
const HEADER: usize = 4 + 8 + 8 + 8;

/// How many cached blobs an isolate accepted or rejected so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeCacheStats {
    pub accepted: u32,
    pub rejected: u32,
}

fn digest(bytes: &[u8]) -> u64 {
    // In-process only: `DefaultHasher::new()` is deterministic within one build.
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// Identity of a source text for [`seal`] and [`open`].
pub fn source_digest(source_text: &str) -> u64 {
    digest(source_text.as_bytes())
}

/// Wraps `payload` (engine bytes produced for the source with this `source_digest`) in a
/// self-checking envelope.
pub fn seal(source_digest: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&source_digest.to_le_bytes());
    out.extend_from_slice(&digest(payload).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// The engine payload of `blob`, only if it was sealed for this `source_digest` and is intact.
pub fn open(source_digest: u64, blob: &[u8]) -> Option<&[u8]> {
    let word = |at: usize| Some(u64::from_le_bytes(blob.get(at..at + 8)?.try_into().ok()?));
    if blob.get(..4)? != MAGIC || word(4)? != source_digest {
        return None;
    }
    let payload = blob.get(HEADER..)?;
    (word(20)? == payload.len() as u64 && word(12)? == digest(payload)).then_some(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> u64 {
        source_digest(text)
    }

    #[test]
    fn round_trips_for_the_same_source() {
        let blob = seal(key("export const a = 1;"), b"payload");
        assert_eq!(
            open(key("export const a = 1;"), &blob),
            Some(&b"payload"[..])
        );
    }

    #[test]
    fn rejects_same_length_different_source() {
        let blob = seal(key("export const a = 1;"), b"payload");
        assert_eq!(open(key("export const a = 2;"), &blob), None);
    }

    #[test]
    fn rejects_garbage_truncated_and_flipped_bits() {
        let blob = seal(key("x"), b"payload");
        assert_eq!(open(key("x"), b"garbage"), None);
        assert_eq!(open(key("x"), &[]), None);
        assert_eq!(open(key("x"), &blob[..blob.len() - 1]), None);
        for index in 0..blob.len() {
            let mut flipped = blob.clone();
            flipped[index] ^= 1;
            assert_eq!(
                open(key("x"), &flipped),
                None,
                "bit flip at {index} was accepted"
            );
        }
    }
}
