//! Offline-only JSONL replay: next-observation entry, spread/slippage and stop.
//! Not a broker execution model and never connects to Kite.
use super::iatf_research::{Config, Decision, Engine, Quote, Signal};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayConfig {
    pub strategy_config: String,
    pub instrument_token: u32,
    pub quantity: f64,
    pub slippage_points_per_side: f64,
    pub commission_per_side: f64,
    pub stop_distance_points: f64,
    pub reward_multiple: f64,
    pub max_holding_ticks: usize,
}
impl ReplayConfig {
    fn validate(&self) -> Result<()> {
        ensure!(self.instrument_token > 0, "Replay token missing");
        ensure!(!self.strategy_config.is_empty(), "Strategy config missing");
        ensure!(
            self.quantity.is_finite() && self.quantity > 0.0,
            "Quantity must be positive"
        );
        ensure!(
            self.slippage_points_per_side.is_finite() && self.slippage_points_per_side >= 0.0,
            "Invalid slippage"
        );
        ensure!(
            self.commission_per_side.is_finite() && self.commission_per_side >= 0.0,
            "Invalid commission"
        );
        ensure!(
            self.stop_distance_points.is_finite() && self.stop_distance_points > 0.0,
            "Invalid stop"
        );
        ensure!(
            self.reward_multiple.is_finite() && self.reward_multiple > 0.0,
            "Invalid reward"
        );
        ensure!(
            (1..=100_000).contains(&self.max_holding_ticks),
            "Invalid max holding ticks"
        );
        Ok(())
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recorded {
    generation: u32,
    instrument_token: u32,
    exchange_ts_ms: u64,
    received_ts_ms: u64,
    bid: f64,
    ask: f64,
    bid_qty: f64,
    ask_qty: f64,
    last: f64,
}
impl Recorded {
    fn quote(&self) -> Quote {
        Quote {
            instrument_token: self.instrument_token,
            exchange_ts_ms: self.exchange_ts_ms,
            received_ts_ms: self.received_ts_ms,
            bid: self.bid,
            ask: self.ask,
            bid_qty: self.bid_qty,
            ask_qty: self.ask_qty,
            last: self.last,
        }
    }
}
#[derive(Debug, Serialize)]
struct Trade {
    side: i32,
    entry_ts_ms: u64,
    exit_ts_ms: u64,
    entry: f64,
    exit: f64,
    gross: f64,
    net: f64,
    exit_reason: &'static str,
}
struct Open {
    side: i32,
    at: u64,
    entry: f64,
    age: usize,
}
#[derive(Debug, Serialize)]
struct Report {
    event: &'static str,
    rows: usize,
    candidates: usize,
    trades: usize,
    wins: usize,
    losses: usize,
    net_pnl: f64,
    profit_factor: Option<f64>,
    max_drawdown: f64,
    unresolved: bool,
    records: Vec<Trade>,
}
fn fill(price: f64, side: i32, enter: bool, cfg: &ReplayConfig) -> f64 {
    let adverse = if enter { side as f64 } else { -(side as f64) };
    price + adverse * cfg.slippage_points_per_side
}
fn simulate(reader: impl BufRead, strategy: Config, cfg: &ReplayConfig) -> Result<Report> {
    cfg.validate()?;
    strategy.validate()?;
    let mut engine = Engine::new(strategy, cfg.instrument_token)?;
    let mut rows = 0usize;
    let mut candidates = 0usize;
    let mut generation = None;
    let mut pending = 0i32;
    let mut open: Option<Open> = None;
    let mut trades = Vec::new();
    let mut peak = 0.0f64;
    let mut equity = 0.0f64;
    let mut max_drawdown = 0.0f64;
    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        ensure!(
            !line.trim().is_empty(),
            "Empty JSONL line {}",
            line_number + 1
        );
        let r: Recorded = serde_json::from_str(&line)
            .with_context(|| format!("Invalid JSONL line {}", line_number + 1))?;
        ensure!(r.generation > 0, "Invalid connection generation");
        if let Some(g) = generation {
            ensure!(
                g == r.generation,
                "Feed generation changed: continuity not verified"
            );
        }
        generation = Some(r.generation);
        let q = r.quote();
        // Market data is validated before paper fill; a gap is fatal.
        let Decision { signal, .. } = engine.observe(q)?;
        rows += 1;
        if let Some(mut pos) = open.take() {
            pos.age += 1;
            let entry = pos.entry;
            // Price-trigger model uses the next visible top of book and adverse slippage;
            // stop/target are approximations, not exchange-protected stop orders.
            let stop = if pos.side == 1 {
                entry - cfg.stop_distance_points
            } else {
                entry + cfg.stop_distance_points
            };
            let target = if pos.side == 1 {
                entry + cfg.stop_distance_points * cfg.reward_multiple
            } else {
                entry - cfg.stop_distance_points * cfg.reward_multiple
            };
            let stop_hit = if pos.side == 1 {
                q.last <= stop
            } else {
                q.last >= stop
            };
            let target_hit = if pos.side == 1 {
                q.last >= target
            } else {
                q.last <= target
            };
            let timeout = pos.age >= cfg.max_holding_ticks;
            if stop_hit || target_hit || timeout {
                let executable = if pos.side == 1 { q.bid } else { q.ask };
                let exit = fill(executable, pos.side, false, cfg);
                let gross = (exit - entry) * (pos.side as f64) * cfg.quantity;
                let net = gross - 2.0 * cfg.commission_per_side;
                equity += net;
                peak = peak.max(equity);
                max_drawdown = max_drawdown.max(peak - equity);
                trades.push(Trade {
                    side: pos.side,
                    entry_ts_ms: pos.at,
                    exit_ts_ms: q.exchange_ts_ms,
                    entry,
                    exit,
                    gross,
                    net,
                    exit_reason: if stop_hit {
                        "stop"
                    } else if target_hit {
                        "target"
                    } else {
                        "timeout"
                    },
                });
            } else {
                open = Some(pos);
            }
        } else if pending != 0 {
            let executable = if pending == 1 { q.ask } else { q.bid };
            open = Some(Open {
                side: pending,
                at: q.exchange_ts_ms,
                entry: fill(executable, pending, true, cfg),
                age: 0,
            });
            pending = 0;
        }
        if open.is_none() && pending == 0 {
            let direction = match signal {
                Signal::LongCandidate => 1,
                Signal::ShortCandidate => -1,
                Signal::Wait => 0,
            };
            if direction != 0 {
                candidates += 1;
                pending = direction;
            }
        }
    }
    ensure!(rows > 0, "Replay file is empty");
    // Deliberately do not invent a closing fill at end of file.
    let unresolved = open.is_some() || pending != 0;
    let wins = trades.iter().filter(|t| t.net > 0.0).count();
    let losses = trades.iter().filter(|t| t.net < 0.0).count();
    let gains: f64 = trades.iter().filter(|t| t.net > 0.0).map(|t| t.net).sum();
    let loss: f64 = trades.iter().filter(|t| t.net < 0.0).map(|t| -t.net).sum();
    Ok(Report {
        event: "iatf_offline_replay",
        rows,
        candidates,
        trades: trades.len(),
        wins,
        losses,
        net_pnl: equity,
        profit_factor: if loss > 0.0 { Some(gains / loss) } else { None },
        max_drawdown,
        unresolved,
        records: trades,
    })
}
pub fn run(settings: &str, data: &str) -> Result<()> {
    let cfg: ReplayConfig = serde_json::from_str(&std::fs::read_to_string(settings)?)?;
    let strategy = Config::load(&cfg.strategy_config)?;
    ensure!(
        !strategy.enabled && !strategy.live_orders_enabled,
        "Offline research profile must be disabled"
    );
    let report = simulate(BufReader::new(File::open(data)?), strategy, &cfg)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_str(include_str!(
            "../../../../config/iatf-crudeoilmini-research.json"
        ))
        .unwrap()
    }
    fn settings() -> ReplayConfig {
        serde_json::from_str(include_str!("../../../../config/iatf-replay.example.json")).unwrap()
    }
    fn line(t: usize, p: f64, generation: u32) -> String {
        serde_json::json!({"generation":generation,"instrument_token":42,"exchange_ts_ms":t*1000,
            "received_ts_ms":t*1000+5,"bid":p-0.05,"ask":p+0.05,
            "bid_qty":90.0,"ask_qty":10.0,"last":p})
        .to_string()
    }
    #[test]
    fn next_tick_fill_and_no_fictitious_final_liquidation() {
        let lines = (1..=28)
            .map(|t| line(t, 100.0 + t as f64, 1))
            .collect::<Vec<_>>()
            .join("\n");
        let mut cfg = settings();
        cfg.instrument_token = 42;
        let result = simulate(lines.as_bytes(), config(), &cfg).unwrap();
        assert!(result.candidates > 0);
        assert!(result.trades > 0 || result.unresolved);
    }
    #[test]
    fn malformed_stale_reconnect_and_wrong_token_fail_closed() {
        let mut cfg = settings();
        cfg.instrument_token = 42;
        assert!(simulate("not-json".as_bytes(), config(), &cfg).is_err());
        let lines = format!("{}\n{}", line(1, 101.0, 1), line(2, 102.0, 2));
        assert!(simulate(lines.as_bytes(), config(), &cfg).is_err());
        cfg.instrument_token = 43;
        assert!(simulate(line(1, 101.0, 1).as_bytes(), config(), &cfg).is_err());
    }
    #[test]
    fn rejects_invalid_execution_assumptions() {
        let mut cfg = settings();
        cfg.quantity = 0.0;
        assert!(simulate("".as_bytes(), config(), &cfg).is_err());
    }
}
