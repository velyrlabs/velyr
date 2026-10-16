//! `compat/wpt/manifest.txt`: the versioned selection of WPT tests, one entry per line.
//! An entry is a test file, or a directory whose `.any.js` files are all selected.

use std::fs;
use std::io;
use std::path::Path;

const ANY_SUFFIX: &str = ".any.js";

pub fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Test files (paths relative to `root`, sorted, unique) selected by `entries`.
pub fn expand(root: &Path, entries: &[String]) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    for entry in entries {
        let path = root.join(entry);
        if path.is_file() {
            files.push(entry.clone());
        } else {
            collect(root, &path, &mut files)?;
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if path.to_string_lossy().ends_with(ANY_SUFFIX) {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            out.push(relative.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_comments_and_blanks() {
        assert_eq!(
            parse("# header\nurl\n\nencoding # trailing\n"),
            ["url", "encoding"]
        );
    }

    #[test]
    fn expand_takes_any_js_from_directories_and_explicit_files() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "url/a.any.js",
            "url/sub/b.any.js",
            "url/c.window.js",
            "url/d.html",
            "x/e.window.js",
        ] {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        let entries = parse("url\nx/e.window.js\nurl/a.any.js\n");
        let files = expand(dir.path(), &entries).unwrap();
        assert_eq!(files, ["url/a.any.js", "url/sub/b.any.js", "x/e.window.js"]);
    }

    #[test]
    fn expand_reports_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(expand(dir.path(), &["nope".into()]).is_err());
    }
}
