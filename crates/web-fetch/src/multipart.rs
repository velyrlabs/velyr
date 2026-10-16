//! `op_multipart_parse`: splits a `multipart/form-data` body into its parts (RFC 7578). Building
//! one is plain concatenation and lives in JS next to `FormData`.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, JsString, Value};
use velyr_runtime_ops::CapabilityKind;

use crate::ops::{string_arg, type_error};

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 1] = [op!(op_multipart_parse, 2, None)];

struct Part<'a> {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    body: &'a [u8],
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

/// Value of `param` in a header like `form-data; name="a"; filename="b"`; quoted or bare.
fn parameter(header: &str, param: &str) -> Option<String> {
    for piece in header.split(';').skip(1) {
        let Some((key, value)) = piece.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(param) {
            let value = value.trim();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|rest| rest.split('"').next())
                .unwrap_or(value);
            return Some(unquoted.to_owned());
        }
    }
    None
}

fn parse_head(head: &[u8]) -> Option<(String, Option<String>, Option<String>)> {
    let (mut name, mut filename, mut content_type) = (None, None, None);
    for line in String::from_utf8_lossy(head).split("\r\n") {
        let (key, value) = line.split_once(':')?;
        match key.trim().to_ascii_lowercase().as_str() {
            "content-disposition" => {
                if !value
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("form-data")
                {
                    return None;
                }
                name = parameter(value, "name");
                filename = parameter(value, "filename");
            }
            "content-type" => content_type = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    Some((name?, filename, content_type))
}

/// `None` for anything that is not a well-formed multipart body for `boundary`.
fn parse<'a>(body: &'a [u8], boundary: &str) -> Option<Vec<Part<'a>>> {
    let delimiter = format!("--{boundary}").into_bytes();
    let separator = [b"\r\n".as_slice(), &delimiter].concat();
    // The first delimiter may sit at the very start or after a preamble line.
    let mut at = if body.starts_with(&delimiter) {
        delimiter.len()
    } else {
        find(body, &separator, 0)? + separator.len()
    };
    let mut parts = Vec::new();
    loop {
        if body.get(at..at + 2)? == b"--" {
            return Some(parts);
        }
        // Optional transport padding, then the line break after the delimiter.
        at = find(body, b"\r\n", at)? + 2;
        let head_end = find(body, b"\r\n\r\n", at)?;
        let (name, filename, content_type) = parse_head(&body[at..head_end])?;
        let start = head_end + 4;
        let end = find(body, &separator, start)?;
        parts.push(Part {
            name,
            filename,
            content_type,
            body: &body[start..end],
        });
        at = end + separator.len();
    }
}

/// `op_multipart_parse(bytes, boundary) -> [[name, filename | null, contentType | null, bytes]]`.
/// A malformed body throws a `TypeError`.
fn op_multipart_parse(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let boundary = string_arg(args, 1, "op_multipart_parse")?;
    let bytes = args
        .first()
        .and_then(|value| ctx.read_bytes(value))
        .ok_or_else(|| type_error("op_multipart_parse", 0, "an ArrayBuffer or view"))?;
    let parts = parse(&bytes, &boundary)
        .ok_or_else(|| HostError::TypeError("Failed to parse the body as FormData".into()))?;
    let mut entries = Vec::with_capacity(parts.len());
    for part in parts {
        let mut text = |value: Option<String>| match value {
            Some(value) => ctx.create_string(JsString::from(value.as_str())),
            None => Value::Null,
        };
        let fields = [
            text(Some(part.name)),
            text(part.filename),
            text(part.content_type),
            ctx.create_array_buffer(part.body),
        ];
        entries.push(ctx.create_array(&fields));
    }
    Ok(ctx.create_array(&entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_fields_and_files() {
        let body = b"pre\r\n--B\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--B\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x.bin\"\r\nContent-Type: a/b\r\n\r\na\r\n--b\r\n--B--\r\n";
        let parts = parse(body, "B").expect("well formed");
        assert_eq!(parts.len(), 2);
        assert_eq!(
            (parts[0].name.as_str(), parts[0].body),
            ("a", b"1".as_slice())
        );
        assert_eq!(parts[1].filename.as_deref(), Some("x.bin"));
        assert_eq!(parts[1].content_type.as_deref(), Some("a/b"));
        assert_eq!(parts[1].body, b"a\r\n--b");
    }

    #[test]
    fn rejects_malformed_bodies() {
        assert!(parse(b"--B\r\nbroken", "B").is_none());
        assert!(parse(b"nothing", "B").is_none());
        assert!(parse(b"--B\r\nContent-Type: a/b\r\n\r\nx\r\n--B--", "B").is_none());
    }

    #[test]
    fn accepts_an_empty_form() {
        assert!(parse(b"--B--\r\n", "B").expect("empty form").is_empty());
    }
}
