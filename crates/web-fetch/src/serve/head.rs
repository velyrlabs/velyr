//! The wire shape of `op_serve_take` / `op_serve_respond`: a request head crosses to JS as a plain
//! array (no JSON), response headers cross back as one `name\0value\0...` string.
//!
//! Head layout (mirrored by `decodeHead` in `60_serve.js`):
//! `[id, method, uri, version, remoteHost|null, remotePort|null, spanId|null, name0, value0, ...]`.
//! A request refused by the `requests` quota is `[null]`.

use velyr_engine_api::{CallContext, HostError, Value};

use crate::ops::type_error;

/// One slot of a head, before it becomes a JS value.
#[derive(Debug, PartialEq)]
pub(super) enum Slot<'a> {
    Null,
    Number(f64),
    Text(&'a str),
}

pub(super) struct Head<'a> {
    pub id: f64,
    pub method: &'a str,
    pub uri: &'a str,
    pub version: &'a str,
    pub remote: Option<(String, u16)>,
    pub span_id: Option<u64>,
    pub headers: &'a [(String, String)],
}

impl<'a> Head<'a> {
    pub(super) fn slots(&'a self) -> Vec<Slot<'a>> {
        let (host, port) = match &self.remote {
            Some((host, port)) => (Slot::Text(host), Slot::Number(f64::from(*port))),
            None => (Slot::Null, Slot::Null),
        };
        let mut slots = vec![
            Slot::Number(self.id),
            Slot::Text(self.method),
            Slot::Text(self.uri),
            Slot::Text(self.version),
            host,
            port,
            self.span_id.map_or(Slot::Null, |id| Slot::Number(id as f64)),
        ];
        for (name, value) in self.headers {
            slots.push(Slot::Text(name));
            slots.push(Slot::Text(value));
        }
        slots
    }
}

pub(super) fn to_value(ctx: &mut dyn CallContext, slots: &[Slot<'_>]) -> Value {
    let items: Vec<Value> = slots
        .iter()
        .map(|slot| match slot {
            Slot::Null => Value::Null,
            Slot::Number(n) => Value::Number(*n),
            Slot::Text(text) => ctx.create_string((*text).into()),
        })
        .collect();
    ctx.create_array(&items)
}

/// `name\0value\0...` to pairs; an odd field count is a `TypeError`.
pub(super) fn parse_header_blob(blob: &str) -> Result<Vec<(String, String)>, HostError> {
    if blob.is_empty() {
        return Ok(Vec::new());
    }
    let fields: Vec<&str> = blob.strip_suffix('\0').unwrap_or(blob).split('\0').collect();
    if !fields.len().is_multiple_of(2) {
        return Err(type_error("op_serve_respond", 2, "name/value header pairs"));
    }
    Ok(fields
        .chunks(2)
        .map(|pair| (pair[0].to_owned(), pair[1].to_owned()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_roundtrip_keeps_order_and_duplicates() {
        let pairs = parse_header_blob("b\0 1\0a\0\x32\0b\0\x33\0").unwrap();
        let expected = [("b", " 1"), ("a", "2"), ("b", "3")];
        assert_eq!(pairs.len(), 3);
        for (got, want) in pairs.iter().zip(expected) {
            assert_eq!((got.0.as_str(), got.1.as_str()), want);
        }
    }

    #[test]
    fn blob_with_odd_fields_is_rejected() {
        assert!(parse_header_blob("a\0b\0c\0").is_err());
    }

    #[test]
    fn blob_keeps_an_empty_value() {
        assert_eq!(parse_header_blob("a\0\0").unwrap(), [("a".into(), "".into())]);
    }

    #[test]
    fn empty_blob_is_no_headers() {
        assert!(parse_header_blob("").unwrap().is_empty());
    }

    #[test]
    fn head_value_has_fixed_layout() {
        let headers = [("x-dup".to_owned(), "1".to_owned())];
        let head = Head {
            id: 7.0,
            method: "GET",
            uri: "/a",
            version: "HTTP/1.1",
            remote: Some(("127.0.0.1".into(), 80)),
            span_id: None,
            headers: &headers,
        };
        assert_eq!(
            head.slots(),
            [
                Slot::Number(7.0),
                Slot::Text("GET"),
                Slot::Text("/a"),
                Slot::Text("HTTP/1.1"),
                Slot::Text("127.0.0.1"),
                Slot::Number(80.0),
                Slot::Null,
                Slot::Text("x-dup"),
                Slot::Text("1"),
            ]
        );
    }
}
