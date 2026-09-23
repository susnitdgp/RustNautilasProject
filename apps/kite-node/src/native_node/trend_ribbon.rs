// Pine-compatible Trend Ribbon signal engine: ALMA + deviation bands + ATR-normalized slope.
use super::{pivot_session::BAR_NS, session_calendar::Calendar};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub alma_length: usize,
    pub alma_offset: f64,
    pub alma_sigma: f64,
    pub deviation_length: usize,
    pub deviation_multiplier: f64,
    pub slope_length: usize,
    pub minimum_slope: f64,
    pub atr_length: usize,
    pub session: super::pivot_session::Session,
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.alma_length >= 5, "ALMA length must be at least 5");
        ensure!(
            (0.0..=1.0).contains(&self.alma_offset),
            "ALMA offset must be 0..1"
        );
        ensure!(
            self.alma_sigma.is_finite() && self.alma_sigma >= 1.0,
            "ALMA sigma must be >= 1"
        );
        ensure!(
            self.deviation_length >= 5,
            "Deviation length must be at least 5"
        );
        ensure!(
            self.deviation_multiplier.is_finite() && self.deviation_multiplier > 0.0,
            "Deviation multiplier must be positive"
        );
        ensure!(
            (1..=10).contains(&self.slope_length),
            "Slope length must be 1..10"
        );
        ensure!(
            self.minimum_slope.is_finite() && self.minimum_slope >= 0.0,
            "Minimum slope must be non-negative"
        );
        ensure!(self.atr_length >= 5, "ATR length must be at least 5");
        self.session.validate()
    }
}
#[derive(Debug)]
struct Atr {
    period: usize,
    count: usize,
    sum: f64,
    previous_close: Option<f64>,
    value: Option<f64>,
}
impl Atr {
    fn new(period: usize) -> Self {
        Self {
            period,
            count: 0,
            sum: 0.0,
            previous_close: None,
            value: None,
        }
    }
    fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let tr = self.previous_close.map_or(high - low, |p| {
            (high - low).max((high - p).abs()).max((low - p).abs())
        });
        self.previous_close = Some(close);
        self.value = match self.value {
            Some(v) => Some((v * (self.period - 1) as f64 + tr) / self.period as f64),
            None => {
                self.count += 1;
                self.sum += tr;
                (self.count == self.period).then(|| self.sum / self.period as f64)
            }
        };
        self.value
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub bar_close_ns: u64,
    pub close: f64,
    pub alma: Option<f64>,
    pub deviation: Option<f64>,
    pub atr: Option<f64>,
    pub slope_score: Option<f64>,
    pub upper_confirm: Option<f64>,
    pub lower_confirm: Option<f64>,
    pub direction: i8,
    pub signal: i8,
    pub in_session: bool,
    pub new_session: bool,
    pub initialized: bool,
}
#[derive(Debug)]
pub struct TrendRibbon {
    pub settings: Settings,
    calendar: Calendar,
    closes: VecDeque<f64>,
    almas: VecDeque<f64>,
    atr: Atr,
    trend: i8,
    session_date: Option<chrono::NaiveDate>,
    last_bar: u64,
    bar_ns: u64,
}
impl TrendRibbon {
    #[cfg(test)]
    pub fn new(settings: Settings, calendar: Calendar) -> Result<Self> {
        Self::new_for_interval(settings, calendar, BAR_NS)
    }
    pub fn new_for_interval(settings: Settings, calendar: Calendar, bar_ns: u64) -> Result<Self> {
        settings.validate()?;
        calendar.validate()?;
        ensure!(
            matches!(bar_ns, 180_000_000_000 | BAR_NS),
            "Trend Ribbon interval must be three or five minutes"
        );
        Ok(Self {
            atr: Atr::new(settings.atr_length),
            settings,
            calendar,
            closes: VecDeque::new(),
            almas: VecDeque::new(),
            trend: 0,
            session_date: None,
            last_bar: 0,
            bar_ns,
        })
    }
    pub fn rebuild_empty(&self) -> Result<Self> {
        Self::new_for_interval(self.settings.clone(), self.calendar.clone(), self.bar_ns)
    }
    pub fn in_session(&self, ns: u64) -> Result<bool> {
        self.settings.session.contains(ns, &self.calendar)
    }
    fn alma(&self) -> Option<f64> {
        let n = self.settings.alma_length;
        if self.closes.len() < n {
            return None;
        }
        let m = self.settings.alma_offset * (n - 1) as f64;
        let s = n as f64 / self.settings.alma_sigma;
        let start = self.closes.len() - n;
        let mut num = 0.;
        let mut den = 0.;
        for (i, x) in self.closes.iter().skip(start).enumerate() {
            let w = (-((i as f64 - m).powi(2)) / (2. * s * s)).exp();
            num += x * w;
            den += w
        }
        Some(num / den)
    }
    fn deviation(&self) -> Option<f64> {
        let n = self.settings.deviation_length;
        if self.closes.len() < n {
            return None;
        }
        let xs = self.closes.iter().skip(self.closes.len() - n);
        let mean = xs.clone().sum::<f64>() / n as f64;
        Some((xs.map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt())
    }
    pub fn update(
        &mut self,
        high: f64,
        low: f64,
        close: f64,
        bar_close_ns: u64,
    ) -> Result<Observation> {
        ensure!(
            bar_close_ns >= self.bar_ns
                && bar_close_ns > self.last_bar
                && bar_close_ns.is_multiple_of(self.bar_ns),
            "Trend Ribbon bars must be ordered completed configured-interval candles"
        );
        ensure!(
            [high, low, close].iter().all(|x| x.is_finite() && *x > 0.)
                && high >= close
                && close >= low,
            "Invalid Trend Ribbon OHLC"
        );
        let open_ns = bar_close_ns - self.bar_ns;
        let inside = self.in_session(open_ns)?;
        let day = super::pivot_session::date(open_ns);
        let new_session = inside && self.session_date != Some(day);
        if new_session {
            self.session_date = Some(day)
        }
        self.last_bar = bar_close_ns;
        let atr = self.atr.update(high, low, close);
        self.closes.push_back(close);
        let keep = self
            .settings
            .alma_length
            .max(self.settings.deviation_length)
            + self.settings.slope_length
            + 2;
        while self.closes.len() > keep {
            self.closes.pop_front();
        }
        let alma = self.alma();
        if let Some(v) = alma {
            self.almas.push_back(v);
            while self.almas.len() > self.settings.slope_length + 1 {
                self.almas.pop_front();
            }
        }
        let deviation = self.deviation();
        let slope_score = match (atr, alma) {
            (Some(a), Some(v)) if a > 0. && self.almas.len() > self.settings.slope_length => {
                Some((v - self.almas[self.almas.len() - 1 - self.settings.slope_length]) / a)
            }
            _ => None,
        };
        let upper_confirm = alma
            .zip(deviation)
            .map(|(a, d)| a + d * self.settings.deviation_multiplier);
        let lower_confirm = alma
            .zip(deviation)
            .map(|(a, d)| a - d * self.settings.deviation_multiplier);
        let previous = self.trend;
        let bull = inside
            && slope_score.is_some_and(|s| s > self.settings.minimum_slope)
            && upper_confirm.is_some_and(|u| close > u);
        let bear = inside
            && slope_score.is_some_and(|s| s < -self.settings.minimum_slope)
            && lower_confirm.is_some_and(|l| close < l);
        if self.trend != 1 && bull {
            self.trend = 1
        } else if self.trend != -1 && bear {
            self.trend = -1
        }
        let entry_window = inside && self.in_session(bar_close_ns)?;
        let signal = if entry_window && self.trend != previous {
            self.trend
        } else {
            0
        };
        Ok(Observation {
            bar_close_ns,
            close,
            alma,
            deviation,
            atr,
            slope_score,
            upper_confirm,
            lower_confirm,
            direction: self.trend,
            signal,
            in_session: entry_window,
            new_session,
            initialized: alma.is_some()
                && deviation.is_some()
                && atr.is_some()
                && slope_score.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ts(s: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap() as u64
    }
    fn settings() -> Settings {
        serde_json::from_value(serde_json::json!({"alma_length":34,"alma_offset":0.85,"alma_sigma":6.0,"deviation_length":34,"deviation_multiplier":0.65,"slope_length":3,"minimum_slope":0.08,"atr_length":14,"session":{"start":"09:00:00","end":"23:15:00","days":"23456","reset_daily":false}})).unwrap()
    }
    #[test]
    fn validates_pine_defaults() {
        settings().validate().unwrap()
    }
    #[test]
    fn flips_both_directions_and_rebuild_is_deterministic() {
        let c = super::super::session_calendar::fixture();
        let mut a = TrendRibbon::new(settings(), c.clone()).unwrap();
        let mut rows = vec![];
        let start = ts("2026-09-22T09:05:00+05:30");
        for i in 0..150u64 {
            let x = i % 50;
            let p = if x < 25 {
                6000. + x as f64 * 8.
            } else {
                6200. - (x - 25) as f64 * 8.
            };
            rows.push((p + 3., p - 3., p, start + i * BAR_NS));
        }
        let expected: Vec<_> = rows
            .iter()
            .map(|&(h, l, c, t)| serde_json::to_value(a.update(h, l, c, t).unwrap()).unwrap())
            .collect();
        assert!(expected.iter().any(|v| v["signal"] == 1));
        assert!(expected.iter().any(|v| v["signal"] == -1));
        let mut b = a.rebuild_empty().unwrap();
        for (row, want) in rows.into_iter().zip(expected) {
            assert_eq!(
                serde_json::to_value(b.update(row.0, row.1, row.2, row.3).unwrap()).unwrap(),
                want
            )
        }
    }
    #[test]
    fn session_gate_prevents_late_entry() {
        let a = TrendRibbon::new(settings(), super::super::session_calendar::fixture()).unwrap();
        assert!(!a.in_session(ts("2026-09-22T23:15:00+05:30")).unwrap())
    }
    #[test]
    fn three_minute_engine_uses_configured_bar_step() {
        let mut engine = TrendRibbon::new_for_interval(
            settings(),
            super::super::session_calendar::fixture(),
            180_000_000_000,
        )
        .unwrap();
        let first = ts("2026-09-22T09:03:00+05:30");
        engine.update(101., 99., 100., first).unwrap();
        assert!(
            engine
                .update(101., 99., 100., first + 180_000_000_000)
                .is_ok()
        );
        assert!(
            engine
                .update(101., 99., 100., first + 300_000_000_000)
                .is_err()
        );
        assert_eq!(engine.rebuild_empty().unwrap().bar_ns, 180_000_000_000);
    }
    #[test]
    fn real_kite_sep18_21_22_matches_reviewed_tradingview_flip_times() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            candles: Vec<kite_adapter::http::historical::Candle>,
        }
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../tests/fixtures/trend_ribbon_sep18_21_22.json"
        ))
        .unwrap();
        let calendar: super::super::session_calendar::Calendar =
            serde_json::from_value(serde_json::json!({
                "timezone":"Asia/Kolkata","valid_from":"2026-08-17","valid_through":"2026-10-19",
                "regular":{"open":"09:00:00","close":"23:30:00"},"overrides":{}
            }))
            .unwrap();
        let mut engine = TrendRibbon::new(settings(), calendar).unwrap();
        let mut actual = std::collections::BTreeMap::<String, Vec<(String, i8)>>::new();
        for candle in fixture.candles {
            let open = candle.time().unwrap();
            let close_ns = (open.timestamp_nanos_opt().unwrap() as u64) + BAR_NS;
            let observation = engine
                .update(candle.high, candle.low, candle.close, close_ns)
                .unwrap();
            let date = open.date_naive().to_string();
            if observation.signal != 0
                && matches!(date.as_str(), "2026-09-18" | "2026-09-21" | "2026-09-22")
            {
                actual
                    .entry(date)
                    .or_default()
                    .push((open.format("%H:%M").to_string(), observation.signal));
            }
        }
        let expected = std::collections::BTreeMap::from([
            (
                "2026-09-18".into(),
                vec![
                    ("09:05".into(), -1),
                    ("13:35".into(), 1),
                    ("14:45".into(), -1),
                    ("15:05".into(), 1),
                    ("16:10".into(), -1),
                    ("16:55".into(), 1),
                    ("20:20".into(), -1),
                ],
            ),
            (
                "2026-09-21".into(),
                vec![
                    ("14:30".into(), 1),
                    ("15:20".into(), -1),
                    ("17:00".into(), 1),
                    ("18:10".into(), -1),
                    ("23:00".into(), 1),
                ],
            ),
            (
                "2026-09-22".into(),
                vec![
                    ("12:35".into(), -1),
                    ("16:45".into(), 1),
                    ("18:40".into(), -1),
                    ("19:15".into(), 1),
                    ("22:20".into(), -1),
                ],
            ),
        ]);
        assert_eq!(actual, expected);
    }
}
