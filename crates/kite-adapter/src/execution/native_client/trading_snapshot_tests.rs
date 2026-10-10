//! Order-path snapshot (kite-adapter 0.6.0): orders, trades and positions read together
//! and the book read again; margins never read; an order book that changes between the
//! reads is still refused.
use super::*;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const ORDER: &str = r#"{"order_id":"1001","exchange":"MCX","tradingsymbol":"CRUDEOILM26OCTFUT",
"instrument_token":145894663,"product":"MIS","transaction_type":"BUY","variety":"regular",
"order_type":"MARKET","market_protection":-1,"validity":"DAY","status":"OPEN","quantity":1,
"filled_quantity":0,"price":0,"trigger_price":0,"tag":"t1","exchange_timestamp":null,
"exchange_update_timestamp":null,"order_timestamp":"2026-10-12 10:00:00"}"#;

/// A minimal HTTP/1.1 Kite stand-in: records each path; the second `/orders` answer
/// carries one order when `changing_book` is set (the book changed between the reads).
async fn fake_kite(changing_book: bool) -> (&'static str, Arc<StdMutex<Vec<String>>>) {
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
                    let orders_before = {
                        let mut seen = log.lock().unwrap();
                        let n = seen.iter().filter(|p| p.as_str() == "/orders").count();
                        seen.push(path.clone());
                        n
                    };
                    let data = match path.as_str() {
                        "/orders" if changing_book && orders_before >= 1 => format!("[{ORDER}]"),
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
    // as after the start-up (full) snapshot
    *b.last_funds.lock().unwrap() = Some(Funds {
        ledger: None,
        enabled: true,
        net: Decimal::from(100_000),
        utilised: Utilised { debits: Decimal::ZERO },
    });
    b
}

#[tokio::test]
async fn trading_snapshot_reads_book_trades_positions_and_the_book_again_never_margins() {
    let (root, seen) = fake_kite(false).await;
    let s = broker(root).trading_snapshot().await.unwrap();
    assert!(s.orders.is_empty() && s.trades.is_empty() && s.positions.is_empty());
    assert_eq!(s.funds.net, Decimal::from(100_000), "funds from the last full snapshot");
    let mut paths = seen.lock().unwrap().clone();
    paths.sort();
    assert_eq!(paths, ["/orders", "/orders", "/portfolio/positions", "/trades"], "4 reads, no margins");
}

#[tokio::test]
async fn a_book_that_changes_during_the_reads_is_refused_as_transient() {
    let (root, _) = fake_kite(true).await;
    let error = broker(root).trading_snapshot().await.unwrap_err();
    assert!(
        matches!(error.downcast_ref::<super::super::outage::ReadFailure>(), Some(super::super::outage::ReadFailure::Transient)),
        "{error:#}"
    );
}
