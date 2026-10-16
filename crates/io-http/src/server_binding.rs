//! Bridge between the hyper tasks and a single consumer thread (the isolate): the [`Handler`]
//! queues each request with a one-shot reply slot and waits for the response head.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use velyr_runtime_errors::VelyrError;

use crate::{Handler, HttpRequestParts, HttpResponseParts};

/// Requests waiting for the consumer; a full queue pushes back on the connections.
const QUEUE_CAPACITY: usize = 64;

/// A request and where its response goes. Dropping the reply makes the client see a 500.
pub struct Incoming {
    pub request: HttpRequestParts,
    pub reply: oneshot::Sender<Result<HttpResponseParts, VelyrError>>,
    /// Cancelled when the client goes away before the response head is sent.
    pub disconnected: CancellationToken,
}

/// A handler that forwards every request to the returned receiver. The receiver ends once the
/// server has dropped the handler, i.e. after the last connection finished.
pub fn channel_handler() -> (Handler, mpsc::Receiver<Incoming>) {
    let (requests, rx) = mpsc::channel(QUEUE_CAPACITY);
    let handler: Handler = Arc::new(move |request| {
        let requests = requests.clone();
        Box::pin(async move {
            // Hyper drops this future when the connection dies while the handler is pending.
            let disconnected = CancellationToken::new();
            let guard = disconnected.clone().drop_guard();
            let (reply, response) = oneshot::channel();
            requests
                .send(Incoming {
                    request,
                    reply,
                    disconnected,
                })
                .await
                .map_err(|_| VelyrError::internal("the request consumer is gone"))?;
            let result = response
                .await
                .map_err(|_| VelyrError::internal("the request was dropped without a response"));
            guard.disarm();
            result?
        })
    });
    (handler, rx)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::{BodyReceiver, Limits, serve_listener_graceful};

    async fn exchange(addr: std::net::SocketAddr) -> String {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(b"GET /x HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
            .await
            .expect("write");
        let mut out = String::new();
        stream.read_to_string(&mut out).await.expect("read");
        out
    }

    #[tokio::test]
    async fn responds_through_the_channel_and_drains_on_shutdown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (handler, mut rx) = channel_handler();
        let shutdown = CancellationToken::new();
        let server = tokio::spawn(serve_listener_graceful(
            listener,
            None,
            Limits::default(),
            handler,
            shutdown.clone(),
            Duration::from_secs(5),
        ));
        let client = tokio::spawn(exchange(addr));
        let incoming = rx.recv().await.expect("a request");
        assert_eq!(incoming.request.uri, "/x");
        let reply = Ok(HttpResponseParts {
            status: 201,
            headers: Vec::new(),
            body: BodyReceiver::once("hi"),
            upgrade: None,
        });
        assert!(incoming.reply.send(reply).is_ok());
        let text = client.await.expect("client");
        assert!(text.starts_with("HTTP/1.1 201"), "{text}");
        assert!(text.ends_with("hi"), "{text}");
        shutdown.cancel();
        server.await.expect("server task");
        assert!(rx.recv().await.is_none(), "receiver ends with the server");
    }

    #[tokio::test]
    async fn dropped_reply_is_a_500() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (handler, mut rx) = channel_handler();
        let shutdown = CancellationToken::new();
        tokio::spawn(serve_listener_graceful(
            listener,
            None,
            Limits::default(),
            handler,
            shutdown.clone(),
            Duration::from_secs(1),
        ));
        let client = tokio::spawn(exchange(addr));
        drop(rx.recv().await.expect("a request"));
        assert!(client.await.expect("client").starts_with("HTTP/1.1 500"));
        shutdown.cancel();
    }
}
