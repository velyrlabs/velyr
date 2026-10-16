use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_permissions::Request;

use crate::OpContext;
use crate::context::string_arg;

pub const OP_PERMISSIONS_QUERY: HostFunctionDef = HostFunctionDef {
    name: "op_permissions_query",
    arity: 3,
    callback: op_permissions_query,
};

/// `op_permissions_query(name, target, port)`: `true` when the capability set allows it. It is a
/// probe, not an access: it never prompts and emits no denial event. `target` is `undefined` when
/// the descriptor carried none; `name` for imports is `import-local` or `import-remote`.
fn op_permissions_query(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let _span = tracing::debug_span!("velyr.ops.permissions_query").entered();
    let name = string_arg(args, 0, "op_permissions_query")?;
    let target = match args.get(1) {
        Some(Value::String(text)) => Some(text.to_string_lossy()),
        _ => None,
    };
    let port = match args.get(2) {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    };
    let request = build_request(&name, target.as_deref(), port)?;
    let op_ctx = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    Ok(Value::Bool(op_ctx.permissions.check(&request).is_allow()))
}

/// Env and run queries without a target ask about everything (`*`), like `Velyr.env.toObject`.
fn build_request<'a>(
    name: &str,
    target: Option<&'a str>,
    port: Option<f64>,
) -> Result<Request<'a>, HostError> {
    let need = |what: &str| {
        target.ok_or_else(|| HostError::TypeError(format!("`{name}` query needs `{what}`")))
    };
    Ok(match name {
        "read" => Request::FsRead(need("path")?),
        "write" => Request::FsWrite(need("path")?),
        "native" => Request::NativeLoad(need("path")?),
        "env" => Request::EnvRead(target.unwrap_or("*")),
        "run" => Request::ProcessSpawn(target.unwrap_or("*")),
        "import-local" => Request::ImportLocal(need("path")?),
        "import-remote" => Request::ImportRemote(need("host")?),
        "net" | "listen" => {
            let (host, port) = host_and_port(need("host")?, port)?;
            if name == "net" {
                Request::NetConnect(host, port)
            } else {
                Request::NetListen(host, port)
            }
        }
        other => {
            return Err(HostError::TypeError(format!(
                "unknown permission name `{other}`"
            )));
        }
    })
}

/// Splits `host:port` (`[::1]:80` for IPv6); an explicit `port` argument wins.
fn host_and_port(target: &str, port: Option<f64>) -> Result<(&str, u16), HostError> {
    let invalid = || HostError::TypeError("net query needs a host and a port in 0-65535".into());
    let (host, inline) = match target.rsplit_once(':') {
        Some((host, p)) if !host.contains(':') || host.ends_with(']') => (host, p.parse().ok()),
        _ => (target, None),
    };
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let port = match port {
        Some(p) if p.fract() == 0.0 && (0.0..=65535.0).contains(&p) => Some(p as u16),
        Some(_) => None,
        None => inline,
    };
    port.map(|p| (host, p)).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_host_and_port() {
        assert_eq!(host_and_port("a.com:443", None).unwrap(), ("a.com", 443));
        assert_eq!(host_and_port("[::1]:80", None).unwrap(), ("::1", 80));
        assert_eq!(host_and_port("a.com", Some(8.0)).unwrap(), ("a.com", 8));
        assert!(host_and_port("a.com", None).is_err());
        assert!(host_and_port("::1", None).is_err());
        assert!(host_and_port("a.com:99999", None).is_err());
        assert!(host_and_port("a.com", Some(-1.0)).is_err());
    }
}
