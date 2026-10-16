//! Key agreement (ECDH, X25519) and key derivation (PBKDF2, HKDF) over raw bytes.
use std::num::NonZeroU32;

use aws_lc_rs::agreement::{self, PrivateKey, UnparsedPublicKey};
use aws_lc_rs::{hkdf, pbkdf2};

use crate::der::Kind;
use crate::error::{CryptoError, CryptoResult};
use crate::keys::agreement_algorithm;

/// Upper bound on derived output, checked before anything is allocated (the length comes straight from JS).
const MAX_DERIVED_BYTES: usize = 1 << 20;

/// Upper bound on PBKDF2 iterations: the work runs on the blocking pool and cannot be cancelled.
const MAX_PBKDF2_ITERATIONS: u32 = 10_000_000;

fn number(bytes: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(bytes).parse().ok()
}

fn length(bytes: &[u8]) -> CryptoResult<usize> {
    number(bytes)
        .filter(|&size| size <= MAX_DERIVED_BYTES)
        .ok_or_else(|| CryptoError::operation("Invalid or too large derived key length"))
}

/// `ecdh_derive(kind, private, peer_public) -> [shared secret]`
pub(crate) fn agree(args: &[Vec<u8>]) -> CryptoResult {
    let [kind, private, peer] = args else {
        return Err(CryptoError::operation(
            "ecdh_derive expects (kind, private, peer_public)",
        ));
    };
    let algorithm = agreement_algorithm(Kind::parse(kind)?);
    let key = PrivateKey::from_private_key(algorithm, private)
        .map_err(|_| CryptoError::data("Invalid private key"))?;
    let secret = agreement::agree(
        &key,
        UnparsedPublicKey::new(algorithm, peer),
        (),
        |secret| Ok(secret.to_vec()),
    )
    .map_err(|()| CryptoError::operation("Key agreement failed"))?;
    Ok(vec![secret])
}

/// `pbkdf2(hash, password, salt, iterations, length) -> [bytes]`
pub(crate) fn pbkdf2(args: &[Vec<u8>]) -> CryptoResult {
    let [hash, password, salt, iterations, length_arg] = args else {
        return Err(CryptoError::operation(
            "pbkdf2 expects (hash, password, salt, iterations, length)",
        ));
    };
    let algorithm = match hash.as_slice() {
        b"SHA-1" => pbkdf2::PBKDF2_HMAC_SHA1,
        b"SHA-256" => pbkdf2::PBKDF2_HMAC_SHA256,
        b"SHA-384" => pbkdf2::PBKDF2_HMAC_SHA384,
        b"SHA-512" => pbkdf2::PBKDF2_HMAC_SHA512,
        _ => return Err(CryptoError::not_supported("Unrecognized hash")),
    };
    let iterations = number(iterations)
        .and_then(|count| u32::try_from(count).ok())
        .filter(|&count| count <= MAX_PBKDF2_ITERATIONS)
        .and_then(NonZeroU32::new)
        .ok_or_else(|| {
            CryptoError::operation("PBKDF2 iterations must be between 1 and 10000000")
        })?;
    let mut out = vec![0u8; length(length_arg)?];
    pbkdf2::derive(algorithm, iterations, salt, password, &mut out);
    Ok(vec![out])
}

struct OutputLength(usize);

impl hkdf::KeyType for OutputLength {
    fn len(&self) -> usize {
        self.0
    }
}

/// `hkdf(hash, ikm, salt, info, length) -> [bytes]`
pub(crate) fn hkdf(args: &[Vec<u8>]) -> CryptoResult {
    let [hash, ikm, salt, info, length_arg] = args else {
        return Err(CryptoError::operation(
            "hkdf expects (hash, ikm, salt, info, length)",
        ));
    };
    let algorithm = match hash.as_slice() {
        b"SHA-1" => hkdf::HKDF_SHA1_FOR_LEGACY_USE_ONLY,
        b"SHA-256" => hkdf::HKDF_SHA256,
        b"SHA-384" => hkdf::HKDF_SHA384,
        b"SHA-512" => hkdf::HKDF_SHA512,
        _ => return Err(CryptoError::not_supported("Unrecognized hash")),
    };
    let size = length(length_arg)?;
    let prk = hkdf::Salt::new(algorithm, salt).extract(ikm);
    let info = [info.as_slice()];
    let okm = prk
        .expand(&info, OutputLength(size))
        .map_err(|_| CryptoError::operation("HKDF output length is too large"))?;
    let mut out = vec![0u8; size];
    okm.fill(&mut out)
        .map_err(|_| CryptoError::operation("HKDF failed"))?;
    Ok(vec![out])
}
