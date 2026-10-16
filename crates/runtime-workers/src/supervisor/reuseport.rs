//! Distribution by `SO_REUSEPORT` (Linux and FreeBSD): every worker binds its own listener on the
//! same address and the kernel spreads connections among them. The supervisor only keeps a bound,
//! non-listening socket so that a requested port 0 resolves to one port all workers share.

use std::io;
use std::net::SocketAddr;

use socket2::{Domain, Socket, Type};

/// Binds `addr` with `SO_REUSEPORT`; the socket must stay alive as long as the workers run.
pub(super) fn hold(addr: SocketAddr) -> io::Result<(Socket, SocketAddr)> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.bind(&addr.into())?;
    let bound = socket
        .local_addr()?
        .as_socket()
        .ok_or_else(|| io::Error::other("bound address is not an IP address"))?;
    Ok((socket, bound))
}
