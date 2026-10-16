use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use tokio::sync::mpsc;
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_resources::QuotaClass;

use crate::Limits;

/// Chunks buffered between producer and consumer; a full channel stops the producer.
const CHANNEL_CAPACITY: usize = 4;

pub type Chunk = Result<Bytes, VelyrError>;

pub struct BodySender(mpsc::Sender<Chunk>);

impl BodySender {
    /// Waits for room; `false` when the receiver is gone.
    pub async fn send(&self, chunk: Chunk) -> bool {
        self.0.send(chunk).await.is_ok()
    }
}

pub struct BodyReceiver {
    rx: mpsc::Receiver<Chunk>,
    /// Total length when known up front, so hyper can send `Content-Length`.
    len: Option<u64>,
}

impl BodyReceiver {
    /// `None` at the end of the body.
    pub async fn recv(&mut self) -> Option<Chunk> {
        self.rx.recv().await
    }

    /// A body that is already complete.
    pub fn once(bytes: impl Into<Bytes>) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let bytes: Bytes = bytes.into();
        let bytes_len = bytes.len() as u64;
        if !bytes.is_empty() {
            tx.try_send(Ok(bytes)).expect("fresh channel has room");
        }
        Self {
            rx,
            len: Some(bytes_len),
        }
    }

    pub fn empty() -> Self {
        Self::once(Bytes::new())
    }

    /// Reads the whole body.
    pub async fn collect(mut self) -> Result<Vec<u8>, VelyrError> {
        let mut all = Vec::new();
        while let Some(chunk) = self.recv().await {
            all.extend_from_slice(&chunk?);
        }
        Ok(all)
    }
}

pub fn body_channel() -> (BodySender, BodyReceiver) {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    (BodySender(tx), BodyReceiver { rx, len: None })
}

/// Lets a [`BodyReceiver`] be the body of an outgoing hyper message.
impl Body for BodyReceiver {
    type Data = Bytes;
    type Error = VelyrError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, VelyrError>>> {
        self.rx
            .poll_recv(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map(Frame::data)))
    }

    fn size_hint(&self) -> SizeHint {
        self.len.map_or_else(SizeHint::new, SizeHint::with_exact)
    }
}

/// Copies `incoming` into `tx`, enforcing `limits`: a body past `max_body_bytes`, or whose bytes
/// the `body_bytes` quota refuses, ends with `QuotaExceeded`. Waiting on a full channel is what
/// stops reading from the peer (backpressure). Quota reservations last until the body ends.
pub(crate) async fn pump<B>(mut incoming: B, tx: BodySender, limits: Limits)
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut total = 0u64;
    let mut reservations = Vec::new();
    while let Some(frame) = incoming.frame().await {
        let data = match frame {
            Ok(frame) => match frame.into_data() {
                Ok(data) => data,
                Err(_) => continue,
            },
            Err(err) => {
                tx.send(Err(VelyrError::new(ErrorCode::Network, err.to_string())))
                    .await;
                return;
            }
        };
        total += data.len() as u64;
        let admitted = if total > limits.max_body_bytes {
            Err(VelyrError::quota_exceeded("body_bytes"))
        } else if let Some(quotas) = &limits.quotas {
            quotas
                .reserve(QuotaClass::BodyBytes, data.len() as u64)
                .map(|guard| reservations.push(guard))
        } else {
            Ok(())
        };
        let chunk = admitted.map(|()| data);
        let failed = chunk.is_err();
        if !tx.send(chunk).await || failed {
            return;
        }
    }
}
