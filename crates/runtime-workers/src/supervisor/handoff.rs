//! Distribution by handoff (macOS and Windows): one acceptor thread takes connections from the
//! listener and passes each, round-robin, to the next worker that is ready for it.

use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc::{self, error::TrySendError};

/// Connections buffered per worker before the acceptor tries the next one.
const QUEUE: usize = 256;

/// The channel to one worker; empty while the worker is down.
#[derive(Default)]
pub(super) struct Slot(Mutex<Option<mpsc::Sender<TcpStream>>>);

impl Slot {
    /// Opens a fresh channel for a (re)started worker and returns its receiving end.
    pub(super) fn open(&self) -> mpsc::Receiver<TcpStream> {
        let (tx, rx) = mpsc::channel(QUEUE);
        *self.0.lock().expect("slot") = Some(tx);
        rx
    }

    pub(super) fn close(&self) {
        self.0.lock().expect("slot").take();
    }

    fn offer(&self, stream: TcpStream) -> Result<(), TcpStream> {
        match &*self.0.lock().expect("slot") {
            None => Err(stream),
            Some(tx) => tx.try_send(stream).map_err(|err| match err {
                TrySendError::Full(stream) | TrySendError::Closed(stream) => stream,
            }),
        }
    }
}

/// Accepts until `stopping` is set (then [`wake`] unblocks the pending `accept`). A connection
/// no worker can take right now is dropped.
pub(super) fn accept_loop(
    listener: &TcpListener,
    slots: &[std::sync::Arc<Slot>],
    stopping: &AtomicBool,
) {
    let mut next = 0usize;
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(err) => {
                tracing::warn!(%err, "supervisor accept failed");
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
        };
        if stopping.load(Ordering::Acquire) {
            return;
        }
        let mut stream = Some(stream);
        for step in 0..slots.len() {
            let Some(pending) = stream.take() else { break };
            if let Err(back) = slots[(next + step) % slots.len()].offer(pending) {
                stream = Some(back);
            }
        }
        next = (next + 1) % slots.len();
    }
}

/// Unblocks an `accept` on `addr` by connecting to it.
pub(super) fn wake(addr: SocketAddr) {
    let target = match addr.ip() {
        ip if ip.is_unspecified() => SocketAddr::new(
            if ip.is_ipv4() {
                IpAddr::from([127, 0, 0, 1])
            } else {
                IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1])
            },
            addr.port(),
        ),
        _ => addr,
    };
    let _ = TcpStream::connect(target);
}
