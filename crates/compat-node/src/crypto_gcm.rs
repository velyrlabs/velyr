//! Streaming AES-GCM encryption. aws-lc-rs only offers GCM as a one-shot AEAD, so this builds it
//! the way NIST SP 800-38D defines it: AES-CTR from aws-lc-rs for the keystream (counter block
//! `IV || 2` onward), GHASH computed here, and a tag masked with `AES(K, IV || 1)`. Only
//! encryption streams; decryption stays one-shot so no plaintext is released before the tag is
//! verified. Output is checked against the one-shot AEAD in the tests below.

use aws_lc_rs::cipher::{
    AES_128, AES_192, AES_256, Algorithm, EncryptionContext, StreamingEncryptingKey,
    UnboundCipherKey,
};
use aws_lc_rs::iv::FixedLength;

const BLOCK: usize = 16;
/// NIST limit: 2^36 - 32 bytes of data per (key, IV), which is where the 32-bit counter wraps.
const MAX_DATA: u64 = (1 << 36) - 32;
/// GHASH reduction polynomial `x^128 + x^7 + x^2 + x + 1`, bit-reflected.
const REDUCTION: u128 = 0xe1 << 120;

const INVALID_KEY: &str = "ERR_CRYPTO_INVALID_KEYLEN|Invalid key length";
const INVALID_IV: &str = "ERR_CRYPTO_INVALID_IV|Invalid initialization vector";

/// Multiplication in GF(2^128) with the GCM bit order. Branch-free: every step uses masks.
fn gf_multiply(x: u128, y: u128) -> u128 {
    let (mut product, mut shifted) = (0u128, y);
    for bit in (0..128).rev() {
        product ^= shifted & 0u128.wrapping_sub((x >> bit) & 1);
        let carry = shifted & 1;
        shifted = (shifted >> 1) ^ (REDUCTION & 0u128.wrapping_sub(carry));
    }
    product
}

struct Ghash {
    key: u128,
    state: u128,
    partial: [u8; BLOCK],
    filled: usize,
}

impl Ghash {
    fn new(key: [u8; BLOCK]) -> Self {
        Self {
            key: u128::from_be_bytes(key),
            state: 0,
            partial: [0; BLOCK],
            filled: 0,
        }
    }

    fn block(&mut self, block: [u8; BLOCK]) {
        self.state = gf_multiply(self.state ^ u128::from_be_bytes(block), self.key);
    }

    fn absorb(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = data.len().min(BLOCK - self.filled);
            self.partial[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == BLOCK {
                self.block(self.partial);
                self.filled = 0;
            }
        }
    }

    /// Zero-pads a partial block, as GCM does between the AAD and the ciphertext.
    fn pad(&mut self) {
        if self.filled > 0 {
            self.partial[self.filled..].fill(0);
            self.block(self.partial);
            self.filled = 0;
        }
    }
}

pub(crate) fn algorithm(key: &[u8]) -> Result<&'static Algorithm, String> {
    match key.len() {
        16 => Ok(&AES_128),
        24 => Ok(&AES_192),
        32 => Ok(&AES_256),
        _ => Err(INVALID_KEY.into()),
    }
}

/// AES-CTR starting at `counter_block`; the keystream is what `update` XORs into the data.
fn counter_mode(key: &[u8], counter_block: [u8; BLOCK]) -> Result<StreamingEncryptingKey, String> {
    let key = UnboundCipherKey::new(algorithm(key)?, key).map_err(|_| INVALID_KEY.to_string())?;
    let context = EncryptionContext::Iv128(FixedLength::from(counter_block));
    StreamingEncryptingKey::less_safe_ctr(key, context).map_err(|_| INVALID_KEY.to_string())
}

fn counter_block(iv: &[u8], counter: u32) -> Result<[u8; BLOCK], String> {
    let iv: [u8; 12] = iv.try_into().map_err(|_| INVALID_IV.to_string())?;
    let mut block = [0u8; BLOCK];
    block[..12].copy_from_slice(&iv);
    block[12..].copy_from_slice(&counter.to_be_bytes());
    Ok(block)
}

/// `AES(K, block)`, taken as the first keystream block of a CTR run that starts at `block`.
fn encrypt_block(key: &[u8], block: [u8; BLOCK]) -> Result<[u8; BLOCK], String> {
    let mut stream = counter_mode(key, block)?;
    let mut output = [0u8; 2 * BLOCK];
    let update = stream
        .update(&[0u8; BLOCK], &mut output)
        .map_err(|_| "Encryption failed".to_string())?;
    update
        .written()
        .try_into()
        .map_err(|_| "Encryption failed".to_string())
}

pub(crate) struct GcmEncrypt {
    stream: StreamingEncryptingKey,
    ghash: Ghash,
    tag_mask: [u8; BLOCK],
    aad_len: u64,
    data_len: u64,
}

impl GcmEncrypt {
    pub fn new(key: &[u8], iv: &[u8], aad: &[u8]) -> Result<Self, String> {
        let mut ghash = Ghash::new(encrypt_block(key, [0; BLOCK])?);
        ghash.absorb(aad);
        ghash.pad();
        Ok(Self {
            stream: counter_mode(key, counter_block(iv, 2)?)?,
            ghash,
            tag_mask: encrypt_block(key, counter_block(iv, 1)?)?,
            aad_len: aad.len() as u64,
            data_len: 0,
        })
    }

    pub fn update(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
        self.data_len += data.len() as u64;
        if self.data_len > MAX_DATA {
            return Err("ERR_CRYPTO_INVALID_STATE|Too much data for one AES-GCM message".into());
        }
        let mut output = vec![0u8; data.len() + BLOCK];
        let written = self
            .stream
            .update(data, &mut output)
            .map_err(|_| "Encryption failed".to_string())?
            .written()
            .len();
        output.truncate(written);
        self.ghash.absorb(&output);
        Ok(output)
    }

    /// The 16-byte authentication tag; the ciphertext was complete after the last `update`.
    pub fn finish(mut self) -> [u8; BLOCK] {
        self.ghash.pad();
        let mut lengths = [0u8; BLOCK];
        lengths[..8].copy_from_slice(&(self.aad_len * 8).to_be_bytes());
        lengths[8..].copy_from_slice(&(self.data_len * 8).to_be_bytes());
        self.ghash.block(lengths);
        (self.ghash.state ^ u128::from_be_bytes(self.tag_mask)).to_be_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto_cipher;

    fn one_shot(key: &[u8], iv: &[u8], aad: &[u8], data: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let name = format!("aes-{}-gcm", key.len() * 8).into_bytes();
        let args = [
            name,
            b"encrypt".to_vec(),
            key.to_vec(),
            iv.to_vec(),
            aad.to_vec(),
            data.to_vec(),
            vec![],
            b"1".to_vec(),
        ];
        let mut parts = crypto_cipher::run(&args).unwrap();
        let tag = parts.pop().unwrap();
        (parts.pop().unwrap(), tag)
    }

    fn streamed(key: &[u8], iv: &[u8], aad: &[u8], data: &[u8], step: usize) -> (Vec<u8>, Vec<u8>) {
        let mut gcm = GcmEncrypt::new(key, iv, aad).unwrap();
        let mut out = Vec::new();
        for chunk in data.chunks(step.max(1)) {
            out.extend(gcm.update(chunk).unwrap());
        }
        (out, gcm.finish().to_vec())
    }

    #[test]
    fn streaming_matches_the_one_shot_aead_for_every_split() {
        let iv = [9u8; 12];
        for key_len in [16, 24, 32] {
            let key: Vec<u8> = (0..key_len as u8).collect();
            for aad_len in [0, 1, 15, 16, 17, 40] {
                let aad: Vec<u8> = (0..aad_len as u8).map(|b| b ^ 0x5a).collect();
                for data_len in [0, 1, 15, 16, 17, 31, 32, 100, 1000] {
                    let data: Vec<u8> = (0..data_len).map(|i| (i * 7 % 251) as u8).collect();
                    let expected = one_shot(&key, &iv, &aad, &data);
                    for step in [1, 3, 16, 17, 64, 5000] {
                        let got = streamed(&key, &iv, &aad, &data, step);
                        assert_eq!(
                            got, expected,
                            "key {key_len} aad {aad_len} data {data_len} step {step}"
                        );
                    }
                }
            }
        }
    }

    // NIST GCM test case 4 (AES-128, with AAD, 60 bytes of plaintext).
    #[test]
    fn matches_nist_test_case_4() {
        let hex = |text: &str| -> Vec<u8> {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect()
        };
        let key = hex("feffe9928665731c6d6a8f9467308308");
        let iv = hex("cafebabefacedbaddecaf888");
        let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        let plain = hex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
        );
        let (cipher, tag) = streamed(&key, &iv, &aad, &plain, 13);
        assert_eq!(
            cipher,
            hex(
                "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091"
            )
        );
        assert_eq!(tag, hex("5bc94fbc3221a5db94fae95ae7121a47"));
    }

    #[test]
    fn rejects_bad_key_and_iv_lengths() {
        assert!(GcmEncrypt::new(&[0; 15], &[0; 12], b"").is_err());
        assert!(GcmEncrypt::new(&[0; 16], &[0; 11], b"").is_err());
    }
}
