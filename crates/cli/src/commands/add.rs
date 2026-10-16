//! `velyr add <pkg>[@range]... [-D|--dev] [--exact]`.

use std::process::ExitCode;

use velyr_runtime_errors::VelyrError;

use super::install::{self, Ctx};
use super::manifest::Manifest;

pub fn add(specs: &[String], dev: bool, exact: bool) -> ExitCode {
    install::finish(install::block_on(run(specs, dev, exact)))
}

async fn run(specs: &[String], dev: bool, exact: bool) -> Result<(), VelyrError> {
    let ctx = Ctx::open()?;
    let mut manifest = Manifest::load(&ctx.project)?;
    let section = if dev {
        "devDependencies"
    } else {
        "dependencies"
    };
    for raw in specs {
        let (name, range) = parse_spec(raw)?;
        let spec = resolve_spec(&ctx, name, range, exact).await?;
        manifest.set(section, name, &spec);
    }
    install::sync(&ctx, &manifest, Some(&manifest), install::Mode::default()).await
}

/// `name`, `name@range`, `@scope/name`, `@scope/name@range`.
fn parse_spec(raw: &str) -> Result<(&str, Option<&str>), VelyrError> {
    let split = raw
        .char_indices()
        .skip(1)
        .find(|(_, c)| *c == '@')
        .map(|(i, _)| i);
    let (name, range) = match split {
        Some(i) => (&raw[..i], Some(&raw[i + 1..]).filter(|r| !r.is_empty())),
        None => (raw, None),
    };
    if valid_name(name) {
        Ok((name, range))
    } else {
        Err(VelyrError::invalid_data(format!(
            "invalid package name `{name}`"
        )))
    }
}

fn valid_name(name: &str) -> bool {
    let segment = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['.', '_'])
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c))
    };
    match name.strip_prefix('@') {
        Some(rest) => rest
            .split_once('/')
            .is_some_and(|(a, b)| segment(a) && segment(b)),
        None => segment(name),
    }
}

/// A dist-tag (default `latest`) becomes `^version` (or the bare version with `--exact`); any
/// other range is written exactly as typed.
async fn resolve_spec(
    ctx: &Ctx,
    name: &str,
    range: Option<&str>,
    exact: bool,
) -> Result<String, VelyrError> {
    let packument = ctx.client.packument(name).await?;
    let tag = range.unwrap_or("latest");
    match packument.dist_tags.get(tag) {
        Some(version) if exact => Ok(version.clone()),
        Some(version) => Ok(format!("^{version}")),
        None => range
            .map(str::to_owned)
            .ok_or_else(|| VelyrError::not_found(format!("`{name}` has no `latest` version"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_ranges() {
        assert_eq!(parse_spec("a").unwrap(), ("a", None));
        assert_eq!(parse_spec("a@^1.0.0").unwrap(), ("a", Some("^1.0.0")));
        assert_eq!(parse_spec("@s/a").unwrap(), ("@s/a", None));
        assert_eq!(parse_spec("@s/a@next").unwrap(), ("@s/a", Some("next")));
        assert!(parse_spec("../x").is_err());
        assert!(parse_spec("a/b").is_err());
        assert!(parse_spec("").is_err());
    }
}
