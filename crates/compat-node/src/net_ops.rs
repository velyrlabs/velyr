//! Host op behind `node:dns`: `op_node_dns_lookup(hostname) -> Promise<string>` resolves through
//! the resolver io-net installed (the system resolver unless the host injected one) and answers one
//! address per line, in resolver order. Lookup needs no permission and grants none: connecting to
//! the answer is still authorized by `NetConnect` in io-net.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_net::{resolver_of, system_resolver};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

pub(crate) fn net_node_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_node_dns_lookup",
        arity: 1,
        callback: op_dns_lookup,
    }]
}

fn op_dns_lookup(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(host)) = args.first() else {
        return Err(HostError::TypeError(
            "op_node_dns_lookup expects argument 0 to be a string".into(),
        ));
    };
    let host = host.to_string_lossy();
    // Hosts that skip install_net still get the system resolver.
    let resolver = resolver_of(ctx).unwrap_or_else(|_| system_resolver());
    op_async(ctx, |token| async move {
        let found = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            found = resolver(host, 0) => found,
        };
        let addrs = found.map_err(velyr_io_net::net_error)?;
        let lines: Vec<String> = addrs.iter().map(|addr| addr.ip().to_string()).collect();
        Ok(CompletionValue::String(lines.join("\n")))
    })
}
