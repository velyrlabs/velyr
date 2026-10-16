//! Minimal DER and PEM plumbing for key envelopes (PKCS#1, PKCS#8, SPKI, SEC1). Plain data
//! formats only: every cryptographic step is left to aws-lc-rs.

pub const SEQUENCE: u8 = 0x30;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const CONTEXT_0: u8 = 0xa0;
pub const CONTEXT_1: u8 = 0xa1;

// Full OBJECT IDENTIFIER TLVs.
pub const OID_RSA: &[u8] = &[
    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
];
pub const OID_EC: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
pub const OID_P256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
pub const OID_P384: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22];
pub const OID_P521: &[u8] = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x23];
pub const OID_ED25519: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x70];
const NULL: &[u8] = &[0x05, 0x00];

pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match content.len() {
        len @ 0..=127 => out.push(len as u8),
        len => {
            let bytes: Vec<u8> = len
                .to_be_bytes()
                .into_iter()
                .skip_while(|&byte| byte == 0)
                .collect();
            out.push(0x80 | bytes.len() as u8);
            out.extend(bytes);
        }
    }
    out.extend_from_slice(content);
    out
}

/// Splits one TLV off the front: `(tag, content, rest)`.
pub fn read(der: &[u8]) -> Result<(u8, &[u8], &[u8]), String> {
    let bad = || "Invalid DER encoding".to_string();
    let (&tag, rest) = der.split_first().ok_or_else(bad)?;
    let (&first, rest) = rest.split_first().ok_or_else(bad)?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return Err(bad());
        }
        let (digits, rest) = rest.split_at(count);
        (
            digits.iter().fold(0, |acc, &d| acc << 8 | usize::from(d)),
            rest,
        )
    };
    if rest.len() < len {
        return Err(bad());
    }
    let (content, rest) = rest.split_at(len);
    Ok((tag, content, rest))
}

pub fn expect(der: &[u8], tag: u8) -> Result<(&[u8], &[u8]), String> {
    match read(der)? {
        (found, content, rest) if found == tag => Ok((content, rest)),
        _ => Err("Invalid DER encoding".into()),
    }
}

pub fn sequence(parts: &[&[u8]]) -> Vec<u8> {
    tlv(SEQUENCE, &parts.concat())
}

pub fn rsa_algorithm() -> Vec<u8> {
    sequence(&[OID_RSA, NULL])
}

pub fn ec_algorithm(curve_oid: &[u8]) -> Vec<u8> {
    sequence(&[OID_EC, curve_oid])
}

pub fn ed25519_algorithm() -> Vec<u8> {
    sequence(&[OID_ED25519])
}

pub fn spki(algorithm: &[u8], public: &[u8]) -> Vec<u8> {
    sequence(&[algorithm, &tlv(BIT_STRING, &[&[0u8], public].concat())])
}

pub fn pkcs8(algorithm: &[u8], inner: &[u8]) -> Vec<u8> {
    sequence(&[&[INTEGER, 1, 0], algorithm, &tlv(OCTET_STRING, inner)])
}

type Envelope = (Vec<u8>, Vec<u8>, Vec<u8>);

/// `(algorithm OID, parameters or empty, payload)` of a PKCS#8 (payload = privateKey octets) or
/// SPKI (payload = subjectPublicKey bits) envelope.
pub fn envelope(der: &[u8], private: bool) -> Result<Envelope, String> {
    let (body, trailing) = expect(der, SEQUENCE)?;
    if !trailing.is_empty() {
        return Err("Invalid DER encoding".into());
    }
    let rest = if private {
        expect(body, INTEGER)?.1
    } else {
        body
    };
    let (algorithm, rest) = expect(rest, SEQUENCE)?;
    let (oid, params) = read(algorithm).map(|(tag, content, rest)| (tlv(tag, content), rest))?;
    let (payload, _) = expect(rest, if private { OCTET_STRING } else { BIT_STRING })?;
    let payload = if private {
        payload
    } else {
        payload.strip_prefix(&[0]).ok_or("Invalid DER encoding")?
    };
    Ok((oid, params.to_vec(), payload.to_vec()))
}

/// A DER INTEGER for an unsigned big-endian magnitude.
pub fn unsigned_integer(magnitude: &[u8]) -> Vec<u8> {
    let start = magnitude
        .iter()
        .position(|&byte| byte != 0)
        .unwrap_or(magnitude.len().saturating_sub(1));
    let digits = &magnitude[start..];
    let mut content = Vec::new();
    if digits.first().is_none_or(|&byte| byte & 0x80 != 0) {
        content.push(0);
    }
    content.extend_from_slice(digits);
    tlv(INTEGER, &content)
}

/// `SEQUENCE { INTEGER* }` as unsigned magnitudes without leading zeros.
pub fn integers(der: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let (mut body, trailing) = expect(der, SEQUENCE)?;
    let mut out = Vec::new();
    while !body.is_empty() {
        let (value, rest) = expect(body, INTEGER)?;
        let start = value
            .iter()
            .position(|&byte| byte != 0)
            .unwrap_or(value.len());
        out.push(value[start..].to_vec());
        body = rest;
    }
    if trailing.is_empty() {
        Ok(out)
    } else {
        Err("Invalid DER encoding".into())
    }
}

/// `(label, DER)` of the first PEM block in `text`.
pub fn pem_decode(text: &str) -> Result<(String, Vec<u8>), String> {
    let invalid = || "Invalid PEM".to_string();
    let start = text.find("-----BEGIN ").ok_or_else(invalid)?;
    let after = &text[start + 11..];
    let label_end = after.find("-----").ok_or_else(invalid)?;
    let label = &after[..label_end];
    let body = &after[label_end + 5..];
    let end = body
        .find(&format!("-----END {label}-----"))
        .ok_or_else(invalid)?;
    let base64: String = body[..end].chars().filter(|c| !c.is_whitespace()).collect();
    Ok((
        label.to_string(),
        base64_decode(&base64).ok_or_else(invalid)?,
    ))
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim_end_matches('=');
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::with_capacity(trimmed.len() * 3 / 4));
    for c in trimmed.bytes() {
        let sextet = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = acc << 6 | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}
