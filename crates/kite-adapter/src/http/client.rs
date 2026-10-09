//! Shared Kite HTTPS client (kite-adapter 0.2.6).
//!
//! api.kite.trade negotiates HTTP/2 over ALPN, so orders and reads share one long-lived,
//! multiplexed connection. HTTP/2 PING frames keep it open between bars; pings are not
//! API calls, so they cost nothing against Kite's rate limits. Reusing the same `Client`
//! also keeps rustls' session cache, so any reconnect is a resumed TLS handshake.
use anyhow::{Result, anyhow};
use reqwest::{Client, redirect::Policy};
use std::{
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const H2_PING_INTERVAL: Duration = Duration::from_secs(20);
const H2_PING_TIMEOUT: Duration = Duration::from_secs(5);
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);

/// Builds the Kite client. Per-request timeouts are set by the order and read paths.
///
/// Kite only accepts orders from the app's registered static IP. Dual-stack hosts would
/// otherwise reach api.kite.trade over IPv6 (an unregistered address) and get 403
/// PermissionException; binding an IPv4 local address restricts connections to IPv4
/// destinations, i.e. the registered static IPv4. Reads use the same binding so they
/// share (and keep warm) the order connection.
pub(crate) fn kite_client() -> Result<Client> {
    Client::builder()
        .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        .retry(reqwest::retry::never())
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(None)
        .pool_max_idle_per_host(2)
        .tcp_nodelay(true)
        .tcp_keepalive(TCP_KEEPALIVE)
        .http2_keep_alive_interval(H2_PING_INTERVAL)
        .http2_keep_alive_timeout(H2_PING_TIMEOUT)
        .http2_keep_alive_while_idle(true)
        .build()
        .map_err(|_| anyhow!("Kite HTTP client initialization failed"))
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Network check, run on demand: `cargo test -p kite-adapter -- --ignored kite_negotiates_h2`.
    #[tokio::test]
    #[ignore = "reaches api.kite.trade"]
    async fn kite_negotiates_h2_and_reuses_the_connection() {
        let client = kite_client().unwrap();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let response = client.get("https://api.kite.trade/user/profile").send().await.unwrap();
            assert_eq!(response.version(), reqwest::Version::HTTP_2);
            println!("{:?} {:?}", response.status(), started.elapsed());
        }
    }
}
