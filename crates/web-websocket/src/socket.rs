//! The task that owns one WebSocket connection: it multiplexes the messages JS queued, the frames
//! the peer sends and the close handshake, and reports everything as [`Event`]s.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message, WebSocketConfig};
use tokio_util::sync::CancellationToken;
use velyr_io_net::Duplex;
use velyr_runtime_resources::{QuotaClass, QuotaContext, QuotaGuard};

/// Close codes the API reports for connections that did not close with a code of their own.
const NO_STATUS: u16 = 1005;
const ABNORMAL: u16 = 1006;
const TOO_BIG: u16 = 1009;
/// Events buffered for JS; a full queue stops reading from the peer.
pub(crate) const EVENT_QUEUE: usize = 16;
const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// What JS queued for the peer.
pub(crate) enum Outgoing {
    Text(String),
    Binary(Vec<u8>),
    Close(Option<(u16, String)>),
}

/// What the connection reports to JS. Received messages keep their `body_bytes` reservation
/// until JS took them.
pub(crate) enum Event {
    Open {
        protocol: String,
    },
    Text(String, QuotaGuard),
    Binary(Vec<u8>, QuotaGuard),
    Error(String),
    Close {
        code: u16,
        reason: String,
        clean: bool,
    },
}

/// Frame and message size limits come from the `body_bytes` quota.
pub(crate) fn config(quotas: &QuotaContext) -> WebSocketConfig {
    let defaults = WebSocketConfig::default();
    let limit = usize::try_from(quotas.limit(QuotaClass::BodyBytes)).unwrap_or(usize::MAX);
    defaults
        .max_message_size(Some(
            limit.min(defaults.max_message_size.unwrap_or(usize::MAX)),
        ))
        .max_frame_size(Some(
            limit.min(defaults.max_frame_size.unwrap_or(usize::MAX)),
        ))
}

pub(crate) struct Link {
    pub(crate) outgoing: mpsc::UnboundedReceiver<Outgoing>,
    pub(crate) events: mpsc::Sender<Event>,
    pub(crate) cancel: CancellationToken,
    pub(crate) quotas: Arc<QuotaContext>,
    /// Bytes queued by `send` and not written yet (`bufferedAmount`).
    pub(crate) buffered: Arc<AtomicU64>,
}

enum End {
    /// JS released the connection: nothing more to report.
    Dropped,
    /// The closing handshake completed (or the peer closed first).
    Clean,
    TooBig,
    Abnormal(Option<String>),
}

enum Delivery {
    Sent,
    TooBig,
    Dropped,
}

async fn deliver(link: &Link, size: usize, event: impl FnOnce(QuotaGuard) -> Event) -> Delivery {
    let Ok(guard) = link.quotas.reserve(QuotaClass::BodyBytes, size as u64) else {
        return Delivery::TooBig;
    };
    tokio::select! {
        () = link.cancel.cancelled() => Delivery::Dropped,
        sent = link.events.send(event(guard)) => {
            if sent.is_ok() { Delivery::Sent } else { Delivery::Dropped }
        }
    }
}

fn frame(code: u16, reason: String) -> CloseFrame {
    CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    }
}

/// Runs the connection to its end and reports how it ended.
pub(crate) async fn drive(mut ws: WebSocketStream<Box<dyn Duplex>>, mut link: Link) {
    let mut closing = false;
    // The peer gets this long to answer our close frame.
    let mut deadline = tokio::time::Instant::now() + CLOSE_TIMEOUT;
    let mut peer_close: Option<Option<(u16, String)>> = None;
    let end = loop {
        tokio::select! {
            () = link.cancel.cancelled() => break End::Dropped,
            out = link.outgoing.recv(), if !closing => {
                let Some(out) = out else { break End::Dropped };
                let (message, size) = match out {
                    Outgoing::Text(text) => { let n = text.len(); (Message::text(text), n) }
                    Outgoing::Binary(bytes) => { let n = bytes.len(); (Message::binary(bytes), n) }
                    Outgoing::Close(close) => {
                        closing = true;
                        deadline = tokio::time::Instant::now() + CLOSE_TIMEOUT;
                        (Message::Close(close.map(|(code, reason)| frame(code, reason))), 0)
                    }
                };
                let sent = ws.send(message).await;
                link.buffered.fetch_sub(size as u64, Ordering::Relaxed);
                if let Err(err) = sent {
                    break End::Abnormal(Some(err.to_string()));
                }
            }
            () = tokio::time::sleep_until(deadline), if closing => break End::Abnormal(None),
            incoming = ws.next() => match incoming {
                None | Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => {
                    break if peer_close.is_some() { End::Clean } else { End::Abnormal(None) };
                }
                Some(Ok(Message::Text(text))) => {
                    match deliver(&link, text.len(), |guard| Event::Text(text.as_str().to_owned(), guard)).await {
                        Delivery::Sent => {}
                        Delivery::TooBig => break End::TooBig,
                        Delivery::Dropped => break End::Dropped,
                    }
                }
                Some(Ok(Message::Binary(bytes))) => {
                    match deliver(&link, bytes.len(), |guard| Event::Binary(bytes.to_vec(), guard)).await {
                        Delivery::Sent => {}
                        Delivery::TooBig => break End::TooBig,
                        Delivery::Dropped => break End::Dropped,
                    }
                }
                // The reply to the peer's close is written by the next read, so keep reading.
                Some(Ok(Message::Close(close))) => {
                    peer_close = Some(close.map(|c| (u16::from(c.code), c.reason.as_str().to_owned())));
                }
                Some(Ok(_)) => {}
                Some(Err(WsError::Capacity(_))) => break End::TooBig,
                Some(Err(err)) => break End::Abnormal(Some(err.to_string())),
            },
        }
    };
    let too_big = matches!(end, End::TooBig);
    let (code, reason, clean) = match end {
        End::Dropped => return,
        End::Clean => match peer_close.flatten() {
            Some((code, reason)) => (code, reason, true),
            None => (NO_STATUS, String::new(), true),
        },
        End::TooBig => {
            let _ = ws
                .send(Message::Close(Some(frame(TOO_BIG, String::new()))))
                .await;
            (TOO_BIG, String::new(), false)
        }
        End::Abnormal(error) => {
            if let Some(message) = error {
                let _ = link.events.send(Event::Error(message)).await;
            }
            (ABNORMAL, String::new(), false)
        }
    };
    let _ = link
        .events
        .send(Event::Close {
            code,
            reason,
            clean,
        })
        .await;
    if too_big {
        drain(&mut ws).await;
    }
}

/// Reads what the peer still sends until it hangs up. Closing with unread bytes resets the
/// connection, and on Windows a reset can discard the close frame the peer has not read yet.
async fn drain(ws: &mut WebSocketStream<Box<dyn Duplex>>) {
    let stream = ws.get_mut();
    let _ = stream.shutdown().await;
    let _ = tokio::time::timeout(
        CLOSE_TIMEOUT,
        tokio::io::copy(stream, &mut tokio::io::sink()),
    )
    .await;
}
