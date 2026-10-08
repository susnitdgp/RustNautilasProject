//! Research-only independent trade simulator on historical candle OHLCV.
//! No Kite order API; intrabar stop execution is a pessimistic approximation.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    instrument_token: u32,
    quantity: f64,
    stop_points: f64,
    trail_points: f64,
    slippage_points_per_side: f64,
    fees_per_round_trip: f64,
    max_holding_minutes: usize,
    entry_cutoff_ist: String,
}
impl Settings {
    fn validate(&self) -> Result<()> {
        ensure!(self.instrument_token > 0, "Invalid token");
        for v in [self.quantity, self.stop_points, self.trail_points] {
            ensure!(
                v.is_finite() && v > 0.0,
                "Positive finite quantity and risk required"
            );
        }
        for v in [self.slippage_points_per_side, self.fees_per_round_trip] {
            ensure!(v.is_finite() && v >= 0.0, "Invalid friction");
        }
        ensure!(
            (1..=300).contains(&self.max_holding_minutes),
            "Invalid holding duration"
        );
        ensure!(
            self.entry_cutoff_ist == "23:15",
            "Research requires fixed 23:15 entry cutoff"
        );
        Ok(())
    }
}
#[derive(Default)]
struct Score {
    trades: usize,
    wins: usize,
    net: f64,
    profit: f64,
    loss: f64,
    peak: f64,
    drawdown: f64,
}
impl Score {
    fn add(&mut self, net: f64) {
        self.trades += 1;
        self.wins += usize::from(net > 0.0);
        self.net += net;
        if net > 0.0 {
            self.profit += net;
        } else {
            self.loss -= net;
        }
        self.peak = self.peak.max(self.net);
        self.drawdown = self.drawdown.max(self.peak - self.net);
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"trades":self.trades,"wins":self.wins,"losses":self.trades-self.wins,
            "win_rate":if self.trades>0{Some(self.wins as f64/self.trades as f64)}else{None},
            "net_model_inr":self.net,"profit_factor":if self.loss>0.0{Some(self.profit/self.loss)}else{None},
            "realized_max_drawdown":self.drawdown})
    }
}
#[derive(Default)]
struct Results {
    all: Score,
    trend: Score,
    chop: Score,
    transition: Score,
}
fn efficiency(x: &[f64]) -> Option<f64> {
    if x.len() < 11 {
        return None;
    }
    let tail = &x[x.len() - 11..];
    let travel: f64 = tail.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    Some(if travel > 0.0 {
        (tail[10] - tail[0]).abs() / travel
    } else {
        0.0
    })
}
fn research(minute: &[Candle], three: &[Candle], s: &Settings) -> Result<serde_json::Value> {
    s.validate()?;
    ensure!(!minute.is_empty() && !three.is_empty(), "Empty candles");
    let mut results = Results::default();
    let mut previous: Vec<f64> = Vec::new();
    let mut three_closes: Vec<f64> = Vec::new();
    let mut cursor = 0usize;
    let mut idx = 0usize;
    let mut previous_day = None;
    // Count actual source-data gaps, independently of position-driven scan jumps.
    let missing = minute
        .windows(2)
        .filter(|pair| match (pair[0].time(), pair[1].time()) {
            (Ok(a), Ok(b)) => {
                a.date_naive() == b.date_naive() && b.timestamp() != a.timestamp() + 60
            }
            _ => true,
        })
        .count();
    let mut occupied = 0usize;
    let mut trend_days = 0usize;
    let mut chop_days = 0usize;
    let mut daily = Vec::new();
    while idx < minute.len() {
        let c = &minute[idx];
        let t = c.time()?;
        let day = t.date_naive();
        if previous_day != Some(day) {
            previous_day = Some(day);
            previous.clear();
            three_closes.clear();
        }
        // Discontinuities invalidate signal warmup, independently of trade exits.
        if idx > 0
            && minute[idx - 1].time()?.date_naive() == day
            && t.timestamp() != minute[idx - 1].time()?.timestamp() + 60
        {
            previous.clear();
        }
        while cursor < three.len() {
            let b = three[cursor].time()?;
            if b.timestamp() + 180 > t.timestamp() + 60 {
                break;
            }
            if b.date_naive() == day {
                three_closes.push(three[cursor].close);
            }
            cursor += 1;
        }
        let regime = match efficiency(&three_closes) {
            Some(e) if e >= 0.45 => "trend",
            Some(e) if e < 0.25 => "chop",
            _ => "transition",
        };
        // Signals derive from the completed current 1-minute candle.
        let direction = if previous.len() >= 8
            && regime == "trend"
            && t.format("%H:%M").to_string() < s.entry_cutoff_ist
        {
            let past = &previous[previous.len() - 8..];
            if c.close > past.iter().copied().fold(f64::NEG_INFINITY, f64::max) {
                1.0
            } else if c.close < past.iter().copied().fold(f64::INFINITY, f64::min) {
                -1.0
            } else {
                0.0
            }
        } else {
            0.0
        };
        previous.push(c.close);
        if direction == 0.0 || idx + 1 >= minute.len() {
            idx += 1;
            continue;
        }
        let next = &minute[idx + 1];
        if next.time()?.timestamp() != t.timestamp() + 60 || next.time()?.date_naive() != day {
            idx += 1;
            continue;
        }
        let entry = next.open + direction * s.slippage_points_per_side;
        let mut stop = entry - direction * s.stop_points;
        let mut exit = None;
        let mut finish = idx + 1;
        let mut consecutive = true;
        for j in idx + 1..minute.len() {
            let bar = &minute[j];
            if bar.time()?.date_naive() != day {
                break;
            }
            if j > idx + 1 && bar.time()?.timestamp() != minute[j - 1].time()?.timestamp() + 60 {
                consecutive = false;
                break;
            }
            finish = j;
            // Stops applied before trailing updates; gaps filled at worse of stop and opening price.
            let hit = if direction > 0.0 {
                bar.low <= stop
            } else {
                bar.high >= stop
            };
            if hit {
                let worst = if direction > 0.0 {
                    stop.min(bar.open)
                } else {
                    stop.max(bar.open)
                };
                exit = Some(worst - direction * s.slippage_points_per_side);
                break;
            }
            if j - (idx + 1) + 1 >= s.max_holding_minutes
                || bar.time()?.format("%H:%M").to_string() >= "23:29".to_string()
            {
                exit = Some(bar.close - direction * s.slippage_points_per_side);
                break;
            }
            let candidate = if direction > 0.0 {
                bar.close - s.trail_points
            } else {
                bar.close + s.trail_points
            };
            stop = if direction > 0.0 {
                stop.max(candidate)
            } else {
                stop.min(candidate)
            };
        }
        if !consecutive || exit.is_none() {
            idx += 1;
            continue;
        }
        let filled = exit.expect("checked");
        let net = (filled - entry) * direction * s.quantity - s.fees_per_round_trip;
        results.all.add(net);
        match regime {
            "trend" => results.trend.add(net),
            "chop" => results.chop.add(net),
            _ => results.transition.add(net),
        }
        daily.push(
            serde_json::json!({"date":day.to_string(),"entry_time":next.timestamp,
            "exit_time":minute[finish].timestamp,"side":if direction>0.0{"LONG"}else{"SHORT"},
            "entry":entry,"exit":filled,"net_model_inr":net}),
        );
        occupied += finish - (idx + 1) + 1;
        idx = finish + 1; // no overlapping positions or same-bar re-entry
    }
    let daily_pnl = {
        let mut grouped = std::collections::BTreeMap::<String, f64>::new();
        for x in &daily {
            *grouped
                .entry(x["date"].as_str().unwrap_or("").to_owned())
                .or_default() += x["net_model_inr"].as_f64().unwrap_or(0.0);
        }
        grouped
    };
    for (date, net) in &daily_pnl {
        let _ = date;
        if *net > 0.0 {
            trend_days += 1;
        } else if *net < 0.0 {
            chop_days += 1;
        }
    }
    Ok(serde_json::json!({
        "event":"iatf_september_trade_research","instrument_token":s.instrument_token,
        "input_1minute_bars":minute.len(),"input_3minute_bars":three.len(),
        "missing_minute_intervals":missing,"occupied_minutes":occupied,
        "all":results.all.json(),"entry_trend_regime":results.trend.json(),
        "entry_chop_regime":results.chop.json(),"entry_transition_regime":results.transition.json(),
        "profitable_trade_days":trend_days,"losing_trade_days":chop_days,"trade_log":daily,
        "warning":"Candle-only hypothetical fills; not Kite broker trades. Intrabar sequencing unknown, no market-depth confirmation, simplified fees and stop execution."
    }))
}
pub fn run(path: &str) -> Result<()> {
    let s: Settings = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    s.validate()?;
    let rt = tokio::runtime::Runtime::new()?;
    let minute = rt.block_on(super::iatf_multitimeframe::month(
        s.instrument_token,
        Interval::OneMinute,
    ))?;
    let three = rt.block_on(super::iatf_multitimeframe::month(
        s.instrument_token,
        Interval::ThreeMinute,
    ))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&research(&minute, &three, &s)?)?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn risk_parameters_fail_closed() {
        let mut s = Settings {
            instrument_token: 42,
            quantity: 1.0,
            stop_points: 5.0,
            trail_points: 5.0,
            slippage_points_per_side: 0.5,
            fees_per_round_trip: 3.0,
            max_holding_minutes: 60,
            entry_cutoff_ist: "23:15".into(),
        };
        s.validate().unwrap();
        s.slippage_points_per_side = -1.0;
        assert!(s.validate().is_err());
    }
    #[test]
    fn efficiency_directionality() {
        assert_eq!(
            efficiency(&(0..11).map(|v| v as f64).collect::<Vec<_>>()),
            Some(1.0)
        );
    }
}
