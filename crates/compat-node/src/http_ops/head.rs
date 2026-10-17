//! Wire shapes of the `node:http` request head and response headers, without JSON.
//!
//! Request head (one JS array per request, layout fixed here and decoded in `js/http/server.js`):
//! `[id, method, uri, version, remoteHost|null, remotePort|null, name0, value0, ...]`.
//! A request refused by the `requests` quota is `[null]`.
//! Response headers travel as one string `name\0value\0name\0value...`.

use velyr_engine_api::{HostError, Value};

use super::HttpRequestParts;

fn text(value: &str) -> Value {
    Value::String(value.into())
}

/// The head of a request that got resource `id`.
pub(super) fn head_items(id: f64, request: &HttpRequestParts) -> Vec<Value> {
    let (host, port) = match request.remote_addr {
        Some(addr) => (
            text(&addr.ip().to_string()),
            Value::Number(f64::from(addr.port())),
        ),
        None => (Value::Null, Value::Null),
    };
    let mut items = vec![
        Value::Number(id),
        text(&request.method),
        text(&request.uri),
        text(&request.version),
        host,
        port,
    ];
    for (name, value) in &request.headers {
        items.push(text(name));
        items.push(text(value));
    }
    items
}

/// Splits `name\0value\0...` back into pairs; an odd field count is a `TypeError`.
pub(super) fn parse_header_blob(blob: &str) -> Result<Vec<(String, String)>, HostError> {
    if blob.is_empty() {
        return Ok(Vec::new());
    }
    let fields: Vec<&str> = blob.split('\0').collect();
    if !fields.len().is_multiple_of(2) {
        return Err(HostError::TypeError(
            "op_node_http_respond expects argument 2 to be name/value pairs".into(),
        ));
    }
    Ok(fields
        .chunks(2)
        .map(|pair| (pair[0].to_owned(), pair[1].to_owned()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use velyr_io_http::BodyReceiver;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(n, v)| ((*n).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn blob_roundtrip_keeps_order_and_duplicates() {
        let blob = "X-Dup\0a\0x-dup\0b\0Empty\0";
        let parsed = parse_header_blob(blob).unwrap();
        assert_eq!(
            parsed,
            pairs(&[("X-Dup", "a"), ("x-dup", "b"), ("Empty", "")])
        );
    }

    #[test]
    fn blob_with_odd_fields_is_rejected() {
        assert!(parse_header_blob("lonely").is_err());
    }

    #[test]
    fn empty_blob_is_no_headers() {
        assert!(parse_header_blob("").unwrap().is_empty());
    }

    #[test]
    fn head_value_has_fixed_layout() {
        let request = HttpRequestParts {
            remote_addr: Some("127.0.0.1:4000".parse().unwrap()),
            method: "PATCH".into(),
            uri: "/a?b=1".into(),
            version: "HTTP/1.1".into(),
            headers: pairs(&[("Host", "h"), ("X-A", "1")]),
            body: BodyReceiver::empty(),
        };
        let items = head_items(7.0, &request);
        assert_eq!(items.len(), 6 + 4);
        assert_eq!(items[0], Value::Number(7.0));
        assert_eq!(items[1], text("PATCH"));
        assert_eq!(items[4], text("127.0.0.1"));
        assert_eq!(items[5], Value::Number(4000.0));
        assert_eq!(items[6], text("Host"));
        assert_eq!(items[9], text("1"));
    }
}
