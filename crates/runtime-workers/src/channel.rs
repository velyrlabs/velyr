//! Message plumbing shared by workers, `MessagePort` and `BroadcastChannel`: packets, the
//! receiving half ([`Inbox`]) and the movable endpoint that owns one side of a channel.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use velyr_engine_api::SerializedValue;
use velyr_runtime_errors::VelyrError;

use crate::report::ErrorReport;

/// Packets a receiver may have waiting; a sender beyond it fails instead of growing the queue.
pub(crate) const QUEUE_LIMIT: usize = 10_000;

/// A structured-cloned message plus the endpoints transferred with it.
pub(crate) struct Message {
    pub value: SerializedValue,
    pub ports: Vec<Endpoint>,
}

pub(crate) enum Packet {
    Message(Message),
    /// An error the worker reported (`reportError`, uncaught exception); parent side only.
    Error(ErrorReport),
    /// The worker's isolate is gone, with its exit code; always the last packet, parent side only.
    Ended(i32),
}

impl Packet {
    pub(crate) fn plain(value: SerializedValue) -> Self {
        Self::Message(Message {
            value,
            ports: Vec::new(),
        })
    }
}

/// Result of [`Inbox::fill`], also the number `op_port_recv` resolves with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fill {
    Closed = 0,
    Ready = 1,
    Interrupted = 2,
}

/// Sending half of a channel; counts what the receiver has not consumed yet.
#[derive(Clone)]
pub(crate) struct Tx {
    tx: UnboundedSender<Packet>,
    pending: Arc<AtomicUsize>,
}

impl Tx {
    /// A closed peer drops the packet silently; a full queue is an error.
    pub(crate) fn send(&self, packet: Packet) -> Result<(), VelyrError> {
        let queued = self.pending.fetch_add(1, Ordering::AcqRel);
        if queued >= QUEUE_LIMIT {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return Err(VelyrError::quota_exceeded("message_queue"));
        }
        if self.tx.send(packet).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
        Ok(())
    }
}

/// A sender and the [`Inbox`] it feeds.
pub(crate) fn channel() -> (Tx, Inbox) {
    let (tx, rx) = unbounded_channel();
    let pending = Arc::new(AtomicUsize::new(0));
    let inbox = Inbox {
        rx: Arc::new(tokio::sync::Mutex::new(rx)),
        ready: Arc::default(),
        gate: Arc::new(Mutex::new(CancellationToken::new())),
        pending: pending.clone(),
    };
    (Tx { tx, pending }, inbox)
}

/// Receiving half. Cloneable so a pending `fill` future (which must be `'static`) and the
/// isolate-side table share it; it travels with its endpoint when the endpoint is transferred.
#[derive(Clone)]
pub(crate) struct Inbox {
    rx: Arc<tokio::sync::Mutex<UnboundedReceiver<Packet>>>,
    /// Packets received but not yet taken by the isolate (`CompletionValue` cannot carry them).
    ready: Arc<Mutex<VecDeque<Packet>>>,
    /// Cancelled by [`Inbox::interrupt`] to stop the pending `fill`.
    gate: Arc<Mutex<CancellationToken>>,
    /// Shared with the [`Tx`]: sent but not yet popped.
    pending: Arc<AtomicUsize>,
}

impl Inbox {
    /// Waits until a packet is ready to [`pop`](Self::pop), the channel closes or the wait is
    /// interrupted.
    pub(crate) async fn fill(&self) -> Fill {
        if !self.ready.lock().expect("inbox poisoned").is_empty() {
            return Fill::Ready;
        }
        let gate = self.gate.lock().expect("inbox poisoned").clone();
        let received = tokio::select! {
            () = gate.cancelled() => return Fill::Interrupted,
            received = async { self.rx.lock().await.recv().await } => received,
        };
        let Some(packet) = received else {
            return Fill::Closed;
        };
        self.ready.lock().expect("inbox poisoned").push_back(packet);
        Fill::Ready
    }

    pub(crate) fn pop(&self) -> Option<Packet> {
        let packet = self.ready.lock().expect("inbox poisoned").pop_front();
        if packet.is_some() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
        packet
    }

    /// Stops the pending `fill` (it resolves [`Fill::Interrupted`]); later fills are unaffected.
    pub(crate) fn interrupt(&self) {
        let fresh = CancellationToken::new();
        std::mem::replace(&mut *self.gate.lock().expect("inbox poisoned"), fresh).cancel();
    }
}

type Subscribers = HashMap<(u64, String), Vec<(u64, Tx)>>;

/// `BroadcastChannel` subscribers by (isolate group, name): a main isolate and the workers it
/// created, transitively, share a group; other groups in the process never see its messages.
static BROADCAST: LazyLock<Mutex<Subscribers>> = LazyLock::new(Mutex::default);
static NEXT_SUBSCRIBER: AtomicU64 = AtomicU64::new(1);
static NEXT_GROUP: AtomicU64 = AtomicU64::new(1);

/// A fresh isolate group, for an isolate that is not a worker.
pub(crate) fn new_group() -> u64 {
    NEXT_GROUP.fetch_add(1, Ordering::Relaxed)
}

pub(crate) struct Subscription {
    key: (u64, String),
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut registry = BROADCAST.lock().expect("broadcast registry poisoned");
        if let Some(list) = registry.get_mut(&self.key) {
            list.retain(|(id, _)| *id != self.id);
            if list.is_empty() {
                registry.remove(&self.key);
            }
        }
    }
}

enum Peer {
    Direct {
        tx: Tx,
        /// A worker's channel refuses messages once the worker is shutting down.
        shutdown: Option<CancellationToken>,
    },
    Broadcast(Subscription),
}

/// One side of a channel. Dropping it closes the channel for the other side.
pub(crate) struct Endpoint {
    peer: Peer,
    pub(crate) inbox: Inbox,
}

impl Endpoint {
    /// Two entangled endpoints: what one sends, the other receives.
    pub(crate) fn pair() -> (Self, Self) {
        let (tx_a, inbox_a) = channel();
        let (tx_b, inbox_b) = channel();
        (
            Self::direct(tx_b, inbox_a, None),
            Self::direct(tx_a, inbox_b, None),
        )
    }

    pub(crate) fn direct(tx: Tx, inbox: Inbox, shutdown: Option<CancellationToken>) -> Self {
        Self {
            peer: Peer::Direct { tx, shutdown },
            inbox,
        }
    }

    /// Joins the broadcast channel `name` of isolate group `group`.
    pub(crate) fn broadcast(group: u64, name: &str) -> Self {
        let (tx, inbox) = channel();
        let id = NEXT_SUBSCRIBER.fetch_add(1, Ordering::Relaxed);
        let key = (group, name.to_owned());
        BROADCAST
            .lock()
            .expect("broadcast registry poisoned")
            .entry(key.clone())
            .or_default()
            .push((id, tx));
        Self {
            peer: Peer::Broadcast(Subscription { key, id }),
            inbox,
        }
    }

    pub(crate) fn is_transferable(&self) -> bool {
        matches!(self.peer, Peer::Direct { .. })
    }

    /// Delivers `packet` to the other side(s). A closed or shutting-down peer drops it; a peer
    /// whose queue is full makes this fail (for broadcast: after the others got theirs).
    pub(crate) fn send(&self, packet: Packet) -> Result<(), VelyrError> {
        match &self.peer {
            Peer::Direct { tx, shutdown } => {
                if shutdown
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    return Ok(());
                }
                tx.send(packet)
            }
            Peer::Broadcast(me) => {
                let Packet::Message(message) = packet else {
                    return Ok(());
                };
                let registry = BROADCAST.lock().expect("broadcast registry poisoned");
                let mut result = Ok(());
                for (id, tx) in registry.get(&me.key).into_iter().flatten() {
                    if *id == me.id {
                        continue;
                    }
                    // `SerializedValue` is not `Clone`; broadcast carries no transfer list.
                    let copy = SerializedValue {
                        bytes: message.value.bytes.clone(),
                        transferred: Vec::new(),
                    };
                    if let Err(err) = tx.send(Packet::plain(copy)) {
                        result = Err(err);
                    }
                }
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn message(byte: u8) -> Packet {
        Packet::plain(SerializedValue {
            bytes: vec![byte],
            transferred: Vec::new(),
        })
    }

    fn received(endpoint: &Endpoint) -> Option<u8> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let fill = rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), endpoint.inbox.fill()).await
        });
        match (fill, endpoint.inbox.pop()) {
            (Ok(Fill::Ready), Some(Packet::Message(m))) => Some(m.value.bytes[0]),
            _ => None,
        }
    }

    #[test]
    fn broadcast_stays_inside_its_isolate_group() {
        let (group_a, group_b) = (new_group(), new_group());
        let sender = Endpoint::broadcast(group_a, "news");
        let same_group = Endpoint::broadcast(group_a, "news");
        let other_group = Endpoint::broadcast(group_b, "news");
        let other_name = Endpoint::broadcast(group_a, "sports");
        sender.send(message(7)).unwrap();
        assert_eq!(received(&same_group), Some(7));
        assert_eq!(received(&other_group), None);
        assert_eq!(received(&other_name), None);
        assert_eq!(received(&sender), None);
    }

    #[test]
    fn a_full_queue_refuses_more_and_frees_up_as_it_is_read() {
        let (first, second) = Endpoint::pair();
        for n in 0..QUEUE_LIMIT {
            first.send(message((n % 251) as u8)).unwrap();
        }
        let err = first.send(message(0)).unwrap_err();
        assert_eq!(err.code(), velyr_runtime_errors::ErrorCode::QuotaExceeded);
        assert_eq!(received(&second), Some(0));
        first.send(message(1)).unwrap();
    }

    #[test]
    fn full_broadcast_subscriber_does_not_starve_the_others() {
        let group = new_group();
        let sender = Endpoint::broadcast(group, "c");
        let slow = Endpoint::broadcast(group, "c");
        let fast = Endpoint::broadcast(group, "c");
        let mut failures = 0;
        for _ in 0..=QUEUE_LIMIT {
            if sender.send(message(1)).is_err() {
                failures += 1;
            }
            let _ = received(&fast);
        }
        assert_eq!(failures, 1);
        assert_eq!(received(&slow), Some(1));
    }
}
