use serde_json::json;

/// Explicit subscription intent, replayed once per new socket.
pub fn messages(token: u32) -> [String; 2] {
    [
        json!({"a":"subscribe", "v":[token]}).to_string(),
        json!({"a":"mode", "v":["full", [token]]}).to_string(),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn requests_exact_token_then_full_mode() {
        let messages = super::messages(123);
        let a: serde_json::Value = serde_json::from_str(&messages[0]).unwrap();
        let b: serde_json::Value = serde_json::from_str(&messages[1]).unwrap();
        assert_eq!(a, serde_json::json!({"a":"subscribe","v":[123]}));
        assert_eq!(b, serde_json::json!({"a":"mode","v":["full",[123]]}));
    }
}

/// Subscription payload for a single shared socket, supporting up to 4 configured assets.
/// Tokens are deduplicated before Kite subscription and full-mode assignment.
pub fn multi_messages(tokens: &[u32]) -> anyhow::Result<[String; 2]> {
    use anyhow::ensure;
    ensure!(
        !tokens.is_empty() && tokens.len() <= 4,
        "Expected 1-4 Kite instrument tokens"
    );
    ensure!(
        tokens.iter().all(|token| *token != 0),
        "Zero Kite instrument token"
    );
    let mut unique = tokens.to_vec();
    unique.sort_unstable();
    unique.dedup();
    Ok([
        json!({"a":"subscribe", "v":unique}).to_string(),
        json!({"a":"mode", "v":["full",unique]}).to_string(),
    ])
}
#[cfg(test)]
mod multi_tests {
    #[test]
    fn multi_token_subscription_is_one_socket_payload_and_deduplicated() {
        let messages = super::multi_messages(&[456, 123, 456]).unwrap();
        let subscribe: serde_json::Value = serde_json::from_str(&messages[0]).unwrap();
        let mode: serde_json::Value = serde_json::from_str(&messages[1]).unwrap();
        assert_eq!(
            subscribe,
            serde_json::json!({"a":"subscribe","v":[123,456]})
        );
        assert_eq!(mode, serde_json::json!({"a":"mode","v":["full",[123,456]]}));
        assert!(super::multi_messages(&[0]).is_err());
        assert!(super::multi_messages(&[]).is_err());
    }
}
