//! RSA-PSS, RSASSA-PKCS1-v1_5 and RSA-OAEP over raw key bytes. Canonical material: the private key is a PKCS#1
//! `RSAPrivateKey`, the public key a PKCS#1 `RSAPublicKey`. aws-lc-rs limits this to a 65537 public exponent for
//! generation, 2048..8192-bit moduli, PSS with a digest-length salt and no SHA-1 for PSS, OAEP or signing.
use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rand;
use aws_lc_rs::rsa::{
    KeyPair, KeySize, OAEP_SHA1_MGF1SHA1, OAEP_SHA256_MGF1SHA256, OAEP_SHA384_MGF1SHA384,
    OAEP_SHA512_MGF1SHA512, OaepAlgorithm, OaepPrivateDecryptingKey, OaepPublicEncryptingKey,
    PrivateDecryptingKey, PublicEncryptingKey, PublicKeyComponents,
};
use aws_lc_rs::signature::{
    KeyPair as _, RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY, RSA_PKCS1_2048_8192_SHA256,
    RSA_PKCS1_2048_8192_SHA384, RSA_PKCS1_2048_8192_SHA512, RSA_PKCS1_SHA256, RSA_PKCS1_SHA384,
    RSA_PKCS1_SHA512, RSA_PSS_2048_8192_SHA256, RSA_PSS_2048_8192_SHA384, RSA_PSS_2048_8192_SHA512,
    RSA_PSS_SHA256, RSA_PSS_SHA384, RSA_PSS_SHA512, RsaEncoding, RsaParameters,
};

use crate::der;
use crate::error::{CryptoError, CryptoResult};

const MIN_MODULUS_BITS: usize = 2048;
const MAX_MODULUS_BITS: usize = 8192;
const MIN_EXPONENT: u64 = 65537;
const COMPONENTS_PRIVATE: usize = 8;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn invalid_private() -> CryptoError {
    CryptoError::data("Invalid RSA private key")
}

fn parts(pair: &KeyPair) -> CryptoResult {
    let pkcs8: Pkcs8V1Der = pair.as_der()?;
    let pkcs1 = der::pkcs8_unwrap(&der::rsa_algorithm_identifier(), pkcs8.as_ref())?;
    Ok(vec![pkcs1, pair.public_key().as_ref().to_vec()])
}

fn bit_length(magnitude: &[u8]) -> usize {
    magnitude.first().map_or(0, |&top| {
        (magnitude.len() - 1) * 8 + (8 - top.leading_zeros() as usize)
    })
}

fn exponent_value(magnitude: &[u8]) -> u64 {
    if magnitude.len() > 8 {
        u64::MAX
    } else {
        magnitude
            .iter()
            .fold(0, |acc, &b| (acc << 8) | u64::from(b))
    }
}

/// Checks the limits of the verification and OAEP primitives so a key they cannot use is rejected at import.
fn validate_public(pkcs1: &[u8]) -> CryptoResult<()> {
    let numbers = der::read_integers(pkcs1)?;
    let [n, e] = numbers.as_slice() else {
        return Err(CryptoError::data("Invalid RSA public key"));
    };
    if !(MIN_MODULUS_BITS..=MAX_MODULUS_BITS).contains(&bit_length(n)) {
        return Err(CryptoError::not_supported(
            "Only RSA moduli of 2048 to 8192 bits are supported",
        ));
    }
    if exponent_value(e) < MIN_EXPONENT || e.last().is_none_or(|&b| b & 1 == 0) {
        return Err(CryptoError::not_supported(
            "Only RSA public exponents of at least 65537 are supported",
        ));
    }
    Ok(())
}

/// `rsa_generate(modulus_bits, public_exponent) -> [private, public]`
pub(crate) fn generate(args: &[Vec<u8>]) -> CryptoResult {
    let [bits, exponent] = args else {
        return Err(CryptoError::operation(
            "rsa_generate expects (modulus_bits, public_exponent)",
        ));
    };
    let size = match text(bits).as_str() {
        "2048" => KeySize::Rsa2048,
        "3072" => KeySize::Rsa3072,
        "4096" => KeySize::Rsa4096,
        "8192" => KeySize::Rsa8192,
        _ => {
            return Err(CryptoError::not_supported(
                "Only 2048, 3072, 4096 and 8192-bit RSA keys can be generated",
            ));
        }
    };
    let start = exponent
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(exponent.len());
    if exponent_value(&exponent[start..]) != MIN_EXPONENT {
        return Err(CryptoError::not_supported(
            "Only the public exponent 65537 is supported for generation",
        ));
    }
    parts(&KeyPair::generate(size)?)
}

/// `rsa_decode(format, der) -> [private, public]` (pkcs8) or `[public]` (spki)
pub(crate) fn decode(args: &[Vec<u8>]) -> CryptoResult {
    let [format, bytes] = args else {
        return Err(CryptoError::operation("rsa_decode expects (format, der)"));
    };
    match format.as_slice() {
        b"pkcs8" => parts(&KeyPair::from_pkcs8(bytes).map_err(|_| invalid_private())?),
        b"spki" => {
            let pkcs1 = der::spki_unwrap(&der::rsa_algorithm_identifier(), bytes)?;
            validate_public(&pkcs1)?;
            Ok(vec![pkcs1])
        }
        _ => Err(CryptoError::not_supported("Unsupported key format")),
    }
}

/// `rsa_encode(format, private, public) -> [der]`
pub(crate) fn encode(args: &[Vec<u8>]) -> CryptoResult {
    let [format, private, public] = args else {
        return Err(CryptoError::operation(
            "rsa_encode expects (format, private, public)",
        ));
    };
    let algorithm = der::rsa_algorithm_identifier();
    match format.as_slice() {
        b"pkcs8" => Ok(vec![der::pkcs8_wrap(&algorithm, private)]),
        b"spki" => Ok(vec![der::spki_wrap(&algorithm, public)]),
        _ => Err(CryptoError::not_supported("Unsupported key format")),
    }
}

/// `rsa_components(der) -> [n, e]` for a public key, `[n, e, d, p, q, dp, dq, qi]` for a private key.
pub(crate) fn components(args: &[Vec<u8>]) -> CryptoResult {
    let [bytes] = args else {
        return Err(CryptoError::operation("rsa_components expects (der)"));
    };
    let mut numbers = der::read_integers(bytes)?;
    if numbers.len() == COMPONENTS_PRIVATE + 1 {
        numbers.remove(0); // the version
    }
    Ok(numbers)
}

/// `rsa_assemble(n, e)` -> `[public]`; `rsa_assemble(n, e, d, p, q, dp, dq, qi)` -> `[private, public]` (JWK import).
pub(crate) fn assemble(args: &[Vec<u8>]) -> CryptoResult {
    let integers = |numbers: &[Vec<u8>]| -> Vec<u8> {
        numbers
            .iter()
            .flat_map(|n| der::unsigned_integer(n))
            .collect()
    };
    match args.len() {
        2 => {
            let public = der::sequence(&integers(args));
            validate_public(&public)?;
            Ok(vec![public])
        }
        COMPONENTS_PRIVATE => {
            let private = der::sequence(&[der::unsigned_integer(&[0]), integers(args)].concat());
            parts(&KeyPair::from_der(&private).map_err(|_| invalid_private())?)
        }
        _ => Err(CryptoError::data("Invalid RSA key components")),
    }
}

struct Scheme {
    encoding: &'static dyn RsaEncoding,
    verification: &'static RsaParameters,
    digest_len: usize,
}

fn scheme(kind: &[u8], hash: &[u8]) -> CryptoResult<Scheme> {
    let pss = kind == b"pss";
    let (encoding, verification, digest_len): (
        &'static dyn RsaEncoding,
        &'static RsaParameters,
        usize,
    ) = match (pss, hash) {
        (true, b"SHA-256") => (&RSA_PSS_SHA256, &RSA_PSS_2048_8192_SHA256, 32),
        (true, b"SHA-384") => (&RSA_PSS_SHA384, &RSA_PSS_2048_8192_SHA384, 48),
        (true, b"SHA-512") => (&RSA_PSS_SHA512, &RSA_PSS_2048_8192_SHA512, 64),
        (false, b"SHA-256") => (&RSA_PKCS1_SHA256, &RSA_PKCS1_2048_8192_SHA256, 32),
        (false, b"SHA-384") => (&RSA_PKCS1_SHA384, &RSA_PKCS1_2048_8192_SHA384, 48),
        (false, b"SHA-512") => (&RSA_PKCS1_SHA512, &RSA_PKCS1_2048_8192_SHA512, 64),
        _ => {
            return Err(CryptoError::not_supported(
                "RSA signatures with this hash are not supported",
            ));
        }
    };
    Ok(Scheme {
        encoding,
        verification,
        digest_len,
    })
}

/// PSS salts other than the digest length cannot be produced or verified; reject instead of ignoring them.
fn check_salt(kind: &[u8], scheme: &Scheme, salt: &[u8]) -> CryptoResult<()> {
    if kind == b"pss" && text(salt).parse::<usize>().ok() != Some(scheme.digest_len) {
        return Err(CryptoError::not_supported(
            "Only an RSA-PSS salt as long as the digest is supported",
        ));
    }
    Ok(())
}

/// `rsa_sign(kind, hash, private, data, salt) -> [signature]`; `kind` is `pss` or `pkcs1`.
pub(crate) fn sign(args: &[Vec<u8>]) -> CryptoResult {
    let [kind, hash, private, data, salt] = args else {
        return Err(CryptoError::operation(
            "rsa_sign expects (kind, hash, private, data, salt)",
        ));
    };
    let scheme = scheme(kind, hash)?;
    check_salt(kind, &scheme, salt)?;
    let pair = KeyPair::from_der(private).map_err(|_| invalid_private())?;
    let mut signature = vec![0u8; pair.public_modulus_len()];
    pair.sign(
        scheme.encoding,
        &rand::SystemRandom::new(),
        data,
        &mut signature,
    )?;
    Ok(vec![signature])
}

/// `rsa_verify(kind, hash, public, signature, data, salt) -> [flag]`
pub(crate) fn verify(args: &[Vec<u8>]) -> CryptoResult {
    let [kind, hash, public, sig, data, salt] = args else {
        return Err(CryptoError::operation(
            "rsa_verify expects (kind, hash, public, signature, data, salt)",
        ));
    };
    let scheme = match (kind.as_slice(), hash.as_slice()) {
        (b"pkcs1", b"SHA-1") => Scheme {
            encoding: &RSA_PKCS1_SHA256,
            verification: &RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY,
            digest_len: 20,
        },
        _ => scheme(kind, hash)?,
    };
    check_salt(kind, &scheme, salt)?;
    let numbers = der::read_integers(public)?;
    let [n, e] = numbers.as_slice() else {
        return Err(CryptoError::data("Invalid RSA public key"));
    };
    let key = PublicKeyComponents {
        n: n.clone(),
        e: e.clone(),
    };
    Ok(vec![vec![u8::from(
        key.verify(scheme.verification, data, sig).is_ok(),
    )]])
}

fn oaep_algorithm(hash: &[u8]) -> CryptoResult<&'static OaepAlgorithm> {
    match hash {
        b"SHA-1" => Ok(&OAEP_SHA1_MGF1SHA1),
        b"SHA-256" => Ok(&OAEP_SHA256_MGF1SHA256),
        b"SHA-384" => Ok(&OAEP_SHA384_MGF1SHA384),
        b"SHA-512" => Ok(&OAEP_SHA512_MGF1SHA512),
        _ => Err(CryptoError::not_supported(
            "RSA-OAEP with this hash is not supported",
        )),
    }
}

/// `rsa_oaep(mode, hash, key, data, label) -> [output]`; `key` is the public key to encrypt, the private key to decrypt.
pub(crate) fn oaep(args: &[Vec<u8>]) -> CryptoResult {
    let [mode, hash, key, data, label] = args else {
        return Err(CryptoError::operation(
            "rsa_oaep expects (mode, hash, key, data, label)",
        ));
    };
    let algorithm = oaep_algorithm(hash)?;
    let label = (!label.is_empty()).then_some(label.as_slice());
    let failed =
        |_| CryptoError::operation("The operation failed for an operation-specific reason");
    let algorithm_id = der::rsa_algorithm_identifier();
    if mode.as_slice() == b"encrypt" {
        let spki = der::spki_wrap(&algorithm_id, key);
        let public = OaepPublicEncryptingKey::new(
            PublicEncryptingKey::from_der(&spki).map_err(|_| invalid_private())?,
        )?;
        let mut out = vec![0u8; public.ciphertext_size()];
        let len = public
            .encrypt(algorithm, data, &mut out, label)
            .map_err(failed)?
            .len();
        out.truncate(len);
        return Ok(vec![out]);
    }
    let pkcs8 = der::pkcs8_wrap(&algorithm_id, key);
    let private = OaepPrivateDecryptingKey::new(
        PrivateDecryptingKey::from_pkcs8(&pkcs8).map_err(|_| invalid_private())?,
    )?;
    let mut out = vec![0u8; private.min_output_size()];
    let len = private
        .decrypt(algorithm, data, &mut out, label)
        .map_err(failed)?
        .len();
    out.truncate(len);
    Ok(vec![out])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_length_and_exponent_helpers() {
        assert_eq!(bit_length(&[1, 0, 0]), 17);
        assert_eq!(bit_length(&[0x80]), 8);
        assert_eq!(exponent_value(&[1, 0, 1]), 65537);
        assert_eq!(exponent_value(&[1; 9]), u64::MAX);
    }

    #[test]
    fn signature_import_limits_are_enforced() {
        let small = der::sequence(
            &[
                der::unsigned_integer(&[0xff; 128]),
                der::unsigned_integer(&[1, 0, 1]),
            ]
            .concat(),
        );
        assert_eq!(
            validate_public(&small).unwrap_err().name,
            "NotSupportedError"
        );
        let three = der::sequence(
            &[
                der::unsigned_integer(&[0xff; 256]),
                der::unsigned_integer(&[3]),
            ]
            .concat(),
        );
        assert_eq!(
            validate_public(&three).unwrap_err().name,
            "NotSupportedError"
        );
    }
}
