#![cfg(test)]

use super::*;
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn canceling_a_duplex_request_drops_the_open_upload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (seen, started) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        seen.send(()).unwrap();
        while socket.read(&mut bytes).await.unwrap() > 0 {}
    });
    let client = ReqwestHttpClient {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        request_timeout: None,
    };
    let cancel = CancellationToken::new();
    let request_cancel = cancel.clone();
    let (replies, receiver) = tokio::sync::mpsc::channel(1);
    let request = tokio::spawn(async move {
        client
            .post_bytes_duplex(
                &format!("http://{address}"),
                vec![],
                b"initial frame",
                receiver,
                request_cancel,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), started)
        .await
        .unwrap()
        .unwrap();
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .unwrap()
        .unwrap();
    let Err(error) = result else {
        panic!("canceled request returned a response")
    };
    assert!(matches!(
        error.downcast_ref::<HttpClientError>(),
        Some(HttpClientError::Cancelled { .. })
    ));
    tokio::time::timeout(Duration::from_secs(3), replies.closed())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
