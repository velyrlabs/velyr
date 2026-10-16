//! Digests, HMAC, PBKDF2 and HKDF for `node:crypto`. SHA-1/2/3 come from aws-lc-rs; MD5 (which
//! aws-lc-rs does not expose) is implemented here from RFC 1321. HMAC is generic over every
//! digest so it covers MD5 and SHA-3 as well.

use std::num::NonZeroU32;
use std::sync::OnceLock;

use aws_lc_rs::{digest, hkdf, pbkdf2};

fn algorithm(name: &str) -> Option<&'static digest::Algorithm> {
    Some(match name {
        "sha1" => &digest::SHA1_FOR_LEGACY_USE_ONLY,
        "sha224" => &digest::SHA224,
        "sha256" => &digest::SHA256,
        "sha384" => &digest::SHA384,
        "sha512" => &digest::SHA512,
        "sha512-256" => &digest::SHA512_256,
        "sha3-256" => &digest::SHA3_256,
        "sha3-384" => &digest::SHA3_384,
        "sha3-512" => &digest::SHA3_512,
        _ => return None,
    })
}

/// Input block size in bytes (the HMAC key padding width).
fn block_size(name: &str) -> usize {
    match name {
        "sha384" | "sha512" | "sha512-256" => 128,
        "sha3-256" => 136,
        "sha3-384" => 104,
        "sha3-512" => 72,
        _ => 64,
    }
}

pub fn hash(name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    if name == "md5" {
        return Ok(md5(data).to_vec());
    }
    let algorithm = algorithm(name).ok_or_else(|| "Digest method not supported".to_string())?;
    Ok(digest::digest(algorithm, data).as_ref().to_vec())
}

pub fn hmac(name: &str, key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let native = match name {
        "sha1" => Some(aws_lc_rs::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY),
        "sha224" => Some(aws_lc_rs::hmac::HMAC_SHA224),
        "sha256" => Some(aws_lc_rs::hmac::HMAC_SHA256),
        "sha384" => Some(aws_lc_rs::hmac::HMAC_SHA384),
        "sha512" => Some(aws_lc_rs::hmac::HMAC_SHA512),
        _ => None,
    };
    if let Some(algorithm) = native {
        let key = aws_lc_rs::hmac::Key::new(algorithm, key);
        return Ok(aws_lc_rs::hmac::sign(&key, data).as_ref().to_vec());
    }
    // md5, sha3 and sha512-256 have no aws-lc-rs HMAC.
    let mut mac = Hmac::new(name, key)?;
    mac.update(data);
    Ok(mac.finish())
}

pub fn pbkdf2(
    name: &str,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    length: usize,
) -> Result<Vec<u8>, String> {
    let algorithm = match name {
        "sha1" => pbkdf2::PBKDF2_HMAC_SHA1,
        "sha256" => pbkdf2::PBKDF2_HMAC_SHA256,
        "sha384" => pbkdf2::PBKDF2_HMAC_SHA384,
        "sha512" => pbkdf2::PBKDF2_HMAC_SHA512,
        _ => return Err(format!("ERR_CRYPTO_INVALID_DIGEST|Invalid digest: {name}")),
    };
    let iterations = NonZeroU32::new(iterations).ok_or("iterations must be positive")?;
    // aws-lc-rs panics past hash_len * (2^32 - 1); stay far below it.
    if length > 1 << 30 {
        return Err("keylen is too large".into());
    }
    let mut out = vec![0u8; length];
    pbkdf2::derive(algorithm, iterations, salt, password, &mut out);
    Ok(out)
}

struct OutputLength(usize);

impl hkdf::KeyType for OutputLength {
    fn len(&self) -> usize {
        self.0
    }
}

pub fn hkdf(
    name: &str,
    key: &[u8],
    salt: &[u8],
    info: &[u8],
    length: usize,
) -> Result<Vec<u8>, String> {
    let algorithm = match name {
        "sha1" => hkdf::HKDF_SHA1_FOR_LEGACY_USE_ONLY,
        "sha256" => hkdf::HKDF_SHA256,
        "sha384" => hkdf::HKDF_SHA384,
        "sha512" => hkdf::HKDF_SHA512,
        _ => return Err(format!("ERR_CRYPTO_INVALID_DIGEST|Invalid digest: {name}")),
    };
    let mut out = vec![0u8; length];
    hkdf::Salt::new(algorithm, salt)
        .extract(key)
        .expand(&[info], OutputLength(length))
        .and_then(|okm| okm.fill(&mut out))
        .map_err(|_| "ERR_CRYPTO_INVALID_KEYLEN|Invalid key length".to_string())?;
    Ok(out)
}

/// scrypt (RFC 7914) through the `scrypt` crate. The parameters Node rejects are checked, with
/// checked arithmetic, before the crate allocates anything: N a power of two >= 2, r * p < 2^30,
/// N < 2^(16 * r), and 128 * r * (N + p + 2) bytes within `maxmem`.
pub fn scrypt(
    password: &[u8],
    salt: &[u8],
    (n, r, p): (u64, u64, u64),
    maxmem: u64,
    length: usize,
) -> Result<Vec<u8>, String> {
    const INVALID: &str = "ERR_CRYPTO_INVALID_SCRYPT_PARAMS|Invalid scrypt params";
    if n < 2 || !n.is_power_of_two() || r == 0 || p == 0 || length > 1 << 30 {
        return Err(INVALID.into());
    }
    if r.checked_mul(p).is_none_or(|rp| rp >= 1 << 30) || (r < 4 && n >= 1 << (16 * r)) {
        return Err(INVALID.into());
    }
    let memory = 128u64
        .checked_mul(r)
        .and_then(|row| row.checked_mul(n.checked_add(p)?.checked_add(2)?))
        .ok_or(INVALID)?;
    if memory > maxmem {
        return Err(format!("{INVALID}: memory limit exceeded"));
    }
    let params = scrypt::Params::new(n.trailing_zeros() as u8, r as u32, p as u32, 32)
        .map_err(|_| INVALID.to_string())?;
    let mut out = vec![0u8; length];
    if length > 0 {
        scrypt::scrypt(password, salt, &params, &mut out).map_err(|_| INVALID.to_string())?;
    }
    Ok(out)
}

/// Incremental MD5 (RFC 1321); aws-lc-rs does not expose it. Only whole 64-byte blocks are
/// compressed as data arrives, so memory stays at one partial block.
#[derive(Clone)]
pub(crate) struct Md5 {
    state: [u32; 4],
    pending: Vec<u8>,
    length: u64,
}

impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            pending: Vec::new(),
            length: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        self.pending.extend_from_slice(data);
        let whole = self.pending.len() / 64 * 64;
        for block in self.pending[..whole].as_chunks::<64>().0 {
            md5_compress(&mut self.state, block);
        }
        self.pending.drain(..whole);
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.length.wrapping_mul(8);
        self.pending.push(0x80);
        while self.pending.len() % 64 != 56 {
            self.pending.push(0);
        }
        self.pending.extend_from_slice(&bits.to_le_bytes());
        for block in self.pending.as_chunks::<64>().0 {
            md5_compress(&mut self.state, block);
        }
        let mut out = [0u8; 16];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }
}

fn md5_compress(state: &mut [u32; 4], block: &[u8; 64]) {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    // RFC 1321: K[i] = floor(2^32 * abs(sin(i + 1))).
    static K: OnceLock<[u32; 64]> = OnceLock::new();
    let k = K.get_or_init(|| {
        std::array::from_fn(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32)
    });
    let words: [u32; 16] =
        std::array::from_fn(|i| u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap()));
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let rotated = a
            .wrapping_add(f)
            .wrapping_add(k[i])
            .wrapping_add(words[g])
            .rotate_left(SHIFTS[i]);
        (a, d, c) = (d, c, b);
        b = b.wrapping_add(rotated);
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d]) {
        *slot = slot.wrapping_add(value);
    }
}

fn md5(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finish()
}

/// A digest in progress: aws-lc-rs for SHA-1/2/3, [`Md5`] for MD5.
#[derive(Clone)]
pub(crate) enum Hasher {
    Native(digest::Context),
    Md5(Md5),
}

impl Hasher {
    pub fn new(name: &str) -> Result<Self, String> {
        if name == "md5" {
            return Ok(Self::Md5(Md5::new()));
        }
        let algorithm = algorithm(name).ok_or("Digest method not supported")?;
        Ok(Self::Native(digest::Context::new(algorithm)))
    }

    pub fn update(&mut self, data: &[u8]) {
        match self {
            Self::Native(context) => context.update(data),
            Self::Md5(md5) => md5.update(data),
        }
    }

    pub fn finish(self) -> Vec<u8> {
        match self {
            Self::Native(context) => context.finish().as_ref().to_vec(),
            Self::Md5(md5) => md5.finish().to_vec(),
        }
    }
}

/// HMAC (RFC 2104) over any [`Hasher`], so md5, sha3 and sha512-256 are covered too.
#[derive(Clone)]
pub(crate) struct Hmac {
    inner: Hasher,
    outer: Hasher,
}

impl Hmac {
    pub fn new(name: &str, key: &[u8]) -> Result<Self, String> {
        let block = block_size(name);
        let mut padded = if key.len() > block {
            hash(name, key)?
        } else {
            key.to_vec()
        };
        padded.resize(block, 0);
        let (mut inner, mut outer) = (Hasher::new(name)?, Hasher::new(name)?);
        inner.update(&padded.iter().map(|byte| byte ^ 0x36).collect::<Vec<_>>());
        outer.update(&padded.iter().map(|byte| byte ^ 0x5c).collect::<Vec<_>>());
        Ok(Self { inner, outer })
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    pub fn finish(self) -> Vec<u8> {
        let mut outer = self.outer;
        outer.update(&self.inner.finish());
        outer.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    // RFC 1321 appendix A.5 test suite (covers one- and two-block messages).
    #[test]
    fn md5_matches_rfc_1321() {
        for (input, expected) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(hex(&md5(input.as_bytes())), expected, "{input}");
        }
    }
}
