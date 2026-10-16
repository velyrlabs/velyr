//! AES-GCM, AES-CBC and ChaCha20-Poly1305 for `createCipheriv`. One-shot: the JS side buffers
//! `update()` input and calls once from `final()`. aws-lc-rs fixes AEAD nonces to 12 bytes and
//! tags to 16 bytes, so other sizes are rejected up front by the JS validation.

use aws_lc_rs::aead::{
    AES_128_GCM, AES_192_GCM, AES_256_GCM, Aad, Algorithm, CHACHA20_POLY1305, LessSafeKey, Nonce,
    UnboundKey,
};
use aws_lc_rs::cipher::{
    AES_128, AES_192, AES_256, DecryptingKey, DecryptionContext, EncryptingKey, EncryptionContext,
    PaddedBlockDecryptingKey, PaddedBlockEncryptingKey, UnboundCipherKey,
};
use aws_lc_rs::iv::FixedLength;

type Parts = Result<Vec<Vec<u8>>, String>;

const TAG_LEN: usize = 16;
const BLOCK: usize = 16;

pub(crate) const INVALID_KEY: &str = "ERR_CRYPTO_INVALID_KEYLEN|Invalid key length";
pub(crate) const INVALID_IV: &str = "ERR_CRYPTO_INVALID_IV|Invalid initialization vector";
pub(crate) const BAD_LENGTH: &str =
    "ERR_OSSL_WRONG_FINAL_BLOCK_LENGTH|error:1C80006B:Provider routines::wrong final block length";
pub(crate) const BAD_DECRYPT: &str =
    "ERR_OSSL_BAD_DECRYPT|error:1C800064:Provider routines::bad decrypt";

fn aead_algorithm(name: &str) -> Option<&'static Algorithm> {
    Some(match name {
        "aes-128-gcm" => &AES_128_GCM,
        "aes-192-gcm" => &AES_192_GCM,
        "aes-256-gcm" => &AES_256_GCM,
        "chacha20-poly1305" => &CHACHA20_POLY1305,
        _ => return None,
    })
}

fn authentication_failed() -> String {
    "Unsupported state or unable to authenticate data".into()
}

/// `cipher(name, mode, key, iv, aad, data, tag, padding) -> [output, tag]`. `mode` is `encrypt`
/// or `decrypt`; AEAD decrypt takes the tag separately, encrypt returns it. `padding` is `1` or
/// `0` (CBC only).
pub(crate) fn run(args: &[Vec<u8>]) -> Parts {
    let [name, mode, key, iv, aad, data, tag, padding] = args else {
        return Err("cipher expects (name, mode, key, iv, aad, data, tag, padding)".into());
    };
    let name = String::from_utf8_lossy(name);
    let encrypt = mode == b"encrypt";
    if let Some(algorithm) = aead_algorithm(&name) {
        return aead(algorithm, encrypt, [key, iv, aad], data, tag);
    }
    cbc(encrypt, key, iv, data, padding == b"1")
}

fn aead(
    algorithm: &'static Algorithm,
    encrypt: bool,
    [key, iv, aad]: [&[u8]; 3],
    data: &[u8],
    tag: &[u8],
) -> Parts {
    let nonce = Nonce::try_assume_unique_for_key(iv).map_err(|_| INVALID_IV.to_string())?;
    let key = UnboundKey::new(algorithm, key).map_err(|_| INVALID_KEY.to_string())?;
    let key = LessSafeKey::new(key);
    let mut buffer = data.to_vec();
    if encrypt {
        key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut buffer)
            .map_err(|_| "Encryption failed".to_string())?;
        let tag = buffer.split_off(buffer.len() - TAG_LEN);
        return Ok(vec![buffer, tag]);
    }
    if tag.len() != TAG_LEN {
        return Err(authentication_failed());
    }
    buffer.extend_from_slice(tag);
    let plain_len = key
        .open_in_place(nonce, Aad::from(aad), &mut buffer)
        .map_err(|_| authentication_failed())?
        .len();
    buffer.truncate(plain_len);
    Ok(vec![buffer, Vec::new()])
}

fn cbc(encrypt: bool, key: &[u8], iv: &[u8], data: &[u8], padded: bool) -> Parts {
    let algorithm = match key.len() {
        16 => &AES_128,
        24 => &AES_192,
        32 => &AES_256,
        _ => return Err(INVALID_KEY.into()),
    };
    let key = UnboundCipherKey::new(algorithm, key).map_err(|_| INVALID_KEY.to_string())?;
    let iv: [u8; BLOCK] = iv.try_into().map_err(|_| INVALID_IV.to_string())?;
    let context = FixedLength::from(iv);
    let mut buffer = data.to_vec();
    let failed = |_| "Cipher operation failed".to_string();
    if encrypt && padded {
        PaddedBlockEncryptingKey::cbc_pkcs7(key)
            .and_then(|key| key.less_safe_encrypt(&mut buffer, EncryptionContext::Iv128(context)))
            .map_err(failed)?;
        return Ok(vec![buffer, Vec::new()]);
    }
    if !buffer.len().is_multiple_of(BLOCK) || (!encrypt && padded && buffer.is_empty()) {
        return Err(BAD_LENGTH.into());
    }
    if buffer.is_empty() {
        return Ok(vec![buffer, Vec::new()]);
    }
    if encrypt {
        EncryptingKey::cbc(key)
            .and_then(|key| key.less_safe_encrypt(&mut buffer, EncryptionContext::Iv128(context)))
            .map_err(failed)?;
    } else if padded {
        let plain_len = PaddedBlockDecryptingKey::cbc_pkcs7(key)
            .and_then(|key| {
                key.decrypt(&mut buffer, DecryptionContext::Iv128(context))
                    .map(|plain| plain.len())
            })
            .map_err(|_| BAD_DECRYPT.to_string())?;
        buffer.truncate(plain_len);
    } else {
        DecryptingKey::cbc(key)
            .and_then(|key| {
                key.decrypt(&mut buffer, DecryptionContext::Iv128(context))
                    .map(drop)
            })
            .map_err(failed)?;
    }
    Ok(vec![buffer, Vec::new()])
}
