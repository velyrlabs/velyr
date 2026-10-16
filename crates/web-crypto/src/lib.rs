//! WebCrypto (`crypto.getRandomValues`, `crypto.randomUUID`, `crypto.subtle`) on aws-lc-rs. The JS layer
//! validates and builds `CryptoKey`s; Rust only does stateless primitives over raw key bytes.

mod aes;
mod der;
mod digest;
mod error;
mod frame;
mod install;
mod kdf;
mod keys;
mod ops;
mod rsa;
mod sign;

pub use install::{CRYPTO_JS, install_web_crypto};
pub use ops::crypto_host_functions;
