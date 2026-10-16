//! The server reports each answered request to the installed `HttpMetrics` sink.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use velyr_io_http::{
    BodyReceiver, Handler, HttpMetrics, HttpResponseParts, Limits, serve_listener, set_http_metrics,
};

#[derive(Default)]
struct Recorder(Mutex<Vec<(u16, Duration)>>);

impl HttpMetrics for Recorder {
    fn request_finished(&self, status: u16, elapsed: Duration) {
        self.0.lock().unwrap().push((status, elapsed));
    }
}

#[tokio::test]
async fn answered_requests_reach_the_sink_with_status_and_latency() {
    let recorder = Arc::new(Recorder::default());
    assert!(set_http_metrics(recorder.clone()));
    let handler: Handler = Arc::new(|request| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let status = if request.uri == "/missing" { 404 } else { 200 };
            Ok(HttpResponseParts {
                status,
                headers: vec![],
                body: BodyReceiver::empty(),
                upgrade: None,
            })
        })
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    tokio::spawn(serve_listener(
        listener,
        None,
        Limits::default(),
        handler,
        shutdown.clone(),
    ));
    for path in ["/", "/missing"] {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
    }
    shutdown.cancel();
    let seen = recorder.0.lock().unwrap().clone();
    let statuses: Vec<u16> = seen.iter().map(|(status, _)| *status).collect();
    assert_eq!(statuses, [200, 404]);
    assert!(
        seen.iter()
            .all(|(_, elapsed)| *elapsed >= Duration::from_millis(40))
    );
}
