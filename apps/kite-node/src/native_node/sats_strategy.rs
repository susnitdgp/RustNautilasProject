//! Nautilus strategy for one `sats` portfolio slot.
//!
//! One instance per slot (strategy ID `SATS-<slot>`), so several slots can run
//! side by side with isolated positions. SATS is bar-close only: every model
//! event (BUY / SELL entries, TP1–3, SL, flip, timeout) is confirmed when a bar
//! closes, and is turned into a market order sized by the slot's `execution`
//! settings — the same thing a once-per-bar-close alert to AlgoMojo does.
//!
//! Orders are submitted only when `orders_enabled` is true. The portfolio
//! validator still rejects `live_orders_enabled`, so today this can run only as
//! a shadow (signals logged) or against a paper/sandbox execution client.
use super::sats_config::{Execution, SatsConfig};
use anyhow::Result;
use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::{Bar, BarType},
    enums::{OrderSide, TimeInForce},
    events::{OrderDenied, OrderFilled, OrderRejected},
    identifiers::ClientId,
    types::Quantity,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use sats::{BarInput, Engine, Event, Side};

#[derive(Debug)]
pub struct SatsStrategy {
    core: StrategyCore,
    instance_id: String,
    bar_type: BarType,
    data_client: Option<ClientId>,
    bar_ns: i64,
    lots: u32,
    execution: Execution,
    engine: Engine,
    orders_enabled: bool,
    /// Lots this run believes are open for the current model trade. Trades the
    /// engine opened during warm-up never reached the broker: their exits are ignored.
    open_lots: u32,
    /// Entry bar of the model trade this run actually opened.
    live_entry_bar: Option<i64>,
    last_close_ns: i64,
    halted: Option<String>,
}

impl SatsStrategy {
    /// `engine` should already be warmed on history (see `SatsConfig::warm_engine`).
    pub fn new(instance_id: &str, bar_type: BarType, config: &SatsConfig, engine: Engine, orders_enabled: bool) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(format!("SATS-{instance_id}").as_str().into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            instance_id: instance_id.to_owned(),
            bar_type,
            data_client: None,
            bar_ns: config.bar_ns(),
            lots: config.lots,
            execution: config.execution,
            engine,
            orders_enabled,
            open_lots: 0,
            live_entry_bar: None,
            last_close_ns: 0,
            halted: None,
        }
    }

    pub fn with_data_client(mut self, client: ClientId) -> Self {
        self.data_client = Some(client);
        self
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn halted(&self) -> Option<&str> {
        self.halted.as_deref()
    }

    fn bar_input(&self, bar: &Bar) -> BarInput {
        let close = bar.ts_event.as_u64() as i64;
        BarInput {
            open_time_ns: close - self.bar_ns,
            close_time_ns: close,
            open: bar.open.as_f64(),
            high: bar.high.as_f64(),
            low: bar.low.as_f64(),
            close: bar.close.as_f64(),
            volume: Some(bar.volume.as_f64()),
        }
    }

    fn position(&self) -> f64 {
        self.cache()
            .positions_open(None, Some(&self.bar_type.instrument_id()), self.strategy_id().as_ref(), None, None)
            .iter()
            .map(|p| p.signed_qty)
            .sum()
    }

    fn halt(&mut self, reason: &str) {
        eprintln!("SATS {} HALTED: {reason}", self.instance_id);
        self.halted = Some(reason.to_owned());
    }

    fn log(&self, ev: &Event, lots: u32, note: &str) {
        println!(
            "{}",
            serde_json::json!({
                "event": "sats_signal",
                "instance": self.instance_id,
                "kind": ev.kind,
                "side": if ev.trade.side == Side::Long { "long" } else { "short" },
                "level": ev.level,
                "model_fill": ev.fill,
                "lots": lots,
                "entry": ev.trade.entry, "sl": ev.trade.sl,
                "tp": [ev.trade.tp1, ev.trade.tp2, ev.trade.tp3],
                "score": ev.trade.entry_score, "tqi": ev.trade.entry_tqi,
                "orders_enabled": self.orders_enabled,
                "note": note,
            })
        );
    }

    fn handle(&mut self, ev: Event) -> Result<()> {
        let (side, lots, reduce_only) = if ev.kind.is_entry() {
            self.live_entry_bar = Some(ev.trade.entry_bar);
            self.open_lots = self.lots;
            let side = if ev.trade.side == Side::Long { OrderSide::Buy } else { OrderSide::Sell };
            (side, self.lots, false)
        } else {
            if self.live_entry_bar != Some(ev.trade.entry_bar) {
                self.log(&ev, 0, "exit of a trade opened during warm-up; ignored");
                return Ok(());
            }
            let n = self.execution.lots_to_close(ev.kind, self.lots, self.open_lots);
            self.open_lots -= n;
            if ev.closes_trade {
                self.live_entry_bar = None;
            }
            if n == 0 {
                self.log(&ev, 0, "model event; no order in this execution mode");
                return Ok(());
            }
            let side = if ev.trade.side == Side::Long { OrderSide::Sell } else { OrderSide::Buy };
            (side, n, true)
        };
        if let Some(reason) = &self.halted {
            let note = format!("halted: {reason}");
            self.log(&ev, lots, &note);
            return Ok(());
        }
        if !self.orders_enabled {
            self.log(&ev, lots, "shadow");
            return Ok(());
        }
        let position = self.position();
        let consistent = if reduce_only {
            (side == OrderSide::Sell && position >= f64::from(lots)) || (side == OrderSide::Buy && -position >= f64::from(lots))
        } else {
            position == 0.0
        };
        if !consistent {
            self.halt(&format!("{:?} for {lots} lot(s) with broker position {position}", ev.kind));
            self.log(&ev, lots, "position mismatch");
            return Ok(());
        }
        let order = self.order().market(
            self.bar_type.instrument_id(),
            side,
            Quantity::new(f64::from(lots), 0),
            Some(TimeInForce::Day),
            Some(reduce_only),
            Some(false),
            None,
            None,
            None,
            None,
        );
        self.log(&ev, lots, "order submitted");
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}

impl DataActor for SatsStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_bars(self.bar_type, self.data_client, None);
        Ok(())
    }

    fn on_bar(&mut self, bar: &Bar) -> Result<()> {
        if bar.bar_type != self.bar_type {
            return Ok(());
        }
        let input = self.bar_input(bar);
        if input.close_time_ns <= self.last_close_ns {
            return Ok(()); // repeated or out-of-order bar; already processed
        }
        self.last_close_ns = input.close_time_ns;
        for ev in self.engine.on_bar(&input) {
            self.handle(ev)?;
        }
        Ok(())
    }
}

nautilus_strategy!(SatsStrategy, {
    fn on_order_filled(&mut self, e: &OrderFilled) {
        println!(
            "{}",
            serde_json::json!({
                "event": "sats_fill",
                "instance": self.instance_id,
                "client_order_id": e.client_order_id.to_string(),
                "side": format!("{:?}", e.order_side),
                "qty": e.last_qty.as_f64(),
                "price": e.last_px.as_f64(),
            })
        );
    }
    fn on_order_rejected(&mut self, e: OrderRejected) {
        self.halt(&format!("order rejected: {}", e.reason));
    }
    fn on_order_denied(&mut self, e: OrderDenied) {
        self.halt(&format!("order denied: {}", e.reason));
    }
});
