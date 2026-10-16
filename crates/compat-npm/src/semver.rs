//! npm semver matching. Dist-tags (`latest`, `next`) are resolved by the caller.

use nodejs_semver::{Range, Version};

#[derive(Debug, thiserror::Error)]
pub enum SemverError {
    #[error("invalid semver version `{input}`: {reason}")]
    InvalidVersion { input: String, reason: String },
    #[error("invalid semver range `{input}`: {reason}")]
    InvalidRange { input: String, reason: String },
}

/// Returns whether `version` satisfies the npm `range` (`^`, `~`, `x`, `||`, ` - `).
pub fn satisfies(version: &str, range: &str) -> Result<bool, SemverError> {
    let parsed_version = Version::parse(version).map_err(|e| SemverError::InvalidVersion {
        input: version.to_owned(),
        reason: e.to_string(),
    })?;
    let parsed_range = Range::parse(range).map_err(|e| SemverError::InvalidRange {
        input: range.to_owned(),
        reason: e.to_string(),
    })?;
    Ok(parsed_range.satisfies(&parsed_version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_range() {
        assert!(satisfies("1.9.0", "^1.2.3").unwrap());
        assert!(!satisfies("2.0.0", "^1.2.3").unwrap());
        assert!(!satisfies("1.2.2", "^1.2.3").unwrap());
    }

    #[test]
    fn tilde_and_hyphen_ranges() {
        assert!(satisfies("1.2.9", "~1.2.3").unwrap());
        assert!(!satisfies("1.3.0", "~1.2.3").unwrap());
        assert!(satisfies("1.5.0", "1.2.3 - 2.0.0").unwrap());
        assert!(!satisfies("2.0.1", "1.2.3 - 2.0.0").unwrap());
    }

    #[test]
    fn compound_range_with_or_and_x() {
        let range = ">=1 <2 || 3.x";
        assert!(satisfies("1.5.0", range).unwrap());
        assert!(satisfies("3.9.9", range).unwrap());
        assert!(!satisfies("2.5.0", range).unwrap());
        assert!(!satisfies("4.0.0", range).unwrap());
    }

    #[test]
    fn prerelease_excluded_unless_range_opts_in() {
        assert!(!satisfies("1.3.0-beta.1", "^1.2.3").unwrap());
        assert!(satisfies("1.2.3-beta.1", "^1.2.3-alpha").unwrap());
    }

    #[test]
    fn invalid_input_is_an_error() {
        assert!(satisfies("not-a-version", "^1.0.0").is_err());
        assert!(satisfies("1.0.0", "latest").is_err());
    }
}
