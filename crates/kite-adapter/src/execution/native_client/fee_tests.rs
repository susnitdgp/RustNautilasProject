use super::super::request::Command;
use super::{
    broker::{Broker, Snapshot},
    fees::{self, Fees},
    mock::MockBroker,
};
use crate::{credentials::KiteCredentials, http::authenticated::ReadClient};
use nautilus_model::types::{Currency, Money};
use rust_decimal::Decimal;
async fn snapshot() -> Snapshot {
    let broker = MockBroker::new(144870151, "NRML");
    broker
        .execute(&Command::Place {
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: "BUY".into(),
            product: "NRML".into(),
            quantity: 2,
            price_rupees: 6000,
            tag: "TestCharges".into(),
        })
        .await
        .unwrap();
    let mut s = broker.snapshot().await.unwrap();
    s.trades[0].quantity = 1;
    let mut second = s.trades[0].clone();
    second.trade_id = "200".into();
    second.average_price = Decimal::from(5900);
    s.trades.push(second);
    s
}
fn total(fees: &Fees) -> Money {
    fees.values()
        .fold(Money::new(0.0, Currency::INR()), |sum, value| sum + *value)
}
#[tokio::test]
async fn calculated_order_charges_are_allocated_exactly_and_deterministically() {
    let s = snapshot().await;
    let trades: Vec<_> = s.trades.iter().collect();
    let fees = fees::allocate(Decimal::new(1575, 2), &trades).unwrap();
    assert_eq!(total(&fees), Money::new(15.75, Currency::INR()));
    assert_eq!(fees.len(), 2);
    let reversed: Vec<_> = s.trades.iter().rev().collect();
    assert_eq!(
        fees,
        fees::allocate(Decimal::new(1575, 2), &reversed).unwrap()
    );
    assert_eq!(
        total(&fees::allocate(Decimal::new(5, 3), &trades).unwrap()),
        Money::new(0.01, Currency::INR())
    );
    assert!(fees::allocate(Decimal::MAX, &trades).is_err());
    assert!(fees::allocate(Decimal::NEGATIVE_ONE, &trades).is_err());
}
#[tokio::test]
async fn charge_input_rejects_inconsistent_or_duplicate_broker_trades() {
    let mut s = snapshot().await;
    s.trades[0].quantity = 2;
    assert!(fees::groups(&s, "NRML", 144870151).is_err());
    s.trades[0].quantity = 1;
    s.trades.push(s.trades[0].clone());
    assert!(fees::groups(&s, "NRML", 144870151).is_err());
}
#[tokio::test]
async fn virtual_contract_note_http_payload_and_identity_validation() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    for wrong_side in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = [
            "http://",
            &listener.local_addr().unwrap().to_string(),
            "/charges/orders",
        ]
        .concat();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(socket.read_u8().await.unwrap());
                assert!(bytes.len() < 8192);
            }
            let headers = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            assert!(headers.starts_with("post /charges/orders "));
            assert!(headers.contains("authorization: token test-key:test-token"));
            assert!(headers.contains("content-type: application/json"));
            let length = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request[0]["quantity"], 2);
            assert!(request[0]["average_price"].is_number());
            assert_eq!(request[0]["average_price"].as_f64(), Some(5950.0));
            assert_eq!(request[0]["exchange"], "MCX");
            assert_eq!(request[0]["order_type"], "LIMIT");
            let body=serde_json::json!({"status":"success","data":[{"exchange":"MCX","tradingsymbol":"CRUDEOIL26SEPFUT","transaction_type":if wrong_side{"SELL"}else{"BUY"},"variety":"regular","product":"NRML","order_type":"LIMIT","quantity":2,"price":5950,"charges":{"total":5.25}}]}).to_string();
            let reply = [
                "HTTP/1.1 200 OK\r\nContent-Length: ",
                &body.len().to_string(),
                "\r\nConnection: close\r\n\r\n",
                &body,
            ]
            .concat();
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let credentials =
            KiteCredentials::new(Some("test-key".into()), Some("test-token".into())).unwrap();
        let read = ReadClient::new(&credentials).unwrap().charges_test_url(url);
        let result = fees::calculate(&read, &snapshot().await, "NRML", 144870151).await;
        server.await.unwrap();
        if wrong_side {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("identity mismatch")
            );
        } else {
            assert_eq!(total(&result.unwrap()), Money::new(5.25, Currency::INR()));
        }
    }
}
