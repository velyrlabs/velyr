//! Connections accepted elsewhere and handed over as `std::net::TcpStream` are served like any
//! other (the supervisor's acceptor thread on macOS and Windows).

use std::net::TcpListener as StdListener;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use velyr_io_http::{BodyReceiver, Handler, HttpResponseParts, Limits, serve_streams_graceful};

#[tokio::test]
async fn handed_over_streams_are_served_and_the_loop_ends_with_the_channel() {
    let handler: Handler = Arc::new(|request| {
        Box::pin(async move {
            Ok(HttpResponseParts {
                status: 200,
                headers: vec![],
                body: BodyReceiver::once(request.uri.into_bytes()),
                upgrade: None,
            })
        })
    });
    let acceptor = StdListener::bind("127.0.0.1:0").unwrap();
    let port = acceptor.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel(8);
    let handoff = std::thread::spawn(move || {
        let (stream, _) = acceptor.accept().unwrap();
        tx.blocking_send(stream).unwrap();
        // `tx` drops here: the channel closes.
    });
    let served = tokio::spawn(serve_streams_graceful(
        rx,
        None,
        Limits::default(),
        handler,
        CancellationToken::new(),
        Duration::from_secs(5),
    ));
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client
        .write_all(b"GET /hello HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("/hello"), "{response}");
    handoff.join().unwrap();
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("the serve loop ends once the channel closes")
        .unwrap();
}
