use aws_lc_rs::digest;

use crate::error::{CryptoError, CryptoResult};

pub(crate) fn algorithm(name: &str) -> Result<&'static digest::Algorithm, CryptoError> {
    match name {
        "SHA-1" => Ok(&digest::SHA1_FOR_LEGACY_USE_ONLY),
        "SHA-256" => Ok(&digest::SHA256),
        "SHA-384" => Ok(&digest::SHA384),
        "SHA-512" => Ok(&digest::SHA512),
        other => Err(CryptoError::not_supported(format!(
            "Unrecognized hash {other}"
        ))),
    }
}

/// `digest(hash, data)`
pub(crate) fn run(args: &[Vec<u8>]) -> CryptoResult {
    let [hash, data] = args else {
        return Err(CryptoError::operation("digest expects (hash, data)"));
    };
    let alg = algorithm(&String::from_utf8_lossy(hash))?;
    Ok(vec![digest::digest(alg, data).as_ref().to_vec()])
}
