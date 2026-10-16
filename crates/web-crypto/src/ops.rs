use velyr_engine_api::{CallContext, HostError, HostFunctionDef, JsString, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

use crate::error::{CryptoError, CryptoResult};
use crate::frame::{frame, unframe};
use crate::{aes, digest, kdf, keys, rsa, sign};

/// `getRandomValues` limit (WebCrypto): the JS layer throws `QuotaExceededError` first; this is the backstop.
pub(crate) const MAX_RANDOM_BYTES: usize = 65536;

pub fn crypto_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_crypto_random",
            arity: 1,
            callback: op_crypto_random,
        },
        HostFunctionDef {
            name: "op_crypto_run",
            arity: 2,
            callback: op_crypto_run,
        },
    ]
}

fn binary_string(bytes: &[u8]) -> Value {
    Value::String(JsString::from_utf16(
        bytes.iter().map(|&b| u16::from(b)).collect(),
    ))
}

/// `op_crypto_random(length) -> binary string` of CSPRNG bytes (one byte per UTF-16 unit).
fn op_crypto_random(_ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::Number(length)) = args.first() else {
        return Err(HostError::TypeError(
            "op_crypto_random expects a length".into(),
        ));
    };
    if !(0.0..=MAX_RANDOM_BYTES as f64).contains(length) {
        return Err(HostError::TypeError(
            "op_crypto_random length out of range".into(),
        ));
    }
    let mut bytes = vec![0u8; *length as usize];
    aws_lc_rs::rand::fill(&mut bytes)
        .map_err(|_| HostError::TypeError("random generator failed".into()))?;
    Ok(binary_string(&bytes))
}

fn dispatch(name: &str, args: &[Vec<u8>]) -> CryptoResult {
    match name {
        "digest" => digest::run(args),
        "hmac_sign" => sign::hmac_sign(args),
        "hmac_verify" => sign::hmac_verify(args),
        "ecdsa_sign" => sign::ecdsa_sign(args),
        "ecdsa_verify" => sign::ecdsa_verify(args),
        "ed25519_sign" => sign::ed25519_sign(args),
        "ed25519_verify" => sign::ed25519_verify(args),
        "aes_gcm" => aes::gcm(args),
        "aes_cbc" => aes::cbc(args),
        "aes_ctr" => aes::ctr(args),
        "aes_kw" => aes::kw(args),
        "ecdh_derive" => kdf::agree(args),
        "pbkdf2" => kdf::pbkdf2(args),
        "hkdf" => kdf::hkdf(args),
        "rsa_generate" => rsa::generate(args),
        "rsa_decode" => rsa::decode(args),
        "rsa_encode" => rsa::encode(args),
        "rsa_components" => rsa::components(args),
        "rsa_assemble" => rsa::assemble(args),
        "rsa_sign" => rsa::sign(args),
        "rsa_verify" => rsa::verify(args),
        "rsa_oaep" => rsa::oaep(args),
        "key_generate" => keys::generate(args),
        "key_public" => keys::public(args),
        "key_encode" => keys::encode(args),
        "key_decode" => keys::decode(args),
        "key_check_public" => keys::validate_public(args),
        other => Err(CryptoError::not_supported(format!(
            "Unknown crypto operation {other}"
        ))),
    }
}

/// `op_crypto_run(name, input) -> Promise<ArrayBuffer>`: `input` is a framed binary string (see `frame`), the
/// result a framed buffer. The work runs on the blocking pool, off the isolate thread.
fn op_crypto_run(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(name), Value::String(input)] = args else {
        return Err(HostError::TypeError(
            "op_crypto_run expects (name, input)".into(),
        ));
    };
    let name = name.to_string_lossy();
    let input: Vec<u8> = input.as_utf16().iter().map(|&unit| unit as u8).collect();
    op_async(ctx, |_token| async move {
        let output = tokio::task::spawn_blocking(move || dispatch(&name, &unframe(&input)?))
            .await
            .map_err(|err| VelyrError::internal(err.to_string()))??;
        Ok(CompletionValue::Bytes(frame(&output)))
    })
}
