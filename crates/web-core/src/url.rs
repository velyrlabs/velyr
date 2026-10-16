use url::Url;

use crate::blob::{blob_host_functions, install_blob};
use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, JsString, Source, Value,
};

/// Bootstrap script: defines `URL` and `URLSearchParams` on top of the private `op_url_*` binding.
pub const URL_JS: &str = include_str!("url.js");

/// The `URL` ops plus the `Blob` ops: `Blob` is installed with `URL` (it adds
/// `URL.createObjectURL`), so profiles that register one register both.
pub fn url_host_functions() -> Vec<HostFunctionDef> {
    let mut functions = vec![
        HostFunctionDef {
            name: "op_url_parse",
            arity: 2,
            callback: op_url_parse,
        },
        HostFunctionDef {
            name: "op_url_reparse",
            arity: 3,
            callback: op_url_reparse,
        },
    ];
    functions.extend(blob_host_functions());
    functions
}

/// Runs [`URL_JS`] and then [`crate::BLOB_JS`]. Call from the `install` hook of `run_file`,
/// after the `url_host_functions` are registered.
pub fn install_url(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate.eval_script(&Source {
        name: "velyr:url".into(),
        text: URL_JS.into(),
    })?;
    install_blob(isolate)
}

/// `op_url_parse(input, base?) -> components | null`
fn op_url_parse(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(input), base] = args else {
        return Err(HostError::TypeError(
            "op_url_parse expects (input: string, base?: string)".into(),
        ));
    };
    let base = match base {
        Value::String(base) => match Url::parse(&base.to_string_lossy()) {
            Ok(base) => Some(base),
            Err(_) => return Ok(Value::Null),
        },
        _ => None,
    };
    let parsed = Url::options()
        .base_url(base.as_ref())
        .parse(&input.to_string_lossy());
    Ok(parsed.map_or(Value::Null, |url| components(ctx, &url)))
}

/// `op_url_reparse(href, setter, value) -> components | null`
///
/// Applies a URL setter (`protocol`, `host`, ..., `href`) to a parsed URL. A value the spec
/// ignores leaves the components unchanged; only an invalid `href` value returns null.
fn op_url_reparse(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [
        Value::String(href),
        Value::String(setter),
        Value::String(value),
    ] = args
    else {
        return Err(HostError::TypeError(
            "op_url_reparse expects (href: string, setter: string, value: string)".into(),
        ));
    };
    let mut url = Url::parse(&href.to_string_lossy())
        .map_err(|_| HostError::TypeError("op_url_reparse got an invalid href".into()))?;
    let (setter, value) = (setter.to_string_lossy(), value.to_string_lossy());
    if setter == "href" {
        return Ok(Url::parse(&value).map_or(Value::Null, |url| components(ctx, &url)));
    }
    apply_setter(&mut url, &setter, &value)?;
    Ok(components(ctx, &url))
}

/// Setter semantics from the URL Standard API section. Rejected values are silently ignored.
fn apply_setter(url: &mut Url, setter: &str, value: &str) -> Result<(), HostError> {
    match setter {
        "protocol" => {
            let scheme = value.split(':').next().unwrap_or_default();
            let _ = url.set_scheme(scheme);
        }
        "username" => {
            let _ = url.set_username(value);
        }
        "password" => {
            let _ = url.set_password(Some(value).filter(|v| !v.is_empty()));
        }
        "host" => set_host(url, value, true),
        "hostname" => set_host(url, value, false),
        "port" => set_port(url, value),
        "pathname" => url.set_path(value),
        "search" => {
            url.set_query(Some(value.strip_prefix('?').unwrap_or(value)).filter(|q| !q.is_empty()))
        }
        "hash" => url
            .set_fragment(Some(value.strip_prefix('#').unwrap_or(value)).filter(|f| !f.is_empty())),
        other => {
            return Err(HostError::TypeError(format!("unknown URL setter: {other}")));
        }
    }
    Ok(())
}

fn set_host(url: &mut Url, value: &str, accepts_port: bool) {
    // A port separator is the last ':' outside an IPv6 literal's brackets.
    let split = value
        .rfind(':')
        .filter(|&i| !value[i..].contains(']') && !value.is_empty());
    let (host, port) = match split {
        Some(i) if accepts_port => (&value[..i], Some(&value[i + 1..])),
        Some(_) => return,
        None => (value, None),
    };
    if url.set_host(Some(host)).is_err() {
        return;
    }
    if let Some(port) = port {
        set_port(url, port);
    }
}

fn set_port(url: &mut Url, value: &str) {
    if value.is_empty() {
        let _ = url.set_port(None);
        return;
    }
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    if let Ok(port) = digits.parse::<u16>() {
        let _ = url.set_port(Some(port));
    }
}

/// `[href, origin, protocol, username, password, host, hostname, port, pathname, search, hash]`,
/// in the string forms the `URL` getters return.
fn components(ctx: &mut dyn CallContext, url: &Url) -> Value {
    let hostname = url.host_str().unwrap_or_default();
    let port = url.port().map(|p| p.to_string()).unwrap_or_default();
    let host = if port.is_empty() {
        hostname.to_owned()
    } else {
        format!("{hostname}:{port}")
    };
    let prefixed = |prefix: &str, part: Option<&str>| match part {
        Some(part) if !part.is_empty() => format!("{prefix}{part}"),
        _ => String::new(),
    };
    let parts = [
        url.as_str().to_owned(),
        url.origin().ascii_serialization(),
        format!("{}:", url.scheme()),
        url.username().to_owned(),
        url.password().unwrap_or_default().to_owned(),
        host,
        hostname.to_owned(),
        port,
        url.path().to_owned(),
        prefixed("?", url.query()),
        prefixed("#", url.fragment()),
    ];
    let items: Vec<Value> = parts
        .iter()
        .map(|part| ctx.create_string(JsString::from_str(part)))
        .collect();
    ctx.create_array(&items)
}
