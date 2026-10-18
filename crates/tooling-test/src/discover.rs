//! Which files are tests: `**/*.{test,spec}.{ts,tsx,js,mjs}` and code files under `__tests__`.

use std::io;
use std::path::{Path, PathBuf};

const EXTENSIONS: [&str; 4] = ["ts", "tsx", "js", "mjs"];

/// True when `path` matches the test file patterns.
pub fn is_test_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if !EXTENSIONS.contains(&extension) {
        return false;
    }
    let named = stem.ends_with(".test") || stem.ends_with(".spec");
    let in_tests = path
        .components()
        .any(|part| part.as_os_str() == "__tests__");
    named || (in_tests && !name.ends_with(".d.ts"))
}

/// The test files under `roots`: a file root is taken as it is, a directory is searched
/// (`node_modules` and dot-directories are skipped). Sorted, without duplicates.
pub fn discover(roots: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for root in roots {
        if root.is_dir() {
            walk(root, &mut found)?;
        } else if root.is_file() {
            found.push(root.clone());
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}: no such file or directory", root.display()),
            ));
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if name != "node_modules" && !name.starts_with('.') {
                walk(&path, found)?;
            }
        } else if is_test_file(&path) {
            found.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_patterns() {
        for yes in [
            "a.test.ts",
            "a.spec.mjs",
            "dir/a.test.tsx",
            "a.b.test.js",
            "src/__tests__/helper.js",
        ] {
            assert!(is_test_file(Path::new(yes)), "{yes}");
        }
        for no in [
            "a.ts",
            "test.js",
            "a.test.css",
            "a.test",
            "atest.js",
            "src/__tests__/types.d.ts",
            "src/__tests__/readme.md",
        ] {
            assert!(!is_test_file(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn discovery_walks_directories_and_skips_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str| {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        };
        write("a.test.js");
        write("sub/b.spec.ts");
        write("sub/__tests__/c.js");
        write("sub/plain.js");
        write("node_modules/dep/d.test.js");
        write(".hidden/e.test.js");
        let found = discover(&[dir.path().to_owned()]).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            })
            .collect();
        assert_eq!(names, ["a.test.js", "sub/__tests__/c.js", "sub/b.spec.ts"]);
    }

    #[test]
    fn a_file_root_is_taken_as_given_and_a_missing_root_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.js");
        std::fs::write(&file, "").unwrap();
        assert_eq!(discover(&[file.clone(), file.clone()]).unwrap(), vec![file]);
        assert!(discover(&[dir.path().join("missing")]).is_err());
    }
}
