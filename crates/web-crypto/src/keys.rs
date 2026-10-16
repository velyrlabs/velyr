//! Key material in its canonical raw form: EC private = scalar `d`, public = uncompressed point;
//! Ed25519/X25519 private = 32-byte seed, public = 32 bytes. The JS layer holds these bytes in a `CryptoKey`.
use aws_lc_rs::agreement::{self, PrivateKey, UnparsedPublicKey};
use aws_lc_rs::encoding::{AsBigEndian, Curve25519SeedBin, EcPrivateKeyBin};
use aws_lc_rs::rand;
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, ECDSA_P384_SHA384_FIXED_SIGNING,
    ECDSA_P521_SHA512_FIXED_SIGNING, EcdsaKeyPair, EcdsaSigningAlgorithm, Ed25519KeyPair, KeyPair,
};

use crate::der::{self, Kind};
use crate::error::{CryptoError, CryptoResult};

pub(crate) fn ecdsa_algorithm(kind: Kind) -> &'static EcdsaSigningAlgorithm {
    match kind {
        Kind::P384 => &ECDSA_P384_SHA384_FIXED_SIGNING,
        Kind::P521 => &ECDSA_P521_SHA512_FIXED_SIGNING,
        _ => &ECDSA_P256_SHA256_FIXED_SIGNING,
    }
}

pub(crate) fn agreement_algorithm(kind: Kind) -> &'static agreement::Algorithm {
    match kind {
        Kind::P256 => &agreement::ECDH_P256,
        Kind::P384 => &agreement::ECDH_P384,
        Kind::P521 => &agreement::ECDH_P521,
        _ => &agreement::X25519,
    }
}

fn ec_parts(pair: &EcdsaKeyPair) -> CryptoResult {
    let d: EcPrivateKeyBin = pair.private_key().as_be_bytes()?;
    Ok(vec![
        d.as_ref().to_vec(),
        pair.public_key().as_ref().to_vec(),
    ])
}

fn x25519_parts(key: &PrivateKey) -> CryptoResult {
    let seed: Curve25519SeedBin = key.as_be_bytes()?;
    Ok(vec![
        seed.as_ref().to_vec(),
        key.compute_public_key()?.as_ref().to_vec(),
    ])
}

fn invalid_private() -> CryptoError {
    CryptoError::data("Invalid private key")
}

/// `key_generate(kind) -> [private, public]`
pub(crate) fn generate(args: &[Vec<u8>]) -> CryptoResult {
    let [kind] = args else {
        return Err(CryptoError::operation("key_generate expects (kind)"));
    };
    match Kind::parse(kind)? {
        kind if kind.is_ec() => ec_parts(&EcdsaKeyPair::generate(ecdsa_algorithm(kind))?),
        Kind::Ed25519 => {
            let mut seed = [0u8; 32];
            rand::fill(&mut seed)?;
            public(&[b"Ed25519".to_vec(), seed.to_vec()]).map(|p| vec![seed.to_vec(), p[0].clone()])
        }
        _ => x25519_parts(&PrivateKey::generate(&agreement::X25519)?),
    }
}

/// `key_public(kind, private) -> [public]`; fails with DataError when `private` is not a valid key.
pub(crate) fn public(args: &[Vec<u8>]) -> CryptoResult {
    let [kind, private] = args else {
        return Err(CryptoError::operation("key_public expects (kind, private)"));
    };
    let kind = Kind::parse(kind)?;
    let point = if kind == Kind::Ed25519 {
        let pair = Ed25519KeyPair::from_seed_unchecked(private).map_err(|_| invalid_private())?;
        pair.public_key().as_ref().to_vec()
    } else {
        let key = PrivateKey::from_private_key(agreement_algorithm(kind), private)
            .map_err(|_| invalid_private())?;
        key.compute_public_key()?.as_ref().to_vec()
    };
    Ok(vec![point])
}

/// Rejects a public key that is not a valid point of the curve (EC) or has the wrong length (curve25519).
fn check_public(kind: Kind, key: &[u8]) -> CryptoResult<()> {
    let valid = match kind {
        Kind::Ed25519 => key.len() == 32,
        _ => agreement::ParsedPublicKey::try_from(&UnparsedPublicKey::new(
            agreement_algorithm(kind),
            key,
        ))
        .is_ok(),
    };
    if valid {
        Ok(())
    } else {
        Err(CryptoError::data("Invalid public key"))
    }
}

/// `key_encode(kind, format, private, public) -> [der]` for `pkcs8` (private + public) and `spki` (public).
pub(crate) fn encode(args: &[Vec<u8>]) -> CryptoResult {
    let [kind, format, private, public] = args else {
        return Err(CryptoError::operation(
            "key_encode expects (kind, format, private, public)",
        ));
    };
    let kind = Kind::parse(kind)?;
    match format.as_slice() {
        b"pkcs8" => Ok(vec![der::pkcs8_encode(kind, private, public)]),
        b"spki" => Ok(vec![der::spki_encode(kind, public)]),
        _ => Err(CryptoError::not_supported("Unsupported key format")),
    }
}

/// `key_decode(kind, format, der) -> [private, public]` (pkcs8) or `[public]` (spki); everything is validated.
pub(crate) fn decode(args: &[Vec<u8>]) -> CryptoResult {
    let [kind_name, format, bytes] = args else {
        return Err(CryptoError::operation(
            "key_decode expects (kind, format, der)",
        ));
    };
    let kind = Kind::parse(kind_name)?;
    match format.as_slice() {
        b"spki" => {
            let key = der::spki_decode(kind, bytes)?;
            check_public(kind, &key)?;
            Ok(vec![key])
        }
        b"pkcs8" if kind.is_ec() => {
            let pair = EcdsaKeyPair::from_pkcs8(ecdsa_algorithm(kind), bytes)
                .map_err(|_| invalid_private())?;
            ec_parts(&pair)
        }
        b"pkcs8" => {
            let seed = der::pkcs8_payload(kind, bytes)?;
            let public = public(&[kind_name.clone(), seed.clone()])?;
            Ok(vec![seed, public[0].clone()])
        }
        _ => Err(CryptoError::not_supported("Unsupported key format")),
    }
}

/// `key_check_public(kind, public) -> []`: validates a raw public key (JWK / raw import).
pub(crate) fn validate_public(args: &[Vec<u8>]) -> CryptoResult {
    let [kind, key] = args else {
        return Err(CryptoError::operation(
            "key_check_public expects (kind, public)",
        ));
    };
    check_public(Kind::parse(kind)?, key)?;
    Ok(Vec::new())
}
