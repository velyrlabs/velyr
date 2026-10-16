//! AES-GCM, AES-CBC (PKCS#7) and AES-CTR. aws-lc-rs fixes GCM to a 96-bit IV and a 128-bit tag; other
//! parameters are rejected with NotSupportedError instead of being ignored.
use aws_lc_rs::aead::{AES_128_GCM, AES_192_GCM, AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::cipher::{
    AES_128, AES_192, AES_256, DecryptingKey, DecryptionContext, EncryptingKey, EncryptionContext,
    PaddedBlockDecryptingKey, PaddedBlockEncryptingKey, UnboundCipherKey,
};
use aws_lc_rs::iv::FixedLength;

use crate::error::{CryptoError, CryptoResult};

const BLOCK: usize = 16;

fn cipher_key(key: &[u8]) -> CryptoResult<UnboundCipherKey> {
    let algorithm = match key.len() {
        16 => &AES_128,
        24 => &AES_192,
        32 => &AES_256,
        _ => return Err(CryptoError::operation("Invalid AES key length")),
    };
    Ok(UnboundCipherKey::new(algorithm, key)?)
}

fn iv16(iv: &[u8]) -> CryptoResult<FixedLength<BLOCK>> {
    let iv: [u8; BLOCK] = iv
        .try_into()
        .map_err(|_| CryptoError::operation("The iv must be 16 bytes"))?;
    Ok(FixedLength::from(iv))
}

fn decrypting() -> impl Fn(aws_lc_rs::error::Unspecified) -> CryptoError {
    |_| CryptoError::operation("The operation failed for an operation-specific reason")
}

/// `aes_gcm(mode, key, iv, aad, data, tag_bits) -> [output]`; encrypt appends the tag, decrypt expects it.
pub(crate) fn gcm(args: &[Vec<u8>]) -> CryptoResult {
    let [mode, key, iv, aad, data, tag_bits] = args else {
        return Err(CryptoError::operation(
            "aes_gcm expects (mode, key, iv, aad, data, tag_bits)",
        ));
    };
    if tag_bits.as_slice() != b"128" {
        return Err(CryptoError::not_supported(
            "Only a 128-bit AES-GCM tag is supported",
        ));
    }
    let nonce = Nonce::try_assume_unique_for_key(iv)
        .map_err(|_| CryptoError::not_supported("Only a 12-byte AES-GCM iv is supported"))?;
    let algorithm = match key.len() {
        16 => &AES_128_GCM,
        24 => &AES_192_GCM,
        32 => &AES_256_GCM,
        _ => return Err(CryptoError::operation("Invalid AES key length")),
    };
    let key = LessSafeKey::new(UnboundKey::new(algorithm, key)?);
    let mut buffer = data.clone();
    if mode.as_slice() == b"encrypt" {
        key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut buffer)?;
        return Ok(vec![buffer]);
    }
    let plain_len = key
        .open_in_place(nonce, Aad::from(aad), &mut buffer)
        .map_err(decrypting())?
        .len();
    buffer.truncate(plain_len);
    Ok(vec![buffer])
}

/// `aes_cbc(mode, key, iv, data) -> [output]`
pub(crate) fn cbc(args: &[Vec<u8>]) -> CryptoResult {
    let [mode, key, iv, data] = args else {
        return Err(CryptoError::operation(
            "aes_cbc expects (mode, key, iv, data)",
        ));
    };
    let key = cipher_key(key)?;
    let iv = iv16(iv)?;
    let mut buffer = data.clone();
    if mode.as_slice() == b"encrypt" {
        PaddedBlockEncryptingKey::cbc_pkcs7(key)?
            .less_safe_encrypt(&mut buffer, EncryptionContext::Iv128(iv))?;
        return Ok(vec![buffer]);
    }
    let plain_len = PaddedBlockDecryptingKey::cbc_pkcs7(key)?
        .decrypt(&mut buffer, DecryptionContext::Iv128(iv))
        .map_err(decrypting())?
        .len();
    buffer.truncate(plain_len);
    Ok(vec![buffer])
}

/// Number of counter blocks `data_len` bytes consume, checked against the `length`-bit counter field.
/// aws-lc-rs increments the whole 128-bit block, which matches WebCrypto exactly unless the field would wrap.
fn check_counter(counter: &[u8], length_bits: u32, data_len: usize) -> CryptoResult<()> {
    let value = u128::from_be_bytes(
        counter
            .try_into()
            .map_err(|_| CryptoError::operation("The counter must be 16 bytes"))?,
    );
    let mask = if length_bits >= 128 {
        u128::MAX
    } else {
        (1u128 << length_bits) - 1
    };
    let blocks = data_len.div_ceil(BLOCK) as u128;
    let room = (mask - (value & mask)).saturating_add(1);
    if blocks > mask.saturating_add(1) {
        return Err(CryptoError::operation(
            "The data needs more counter blocks than the counter length allows",
        ));
    }
    if blocks > room {
        return Err(CryptoError::not_supported(
            "AES-CTR counter wrap-around is not supported",
        ));
    }
    Ok(())
}

/// `aes_ctr(mode, key, counter, length_bits, data) -> [output]`
pub(crate) fn ctr(args: &[Vec<u8>]) -> CryptoResult {
    let [mode, key, counter, length_bits, data] = args else {
        return Err(CryptoError::operation(
            "aes_ctr expects (mode, key, counter, length_bits, data)",
        ));
    };
    let length: u32 = String::from_utf8_lossy(length_bits).parse().unwrap_or(0);
    if !(1..=128).contains(&length) {
        return Err(CryptoError::operation(
            "The counter length must be between 1 and 128",
        ));
    }
    check_counter(counter, length, data.len())?;
    let key = cipher_key(key)?;
    let iv = iv16(counter)?;
    let mut buffer = data.clone();
    if mode.as_slice() == b"encrypt" {
        EncryptingKey::ctr(key)?.less_safe_encrypt(&mut buffer, EncryptionContext::Iv128(iv))?;
    } else {
        DecryptingKey::ctr(key)?.decrypt(&mut buffer, DecryptionContext::Iv128(iv))?;
    }
    Ok(vec![buffer])
}

/// `aes_kw(mode, key, data) -> [output]`: RFC 3394 / NIST SP 800-38F key wrap. aws-lc-rs has no 192-bit variant.
pub(crate) fn kw(args: &[Vec<u8>]) -> CryptoResult {
    use aws_lc_rs::key_wrap::{AES_128, AES_256, AesKek, KeyWrap};
    let [mode, key, data] = args else {
        return Err(CryptoError::operation("aes_kw expects (mode, key, data)"));
    };
    let cipher = match key.len() {
        16 => &AES_128,
        32 => &AES_256,
        24 => {
            return Err(CryptoError::not_supported(
                "AES-KW with a 192-bit key is not supported",
            ));
        }
        _ => return Err(CryptoError::operation("Invalid AES key length")),
    };
    let kek = AesKek::new(cipher, key)?;
    if mode.as_slice() == b"encrypt" {
        if data.len() % 8 != 0 || data.len() < 16 {
            return Err(CryptoError::operation(
                "AES-KW input must be a multiple of 8 bytes and at least 16",
            ));
        }
        let mut out = vec![0u8; data.len() + 8];
        let len = kek.wrap(data, &mut out)?.len();
        out.truncate(len);
        return Ok(vec![out]);
    }
    if data.len() % 8 != 0 || data.len() < 24 {
        return Err(CryptoError::operation("Invalid AES-KW wrapped key length"));
    }
    let mut out = vec![0u8; data.len() - 8];
    let len = kek.unwrap(data, &mut out).map_err(decrypting())?.len();
    out.truncate(len);
    Ok(vec![out])
}
