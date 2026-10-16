//! npm compatibility primitives: `package.json` model, npm semver and Node package resolution.

pub mod cache;
pub mod linker;
pub mod lockfile;
mod package_json;
pub mod registry;
pub mod resolve;
pub mod resolver;
pub mod scripts;
mod semver;

pub use package_json::{PackageJson, PeerDependencyMeta};
pub use semver::{SemverError, satisfies};
