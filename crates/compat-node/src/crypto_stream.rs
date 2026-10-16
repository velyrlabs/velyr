//! Incremental `node:crypto` state (Hash, Hmac, Cipheriv, Decipheriv). Each context lives in the
//! isolate's resource table and JS holds only its id, so these ops need the call context and run
//! on the sync path only. Ops: `hash_new`, `hmac_new`, `cipher_new`, `ctx_update`, `ctx_final`,
//! `ctx_copy`, `ctx_free`.
//!
//! Streaming covers hashes, HMAC, AES-CBC (PKCS#7) and AES-GCM encryption. GCM decryption,
//! ChaCha20-Poly1305 and CBC without padding stay one-shot (`crypto_cipher`): releasing GCM
//! plaintext before the tag is verified would be unsafe, and those other modes are rare.

use std::any::Any;
use std::cell::RefCell;

use aws_lc_rs::cipher::{
    DecryptionContext, EncryptionContext, StreamingDecryptingKey, StreamingEncryptingKey,
    UnboundCipherKey,
};
use aws_lc_rs::iv::FixedLength;
use velyr_engine_api::CallContext;
use velyr_runtime_ops::OpContext;
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId, ResourceTable};

use crate::crypto_cipher::{BAD_DECRYPT, BAD_LENGTH, INVALID_IV, INVALID_KEY};
use crate::crypto_gcm::{self, GcmEncrypt};
use crate::crypto_hash::{Hasher, Hmac};

type Parts = Result<Vec<Vec<u8>>, String>;

const BLOCK: usize = 16;
const FAILED: &str = "Cipher operation failed";
const CONSUMED: &str =
    "ERR_CRYPTO_INVALID_STATE|Invalid state for operation: context already finished";

enum Machine {
    Hash(Hasher),
    Hmac(Hmac),
    Gcm(Box<GcmEncrypt>),
    CbcEncrypt(Box<StreamingEncryptingKey>),
    CbcDecrypt {
        key: Box<StreamingDecryptingKey>,
        consumed: usize,
    },
}

/// `RefCell` because `ResourceTable::get` only hands out `&T`.
struct CryptoContext(RefCell<Option<Machine>>);

impl Resource for CryptoContext {
    fn kind(&self) -> &'static str {
        "crypto.context"
    }
    fn close(self: Box<Self>) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn cipher_key(key: &[u8]) -> Result<UnboundCipherKey, String> {
    UnboundCipherKey::new(crypto_gcm::algorithm(key)?, key).map_err(|_| INVALID_KEY.to_string())
}

fn iv_block(iv: &[u8]) -> Result<FixedLength<BLOCK>, String> {
    let iv: [u8; BLOCK] = iv.try_into().map_err(|_| INVALID_IV.to_string())?;
    Ok(FixedLength::from(iv))
}

/// Streaming machine for a cipher, or an error when this combination only exists one-shot.
fn cipher_machine(
    name: &str,
    encrypt: bool,
    key: &[u8],
    iv: &[u8],
    aad: &[u8],
) -> Result<Machine, String> {
    if name.ends_with("-gcm") && encrypt {
        return Ok(Machine::Gcm(Box::new(GcmEncrypt::new(key, iv, aad)?)));
    }
    if !name.ends_with("-cbc") {
        return Err(format!("{name} does not support streaming in this mode"));
    }
    let (key, context) = (cipher_key(key)?, iv_block(iv)?);
    if encrypt {
        let key =
            StreamingEncryptingKey::less_safe_cbc_pkcs7(key, EncryptionContext::Iv128(context));
        return Ok(Machine::CbcEncrypt(Box::new(
            key.map_err(|_| FAILED.to_string())?,
        )));
    }
    let key = StreamingDecryptingKey::cbc_pkcs7(key, DecryptionContext::Iv128(context));
    Ok(Machine::CbcDecrypt {
        key: Box::new(key.map_err(|_| FAILED.to_string())?),
        consumed: 0,
    })
}

impl Machine {
    fn update(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let failed = |_| FAILED.to_string();
        let mut output = vec![0u8; data.len() + BLOCK];
        let written = match self {
            Self::Hash(hasher) => {
                hasher.update(data);
                return Ok(Vec::new());
            }
            Self::Hmac(hmac) => {
                hmac.update(data);
                return Ok(Vec::new());
            }
            Self::Gcm(gcm) => return gcm.update(data),
            Self::CbcEncrypt(key) => key
                .update(data, &mut output)
                .map_err(failed)?
                .written()
                .len(),
            Self::CbcDecrypt { key, consumed } => {
                *consumed += data.len();
                key.update(data, &mut output)
                    .map_err(failed)?
                    .written()
                    .len()
            }
        };
        output.truncate(written);
        Ok(output)
    }

    /// Consumes the machine: `[digest]` for hashes, `[output, tag]` for ciphers.
    fn finish(self) -> Parts {
        let mut last = [0u8; BLOCK];
        match self {
            Self::Hash(hasher) => Ok(vec![hasher.finish()]),
            Self::Hmac(hmac) => Ok(vec![hmac.finish()]),
            Self::Gcm(gcm) => Ok(vec![Vec::new(), gcm.finish().to_vec()]),
            Self::CbcEncrypt(key) => {
                let (_, update) = key.finish(&mut last).map_err(|_| FAILED.to_string())?;
                Ok(vec![update.written().to_vec(), Vec::new()])
            }
            Self::CbcDecrypt { key, consumed } => {
                if consumed == 0 || !consumed.is_multiple_of(BLOCK) {
                    return Err(BAD_LENGTH.into());
                }
                let update = key.finish(&mut last).map_err(|_| BAD_DECRYPT.to_string())?;
                Ok(vec![update.written().to_vec(), Vec::new()])
            }
        }
    }

    fn duplicate(&self) -> Result<Machine, String> {
        match self {
            Self::Hash(hasher) => Ok(Self::Hash(hasher.clone())),
            Self::Hmac(hmac) => Ok(Self::Hmac(hmac.clone())),
            _ => Err("ERR_CRYPTO_INVALID_STATE|Only hashes can be copied".into()),
        }
    }
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "hash_new"
            | "hmac_new"
            | "cipher_new"
            | "ctx_update"
            | "ctx_final"
            | "ctx_copy"
            | "ctx_free"
    )
}

fn open(table: &mut ResourceTable, machine: Machine) -> Parts {
    let resource = Box::new(CryptoContext(RefCell::new(Some(machine))));
    // shortcut: no QuotaClass::Crypto yet, so contexts count against Files; add one in runtime-resources.
    let id = table
        .insert(resource, QuotaClass::Files, None)
        .map_err(|error| error.to_string())?;
    Ok(vec![id.to_js_number().to_string().into_bytes()])
}

fn parse_id(bytes: &[u8]) -> Result<ResourceId, String> {
    let value: f64 = text(bytes)
        .parse()
        .map_err(|_| "Invalid context id".to_string())?;
    ResourceId::from_js_number(value).map_err(|error| error.to_string())
}

fn with<T>(
    table: &ResourceTable,
    id: &[u8],
    action: impl FnOnce(&mut Machine) -> Result<T, String>,
) -> Result<T, String> {
    let context = table
        .get::<CryptoContext>(parse_id(id)?)
        .map_err(|error| error.to_string())?;
    let mut slot = context.0.borrow_mut();
    action(slot.as_mut().ok_or(CONSUMED)?)
}

pub(crate) fn run(ctx: &mut dyn CallContext, name: &str, args: &[Vec<u8>]) -> Parts {
    let table = &mut ctx
        .isolate_slot::<OpContext>()
        .ok_or("crypto contexts need an installed OpContext")?
        .resources;
    match (name, args) {
        ("hash_new", [alg]) => open(table, Machine::Hash(Hasher::new(&text(alg))?)),
        ("hmac_new", [alg, key]) => open(table, Machine::Hmac(Hmac::new(&text(alg), key)?)),
        ("cipher_new", [cipher, mode, key, iv, aad]) => open(
            table,
            cipher_machine(&text(cipher), mode == b"encrypt", key, iv, aad)?,
        ),
        ("ctx_update", [id, data]) => Ok(vec![with(table, id, |machine| machine.update(data))?]),
        ("ctx_copy", [id]) => {
            let copy = with(table, id, |machine| machine.duplicate())?;
            open(table, copy)
        }
        ("ctx_final", [id]) => {
            let machine = take(table, id)?;
            let _ = table.close(parse_id(id)?);
            machine.finish()
        }
        // Frees abandoned contexts (JS finalizers); an already-finished one is not an error.
        ("ctx_free", [id]) => {
            let _ = table.close(parse_id(id)?);
            Ok(Vec::new())
        }
        _ => Err(format!("{name}: wrong arguments")),
    }
}

fn take(table: &ResourceTable, id: &[u8]) -> Result<Machine, String> {
    let context = table
        .get::<CryptoContext>(parse_id(id)?)
        .map_err(|error| error.to_string())?;
    context
        .0
        .borrow_mut()
        .take()
        .ok_or_else(|| CONSUMED.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_machine(machine: Machine, chunks: &[&[u8]]) -> Vec<u8> {
        let mut machine = machine;
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(machine.update(chunk).unwrap());
        }
        out.extend(machine.finish().unwrap().remove(0));
        out
    }

    #[test]
    fn cbc_streaming_matches_the_one_shot_cipher_in_both_directions() {
        let (key, iv) = ([7u8; 32], [3u8; 16]);
        for len in [0usize, 1, 15, 16, 17, 100] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let one_shot = |mode: &str, input: &[u8]| {
                let args = [
                    b"aes-256-cbc".to_vec(),
                    mode.as_bytes().to_vec(),
                    key.to_vec(),
                    iv.to_vec(),
                    vec![],
                    input.to_vec(),
                    vec![],
                    b"1".to_vec(),
                ];
                crate::crypto_cipher::run(&args).unwrap().remove(0)
            };
            let sealed = one_shot("encrypt", &data);
            let encrypt = cipher_machine("aes-256-cbc", true, &key, &iv, b"").unwrap();
            let parts: Vec<&[u8]> = data.chunks(5).collect();
            assert_eq!(run_machine(encrypt, &parts), sealed, "encrypt {len}");
            let decrypt = cipher_machine("aes-256-cbc", false, &key, &iv, b"").unwrap();
            let parts: Vec<&[u8]> = sealed.chunks(7).collect();
            assert_eq!(run_machine(decrypt, &parts), data, "decrypt {len}");
        }
    }

    #[test]
    fn cbc_encrypt_update_returns_whole_blocks_as_they_complete() {
        let mut machine = cipher_machine("aes-128-cbc", true, &[1; 16], &[2; 16], b"").unwrap();
        assert_eq!(machine.update(&[0; 10]).unwrap().len(), 0);
        assert_eq!(machine.update(&[0; 10]).unwrap().len(), 16);
    }

    #[test]
    fn cbc_decrypt_reports_length_and_padding_errors() {
        let mut machine = cipher_machine("aes-128-cbc", false, &[1; 16], &[2; 16], b"").unwrap();
        machine.update(&[0; 5]).unwrap();
        assert_eq!(machine.finish().unwrap_err(), BAD_LENGTH);
        let mut machine = cipher_machine("aes-128-cbc", false, &[1; 16], &[2; 16], b"").unwrap();
        machine.update(&[0; 16]).unwrap();
        assert_eq!(machine.finish().unwrap_err(), BAD_DECRYPT);
    }

    #[test]
    fn hash_and_hmac_in_chunks_match_one_shot_and_copies_are_independent() {
        let whole = crate::crypto_hash::hash("sha256", b"hello world").unwrap();
        let mut hash = Machine::Hash(Hasher::new("sha256").unwrap());
        hash.update(b"hello ").unwrap();
        let mut copy = hash.duplicate().unwrap();
        hash.update(b"world").unwrap();
        copy.update(b"there").unwrap();
        assert_eq!(hash.finish().unwrap().remove(0), whole);
        assert_ne!(copy.finish().unwrap().remove(0), whole);

        for alg in ["sha256", "md5", "sha3-256"] {
            let expected = crate::crypto_hash::hmac(alg, b"key", b"hello world").unwrap();
            let hmac = Machine::Hmac(Hmac::new(alg, b"key").unwrap());
            assert_eq!(
                run_machine(hmac, &[b"hello", b" ", b"world"]),
                expected,
                "{alg}"
            );
        }
    }
}
