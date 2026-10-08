//! Research-only IATF decision engine. No broker connectivity, Redis writes or order placement.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, fs};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub strategy: String,
    pub instrument: String,
    pub enabled: bool,
    pub live_orders_enabled: bool,
    pub efficiency_lookback: usize,
    pub efficiency_threshold: f64,
    pub chop_threshold: f64,
    pub breakout_lookback: usize,
    pub min_book_imbalance: f64,
    pub max_spread_fraction: f64,
    pub max_tick_age_ms: u64,
}
impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let c: Self = serde_json::from_str(&fs::read_to_string(path)?)?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.strategy == "iatf_v1_research",
            "Unsupported IATF strategy"
        );
        ensure!(
            self.instrument.ends_with(".MCX"),
            "IATF research expects MCX futures"
        );
        ensure!(!self.live_orders_enabled, "IATF live orders are prohibited");
        ensure!(
            (3..=100).contains(&self.efficiency_lookback),
            "Invalid efficiency window"
        );
        ensure!(
            (3..=100).contains(&self.breakout_lookback),
            "Invalid breakout window"
        );
        ensure!(
            self.efficiency_threshold.is_finite()
                && (0.0..=1.0).contains(&self.efficiency_threshold),
            "Invalid efficiency threshold"
        );
        ensure!(
            self.chop_threshold.is_finite()
                && self.chop_threshold >= 0.0
                && self.chop_threshold < self.efficiency_threshold,
            "Invalid chop threshold"
        );
        ensure!(
            self.min_book_imbalance.is_finite() && (0.0..1.0).contains(&self.min_book_imbalance),
            "Invalid imbalance"
        );
        ensure!(
            self.max_spread_fraction.is_finite()
                && self.max_spread_fraction > 0.0
                && self.max_spread_fraction < 0.05,
            "Invalid spread limit"
        );
        ensure!(
            (1..=30_000).contains(&self.max_tick_age_ms),
            "Invalid freshness limit"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Quote {
    pub instrument_token: u32,
    pub exchange_ts_ms: u64,
    pub received_ts_ms: u64,
    pub bid: f64,
    pub ask: f64,
    pub bid_qty: f64,
    pub ask_qty: f64,
    pub last: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    InsufficientData,
    Chop,
    Transition,
    Trend,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Wait,
    LongCandidate,
    ShortCandidate,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Decision {
    pub regime: Regime,
    pub signal: Signal,
    pub efficiency: f64,
    pub book_imbalance: f64,
}
pub struct Engine {
    config: Config,
    token: u32,
    closes: VecDeque<f64>,
    last_ts: Option<u64>,
}
impl Engine {
    pub fn new(config: Config, token: u32) -> Result<Self> {
        config.validate()?;
        ensure!(token > 0, "Research token must be nonzero");
        Ok(Self {
            config,
            token,
            closes: VecDeque::new(),
            last_ts: None,
        })
    }
    pub fn observe(&mut self, q: Quote) -> Result<Decision> {
        ensure!(
            q.instrument_token == self.token,
            "Unexpected instrument token"
        );
        ensure!(
            q.received_ts_ms >= q.exchange_ts_ms
                && q.received_ts_ms - q.exchange_ts_ms <= self.config.max_tick_age_ms,
            "Stale/future market data"
        );
        ensure!(
            self.last_ts.is_none_or(|ts| q.exchange_ts_ms > ts),
            "Non-monotonic quote time"
        );
        ensure!(
            [q.bid, q.ask, q.bid_qty, q.ask_qty, q.last]
                .iter()
                .all(|v| v.is_finite()),
            "Nonfinite quote"
        );
        ensure!(
            q.bid > 0.0 && q.ask >= q.bid && q.last > 0.0 && q.bid_qty > 0.0 && q.ask_qty > 0.0,
            "Invalid market depth"
        );
        ensure!(
            (q.ask - q.bid) / q.last <= self.config.max_spread_fraction,
            "Spread too wide"
        );
        self.last_ts = Some(q.exchange_ts_ms);
        let imbalance = (q.bid_qty - q.ask_qty) / (q.bid_qty + q.ask_qty);
        let lookback = self.config.efficiency_lookback;
        let required = lookback.max(self.config.breakout_lookback) + 1;
        let old: Vec<f64> = self.closes.iter().copied().collect();
        self.closes.push_back(q.last);
        while self.closes.len() > required {
            self.closes.pop_front();
        }
        if old.len() < required {
            return Ok(Decision {
                regime: Regime::InsufficientData,
                signal: Signal::Wait,
                efficiency: 0.0,
                book_imbalance: imbalance,
            });
        }
        let history: Vec<f64> = self.closes.iter().copied().collect();
        let tail = &history[history.len() - lookback - 1..];
        let distance = (tail[lookback] - tail[0]).abs();
        let movement: f64 = tail.windows(2).map(|p| (p[1] - p[0]).abs()).sum();
        let er = if movement > 0.0 {
            distance / movement
        } else {
            0.0
        };
        let regime = if er < self.config.chop_threshold {
            Regime::Chop
        } else if er >= self.config.efficiency_threshold {
            Regime::Trend
        } else {
            Regime::Transition
        };
        let pre = &history[history.len() - self.config.breakout_lookback - 1..history.len() - 1];
        let high = pre.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let low = pre.iter().copied().fold(f64::INFINITY, f64::min);
        // Candidate only: never an order. A breakout requires trend efficiency and book agreement.
        let signal = if regime == Regime::Trend
            && q.last > high
            && imbalance >= self.config.min_book_imbalance
        {
            Signal::LongCandidate
        } else if regime == Regime::Trend
            && q.last < low
            && imbalance <= -self.config.min_book_imbalance
        {
            Signal::ShortCandidate
        } else {
            Signal::Wait
        };
        Ok(Decision {
            regime,
            signal,
            efficiency: er,
            book_imbalance: imbalance,
        })
    }
}
pub fn inspect(path: &str) -> Result<()> {
    let config = Config::load(path)?;
    println!(
        "{}",
        serde_json::json!({"event":"iatf_research_config_valid","instrument":config.instrument,
        "enabled":config.enabled,"live_orders_enabled":false,"broker_orders_sent":false,
        "engine":"regime_and_order_book_breakout_candidates","mode":"research_only"})
    );
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
    fn quote(t: u64, p: f64, b: f64, a: f64) -> Quote {
        Quote {
            instrument_token: 42,
            exchange_ts_ms: t * 1000,
            received_ts_ms: t * 1000 + 5,
            bid: p - 0.05,
            ask: p + 0.05,
            bid_qty: b,
            ask_qty: a,
            last: p,
        }
    }
    #[test]
    fn valid_config_fail_closed_live() {
        let mut c = config();
        c.validate().unwrap();
        c.live_orders_enabled = true;
        assert!(c.validate().is_err());
    }
    #[test]
    fn detects_directional_breakout_after_warmup() {
        let mut e = Engine::new(config(), 42).unwrap();
        let mut d = e.observe(quote(1, 100.0, 90.0, 10.0)).unwrap();
        assert_eq!(d.signal, Signal::Wait);
        for t in 2..=24 {
            d = e.observe(quote(t, 100.0 + t as f64, 90.0, 10.0)).unwrap();
        }
        assert_eq!(d.regime, Regime::Trend);
        assert_eq!(d.signal, Signal::LongCandidate);
    }
    #[test]
    fn rejects_chop_and_stale_cross_token() {
        let mut e = Engine::new(config(), 42).unwrap();
        for t in 1..=30 {
            let d = e
                .observe(quote(t, if t % 2 == 0 { 101.0 } else { 100.0 }, 90.0, 10.0))
                .unwrap();
            assert_eq!(d.signal, Signal::Wait);
        }
        let mut stale = quote(31, 100.0, 10.0, 90.0);
        stale.received_ts_ms += 100_000;
        assert!(e.observe(stale).is_err());
        assert!(
            e.observe(quote(31, 100.0, 10.0, 90.0).instrument(77))
                .is_err()
        );
    }
    trait WithToken {
        fn instrument(self, token: u32) -> Self;
    }
    impl WithToken for Quote {
        fn instrument(mut self, token: u32) -> Self {
            self.instrument_token = token;
            self
        }
    }
    #[test]
    fn opposing_depth_blocks_breakout() {
        let mut e = Engine::new(config(), 42).unwrap();
        let mut d = e.observe(quote(1, 101.0, 10.0, 90.0)).unwrap();
        for t in 2..=24 {
            d = e.observe(quote(t, 101.0 + t as f64, 10.0, 90.0)).unwrap();
        }
        assert_eq!(d.regime, Regime::Trend);
        assert_eq!(d.signal, Signal::Wait);
    }
}
