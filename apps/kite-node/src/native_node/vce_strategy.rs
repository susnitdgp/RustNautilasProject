//! Nautilus strategy for one `vce-mojo` portfolio slot.
//!
//! One instance per portfolio slot: its strategy ID is derived from the slot ID,
//! so several slots (CRUDEOILM, GOLD, NIFTY …) can run side by side with isolated
//! positions. Bars drive entries (bar-close confirmed); quotes drive intrabar
//! SL / target exits, checked at the price an exit would fill at (bid for a long,
//! ask for a short).
//!
//! Orders are submitted only when `orders_enabled` is true. The portfolio
//! validator still rejects `live_orders_enabled`, so today this can run only as
//! a shadow (signals logged) or against a paper/sandbox execution client.
use super::vce_config::VceConfig;
use anyhow::Result;
use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderDenied, OrderFilled, OrderRejected},
    identifiers::ClientId,
    types::Quantity,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use vce_mojo::{Action, BarInput, Engine, Event, Side};

#[derive(Debug)]
pub struct VceStrategy {
    core: StrategyCore,
    instance_id: String,
    bar_type: BarType,
    data_client: Option<ClientId>,
    bar_ns: i64,
    lots: u32,
    engine: Engine,
    orders_enabled: bool,
    /// True only while a position opened by this run is expected. Trades the
    /// engine "opened" during warm-up never reached the broker and are not exited.
    live_trade: bool,
    last_close_ns: i64,
    halted: Option<String>,
}

impl VceStrategy {
    /// `engine` should already be warmed on history (see `VceConfig::warm_engine`).
    pub fn new(instance_id: &str, bar_type: BarType, config: &VceConfig, engine: Engine, orders_enabled: bool) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(format!("VCE-{instance_id}").as_str().into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            instance_id: instance_id.to_owned(),
            bar_type,
            data_client: None,
            bar_ns: config.bar_ns(),
            lots: config.lots,
            engine,
            orders_enabled,
            live_trade: false,
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
        }
    }

    fn position(&self) -> f64 {
        self.cache()
            .positions_open(
                None,
                Some(&self.bar_type.instrument_id()),
                self.strategy_id().as_ref(),
                None,
                None,
            )
            .iter()
            .map(|p| p.signed_qty)
            .sum()
    }

    fn halt(&mut self, reason: &str) {
        eprintln!("VCE {} HALTED: {reason}", self.instance_id);
        self.halted = Some(reason.to_owned());
    }

    fn log(&self, event: &Event, note: &str) {
        let (reason, price, intrabar) = match event {
            Event::Entry { price, .. } => (None, *price, false),
            Event::Exit { reason, price, intrabar, .. } => (Some(reason.label()), *price, *intrabar),
        };
        println!(
            "{}",
            serde_json::json!({
                "event": "vce_signal",
                "instance": self.instance_id,
                "action": event.action().as_str(),
                "reason": reason,
                "price": price,
                "intrabar": intrabar,
                "orders_enabled": self.orders_enabled,
                "note": note,
            })
        );
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        let action = event.action();
        let is_entry = matches!(action, Action::Buy | Action::Short);
        if !is_entry && !self.live_trade {
            self.log(&event, "exit of a trade not opened by this run; ignored");
            return Ok(());
        }
        self.live_trade = is_entry;
        if let Some(reason) = &self.halted {
            let note = format!("halted: {reason}");
            self.log(&event, &note);
            return Ok(());
        }
        if !self.orders_enabled {
            self.log(&event, "shadow");
            return Ok(());
        }
        let position = self.position();
        let consistent = match action {
            Action::Buy | Action::Short => position == 0.0,
            Action::Sell => position > 0.0,
            Action::Cover => position < 0.0,
        };
        if !consistent {
            self.halt(&format!("{} signal with broker position {position}", action.as_str()));
            self.log(&event, "position mismatch");
            return Ok(());
        }
        let (side, reduce_only) = match action {
            Action::Buy => (OrderSide::Buy, false),
            Action::Short => (OrderSide::Sell, false),
            Action::Sell => (OrderSide::Sell, true),
            Action::Cover => (OrderSide::Buy, true),
        };
        let quantity = if reduce_only { position.abs() } else { f64::from(self.lots) };
        let order = self.order().market(
            self.bar_type.instrument_id(),
            side,
            Quantity::new(quantity, 0),
            Some(TimeInForce::Day),
            Some(reduce_only),
            Some(false),
            None,
            None,
            None,
            None,
        );
        self.log(&event, "order submitted");
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}

impl DataActor for VceStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_bars(self.bar_type, self.data_client, None);
        self.subscribe_quotes(self.bar_type.instrument_id(), self.data_client, None);
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
        if self.last_close_ns != 0 && input.close_time_ns != self.last_close_ns + self.bar_ns {
            // Gaps are normal across sessions; only flag intraday ones.
            eprintln!(
                "VCE {}: bar gap {} -> {}",
                self.instance_id, self.last_close_ns, input.close_time_ns
            );
        }
        self.last_close_ns = input.close_time_ns;
        for event in self.engine.on_bar(&input) {
            self.handle(event)?;
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        if quote.instrument_id != self.bar_type.instrument_id() {
            return Ok(());
        }
        let price = match self.engine.position().map(|t| t.side) {
            Some(Side::Long) => quote.bid_price.as_f64(),
            Some(Side::Short) => quote.ask_price.as_f64(),
            None => return Ok(()),
        };
        if let Some(event) = self.engine.on_price(price, quote.ts_event.as_u64() as i64) {
            self.handle(event)?;
        }
        Ok(())
    }
}

nautilus_strategy!(VceStrategy, {
    fn on_order_filled(&mut self, e: &OrderFilled) {
        println!(
            "{}",
            serde_json::json!({
                "event": "vce_fill",
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
