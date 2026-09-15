use anyhow::Result;
pub fn run(config: &str, input: Option<&str>) -> Result<()> {
    let config = kite_strategy::config::Config::parse(&std::fs::read_to_string(config)?)?;
    let namespace = nautilus_core::UUID4::new().to_string();
    let result = match input {
        Some(input) => {
            kite_strategy::paper::replay(config, &namespace, std::path::Path::new(input))?
        }
        None => kite_strategy::paper::synthetic(config, &namespace)?,
    };
    println!(
        "{}",
        serde_json::json!({"namespace":namespace,"persistence":"redis_aof","result":result})
    );
    Ok(())
}
