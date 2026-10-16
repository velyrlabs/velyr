//! Host ops behind `node:crypto`. Two entry points share one dispatcher: `op_node_crypto` (sync)
//! and `op_node_crypto_async` (a promise; the work runs on the blocking pool). Inputs are an op
//! name plus typed arrays; the result is one ArrayBuffer: a status byte (0 ok, 1 error) and then
//! parts, each a big-endian `u32` length plus bytes. Errors carry `CODE|message` or just a message.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

use crate::{crypto_cipher, crypto_hash, crypto_keys, crypto_sign, crypto_stream};

pub(crate) type Parts = Result<Vec<Vec<u8>>, String>;

/// Largest `randomBytes` request (Node's `kMaxLength` is larger; this bounds one allocation).
const MAX_RANDOM: usize = 1 << 31;

pub(crate) fn crypto_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_node_crypto",
            arity: 1,
            callback: op_sync,
        },
        HostFunctionDef {
            name: "op_node_crypto_async",
            arity: 1,
            callback: op_async_call,
        },
    ]
}

fn number(bytes: &[u8]) -> Result<u64, String> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| "Invalid numeric argument".to_string())
}

fn length(bytes: &[u8]) -> Result<usize, String> {
    usize::try_from(number(bytes)?).map_err(|_| "Invalid length".to_string())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn random(length: usize) -> Result<Vec<u8>, String> {
    if length >= MAX_RANDOM {
        return Err("ERR_OUT_OF_RANGE|The value of \"size\" is out of range".into());
    }
    let mut out = vec![0u8; length];
    aws_lc_rs::rand::fill(&mut out).map_err(|_| "random generator failed".to_string())?;
    Ok(out)
}

fn dispatch(name: &str, args: &[Vec<u8>]) -> Parts {
    let wrong = || format!("{name}: wrong arguments");
    match (name, args) {
        ("hash", [alg, data]) => Ok(vec![crypto_hash::hash(&text(alg), data)?]),
        ("hmac", [alg, key, data]) => Ok(vec![crypto_hash::hmac(&text(alg), key, data)?]),
        ("random", [size]) => Ok(vec![random(length(size)?)?]),
        ("pbkdf2", [alg, password, salt, iterations, size]) => {
            let iterations = u32::try_from(number(iterations)?).map_err(|_| wrong())?;
            Ok(vec![crypto_hash::pbkdf2(
                &text(alg),
                password,
                salt,
                iterations,
                length(size)?,
            )?])
        }
        ("scrypt", [password, salt, n, r, p, maxmem, size]) => Ok(vec![crypto_hash::scrypt(
            password,
            salt,
            (number(n)?, number(r)?, number(p)?),
            number(maxmem)?,
            length(size)?,
        )?]),
        ("timing_safe_equal", [a, b]) => {
            if a.len() != b.len() {
                return Err("ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH|Input buffers must have the same byte length".into());
            }
            let equal = aws_lc_rs::constant_time::verify_slices_are_equal(a, b).is_ok();
            Ok(vec![vec![u8::from(equal)]])
        }
        ("hkdf", [alg, key, salt, info, size]) => Ok(vec![crypto_hash::hkdf(
            &text(alg),
            key,
            salt,
            info,
            length(size)?,
        )?]),
        ("cipher", _) => crypto_cipher::run(args),
        ("sign", _) => crypto_sign::sign(args),
        ("verify", _) => crypto_sign::verify(args),
        ("key_parse", _) => crypto_keys::parse(args),
        ("key_public", _) => crypto_keys::public(args),
        ("key_generate", _) => crypto_keys::generate(args),
        ("key_export", _) => crypto_keys::export(args),
        ("key_jwk_export", _) => crypto_keys::jwk_export(args),
        ("key_jwk_import", _) => crypto_keys::jwk_import(args),
        _ => Err(format!("Unknown crypto operation {name}")),
    }
}

/// Frames an op result: a status byte (0 ok, 1 error) then length-prefixed parts. Shared with zlib.
pub(crate) fn encode(result: Parts) -> Vec<u8> {
    let (status, parts) = match result {
        Ok(parts) => (0u8, parts),
        Err(message) => (1u8, vec![message.into_bytes()]),
    };
    let mut out = vec![status];
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(&part);
    }
    out
}

/// Splits `(name, ...typed arrays)` into the op name and the copied bytes; `label` names the op
/// family in errors. Shared with zlib.
pub(crate) fn read_call(
    ctx: &mut dyn CallContext,
    args: &[Value],
    label: &str,
) -> Result<(String, Vec<Vec<u8>>), HostError> {
    let Some((Value::String(name), rest)) = args.split_first() else {
        return Err(HostError::TypeError(format!(
            "{label} op expects (name, ...bytes)"
        )));
    };
    let parts = rest
        .iter()
        .map(|value| ctx.read_bytes(value))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            HostError::TypeError(format!("{label} op arguments must be typed arrays"))
        })?;
    Ok((name.to_string_lossy(), parts))
}

fn op_sync(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (name, parts) = read_call(ctx, args, "crypto")?;
    let result = if crypto_stream::handles(&name) {
        crypto_stream::run(ctx, &name, &parts)
    } else {
        dispatch(&name, &parts)
    };
    Ok(ctx.create_array_buffer(&encode(result)))
}

fn op_async_call(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (name, parts) = read_call(ctx, args, "crypto")?;
    op_async(ctx, |_token| async move {
        let result = tokio::task::spawn_blocking(move || dispatch(&name, &parts))
            .await
            .map_err(|err| VelyrError::internal(err.to_string()))?;
        Ok(CompletionValue::Bytes(encode(result)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_safe_equal_compares_contents_and_rejects_length_mismatch() {
        let call = |a: &[u8], b: &[u8]| dispatch("timing_safe_equal", &[a.to_vec(), b.to_vec()]);
        assert_eq!(call(b"abc", b"abc"), Ok(vec![vec![1]]));
        assert_eq!(call(b"abc", b"abd"), Ok(vec![vec![0]]));
        assert_eq!(call(b"", b""), Ok(vec![vec![1]]));
        let error = call(b"a", b"ab").unwrap_err();
        assert!(error.starts_with("ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH|"));
    }
}
