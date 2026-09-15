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
