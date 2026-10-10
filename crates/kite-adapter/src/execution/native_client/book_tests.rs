//! Reconciliation reads (kite-adapter 0.7.0): the book is `/orders` + `/trades` only, the
//! audit is `/portfolio/positions` only; margins are never read on either path.
use super::*;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// A minimal HTTP/1.1 Kite stand-in that records each requested path.
async fn fake_kite() -> (&'static str, Arc<StdMutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root: &'static str =
        Box::leak(format!("http://{}", listener.local_addr().unwrap()).into_boxed_str());
    let seen = Arc::new(StdMutex::new(Vec::<String>::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let log = log.clone();
            tokio::spawn(async move {
                loop {
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        match socket.read_u8().await {
                            Ok(b) => head.push(b),
                            Err(_) => return,
                        }
                    }
                    let line = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_owned();
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                    log.lock().unwrap().push(path.clone());
                    let data = match path.as_str() {
                        "/orders" | "/trades" => "[]".to_owned(),
                        "/portfolio/positions" => r#"{"net":[],"day":[]}"#.to_owned(),
                        _ => r#"{"unexpected":true}"#.to_owned(),
                    };
                    let body = format!(r#"{{"status":"success","data":{data}}}"#);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if socket.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (root, seen)
}

fn broker(root: &'static str) -> KiteBroker {
    let credentials = KiteCredentials::new(Some("test-key".into()), Some("test-token".into())).unwrap();
    let mut b = KiteBroker::new(&credentials, "AB1234".into(), "MIS".into()).unwrap();
    b.read = ReadClient::with_client(&credentials, reqwest::Client::new())
        .unwrap()
        .with_test_root(root);
    b
}

#[tokio::test]
async fn book_reads_orders_and_trades_only() {
    let (root, seen) = fake_kite().await;
    let (orders, trades) = broker(root).book().await.unwrap();
    assert!(orders.is_empty() && trades.is_empty());
    let mut paths = seen.lock().unwrap().clone();
    paths.sort();
    assert_eq!(paths, ["/orders", "/trades"], "2 reads, no positions, no margins");
}

#[tokio::test]
async fn audit_reads_positions_only() {
    let (root, seen) = fake_kite().await;
    assert!(broker(root).positions().await.unwrap().is_empty());
    assert_eq!(*seen.lock().unwrap(), ["/portfolio/positions"]);
}
