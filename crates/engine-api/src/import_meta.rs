//! `import.meta.filename` / `import.meta.dirname` derivation shared by every engine.

/// Splits a `file://` module name into `(filename, dirname)`.
///
/// Returns `None` for other schemes, a non-empty host, a relative path or invalid UTF-8.
/// Query and fragment are not part of the path.
pub fn file_url_paths(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let filename = native_path(percent_decode(rest)?);
    let split = filename.rfind(PATH_SEPARATOR)?;
    let dirname = match split {
        0 => PATH_SEPARATOR.to_string(),
        _ => filename[..split].to_owned(),
    };
    Some((filename, dirname))
}

#[cfg(windows)]
const PATH_SEPARATOR: char = '\\';
#[cfg(not(windows))]
const PATH_SEPARATOR: char = '/';

/// `/C:/p/m.mjs` becomes `C:\p\m.mjs` on Windows; POSIX paths are unchanged.
#[cfg(windows)]
fn native_path(decoded: String) -> String {
    decoded.trim_start_matches('/').replace('/', "\\")
}

#[cfg(not(windows))]
fn native_path(decoded: String) -> String {
    decoded
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::file_url_paths;

    #[test]
    #[cfg(not(windows))]
    fn decodes_percent_escapes() {
        assert_eq!(
            file_url_paths("file:///tmp/a%20b/main.ts"),
            Some(("/tmp/a b/main.ts".into(), "/tmp/a b".into()))
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn root_file_has_root_dirname() {
        assert_eq!(
            file_url_paths("file:///main.ts"),
            Some(("/main.ts".into(), "/".into()))
        );
    }

    #[test]
    fn rejects_other_schemes_hosts_and_bad_utf8() {
        assert_eq!(file_url_paths("https://x/y"), None);
        assert_eq!(file_url_paths("file://host/a/b"), None);
        assert_eq!(file_url_paths("file:///a/%ff"), None);
        assert_eq!(file_url_paths("file:///a/%zz"), None);
    }

    #[test]
    #[cfg(windows)]
    fn windows_drive_path() {
        assert_eq!(
            file_url_paths("file:///C:/p/m.mjs"),
            Some((r"C:\p\m.mjs".into(), r"C:\p".into()))
        );
    }
}
