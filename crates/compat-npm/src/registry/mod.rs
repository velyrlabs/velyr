//! npm registry client: abbreviated packuments, `.npmrc` auth and SRI-verified tarballs.

mod client;
pub(crate) mod integrity;
mod npmrc;
mod packument;

pub use client::{PACKUMENT_CONCURRENCY, RegistryClient, RetryPolicy};
pub use npmrc::Npmrc;
pub use packument::{Dist, Packument, VersionManifest};
