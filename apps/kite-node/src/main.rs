use anyhow::{Context, Result, bail, ensure};
use chrono::{FixedOffset, Utc};
use kite_adapter::{config::Config, http::instruments, preflight};
use std::{env, fs, io::Read};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() != 3 && args.len() != 4 {
        bail!(
            "Usage: kite-node preflight CONFIG --download | kite-node preflight CONFIG --csv FILE"
        );
    }
    ensure!(
        args[0] == "preflight",
        "Only read-only preflight is available"
    );
    let config = Config::parse(&fs::read_to_string(&args[1]).context("Read configuration")?)?;
    let (bytes, source) = match args[2].as_str() {
        "--download" if args.len() == 3 => (instruments::download()?, "kite_live_download"),
        "--csv" if args.len() == 4 => {
            let file = fs::File::open(&args[3]).context("Open instrument CSV")?;
            let mut bytes = Vec::new();
            file.take(instruments::MAX_MASTER_BYTES + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= instruments::MAX_MASTER_BYTES,
                "CSV exceeds size limit"
            );
            (bytes, "user_supplied_csv_freshness_unverified")
        }
        _ => bail!("Invalid arguments; use --download or --csv FILE"),
    };
    let india = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid IST offset");
    let now = Utc::now();
    let report = preflight::run(
        &config,
        bytes.as_slice(),
        now.with_timezone(&india).date_naive(),
    )?;
    let output = serde_json::json!({
        "source": source,
        "checked_at_utc": now,
        "report": report,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
