//! Minimal DER for the PKCS#8 / SPKI envelopes of EC, Ed25519 and X25519 keys.
use crate::error::{CryptoError, CryptoResult};

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
const OCTET_STRING: u8 = 0x04;
const CONTEXT_1: u8 = 0xa1;

// Full OBJECT IDENTIFIER TLVs.
const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_P521: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x23];
const OID_RSA_ENCRYPTION: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
];
const OID_ED25519: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x70];
const OID_X25519: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x6e];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    P256,
    P384,
    P521,
    Ed25519,
    X25519,
}

impl Kind {
    pub fn parse(name: &[u8]) -> CryptoResult<Self> {
        match name {
            b"P-256" => Ok(Self::P256),
            b"P-384" => Ok(Self::P384),
            b"P-521" => Ok(Self::P521),
            b"Ed25519" => Ok(Self::Ed25519),
            b"X25519" => Ok(Self::X25519),
            _ => Err(CryptoError::not_supported("Unsupported key kind")),
        }
    }

    pub fn is_ec(self) -> bool {
        matches!(self, Self::P256 | Self::P384 | Self::P521)
    }

    /// AlgorithmIdentifier SEQUENCE for this key kind.
    fn algorithm_identifier(self) -> Vec<u8> {
        let body = match self {
            Self::P256 => [OID_EC_PUBLIC_KEY, OID_P256].concat(),
            Self::P384 => [OID_EC_PUBLIC_KEY, OID_P384].concat(),
            Self::P521 => [OID_EC_PUBLIC_KEY, OID_P521].concat(),
            Self::Ed25519 => OID_ED25519.to_vec(),
            Self::X25519 => OID_X25519.to_vec(),
        };
        tlv(SEQUENCE, &body)
    }
}

pub(crate) fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match content.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend([0x81, n as u8]),
        n => out.extend([0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(content);
    out
}

/// Reads one TLV: `(tag, content, rest)`.
fn read(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = match first {
        n if n < 0x80 => (usize::from(n), rest),
        0x81 => {
            let (&n, rest) = rest.split_first()?;
            (usize::from(n), rest)
        }
        0x82 => {
            let (n, rest) = rest.split_first_chunk::<2>()?;
            (usize::from(u16::from_be_bytes(*n)), rest)
        }
        _ => return None,
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

fn expect(input: &[u8], tag: u8) -> CryptoResult<(&[u8], &[u8])> {
    match read(input) {
        Some((found, content, rest)) if found == tag => Ok((content, rest)),
        _ => Err(CryptoError::data("Invalid key encoding")),
    }
}

fn bit_string(content: &[u8]) -> Vec<u8> {
    tlv(BIT_STRING, &[&[0u8], content].concat())
}

pub(crate) fn spki_wrap(algorithm_identifier: &[u8], public: &[u8]) -> Vec<u8> {
    tlv(
        SEQUENCE,
        &[algorithm_identifier, &bit_string(public)].concat(),
    )
}

/// Returns the subjectPublicKey bits of an SPKI whose AlgorithmIdentifier equals `algorithm_identifier`.
pub(crate) fn spki_unwrap(algorithm_identifier: &[u8], der: &[u8]) -> CryptoResult<Vec<u8>> {
    let (body, trailing) = expect(der, SEQUENCE)?;
    let (algorithm, rest) = expect(body, SEQUENCE)?;
    let (bits, _) = expect(rest, BIT_STRING)?;
    let algorithm_ok = tlv(SEQUENCE, algorithm) == algorithm_identifier;
    match bits.split_first() {
        Some((0, key)) if algorithm_ok && trailing.is_empty() => Ok(key.to_vec()),
        _ => Err(CryptoError::data(
            "Invalid SubjectPublicKeyInfo for this algorithm",
        )),
    }
}

pub(crate) fn spki_encode(kind: Kind, public: &[u8]) -> Vec<u8> {
    spki_wrap(&kind.algorithm_identifier(), public)
}

pub(crate) fn spki_decode(kind: Kind, der: &[u8]) -> CryptoResult<Vec<u8>> {
    spki_unwrap(&kind.algorithm_identifier(), der)
}

pub(crate) fn pkcs8_wrap(algorithm_identifier: &[u8], inner: &[u8]) -> Vec<u8> {
    let version = [INTEGER, 1, 0];
    tlv(
        SEQUENCE,
        &[
            &version[..],
            algorithm_identifier,
            &tlv(OCTET_STRING, inner),
        ]
        .concat(),
    )
}

/// Returns the privateKey OCTET STRING content of a PKCS#8 whose AlgorithmIdentifier equals `algorithm_identifier`.
pub(crate) fn pkcs8_unwrap(algorithm_identifier: &[u8], der: &[u8]) -> CryptoResult<Vec<u8>> {
    let (body, trailing) = expect(der, SEQUENCE)?;
    let (_version, rest) = expect(body, INTEGER)?;
    let (algorithm, rest) = expect(rest, SEQUENCE)?;
    let (payload, _) = expect(rest, OCTET_STRING)?;
    if !trailing.is_empty() || tlv(SEQUENCE, algorithm) != algorithm_identifier {
        return Err(CryptoError::data("Invalid PKCS#8 for this algorithm"));
    }
    Ok(payload.to_vec())
}

/// PKCS#8 PrivateKeyInfo: EC keys carry an RFC 5915 ECPrivateKey (scalar + public point), the
/// curve25519 kinds a CurvePrivateKey (the 32-byte seed).
pub(crate) fn pkcs8_encode(kind: Kind, private: &[u8], public: &[u8]) -> Vec<u8> {
    let inner = if kind.is_ec() {
        let version = [INTEGER, 1, 1];
        let point = tlv(CONTEXT_1, &bit_string(public));
        tlv(
            SEQUENCE,
            &[&version[..], &tlv(OCTET_STRING, private), &point].concat(),
        )
    } else {
        tlv(OCTET_STRING, private)
    };
    pkcs8_wrap(&kind.algorithm_identifier(), &inner)
}

/// Returns the private-key payload of a PKCS#8 structure for `kind`: the seed for curve25519, the whole
/// ECPrivateKey for EC (parsed by aws-lc, which also checks it).
pub(crate) fn pkcs8_payload(kind: Kind, der: &[u8]) -> CryptoResult<Vec<u8>> {
    let payload = pkcs8_unwrap(&kind.algorithm_identifier(), der)?;
    if kind.is_ec() {
        return Ok(payload);
    }
    let (seed, _) = expect(&payload, OCTET_STRING)?;
    Ok(seed.to_vec())
}

/// rsaEncryption with NULL parameters: the AlgorithmIdentifier of every RSA key in this crate.
pub(crate) fn rsa_algorithm_identifier() -> Vec<u8> {
    tlv(SEQUENCE, &[OID_RSA_ENCRYPTION, &[0x05, 0x00]].concat())
}

/// A DER INTEGER for an unsigned big-endian magnitude.
pub(crate) fn unsigned_integer(magnitude: &[u8]) -> Vec<u8> {
    let start = magnitude
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(magnitude.len().saturating_sub(1));
    let digits = &magnitude[start..];
    let mut content = Vec::new();
    if digits.first().is_none_or(|&b| b & 0x80 != 0) {
        content.push(0);
    }
    content.extend_from_slice(digits);
    tlv(INTEGER, &content)
}

pub(crate) fn sequence(content: &[u8]) -> Vec<u8> {
    tlv(SEQUENCE, content)
}

/// Reads `SEQUENCE { INTEGER* }` into unsigned magnitudes without leading zeros.
pub(crate) fn read_integers(der: &[u8]) -> CryptoResult<Vec<Vec<u8>>> {
    let (mut body, trailing) = expect(der, SEQUENCE)?;
    let mut out = Vec::new();
    while !body.is_empty() {
        let (value, rest) = expect(body, INTEGER)?;
        let start = value.iter().position(|&b| b != 0).unwrap_or(value.len());
        out.push(value[start..].to_vec());
        body = rest;
    }
    if trailing.is_empty() {
        Ok(out)
    } else {
        Err(CryptoError::data("Invalid key encoding"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc8410_ed25519_pkcs8_vector() {
        let seed = [
            0xd4, 0xee, 0x72, 0xdb, 0xf9, 0x13, 0x58, 0x4a, 0xd5, 0xb6, 0xd8, 0xf1, 0xf7, 0x69,
            0xf8, 0xad, 0x3a, 0xfe, 0x7c, 0x28, 0xcb, 0xf1, 0xd4, 0xfb, 0xe0, 0x97, 0xa8, 0x8f,
            0x44, 0x75, 0x58, 0x42,
        ];
        let der = pkcs8_encode(Kind::Ed25519, &seed, &[]);
        assert_eq!(
            &der[..16],
            &[
                0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
                0x04, 0x20
            ]
        );
        assert_eq!(pkcs8_payload(Kind::Ed25519, &der).unwrap(), seed);
        assert!(pkcs8_payload(Kind::X25519, &der).is_err());
    }

    #[test]
    fn spki_round_trip_and_wrong_kind() {
        let point = vec![4u8; 65];
        let der = spki_encode(Kind::P256, &point);
        assert_eq!(spki_decode(Kind::P256, &der).unwrap(), point);
        assert!(spki_decode(Kind::P384, &der).is_err());
    }
}
