//! Per-bar orchestration in the script's order: base calculations (6), TQI (6.1),
//! adaptive SuperTrend (6.2/6.3), dynamic TP (6.35), pivots and score (6.4),
//! then section 7 (settle the old trade, then open the new one).
use crate::indicators::{Atr, History, Rsi, highest, lowest, pivot_high, pivot_low, sma, stdev};
use crate::learn::Learn;
use crate::params::{Params, Resolved, TpMode};
use crate::quality::{self, ER_HIGH_THRESH, ER_LOW_THRESH, ScoreInputs, TqiInputs, clamp, safe_div};
use crate::supertrend::{AdaptiveSuperTrend, StInputs, StOutput};
use crate::trade::{self, BarCtx, EntryInputs, Event, EventKind, ExitRules, Side, Ticks, TradeSnapshot};
use serde::{Deserialize, Serialize};

/// One confirmed candle. Times are UTC epoch nanoseconds of the bar's open and close.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct BarInput {
    pub open_time_ns: i64,
    pub close_time_ns: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// `None` when the feed has no volume (Pine `na`).
    pub volume: Option<f64>,
}

/// Instrument facts the script reads from the chart (`syminfo.mintick`, timeframe).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct SymbolSpec {
    pub tick_size: f64,
    pub bar_minutes: f64,
}

/// Snapshot for dashboards / logs (dashboard + trader-card fields that matter).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Status {
    pub bars: i64,
    pub warmed_up: bool,
    pub preset: String,
    pub trend: i8,
    pub supertrend: Option<f64>,
    pub tqi: f64,
    pub efficiency: f64,
    pub regime: String,
    pub vol_ratio: f64,
    pub quality_influence: f64,
    pub calibration: String,
    pub next_r: [f64; 3],
    pub trade: Option<TradeSnapshot>,
    pub last_exit: Option<String>,
    pub last_rejected: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Engine {
    p: Params,
    r: Resolved,
    spec: SymbolSpec,
    bars: i64,
    // history (newest first)
    open: History<f64>,
    high: History<f64>,
    low: History<f64>,
    close: History<f64>,
    src: History<f64>,
    volume: History<Option<f64>>,
    raw_atr_hist: History<Option<f64>>,
    rsi_hist: History<Option<f64>>,
    atr: Atr,
    rsi: Rsi,
    st: AdaptiveSuperTrend,
    learn: Learn,
    last_pivot_high: Option<f64>,
    last_pivot_low: Option<f64>,
    last_pivot_high_bar: Option<i64>,
    last_pivot_low_bar: Option<i64>,
    trade: Option<TradeSnapshot>,
    status: Status,
    /// Host-controlled entry gate (e.g. a trading-hours window). When false a
    /// flip still closes the open trade but opens no new one. Not a Pine input.
    #[serde(default = "entries_on")]
    entries_enabled: bool,
}

fn entries_on() -> bool {
    true
}

impl Engine {
    pub fn new(p: Params, spec: SymbolSpec) -> Result<Self, String> {
        p.validate()?;
        if !(spec.tick_size.is_finite() && spec.tick_size > 0.0) {
            return Err("tick_size must be positive".into());
        }
        if !(spec.bar_minutes.is_finite() && spec.bar_minutes > 0.0) {
            return Err("bar_minutes must be positive".into());
        }
        let r = p.resolve(spec.bar_minutes);
        let price_cap = [
            p.structure_window,
            2 * p.pivot_strength + 1,
            p.momentum_window + 1,
            r.er_len + 1,
            p.char_flip_min_age.max(3) as usize + 1,
            4,
        ]
        .into_iter()
        .max()
        .unwrap_or(4)
            + 1;
        Ok(Self {
            open: History::new(price_cap),
            high: History::new(price_cap),
            low: History::new(price_cap),
            close: History::new(price_cap),
            src: History::new(price_cap),
            volume: History::new(p.volume_z_window + 1),
            raw_atr_hist: History::new(p.atr_baseline_length + 1),
            rsi_hist: History::new(p.rsi_memory_bars + 1),
            atr: Atr::new(r.atr_len),
            rsi: Rsi::new(r.rsi_len),
            st: AdaptiveSuperTrend::new(&p),
            learn: Learn::new(&p),
            last_pivot_high: None,
            last_pivot_low: None,
            last_pivot_high_bar: None,
            last_pivot_low_bar: None,
            trade: None,
            status: Status { preset: format!("{:?}", r.preset), ..Status::default() },
            entries_enabled: true,
            bars: 0,
            p,
            r,
            spec,
        })
    }

    pub fn params(&self) -> &Params {
        &self.p
    }
    pub fn resolved(&self) -> &Resolved {
        &self.r
    }
    pub fn status(&self) -> &Status {
        &self.status
    }
    pub fn position(&self) -> Option<&TradeSnapshot> {
        self.trade.as_ref()
    }
    /// Enables or disables new entries from the next `on_bar` on (exits unaffected).
    pub fn set_entries_enabled(&mut self, enabled: bool) {
        self.entries_enabled = enabled;
    }

    /// Processes one confirmed bar; returns the bar's events in the script's
    /// order (old-trade exits first, then a new entry).
    pub fn on_bar(&mut self, bar: &BarInput) -> Vec<Event> {
        let p = self.p.clone();
        let r = self.r;
        let idx = self.bars;
        self.bars += 1;
        let (o, h, l, c) = (bar.open, bar.high, bar.low, bar.close);
        let src = p.source.of(o, h, l, c);
        self.open.push(o);
        self.high.push(h);
        self.low.push(l);
        self.close.push(c);
        self.src.push(src);
        self.volume.push(bar.volume);

        // ── 6 · base calculations ──
        let raw_atr = self.atr.update(h, l, c);
        self.raw_atr_hist.push(raw_atr);
        let atr_baseline = sma(&self.raw_atr_hist, p.atr_baseline_length);
        let rsi_val = self.rsi.update(c);
        self.rsi_hist.push(rsi_val);
        let vol_ratio = safe_div(raw_atr, atr_baseline, 1.0);
        let rsi_ready = !p.score_use_rsi || matches!(self.rsi_hist.get(p.rsi_memory_bars - 1), Some(Some(_)));
        let mut warmed = idx >= r.warmup_bars
            && raw_atr.is_some_and(|a| a > 0.0)
            && atr_baseline.is_some_and(|b| b > 0.0)
            && self.src.get(r.er_len).is_some()
            && rsi_ready
            && idx >= r.er_len as i64;
        let er = quality::efficiency_ratio(&self.src, r.er_len);
        let atr_value = raw_atr.map(|a| if p.efficiency_weighted_atr { a * (0.5 + 0.5 * er) } else { a });

        // ── 6.1 · TQI ──
        let vn = p.volume_z_window;
        let valid_count = (self.volume.len() >= vn).then(|| self.volume.last_n(vn).filter(Option::is_some).count());
        let vol_std = stdev(&self.volume, vn);
        let vol_mean = sma(&self.volume, vn);
        let vol_z_raw = safe_div(bar.volume.zip(vol_mean).map(|(v, m)| v - m), vol_std, 0.0);
        let has_volume = valid_count == Some(vn) && vol_std.is_some_and(|s| s > 0.0);
        let mn = p.momentum_window;
        let (up_moves, down_moves) = if self.close.len() >= mn {
            let step = |k: usize| self.close.get(k).zip(self.close.get(k + 1));
            (
                (0..mn).filter(|&k| step(k).is_some_and(|(a, b)| a > b)).count() as f64,
                (0..mn).filter(|&k| step(k).is_some_and(|(a, b)| a < b)).count() as f64,
            )
        } else {
            (0.0, 0.0)
        };
        let tqi = quality::tqi(
            &p,
            &TqiInputs {
                er,
                vol_ratio,
                has_volume,
                vol_z_raw,
                struct_hi: highest(&self.high, p.structure_window),
                struct_lo: lowest(&self.low, p.structure_window),
                close: c,
                window_change: self.close.get(mn).map(|then| c - then),
                up_moves,
                down_moves,
            },
        );
        warmed = warmed && (!p.use_tqi || tqi.weight_sum > 0.0);

        // ── 6.2 / 6.3 · adaptive SuperTrend ──
        self.learn.on_bar(&p);
        let window = p.char_flip_min_age.max(3) as usize;
        let st: StOutput = self.st.update(
            &p,
            &StInputs {
                bar_index: idx,
                src,
                close: c,
                close_prev: self.close.get(1),
                close_window: self.close.get(window),
                atr_value,
                er,
                tqi: tqi.value,
                eff_quality: self.learn.eff_q,
                base_mult: r.base_mult,
            },
        );

        // ── 6.35 · dynamic TP R-multiples ──
        let dynamic = p.tp_mode == TpMode::Dynamic;
        let scale = if dynamic { quality::dyn_tp_scale(&p, tqi.value, vol_ratio) } else { 1.0 };
        let ceil = p.dyn_tp_ceiling_r;
        let base1 = r.fixed_tp1_r.max(0.01);
        let floors = [
            p.dyn_tp1_floor_r.min(ceil),
            (p.dyn_tp1_floor_r * (r.fixed_tp2_r / base1)).min(ceil),
            (p.dyn_tp1_floor_r * (r.fixed_tp3_r / base1)).min(ceil),
        ];
        let fixed = [r.fixed_tp1_r, r.fixed_tp2_r, r.fixed_tp3_r];
        let eff: [f64; 3] = std::array::from_fn(|k| if dynamic { clamp(fixed[k] * scale, floors[k], ceil) } else { fixed[k] });
        let lo = eff[0].min(eff[1].min(eff[2]));
        let hi = eff[0].max(eff[1].max(eff[2]));
        let live_r = [lo, eff[0] + eff[1] + eff[2] - lo - hi, hi];

        // ── 6.4 · pivots and score ──
        let ps = p.pivot_strength;
        if let Some(ph) = pivot_high(&self.high, ps) {
            self.last_pivot_high = Some(ph);
            self.last_pivot_high_bar = Some(idx - ps as i64);
        }
        if let Some(pl) = pivot_low(&self.low, ps) {
            self.last_pivot_low = Some(pl);
            self.last_pivot_low_bar = Some(idx - ps as i64);
        }
        let fresh = |b: Option<i64>| b.is_some_and(|b| idx - b <= p.max_pivot_age_bars);
        let valid_low = fresh(self.last_pivot_low_bar) && self.last_pivot_low.is_some_and(|v| v < c);
        let valid_high = fresh(self.last_pivot_high_bar) && self.last_pivot_high.is_some_and(|v| v > c);
        let rsi_window = |f: fn(f64, f64) -> f64, init: f64| {
            (self.rsi_hist.len() >= p.rsi_memory_bars)
                .then(|| self.rsi_hist.last_n(p.rsi_memory_bars).try_fold(init, |a, v| v.map(|v| f(a, v))))
                .flatten()
        };
        let (score, _available) = quality::score(
            &p,
            &ScoreInputs {
                is_buy: st.trend == 1,
                close: c,
                close_3: self.close.get(3),
                atr_value,
                er,
                has_volume,
                vol_z: if has_volume { vol_z_raw } else { 0.0 },
                rsi_lo: rsi_window(f64::min, f64::INFINITY),
                rsi_hi: rsi_window(f64::max, f64::NEG_INFINITY),
                last_pivot_low: self.last_pivot_low,
                last_pivot_high: self.last_pivot_high,
                valid_low_pivot: valid_low,
                valid_high_pivot: valid_high,
                upper_band_prev: st.upper_band_prev,
                lower_band_prev: st.lower_band_prev,
            },
        );

        // ── 7 · settle the old trade, then open the new one ──
        let tick = Ticks(self.spec.tick_size);
        let slip = f64::from(p.slippage_ticks) * self.spec.tick_size;
        let calculation_epoch = self.learn.epoch;
        let raw_buy = st.flip_up && warmed;
        let raw_sell = st.flip_down && warmed;
        let mut events = Vec::new();
        if let Some(t) = self.trade.as_mut() {
            let ctx = BarCtx { bar_index: idx, time_ns: bar.close_time_ns, open: o, high: h, low: l, close: c, trend: st.trend };
            let rules = ExitRules { timeout_bars: p.trade_timeout_bars, slip, fee_pct: p.commission_pct_per_fill };
            let (evs, closed) = trade::settle(t, &ctx, &rules);
            events.extend(evs);
            if let Some(net) = closed {
                let epoch = t.epoch;
                self.status.last_exit = events.last().map(|e| format!("{} {:+.2}R", e.kind.label(), net));
                self.learn.record(&p, net, epoch);
                self.trade = None;
            }
        }
        if (raw_buy || raw_sell) && self.trade.is_none() && !self.entries_enabled {
            self.status.last_rejected = Some("Entry skipped: outside the entry window".into());
        } else if (raw_buy || raw_sell) && self.trade.is_none() {
            let side = if raw_buy { Side::Long } else { Side::Short };
            match atr_value {
                Some(atr_v) => {
                    let plan = trade::plan(&EntryInputs {
                        side,
                        close: c,
                        low: l,
                        high: h,
                        atr_value: atr_v,
                        sl_mult: r.sl_mult,
                        max_sl_dist: p.max_sl_distance_atr,
                        last_pivot_low: self.last_pivot_low,
                        last_pivot_high: self.last_pivot_high,
                        valid_low_pivot: valid_low,
                        valid_high_pivot: valid_high,
                        live_r,
                        dyn_floors: dynamic.then_some(floors),
                        dyn_ceiling: ceil,
                        min_risk_ticks: p.min_risk_ticks,
                        slip,
                        fee_pct: p.commission_pct_per_fill,
                        ticks: tick,
                    });
                    match plan {
                        Ok(pl) => {
                            let (via_price, via_quality) = if side == Side::Long {
                                (st.price_flip_up, st.char_flip_up)
                            } else {
                                (st.price_flip_down, st.char_flip_down)
                            };
                            let reason = match (via_price, via_quality) {
                                (true, true) => "Price break + quality collapse",
                                (false, true) => "Quality collapse",
                                _ => "Price band break",
                            };
                            let cap = p.max_sl_distance_atr.max(r.sl_mult) * atr_v;
                            let limitations = format!(
                                "{}{}{}",
                                if has_volume { "" } else { "Volume unavailable/flat; " },
                                if pl.pivot_usable { "" } else { "No recent valid pivot; " },
                                if pl.risk >= cap { "SL capped/rounded; " } else { "" }
                            );
                            let snap = TradeSnapshot {
                                side,
                                entry_bar: idx,
                                entry_time_ns: bar.close_time_ns,
                                signal_price: c,
                                entry: pl.entry,
                                sl: pl.sl,
                                risk: pl.risk,
                                tp1: pl.tp[0],
                                tp2: pl.tp[1],
                                tp3: pl.tp[2],
                                r1: pl.r[0],
                                r2: pl.r[1],
                                r3: pl.r[2],
                                hit1: false,
                                hit2: false,
                                hit3: false,
                                taken_r: 0.0,
                                cost_r: pl.cost_r,
                                remaining: 1.0,
                                entry_tqi: tqi.value,
                                entry_score: score,
                                reason: reason.into(),
                                limitations,
                                epoch: calculation_epoch,
                            };
                            events.push(Event {
                                kind: if side == Side::Long { EventKind::Buy } else { EventKind::Sell },
                                bar_index: idx,
                                time_ns: bar.close_time_ns,
                                level: c,
                                fill: pl.entry,
                                fraction: 1.0,
                                closes_trade: false,
                                trade: snap.clone(),
                            });
                            self.trade = Some(snap);
                            self.status.last_rejected = None;
                        }
                        Err(why) => self.status.last_rejected = Some(why),
                    }
                }
                None => self.status.last_rejected = Some("Rejected: ATR unavailable".into()),
            }
        } else if raw_buy || raw_sell {
            self.status.last_rejected = Some("Entry skipped: model position is still active".into());
        }

        // ── status ──
        let s = &mut self.status;
        s.bars = self.bars;
        s.warmed_up = warmed;
        s.trend = st.trend;
        s.supertrend = st.line();
        s.tqi = tqi.value;
        s.efficiency = er;
        s.regime = if er >= ER_HIGH_THRESH {
            "Trending"
        } else if er >= ER_LOW_THRESH {
            "Mixed"
        } else {
            "Choppy"
        }
        .into();
        s.vol_ratio = vol_ratio;
        s.quality_influence = self.learn.eff_q;
        s.calibration = if p.auto_calibration { self.learn.status.clone() } else { "Off".into() };
        s.next_r = live_r;
        s.trade = self.trade.clone();
        events
    }
}
