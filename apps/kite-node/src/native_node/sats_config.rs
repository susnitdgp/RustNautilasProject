//! Per-slot settings for portfolio instances with `"strategy": "sats"`, read
//! from the slot's own JSON file (`strategy_config`). `params` holds every
//! SATS v1.13.1 input; `execution` decides how the model's events become
//! orders for the configured number of lots.
use anyhow::{Context, Result, ensure};
use kite_adapter::http::historical::{Candle, Interval, KiteInterval};
use sats::{BarInput, Engine, EventKind, Params, SymbolSpec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const STRATEGY: &str = "sats";

/// Used when a config file has no `candle_sources`: each size from the finest
/// Kite interval that divides it.
pub fn default_candle_sources() -> BTreeMap<String, KiteInterval> {
    [
        (1, KiteInterval::Minute),
        (2, KiteInterval::Minute),
        (3, KiteInterval::ThreeMinute),
        (5, KiteInterval::FiveMinute),
        (6, KiteInterval::ThreeMinute),
        (10, KiteInterval::TenMinute),
        (15, KiteInterval::FifteenMinute),
        (30, KiteInterval::ThirtyMinute),
    ]
    .into_iter()
    .map(|(m, k)| (m.to_string(), k))
    .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SatsConfig {
    /// Must be "sats"; guards against pointing a slot at another strategy's file.
    pub strategy: String,
    /// Candle size the engine runs on, in minutes (also drives preset "Auto").
    /// Must have an entry in `candle_sources`.
    pub bar_minutes: u32,
    /// Candle minutes → Kite historical interval its warm-up/backtest history is
    /// fetched at, e.g. {"2": "minute"} builds 2m candles from 1m history. Kite serves
    /// "minute", "3minute", "5minute", "10minute", "15minute", "30minute". Live bars
    /// are always built from WebSocket ticks at `bar_minutes`.
    #[serde(default = "default_candle_sources")]
    pub candle_sources: BTreeMap<String, KiteInterval>,
    /// Instrument tick (`syminfo.mintick`), e.g. 1.0 for CRUDEOILM.
    pub tick_size: f64,
    pub lots: u32,
    /// Rupees per 1.0 price move for one lot (CRUDEOILM 10).
    pub point_value: f64,
    /// Optional IST window for NEW entries, by signal-bar close time:
    /// {"from": "20:00:00", "to": "23:00:00"} allows entries on bars closing at
    /// or after `from` and before `to`. Exits are never blocked. Omit or null
    /// to allow entries all session.
    #[serde(default)]
    pub entry_window: Option<EntryWindow>,
    /// Optional minimum quality for NEW entries: SATS signal score (0-100) and
    /// TQI (0-1) at the signal bar. 0 = no minimum. Exits are never blocked.
    #[serde(default)]
    pub entry_filter: EntryFilter,
    /// Live-runner settings (calendar, daily square-off, product).
    pub live: LiveSettings,
    pub execution: Execution,
    pub params: Params,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LiveSettings {
    /// Reviewed exchange session calendar JSON (holidays, special sessions).
    pub session_calendar: String,
    /// IST time of the daily square-off; no new entries from this bar onwards.
    pub square_off: chrono::NaiveTime,
    /// Broker product; the production client only accepts MIS.
    pub product: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EntryWindow {
    pub from: chrono::NaiveTime,
    pub to: chrono::NaiveTime,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EntryFilter {
    #[serde(default)]
    pub min_score: f64,
    #[serde(default)]
    pub min_tqi: f64,
}

impl LiveSettings {
    pub fn square_off_minute(&self) -> u32 {
        use chrono::Timelike;
        self.square_off.hour() * 60 + self.square_off.minute()
    }
}

/// How model events map to orders.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    /// "thirds": ⅓ of the lots at TP1, ⅓ at TP2, rest at TP3 (lots must divide by 3).
    /// "single": the whole position exits at `single_exit_at` (or SL / flip / timeout first).
    /// "trail":  no profit target. The stop is checked on every tick; at TP1 it
    ///           moves to breakeven and then trails on the SuperTrend line. Exits
    ///           on the stop, a trend flip, a timeout or the square-off.
    pub exit_mode: ExitMode,
    pub single_exit_at: Target,
    /// `exit_mode: "single"` only: true = the SL and `single_exit_at` target are
    /// checked on every tick and exit immediately when crossed; false = checked
    /// on the closed bar's high/low and exited at the bar close.
    #[serde(default)]
    pub intrabar_exits: bool,
    /// Settings for `exit_mode: "trail"` (ignored otherwise).
    #[serde(default)]
    pub trail: TrailSettings,
    /// Backtest friction per lot per round trip, in price points.
    pub round_trip_cost_points: f64,
    /// Backtest adverse slippage per order, in price points.
    pub slippage_points_per_side: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExitMode {
    Thirds,
    Single,
    Trail,
}

/// Trailing-stop rules for `exit_mode: "trail"`. Levels only ever move in the
/// trade's favour.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrailSettings {
    /// At TP1 the stop moves to breakeven = actual entry fill ± this many points
    /// in the trade's favour (e.g. 2.0 to cover costs; 0.0 = exact entry).
    pub breakeven_offset_points: f64,
    /// After TP1, also trail the stop on the SATS SuperTrend line at each bar
    /// close (the tighter of breakeven and the line is used).
    pub supertrend_trail: bool,
}

impl Default for TrailSettings {
    fn default() -> Self {
        Self { breakeven_offset_points: 0.0, supertrend_trail: true }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Target {
    #[serde(rename = "TP1")]
    Tp1,
    #[serde(rename = "TP2")]
    Tp2,
    #[serde(rename = "TP3")]
    Tp3,
}

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tp1 => "TP1",
            Self::Tp2 => "TP2",
            Self::Tp3 => "TP3",
        }
    }
}

impl Execution {
    /// Price of the `single_exit_at` target for a SATS trade.
    pub fn single_target(&self, t: &sats::TradeSnapshot) -> f64 {
        match self.single_exit_at {
            Target::Tp1 => t.tp1,
            Target::Tp2 => t.tp2,
            Target::Tp3 => t.tp3,
        }
    }

    /// Single mode with SL / target checked on every tick.
    pub fn intrabar_single(&self) -> bool {
        self.exit_mode == ExitMode::Single && self.intrabar_exits
    }

    /// Lots to close for a model exit event, given the lots still open.
    /// Entries are not handled here (they always open `lots`).
    pub fn lots_to_close(&self, kind: EventKind, lots: u32, open_lots: u32) -> u32 {
        if open_lots == 0 {
            return 0;
        }
        let closing = matches!(kind, EventKind::SlHit | EventKind::FlipExit | EventKind::TimeoutExit | EventKind::Tp3Hit);
        match self.exit_mode {
            ExitMode::Thirds => match kind {
                EventKind::Tp1Hit | EventKind::Tp2Hit => (lots / 3).min(open_lots),
                _ if closing => open_lots,
                _ => 0,
            },
            ExitMode::Single => {
                let target = match self.single_exit_at {
                    Target::Tp1 => EventKind::Tp1Hit,
                    Target::Tp2 => EventKind::Tp2Hit,
                    Target::Tp3 => EventKind::Tp3Hit,
                };
                if closing || kind == target { open_lots } else { 0 }
            }
            // No targets: TP hits only move the trailing stop. SATS's SL, flip and
            // timeout still close what is open; its TP3 does not (the position keeps
            // trailing after SATS's own trade has ended).
            ExitMode::Trail => {
                if matches!(kind, EventKind::SlHit | EventKind::FlipExit | EventKind::TimeoutExit) { open_lots } else { 0 }
            }
        }
    }
}

impl SatsConfig {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("Cannot read sats strategy config {path}"))?;
        let config: Self =
            serde_json::from_str(&text).with_context(|| format!("Invalid sats strategy config {path}"))?;
        config.validate().with_context(|| format!("Rejected sats strategy config {path}"))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.strategy == STRATEGY, "strategy must be \"{STRATEGY}\"");
        for (minutes, source) in &self.candle_sources {
            let m: u32 = minutes.parse().map_err(|_| anyhow::anyhow!("candle_sources key \"{minutes}\" is not a whole number of minutes"))?;
            Interval::built_from(m, *source).with_context(|| format!("candle_sources \"{minutes}\""))?;
        }
        self.interval()?;
        ensure!(self.tick_size.is_finite() && self.tick_size > 0.0, "tick_size must be positive");
        ensure!((1..=100).contains(&self.lots), "lots must be 1..100");
        ensure!(self.point_value.is_finite() && self.point_value > 0.0, "point_value must be positive");
        let e = &self.execution;
        ensure!(
            [e.round_trip_cost_points, e.slippage_points_per_side].iter().all(|v| v.is_finite() && *v >= 0.0),
            "execution costs must be non-negative"
        );
        ensure!(
            e.exit_mode != ExitMode::Thirds || self.lots.is_multiple_of(3),
            "exit_mode \"thirds\" needs lots divisible by 3 (use \"single\" for {} lot(s))",
            self.lots
        );
        if e.exit_mode == ExitMode::Trail {
            ensure!(
                e.trail.breakeven_offset_points.is_finite() && e.trail.breakeven_offset_points >= 0.0,
                "trail.breakeven_offset_points must be zero or positive"
            );
        }
        let f = self.entry_filter;
        ensure!((0.0..=100.0).contains(&f.min_score), "entry_filter.min_score must be 0..100");
        ensure!((0.0..=1.0).contains(&f.min_tqi), "entry_filter.min_tqi must be 0..1");
        if let Some(w) = self.entry_window {
            ensure!(w.from < w.to, "entry_window.from must be before entry_window.to");
            ensure!(w.to <= self.live.square_off, "entry_window.to must not be after live.square_off");
        }
        ensure!(self.live.product == "MIS", "live.product must be MIS (intraday)");
        ensure!(
            chrono::Timelike::second(&self.live.square_off) == 0
                && chrono::Timelike::minute(&self.live.square_off).is_multiple_of(self.bar_minutes),
            "live.square_off must fall on a {}-minute bar boundary",
            self.bar_minutes
        );
        self.params.validate().map_err(anyhow::Error::msg)?;
        Ok(())
    }

    /// Candle size plus the Kite history interval it is built from (`candle_sources`).
    pub fn interval(&self) -> Result<Interval> {
        let source = self.candle_sources.get(&self.bar_minutes.to_string()).ok_or_else(|| {
            anyhow::anyhow!(
                "bar_minutes {} has no entry in candle_sources (configured: {})",
                self.bar_minutes,
                self.candle_sources.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
        Interval::built_from(self.bar_minutes, *source)
    }

    /// Whether a signal on the bar closing at `close_time_ns` may open a trade.
    pub fn entries_allowed(&self, close_time_ns: i64) -> bool {
        let Some(w) = self.entry_window else { return true };
        let t = chrono::DateTime::from_timestamp_nanos(close_time_ns)
            .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
            .time();
        w.from <= t && t < w.to
    }

    pub fn bar_ns(&self) -> i64 {
        i64::from(self.bar_minutes) * 60_000_000_000
    }

    pub fn engine(&self) -> Result<Engine> {
        let mut engine = Engine::new(
            self.params.clone(),
            SymbolSpec { tick_size: self.tick_size, bar_minutes: f64::from(self.bar_minutes) },
        )
        .map_err(anyhow::Error::msg)?;
        engine.set_entry_quality(self.entry_filter.min_score, self.entry_filter.min_tqi);
        Ok(engine)
    }

    /// Kite candle (timestamped at its open, IST) -> engine bar.
    pub fn bar(&self, candle: &Candle) -> Result<BarInput> {
        let open = candle
            .time()?
            .timestamp_nanos_opt()
            .ok_or_else(|| anyhow::anyhow!("Candle timestamp overflow"))?;
        Ok(BarInput {
            open_time_ns: open,
            close_time_ns: open + self.bar_ns(),
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: Some(candle.volume as f64),
        })
    }

    /// Engine primed on completed history; events during warm-up are discarded.
    #[allow(dead_code)] // used by the live runner once a slot is wired to a node
    pub fn warm_engine(&self, candles: &[Candle]) -> Result<Engine> {
        let mut engine = self.engine()?;
        for candle in candles {
            let bar = self.bar(candle)?;
            engine.set_entries_enabled(self.entries_allowed(bar.close_time_ns));
            engine.on_bar(&bar);
        }
        Ok(engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SHIPPED: &str = include_str!("../../../../config/sats-crudeoilm.json");

    #[test]
    fn shipped_config_is_valid_and_spells_out_every_param() {
        let c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.validate().unwrap();
        let expected = Params { tp_mode: sats::TpMode::Dynamic, ..Params::default() };
        assert_eq!(c.params, expected, "script defaults except Dynamic TP mode");
        let raw: serde_json::Value = serde_json::from_str(SHIPPED).unwrap();
        let written: Vec<_> = raw["params"].as_object().unwrap().keys().cloned().collect();
        let all = serde_json::to_value(Params::default()).unwrap();
        for key in all.as_object().unwrap().keys() {
            assert!(written.contains(key), "param {key} missing from config/sats-crudeoilm.json");
        }
    }

    fn exec(mode: ExitMode, at: Target) -> Execution {
        Execution {
            exit_mode: mode,
            single_exit_at: at,
            intrabar_exits: false,
            trail: TrailSettings::default(),
            round_trip_cost_points: 0.0,
            slippage_points_per_side: 0.0,
        }
    }

    #[test]
    fn entry_window_gates_by_ist_bar_close() {
        let mut c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.entry_window = None;
        let ist = |h: u32, m: u32| {
            chrono::FixedOffset::east_opt(19_800)
                .unwrap()
                .with_ymd_and_hms(2026, 10, 9, h, m, 0)
                .unwrap()
                .timestamp_nanos_opt()
                .unwrap()
        };
        assert!(c.entries_allowed(ist(10, 0)));
        c.entry_window = Some(EntryWindow {
            from: chrono::NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
            to: chrono::NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
        });
        c.validate().unwrap();
        assert!(!c.entries_allowed(ist(19, 55)));
        assert!(c.entries_allowed(ist(20, 0)));
        assert!(c.entries_allowed(ist(22, 55)));
        assert!(!c.entries_allowed(ist(23, 0)));
        c.entry_window = Some(EntryWindow { from: c.entry_window.unwrap().to, to: c.entry_window.unwrap().from });
        assert!(c.validate().is_err());
    }

    #[test]
    fn trail_mode_targets_only_move_the_stop() {
        let e = exec(ExitMode::Trail, Target::Tp1);
        for kind in [EventKind::Tp1Hit, EventKind::Tp2Hit, EventKind::Tp3Hit] {
            assert_eq!(e.lots_to_close(kind, 1, 1), 0, "{kind:?}");
        }
        for kind in [EventKind::SlHit, EventKind::FlipExit, EventKind::TimeoutExit] {
            assert_eq!(e.lots_to_close(kind, 1, 1), 1, "{kind:?}");
        }
        let mut c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.execution.exit_mode = ExitMode::Trail;
        c.validate().unwrap();
        c.execution.trail.breakeven_offset_points = -1.0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn single_mode_exits_everything_at_the_chosen_target() {
        let e = exec(ExitMode::Single, Target::Tp2);
        assert_eq!(e.lots_to_close(EventKind::Tp1Hit, 1, 1), 0);
        assert_eq!(e.lots_to_close(EventKind::Tp2Hit, 1, 1), 1);
        assert_eq!(e.lots_to_close(EventKind::Tp3Hit, 1, 0), 0, "already flat");
        assert_eq!(e.lots_to_close(EventKind::SlHit, 1, 1), 1);
        assert_eq!(e.lots_to_close(EventKind::FlipExit, 1, 1), 1);
    }

    #[test]
    fn thirds_mode_scales_out() {
        let e = exec(ExitMode::Thirds, Target::Tp3);
        assert_eq!(e.lots_to_close(EventKind::Tp1Hit, 6, 6), 2);
        assert_eq!(e.lots_to_close(EventKind::Tp2Hit, 6, 4), 2);
        assert_eq!(e.lots_to_close(EventKind::Tp3Hit, 6, 2), 2);
        assert_eq!(e.lots_to_close(EventKind::TimeoutExit, 6, 4), 4);
    }

    #[test]
    fn thirds_with_one_lot_is_rejected() {
        let mut c: SatsConfig = serde_json::from_str(SHIPPED).unwrap();
        c.execution.exit_mode = ExitMode::Thirds;
        assert!(c.validate().is_err());
        c.lots = 3;
        c.validate().unwrap();
        c.strategy = "vce-mojo".into();
        assert!(c.validate().is_err());
    }
}
