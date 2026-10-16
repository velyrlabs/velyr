#[cfg(feature = "npm")]
pub mod add;
pub mod bench;
pub mod check;
pub mod compat;
#[cfg(feature = "npm")]
pub mod install;
#[cfg(feature = "npm")]
mod manifest;
#[cfg(feature = "npm")]
pub mod remove;
#[cfg(feature = "web")]
pub mod serve;
pub mod test;
#[cfg(feature = "node")]
pub mod x;
