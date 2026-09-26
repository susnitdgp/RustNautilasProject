mod native_node;

fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    native_node::cli::dispatch(&args).unwrap_or_else(|| {
        Err(anyhow::anyhow!(
            "Unknown command. Run a native-trend-ribbon-* or native-kite-* operational command."
        ))
    })
}
