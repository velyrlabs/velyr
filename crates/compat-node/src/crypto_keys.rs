//! Asymmetric key ops for `node:crypto`. Canonical forms: private = PKCS#8 DER, public = SPKI
//! DER. PEM/PKCS#1/SEC1/JWK are converted at the edges; aws-lc-rs parses and validates.

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1_SIGNING, ECDSA_P384_SHA384_ASN1_SIGNING, ECDSA_P521_SHA512_ASN1_SIGNING,
    EcdsaKeyPair, EcdsaSigningAlgorithm, Ed25519KeyPair, KeyPair as _,
};

use crate::crypto_der::{
    self as der, CONTEXT_0, CONTEXT_1, INTEGER, OCTET_STRING, OID_EC, OID_ED25519, OID_P256,
    OID_P384, OID_P521, OID_RSA, SEQUENCE, tlv,
};

type Parts = Result<Vec<Vec<u8>>, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Curve {
    P256,
    P384,
    P521,
}

impl Curve {
    fn from_oid(oid: &[u8]) -> Result<Self, String> {
        match oid {
            OID_P256 => Ok(Self::P256),
            OID_P384 => Ok(Self::P384),
            OID_P521 => Ok(Self::P521),
            _ => Err("Unsupported elliptic curve".into()),
        }
    }

    fn from_jwk(name: &[u8]) -> Result<Self, String> {
        match name {
            b"P-256" => Ok(Self::P256),
            b"P-384" => Ok(Self::P384),
            b"P-521" => Ok(Self::P521),
            _ => Err("Unsupported elliptic curve".into()),
        }
    }

    pub(crate) fn oid(self) -> &'static [u8] {
        match self {
            Self::P256 => OID_P256,
            Self::P384 => OID_P384,
            Self::P521 => OID_P521,
        }
    }

    fn jwk_name(self) -> &'static str {
        match self {
            Self::P256 => "P-256",
            Self::P384 => "P-384",
            Self::P521 => "P-521",
        }
    }

    /// OpenSSL name, what `asymmetricKeyDetails.namedCurve` reports.
    fn openssl_name(self) -> &'static str {
        match self {
            Self::P256 => "prime256v1",
            Self::P384 => "secp384r1",
            Self::P521 => "secp521r1",
        }
    }

    fn size(self) -> usize {
        match self {
            Self::P256 => 32,
            Self::P384 => 48,
            Self::P521 => 66,
        }
    }

    /// aws-lc-rs binds the curve to a digest at parse time; the real digest is picked at sign time.
    fn parser(self) -> &'static EcdsaSigningAlgorithm {
        match self {
            Self::P256 => &ECDSA_P256_SHA256_ASN1_SIGNING,
            Self::P384 => &ECDSA_P384_SHA384_ASN1_SIGNING,
            Self::P521 => &ECDSA_P521_SHA512_ASN1_SIGNING,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Rsa,
    Ec(Curve),
    Ed25519,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Rsa => "rsa",
            Self::Ec(_) => "ec",
            Self::Ed25519 => "ed25519",
        }
    }
}

/// Algorithm of a PKCS#8 (`private`) or SPKI envelope, plus its payload.
pub(crate) fn inspect(der_bytes: &[u8], private: bool) -> Result<(Kind, Vec<u8>), String> {
    let (oid, params, payload) = der::envelope(der_bytes, private)?;
    let kind = match oid.as_slice() {
        OID_RSA => Kind::Rsa,
        OID_EC => Kind::Ec(Curve::from_oid(&params)?),
        OID_ED25519 => Kind::Ed25519,
        _ => return Err("Unsupported key type".into()),
    };
    Ok((kind, payload))
}

fn ec_pair(curve: Curve, pkcs8: &[u8]) -> Result<EcdsaKeyPair, String> {
    EcdsaKeyPair::from_pkcs8(curve.parser(), pkcs8).map_err(|_| "Invalid private key".to_string())
}

pub(crate) fn rsa_pair(pkcs8: &[u8]) -> Result<RsaKeyPair, String> {
    RsaKeyPair::from_pkcs8(pkcs8).map_err(|_| "Invalid private key".to_string())
}

fn ed_pair(pkcs8: &[u8]) -> Result<Ed25519KeyPair, String> {
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8).map_err(|_| "Invalid private key".to_string())
}

/// SPKI DER of a PKCS#8 private key.
pub(crate) fn public_of(pkcs8: &[u8]) -> Result<Vec<u8>, String> {
    let (kind, _) = inspect(pkcs8, true)?;
    Ok(match kind {
        Kind::Rsa => der::spki(
            &der::rsa_algorithm(),
            rsa_pair(pkcs8)?.public_key().as_ref(),
        ),
        Kind::Ec(curve) => der::spki(
            &der::ec_algorithm(curve.oid()),
            ec_pair(curve, pkcs8)?.public_key().as_ref(),
        ),
        Kind::Ed25519 => der::spki(
            &der::ed25519_algorithm(),
            ed_pair(pkcs8)?.public_key().as_ref(),
        ),
    })
}

fn exponent_text(magnitude: &[u8]) -> String {
    if magnitude.len() > 8 {
        return "0".into();
    }
    magnitude
        .iter()
        .fold(0u64, |acc, &b| acc << 8 | u64::from(b))
        .to_string()
}

fn bit_length(magnitude: &[u8]) -> usize {
    magnitude.first().map_or(0, |&top| {
        (magnitude.len() - 1) * 8 + (8 - top.leading_zeros() as usize)
    })
}

/// Validates a canonical key and describes it: `[type, kind, der, detail, detail]` where the
/// details are `modulusLength, publicExponent` (rsa) or `namedCurve` (ec).
fn describe(canonical: Vec<u8>, private: bool) -> Parts {
    let (kind, payload) = inspect(&canonical, private)?;
    let public_payload = if private {
        match kind {
            Kind::Rsa => {
                let pair = rsa_pair(&canonical)?;
                pair.public_key().as_ref().to_vec()
            }
            Kind::Ec(curve) => ec_pair(curve, &canonical).map(|_| Vec::new())?,
            Kind::Ed25519 => ed_pair(&canonical).map(|_| Vec::new())?,
        }
    } else {
        payload
    };
    let details = match kind {
        Kind::Rsa => {
            let numbers = der::integers(&public_payload)?;
            let [n, e] = numbers.as_slice() else {
                return Err("Invalid public key".into());
            };
            vec![
                bit_length(n).to_string().into_bytes(),
                exponent_text(e).into_bytes(),
            ]
        }
        Kind::Ec(curve) => vec![curve.openssl_name().as_bytes().to_vec()],
        Kind::Ed25519 if !private && public_payload.len() != 32 => {
            return Err("Invalid public key".into());
        }
        Kind::Ed25519 => Vec::new(),
    };
    let mut out = vec![
        kind.name().as_bytes().to_vec(),
        if private { "private" } else { "public" }
            .as_bytes()
            .to_vec(),
        canonical,
    ];
    out.extend(details);
    Ok(out)
}

fn sec1_to_pkcs8(sec1: &[u8]) -> Result<Vec<u8>, String> {
    let invalid = || "Invalid EC private key".to_string();
    let (body, _) = der::expect(sec1, SEQUENCE)?;
    let (_version, rest) = der::expect(body, INTEGER)?;
    let (secret, mut rest) = der::expect(rest, OCTET_STRING)?;
    let (mut curve, mut public) = (None, None);
    while !rest.is_empty() {
        let (tag, content, tail) = der::read(rest)?;
        match tag {
            CONTEXT_0 => curve = Some(Curve::from_oid(content)?),
            CONTEXT_1 => public = Some(content),
            _ => return Err(invalid()),
        }
        rest = tail;
    }
    let curve = curve.ok_or_else(invalid)?;
    Ok(der::pkcs8(
        &der::ec_algorithm(curve.oid()),
        &ec_private(secret, public.map(<[u8]>::to_vec)),
    ))
}

/// RFC 5915 ECPrivateKey as carried inside PKCS#8 (no parameters; the envelope has them).
fn ec_private(secret: &[u8], public_bits: Option<Vec<u8>>) -> Vec<u8> {
    let mut parts = vec![vec![INTEGER, 1, 1], tlv(OCTET_STRING, secret)];
    parts.extend(public_bits.map(|bits| tlv(CONTEXT_1, &bits)));
    tlv(SEQUENCE, &parts.concat())
}

fn pem_label(typ: &str, private: bool) -> Result<&'static str, String> {
    Ok(match (typ, private) {
        ("pkcs1", true) => "RSA PRIVATE KEY",
        ("pkcs1", false) => "RSA PUBLIC KEY",
        ("sec1", true) => "EC PRIVATE KEY",
        ("pkcs8", true) | ("", true) => "PRIVATE KEY",
        ("spki", false) | ("", false) => "PUBLIC KEY",
        _ => return Err("Unsupported key type for this format".into()),
    })
}

/// `key_parse(data, format, type, want) -> [algorithm, "private"|"public", der, detail...]`
/// `format` is `pem` or `der`; `want` is `private` or `public` (or `any` for PEM input).
pub(crate) fn parse(args: &[Vec<u8>]) -> Parts {
    let [data, format, typ, want] = args else {
        return Err("key_parse expects (data, format, type, want)".into());
    };
    let (label, bytes) = if format == b"pem" {
        der::pem_decode(&String::from_utf8_lossy(data))?
    } else {
        let private = want == b"private";
        (
            pem_label(&String::from_utf8_lossy(typ), private)?.to_string(),
            data.clone(),
        )
    };
    match label.as_str() {
        "PRIVATE KEY" => describe(bytes, true),
        "RSA PRIVATE KEY" => describe(der::pkcs8(&der::rsa_algorithm(), &bytes), true),
        "EC PRIVATE KEY" => describe(sec1_to_pkcs8(&bytes)?, true),
        "PUBLIC KEY" => describe(bytes, false),
        "RSA PUBLIC KEY" => describe(der::spki(&der::rsa_algorithm(), &bytes), false),
        "ENCRYPTED PRIVATE KEY" => Err("Encrypted private keys are not supported".into()),
        other => Err(format!("Unsupported PEM label: {other}")),
    }
}

/// `key_public(pkcs8) -> [spki]`
pub(crate) fn public(args: &[Vec<u8>]) -> Parts {
    let [pkcs8] = args else {
        return Err("key_public expects (pkcs8)".into());
    };
    Ok(vec![public_of(pkcs8)?])
}

/// `key_generate(kind, parameter) -> [pkcs8, spki]`; the parameter is the RSA modulus length
/// or the JWK curve name.
pub(crate) fn generate(args: &[Vec<u8>]) -> Parts {
    let [kind, parameter] = args else {
        return Err("key_generate expects (kind, parameter)".into());
    };
    let rng = SystemRandom::new();
    let failed = |_| "Key generation failed".to_string();
    let pkcs8: Vec<u8> = match kind.as_slice() {
        b"rsa" => {
            let size = match parameter.as_slice() {
                b"2048" => KeySize::Rsa2048,
                b"3072" => KeySize::Rsa3072,
                b"4096" => KeySize::Rsa4096,
                b"8192" => KeySize::Rsa8192,
                _ => {
                    return Err(
                        "Only 2048, 3072, 4096 and 8192-bit RSA keys can be generated".into(),
                    );
                }
            };
            let document: Pkcs8V1Der = RsaKeyPair::generate(size)
                .map_err(failed)?
                .as_der()
                .map_err(failed)?;
            document.as_ref().to_vec()
        }
        b"ec" => {
            let curve = Curve::from_jwk(parameter)?;
            EcdsaKeyPair::generate_pkcs8(curve.parser(), &rng)
                .map_err(failed)?
                .as_ref()
                .to_vec()
        }
        b"ed25519" => Ed25519KeyPair::generate_pkcs8v1(&rng)
            .map_err(failed)?
            .as_ref()
            .to_vec(),
        _ => return Err("Unsupported key type".into()),
    };
    let public = public_of(&pkcs8)?;
    Ok(vec![pkcs8, public])
}

/// `key_export(der, "private"|"public", format) -> [der]` with `format` one of
/// `pkcs8`, `spki`, `pkcs1`, `sec1`.
pub(crate) fn export(args: &[Vec<u8>]) -> Parts {
    let [bytes, role, format] = args else {
        return Err("key_export expects (der, role, format)".into());
    };
    let private = role == b"private";
    let (kind, payload) = inspect(bytes, private)?;
    let converted = match (format.as_slice(), private, kind) {
        (b"pkcs8", true, _) | (b"spki", false, _) => bytes.clone(),
        (b"pkcs1", _, Kind::Rsa) => payload,
        (b"sec1", true, Kind::Ec(curve)) => {
            let (body, _) = der::expect(&payload, SEQUENCE)?;
            let (_version, rest) = der::expect(body, INTEGER)?;
            let (secret, rest) = der::expect(rest, OCTET_STRING)?;
            let params = tlv(CONTEXT_0, curve.oid());
            tlv(
                SEQUENCE,
                &[
                    &[INTEGER, 1, 1][..],
                    &tlv(OCTET_STRING, secret),
                    &params,
                    rest,
                ]
                .concat(),
            )
        }
        _ => return Err("Unsupported key export format for this key".into()),
    };
    Ok(vec![converted])
}

fn pad_left(bytes: &[u8], size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size.saturating_sub(bytes.len())];
    out.extend_from_slice(bytes);
    out
}

/// `key_jwk_export(der, role) -> parts`: rsa `n, e[, d, p, q, dp, dq, qi]`; ec `crv, x, y[, d]`;
/// ed25519 `crv, x[, d]`.
pub(crate) fn jwk_export(args: &[Vec<u8>]) -> Parts {
    let [bytes, role] = args else {
        return Err("key_jwk_export expects (der, role)".into());
    };
    let private = role == b"private";
    let (kind, payload) = inspect(bytes, private)?;
    let public_bits = if private {
        inspect(&public_of(bytes)?, false)?.1
    } else {
        payload.clone()
    };
    let mut out = Vec::new();
    match kind {
        Kind::Rsa => {
            let mut numbers = der::integers(if private { &payload } else { &public_bits })?;
            if private {
                numbers.remove(0);
            }
            out = numbers;
        }
        Kind::Ec(curve) => {
            let size = curve.size();
            let point = public_bits
                .get(1..1 + 2 * size)
                .ok_or("Invalid public key")?;
            out.extend([
                curve.jwk_name().as_bytes().to_vec(),
                point[..size].to_vec(),
                point[size..].to_vec(),
            ]);
            if private {
                let (body, _) = der::expect(&payload, SEQUENCE)?;
                let (secret, _) = der::expect(der::expect(body, INTEGER)?.1, OCTET_STRING)?;
                out.push(pad_left(secret, size));
            }
        }
        Kind::Ed25519 => {
            out.extend([b"Ed25519".to_vec(), public_bits]);
            if private {
                out.push(der::expect(&payload, OCTET_STRING)?.0.to_vec());
            }
        }
    }
    Ok(out)
}

/// `key_jwk_import(kind, parts...) -> same as key_parse`. Part layout as in [`jwk_export`].
pub(crate) fn jwk_import(args: &[Vec<u8>]) -> Parts {
    let [kind, parts @ ..] = args else {
        return Err("key_jwk_import expects (kind, ...parts)".into());
    };
    let invalid = || "Invalid JWK".to_string();
    match (kind.as_slice(), parts) {
        (b"rsa", [n, e]) => describe(
            der::spki(
                &der::rsa_algorithm(),
                &der::sequence(&[&der::unsigned_integer(n), &der::unsigned_integer(e)]),
            ),
            false,
        ),
        (b"rsa", numbers) if numbers.len() == 8 => {
            let integers: Vec<u8> = numbers
                .iter()
                .flat_map(|n| der::unsigned_integer(n))
                .collect();
            let pkcs1 = der::sequence(&[&[INTEGER, 1, 0], &integers]);
            describe(der::pkcs8(&der::rsa_algorithm(), &pkcs1), true)
        }
        (b"ec", [crv, x, y, rest @ ..]) if rest.len() <= 1 => {
            let curve = Curve::from_jwk(crv)?;
            let point = [
                &[4u8][..],
                &pad_left(x, curve.size()),
                &pad_left(y, curve.size()),
            ]
            .concat();
            let bits = tlv(der::BIT_STRING, &[&[0u8][..], &point].concat());
            match rest {
                [d] => describe(
                    der::pkcs8(
                        &der::ec_algorithm(curve.oid()),
                        &ec_private(&pad_left(d, curve.size()), Some(bits)),
                    ),
                    true,
                ),
                _ => describe(der::spki(&der::ec_algorithm(curve.oid()), &point), false),
            }
        }
        (b"ed25519", [_crv, x]) => describe(der::spki(&der::ed25519_algorithm(), x), false),
        (b"ed25519", [_crv, _x, d]) => describe(
            der::pkcs8(&der::ed25519_algorithm(), &tlv(OCTET_STRING, d)),
            true,
        ),
        _ => Err(invalid()),
    }
}
