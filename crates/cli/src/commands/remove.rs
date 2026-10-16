//! `velyr remove <pkg>...`.

use std::process::ExitCode;

use velyr_runtime_errors::VelyrError;

use super::install::{self, Ctx};
use super::manifest::Manifest;

pub fn remove(names: &[String]) -> ExitCode {
    install::finish(install::block_on(run(names)))
}

async fn run(names: &[String]) -> Result<(), VelyrError> {
    let ctx = Ctx::open()?;
    let mut manifest = Manifest::load(&ctx.project)?;
    for name in names {
        if !manifest.remove(name) {
            return Err(VelyrError::not_found(format!(
                "`{name}` is not a dependency of this project"
            )));
        }
    }
    install::sync(&ctx, &manifest, Some(&manifest), install::Mode::default()).await
}
