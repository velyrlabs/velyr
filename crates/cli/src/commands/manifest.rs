//! Order- and indent-preserving edits of the project's `package.json`.

use std::path::{Path, PathBuf};

use serde_json::ser::{PrettyFormatter, Serializer};
use serde_json::{Map, Value};
use velyr_compat_npm::PackageJson;
use velyr_runtime_errors::VelyrError;

pub(super) const SECTIONS: [&str; 3] = ["dependencies", "devDependencies", "optionalDependencies"];

pub(super) struct Manifest {
    path: PathBuf,
    root: Map<String, Value>,
    indent: String,
}

impl Manifest {
    pub(super) fn load(project: &Path) -> Result<Self, VelyrError> {
        let path = project.join("package.json");
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            VelyrError::not_found(format!("cannot read {}", path.display())).with_cause(e)
        })?;
        let Value::Object(root) = serde_json::from_str(&raw)
            .map_err(|e| VelyrError::invalid_data("malformed package.json").with_cause(e))?
        else {
            return Err(VelyrError::invalid_data("package.json is not an object"));
        };
        Ok(Self {
            path,
            root,
            indent: detect_indent(&raw),
        })
    }

    pub(super) fn typed(&self) -> Result<PackageJson, VelyrError> {
        serde_json::from_value(Value::Object(self.root.clone()))
            .map_err(|e| VelyrError::invalid_data("invalid package.json").with_cause(e))
    }

    /// Sets `name` in `section` (sorted like npm) and drops it from the other dependency sections
    /// that npm treats as exclusive.
    pub(super) fn set(&mut self, section: &str, name: &str, spec: &str) {
        let other = if section == "dependencies" {
            "devDependencies"
        } else {
            "dependencies"
        };
        self.take(other, name);
        let entry = self
            .root
            .entry(section)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(map) = entry {
            map.insert(name.to_owned(), Value::String(spec.to_owned()));
            map.sort_keys();
        }
    }

    /// Removes `name` from every dependency section; returns whether it was present.
    pub(super) fn remove(&mut self, name: &str) -> bool {
        SECTIONS
            .iter()
            .fold(false, |found, section| self.take(section, name) | found)
    }

    fn take(&mut self, section: &str, name: &str) -> bool {
        let Some(Value::Object(map)) = self.root.get_mut(section) else {
            return false;
        };
        let found = map.shift_remove(name).is_some();
        if map.is_empty() {
            self.root.shift_remove(section);
        }
        found
    }

    pub(super) fn save(&self) -> Result<(), VelyrError> {
        let mut out = Vec::new();
        let formatter = PrettyFormatter::with_indent(self.indent.as_bytes());
        serde::Serialize::serialize(
            &Value::Object(self.root.clone()),
            &mut Serializer::with_formatter(&mut out, formatter),
        )
        .map_err(|e| VelyrError::internal("cannot render package.json").with_cause(e))?;
        out.push(b'\n');
        std::fs::write(&self.path, out)
            .map_err(|e| VelyrError::internal("cannot write package.json").with_cause(e))
    }
}

fn detect_indent(raw: &str) -> String {
    raw.lines()
        .skip(1)
        .find(|line| line.starts_with([' ', '\t']) && !line.trim().is_empty())
        .map(|line| line[..line.len() - line.trim_start().len()].to_owned())
        .unwrap_or_else(|| "  ".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indent_is_detected_with_tabs_and_default() {
        assert_eq!(detect_indent("{\n\t\"a\": 1\n}"), "\t");
        assert_eq!(detect_indent("{\n    \"a\": 1\n}"), "    ");
        assert_eq!(detect_indent("{}"), "  ");
    }
}
