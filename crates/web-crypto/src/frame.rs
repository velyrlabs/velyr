use crate::error::{CryptoError, CryptoResult};

/// Op input and output are lists of byte strings in one buffer: each part is a big-endian `u32` length plus bytes.
pub(crate) fn unframe(mut bytes: &[u8]) -> CryptoResult<Vec<Vec<u8>>> {
    let mut parts = Vec::new();
    while !bytes.is_empty() {
        let malformed = || CryptoError::operation("malformed op input");
        let (len, rest) = bytes.split_first_chunk::<4>().ok_or_else(malformed)?;
        let len = u32::from_be_bytes(*len) as usize;
        if rest.len() < len {
            return Err(malformed());
        }
        let (part, rest) = rest.split_at(len);
        parts.push(part.to_vec());
        bytes = rest;
    }
    Ok(parts)
}

pub(crate) fn frame(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_truncation() {
        let parts = vec![b"ab".to_vec(), Vec::new(), b"xyz".to_vec()];
        assert_eq!(unframe(&frame(&parts)).unwrap(), parts);
        assert!(unframe(&[0, 0, 0, 5, 1]).is_err());
        assert!(unframe(&[0, 0]).is_err());
    }
}
