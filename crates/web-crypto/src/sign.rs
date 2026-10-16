//! HMAC, ECDSA (IEEE P1363 `r || s`) and Ed25519 over raw key bytes. `verify` reports a bad signature as `[1]`/`[0]`.
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING, ECDSA_P384_SHA384_FIXED,
    ECDSA_P384_SHA384_FIXED_SIGNING, ECDSA_P521_SHA256_FIXED, ECDSA_P521_SHA256_FIXED_SIGNING,
    ECDSA_P521_SHA384_FIXED, ECDSA_P521_SHA384_FIXED_SIGNING, ECDSA_P521_SHA512_FIXED,
    ECDSA_P521_SHA512_FIXED_SIGNING, ED25519, EcdsaKeyPair, EcdsaSigningAlgorithm,
    EcdsaVerificationAlgorithm, Ed25519KeyPair, UnparsedPublicKey,
};
use aws_lc_rs::{hmac, rand};

use crate::der::Kind;
use crate::digest;
use crate::error::{CryptoError, CryptoResult};

fn flag(ok: bool) -> CryptoResult {
    Ok(vec![vec![u8::from(ok)]])
}

fn hmac_key(hash: &[u8], key: &[u8]) -> CryptoResult<hmac::Key> {
    let name = String::from_utf8_lossy(hash);
    let algorithm = match name.as_ref() {
        "SHA-1" => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
        "SHA-256" => hmac::HMAC_SHA256,
        "SHA-384" => hmac::HMAC_SHA384,
        "SHA-512" => hmac::HMAC_SHA512,
        other => {
            return Err(CryptoError::not_supported(format!(
                "Unrecognized hash {other}"
            )));
        }
    };
    Ok(hmac::Key::new(algorithm, key))
}

/// `hmac_sign(hash, key, data) -> [tag]`
pub(crate) fn hmac_sign(args: &[Vec<u8>]) -> CryptoResult {
    let [hash, key, data] = args else {
        return Err(CryptoError::operation(
            "hmac_sign expects (hash, key, data)",
        ));
    };
    Ok(vec![
        hmac::sign(&hmac_key(hash, key)?, data).as_ref().to_vec(),
    ])
}

/// `hmac_verify(hash, key, data, tag) -> [flag]` (constant time)
pub(crate) fn hmac_verify(args: &[Vec<u8>]) -> CryptoResult {
    let [hash, key, data, tag] = args else {
        return Err(CryptoError::operation(
            "hmac_verify expects (hash, key, data, tag)",
        ));
    };
    flag(hmac::verify(&hmac_key(hash, key)?, data, tag).is_ok())
}

/// aws-lc-rs binds each ECDSA curve to fixed digests: P-256/SHA-256, P-384/SHA-384 and P-521 with SHA-256, SHA-384
/// or SHA-512. Anything else is NotSupportedError rather than a signature over a different hash.
fn ecdsa_algorithms(
    kind: Kind,
    hash: &[u8],
) -> CryptoResult<(
    &'static EcdsaSigningAlgorithm,
    &'static EcdsaVerificationAlgorithm,
)> {
    digest::algorithm(&String::from_utf8_lossy(hash))?;
    Ok(match (kind, hash) {
        (Kind::P256, b"SHA-256") => (&ECDSA_P256_SHA256_FIXED_SIGNING, &ECDSA_P256_SHA256_FIXED),
        (Kind::P384, b"SHA-384") => (&ECDSA_P384_SHA384_FIXED_SIGNING, &ECDSA_P384_SHA384_FIXED),
        (Kind::P521, b"SHA-256") => (&ECDSA_P521_SHA256_FIXED_SIGNING, &ECDSA_P521_SHA256_FIXED),
        (Kind::P521, b"SHA-384") => (&ECDSA_P521_SHA384_FIXED_SIGNING, &ECDSA_P521_SHA384_FIXED),
        (Kind::P521, b"SHA-512") => (&ECDSA_P521_SHA512_FIXED_SIGNING, &ECDSA_P521_SHA512_FIXED),
        _ => {
            return Err(CryptoError::not_supported(
                "ECDSA with this curve and hash combination is not supported",
            ));
        }
    })
}

/// `ecdsa_sign(curve, hash, d, public, data) -> [signature]`
pub(crate) fn ecdsa_sign(args: &[Vec<u8>]) -> CryptoResult {
    let [curve, hash, d, public, data] = args else {
        return Err(CryptoError::operation(
            "ecdsa_sign expects (curve, hash, d, public, data)",
        ));
    };
    let (signing, _) = ecdsa_algorithms(Kind::parse(curve)?, hash)?;
    let pair = EcdsaKeyPair::from_private_key_and_public_key(signing, d, public)
        .map_err(|_| CryptoError::data("Invalid EC private key"))?;
    Ok(vec![
        pair.sign(&rand::SystemRandom::new(), data)?
            .as_ref()
            .to_vec(),
    ])
}

/// `ecdsa_verify(curve, hash, public, signature, data) -> [flag]`
pub(crate) fn ecdsa_verify(args: &[Vec<u8>]) -> CryptoResult {
    let [curve, hash, public, signature, data] = args else {
        return Err(CryptoError::operation(
            "ecdsa_verify expects (curve, hash, public, signature, data)",
        ));
    };
    let (_, verification) = ecdsa_algorithms(Kind::parse(curve)?, hash)?;
    flag(
        UnparsedPublicKey::new(verification, public)
            .verify(data, signature)
            .is_ok(),
    )
}

/// `ed25519_sign(seed, data) -> [signature]`
pub(crate) fn ed25519_sign(args: &[Vec<u8>]) -> CryptoResult {
    let [seed, data] = args else {
        return Err(CryptoError::operation("ed25519_sign expects (seed, data)"));
    };
    let pair = Ed25519KeyPair::from_seed_unchecked(seed)
        .map_err(|_| CryptoError::data("Invalid Ed25519 private key"))?;
    Ok(vec![pair.sign(data).as_ref().to_vec()])
}

/// `ed25519_verify(public, signature, data) -> [flag]`
pub(crate) fn ed25519_verify(args: &[Vec<u8>]) -> CryptoResult {
    let [public, signature, data] = args else {
        return Err(CryptoError::operation(
            "ed25519_verify expects (public, signature, data)",
        ));
    };
    flag(
        UnparsedPublicKey::new(&ED25519, public)
            .verify(data, signature)
            .is_ok(),
    )
}
