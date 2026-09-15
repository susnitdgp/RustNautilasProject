use anyhow::Result;
pub fn run() -> Result<()> {
    let namespace = nautilus_core::UUID4::new().to_string();
    let result = kite_execution::reports::verification::run(&namespace)?;
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"persistence":"redis_aof","result":result})
    );
    Ok(())
}
