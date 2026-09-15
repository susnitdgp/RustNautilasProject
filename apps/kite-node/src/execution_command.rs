use anyhow::Result;
pub fn run(namespace: Option<&str>) -> Result<()> {
    let generated = nautilus_core::UUID4::new().to_string();
    let namespace = namespace.unwrap_or(&generated);
    let summary = kite_execution::verification::run(namespace)?;
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"persistence":"redis_aof","result":summary})
    );
    Ok(())
}
