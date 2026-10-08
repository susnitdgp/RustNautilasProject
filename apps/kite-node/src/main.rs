mod native_node;

fn main() -> anyhow::Result<()> {
    // Explicit provider selection: multiple transitive TLS backends are present.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    native_node::cli::dispatch(&args).unwrap_or_else(|| {
        Err(anyhow::anyhow!(
            "Unknown command. Run a native-portfolio-* or native-kite-* operational command."
        ))
    })
}
