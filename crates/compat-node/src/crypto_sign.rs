//! `crypto.sign` / `crypto.verify` on aws-lc-rs: RSA (PKCS#1 v1.5 and PSS), ECDSA (DER) and
//! Ed25519. aws-lc-rs hashes the message itself, so the whole message is passed in.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    self, ECDSA_P256_SHA1_ASN1, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING,
    ECDSA_P256_SHA384_ASN1, ECDSA_P256_SHA512_ASN1, ECDSA_P384_SHA256_ASN1, ECDSA_P384_SHA384_ASN1,
    ECDSA_P384_SHA384_ASN1_SIGNING, ECDSA_P384_SHA512_ASN1, ECDSA_P521_SHA1_ASN1,
    ECDSA_P521_SHA224_ASN1, ECDSA_P521_SHA256_ASN1, ECDSA_P521_SHA256_ASN1_SIGNING,
    ECDSA_P521_SHA384_ASN1, ECDSA_P521_SHA384_ASN1_SIGNING, ECDSA_P521_SHA512_ASN1,
    ECDSA_P521_SHA512_ASN1_SIGNING, ED25519, EcdsaKeyPair, EcdsaSigningAlgorithm,
    EcdsaVerificationAlgorithm, RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY,
    RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512,
    RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512, RSA_PSS_2048_8192_SHA256,
    RSA_PSS_2048_8192_SHA384, RSA_PSS_2048_8192_SHA512, RSA_PSS_SHA256, RSA_PSS_SHA384,
    RSA_PSS_SHA512, RsaEncoding, RsaParameters, UnparsedPublicKey,
};

use crate::crypto_keys::{Curve, Kind, inspect, rsa_pair};

type Parts = Result<Vec<Vec<u8>>, String>;

fn unsupported() -> String {
    "ERR_CRYPTO_UNSUPPORTED_OPERATION|This hash and key combination is not supported".into()
}

fn rsa_signing(hash: &str, pss: bool) -> Result<&'static dyn RsaEncoding, String> {
    Ok(match (hash, pss) {
        ("sha256", false) => &RSA_PKCS1_SHA256,
        ("sha384", false) => &RSA_PKCS1_SHA384,
        ("sha512", false) => &RSA_PKCS1_SHA512,
        ("sha256", true) => &RSA_PSS_SHA256,
        ("sha384", true) => &RSA_PSS_SHA384,
        ("sha512", true) => &RSA_PSS_SHA512,
        _ => return Err(unsupported()),
    })
}

fn rsa_verification(hash: &str, pss: bool) -> Result<&'static RsaParameters, String> {
    Ok(match (hash, pss) {
        ("sha1", false) => &RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY,
        ("sha256", false) => &RSA_PKCS1_2048_8192_SHA256,
        ("sha384", false) => &RSA_PKCS1_2048_8192_SHA384,
        ("sha512", false) => &RSA_PKCS1_2048_8192_SHA512,
        ("sha256", true) => &RSA_PSS_2048_8192_SHA256,
        ("sha384", true) => &RSA_PSS_2048_8192_SHA384,
        ("sha512", true) => &RSA_PSS_2048_8192_SHA512,
        _ => return Err(unsupported()),
    })
}

/// aws-lc-rs signs with the digest each curve is paired with (P-521 also with SHA-256/384).
fn ecdsa_signing(curve: Curve, hash: &str) -> Result<&'static EcdsaSigningAlgorithm, String> {
    Ok(match (curve, hash) {
        (Curve::P256, "sha256") => &ECDSA_P256_SHA256_ASN1_SIGNING,
        (Curve::P384, "sha384") => &ECDSA_P384_SHA384_ASN1_SIGNING,
        (Curve::P521, "sha256") => &ECDSA_P521_SHA256_ASN1_SIGNING,
        (Curve::P521, "sha384") => &ECDSA_P521_SHA384_ASN1_SIGNING,
        (Curve::P521, "sha512") => &ECDSA_P521_SHA512_ASN1_SIGNING,
        _ => return Err(unsupported()),
    })
}

fn ecdsa_verification(
    curve: Curve,
    hash: &str,
) -> Result<&'static EcdsaVerificationAlgorithm, String> {
    Ok(match (curve, hash) {
        (Curve::P256, "sha1") => &ECDSA_P256_SHA1_ASN1,
        (Curve::P256, "sha256") => &ECDSA_P256_SHA256_ASN1,
        (Curve::P256, "sha384") => &ECDSA_P256_SHA384_ASN1,
        (Curve::P256, "sha512") => &ECDSA_P256_SHA512_ASN1,
        (Curve::P384, "sha256") => &ECDSA_P384_SHA256_ASN1,
        (Curve::P384, "sha384") => &ECDSA_P384_SHA384_ASN1,
        (Curve::P384, "sha512") => &ECDSA_P384_SHA512_ASN1,
        (Curve::P521, "sha1") => &ECDSA_P521_SHA1_ASN1,
        (Curve::P521, "sha224") => &ECDSA_P521_SHA224_ASN1,
        (Curve::P521, "sha256") => &ECDSA_P521_SHA256_ASN1,
        (Curve::P521, "sha384") => &ECDSA_P521_SHA384_ASN1,
        (Curve::P521, "sha512") => &ECDSA_P521_SHA512_ASN1,
        _ => return Err(unsupported()),
    })
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `sign(hash, pkcs8, data, padding) -> [signature]`; `padding` is `pss` or anything else for
/// PKCS#1 v1.5 (RSA only). `hash` is ignored for Ed25519.
pub(crate) fn sign(args: &[Vec<u8>]) -> Parts {
    let [hash, pkcs8, data, padding] = args else {
        return Err("sign expects (hash, pkcs8, data, padding)".into());
    };
    let hash = text(hash);
    let (kind, _) = inspect(pkcs8, true)?;
    match kind {
        Kind::Rsa => {
            let pair = rsa_pair(pkcs8)?;
            let mut out = vec![0u8; pair.public_modulus_len()];
            pair.sign(
                rsa_signing(&hash, padding == b"pss")?,
                &SystemRandom::new(),
                data,
                &mut out,
            )
            .map_err(|_| "Signing failed".to_string())?;
            Ok(vec![out])
        }
        Kind::Ec(curve) => {
            let pair = EcdsaKeyPair::from_pkcs8(ecdsa_signing(curve, &hash)?, pkcs8)
                .map_err(|_| "Invalid private key".to_string())?;
            let out = pair
                .sign(&SystemRandom::new(), data)
                .map_err(|_| "Signing failed".to_string())?;
            Ok(vec![out.as_ref().to_vec()])
        }
        Kind::Ed25519 => {
            let pair = signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8)
                .map_err(|_| "Invalid private key".to_string())?;
            Ok(vec![pair.sign(data).as_ref().to_vec()])
        }
    }
}

/// `verify(hash, spki, data, signature, padding) -> [flag]`; a bad signature is `[0]`, not an error.
pub(crate) fn verify(args: &[Vec<u8>]) -> Parts {
    let [hash, spki, data, signature, padding] = args else {
        return Err("verify expects (hash, spki, data, signature, padding)".into());
    };
    let hash = text(hash);
    let (kind, public) = inspect(spki, false)?;
    let key = match kind {
        Kind::Rsa => UnparsedPublicKey::new(rsa_verification(&hash, padding == b"pss")?, &public)
            .verify(data, signature),
        Kind::Ec(curve) => UnparsedPublicKey::new(ecdsa_verification(curve, &hash)?, &public)
            .verify(data, signature),
        Kind::Ed25519 => UnparsedPublicKey::new(&ED25519, &public).verify(data, signature),
    };
    Ok(vec![vec![u8::from(key.is_ok())]])
}
