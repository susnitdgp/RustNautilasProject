use kite_adapter::credentials::redis::{ACCESS_TOKEN_KEY, API_KEY, load_from_url};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

fn mock(reply: String) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("redis://{}/0", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(socket);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with('*'));
            let count: usize = line[1..].trim().parse().unwrap();
            let mut command = Vec::new();
            for _ in 0..count {
                line.clear();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with('$'));
                let size: usize = line[1..].trim().parse().unwrap();
                let mut bytes = vec![0; size + 2];
                reader.read_exact(&mut bytes).unwrap();
                command.push(String::from_utf8(bytes[..size].to_vec()).unwrap());
            }
            if command[0] == "MGET" {
                assert_eq!(command, ["MGET", API_KEY, ACCESS_TOKEN_KEY]);
                reader.get_mut().write_all(reply.as_bytes()).unwrap();
                return;
            }
            // Redis client may send CLIENT SETINFO during initialization.
            assert!(matches!(command[0].as_str(), "CLIENT" | "SELECT"));
            reader.get_mut().write_all(b"+OK\r\n").unwrap();
        }
    });
    (url, handle)
}

fn bulk(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("$ {}\r\n{}\r\n", value.len(), value).replace("$ ", "$"),
        None => "$-1\r\n".to_string(),
    }
}
fn load(
    api_key: Option<&str>,
    token: Option<&str>,
) -> anyhow::Result<kite_adapter::credentials::KiteCredentials> {
    let (url, handle) = mock(format!("*2\r\n{}{}", bulk(api_key), bulk(token)));
    let result = load_from_url(&url);
    handle.join().unwrap();
    result
}

#[test]
fn reads_exact_keys_and_returns_values_for_transport() {
    let credentials = load(Some("test-key"), Some("test-token")).unwrap();
    assert_eq!(credentials.api_key(), "test-key");
    assert_eq!(credentials.access_token(), "test-token");
}
#[test]
fn debug_redacts_both_values() {
    let credentials = load(Some("secret-api-sentinel"), Some("secret-token-sentinel")).unwrap();
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("sentinel"));
    assert_eq!(debug.matches("[REDACTED]").count(), 2);
}
#[test]
fn missing_either_key_fails_without_leaking_the_other() {
    for values in [
        (None, Some("secret-sentinel")),
        (Some("secret-sentinel"), None),
    ] {
        let error = format!("{:#}", load(values.0, values.1).unwrap_err());
        assert!(!error.contains("secret-sentinel"));
        assert!(error.contains("missing"));
    }
}
#[test]
fn rejects_empty_whitespace_control_and_oversized_values() {
    for value in [
        "".to_string(),
        " ".to_string(),
        "abc\n".to_string(),
        "x".repeat(4097),
    ] {
        assert!(load(Some(&value), Some("valid")).is_err());
        assert!(load(Some("valid"), Some(&value)).is_err());
    }
}
#[test]
fn redis_error_body_is_redacted() {
    let (url, handle) = mock("-ERR secret-error-sentinel\r\n".to_string());
    let error = format!("{:#}", load_from_url(&url).unwrap_err());
    handle.join().unwrap();
    assert_eq!(error, "Redis credential read failed");
}
#[test]
fn malformed_response_is_redacted() {
    let (url, handle) = mock("*1\r\n$6\r\nsecret\r\n".to_string());
    let error = format!("{:#}", load_from_url(&url).unwrap_err());
    handle.join().unwrap();
    assert_eq!(error, "Redis credential read failed");
}
#[test]
fn invalid_url_is_redacted() {
    let error = format!(
        "{:#}",
        load_from_url("invalid://secret-password-sentinel").unwrap_err()
    );
    assert_eq!(error, "Invalid Redis connection configuration");
}
#[test]
fn unavailable_redis_fails_closed() {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("redis://{}/0", socket.local_addr().unwrap());
    drop(socket);
    assert_eq!(
        format!("{:#}", load_from_url(&url).unwrap_err()),
        "Redis connection or authentication failed"
    );
}
