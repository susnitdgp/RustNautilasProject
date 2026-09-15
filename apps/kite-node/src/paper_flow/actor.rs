use anyhow::{Result, ensure};
use kite_paper::events;
use kite_strategy::{
    config::Config,
    crossover::{Signal, Strategy as Crossover},
};
use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::QuoteTick,
    enums::OrderSide,
    events::{OrderCanceled, OrderDenied, OrderFilled, OrderRejected},
    identifiers::ClientOrderId,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
pub struct CrossoverActor {
    pub core: StrategyCore,
    pub full_mode: bool,
    pub full_ticks: u64,
    pub last_full_tick: Option<kite_adapter::data::full_tick::KiteFullTick>,
    pub logic: Crossover,
    pub decision: Option<(Signal, QuoteTick)>,
    pub pending: Option<(ClientOrderId, Signal, u32)>,
    pub quotes: u64,
    pub signals: u64,
    pub fills: u64,
    pub cancels: u64,
    pub denied: u64,
    pub fatal: Option<String>,
    pub enabled: bool,
}
impl std::fmt::Debug for CrossoverActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrossoverActor").finish_non_exhaustive()
    }
}
impl CrossoverActor {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("CROSSOVER-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            full_mode: false,
            full_ticks: 0,
            last_full_tick: None,
            logic: Crossover::new(config)?,
            decision: None,
            pending: None,
            quotes: 0,
            signals: 0,
            fills: 0,
            cancels: 0,
            denied: 0,
            fatal: None,
            enabled: false,
        })
    }
    pub fn dispatch(&mut self, ts: nautilus_core::UnixNanos) -> Result<()> {
        let (signal, q) = self
            .decision
            .take()
            .ok_or_else(|| anyhow::anyhow!("Missing native decision"))?;
        ensure!(
            self.enabled && self.pending.is_none(),
            "Native strategy is blocked"
        );
        self.signals += 1;
        let side = if matches!(signal, Signal::Buy) {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };
        let price = if side == OrderSide::Buy {
            q.ask_price
        } else {
            q.bid_price
        };
        let id = ClientOrderId::from(format!("A{}", self.signals).as_str());
        let mut o = crate::native_paper_command::order(id.as_str(), side, price, ts);
        // Bind the paper broker account before native risk checks; venue lookup uses MCX,
        // while this broker account's issuer is KITE.
        if let nautilus_model::orders::OrderAny::Limit(order) = &mut o {
            order.account_id = Some(events::account_id());
        }
        self.pending = Some((id, signal, 0));
        self.submit_order(o, None, Some(events::client_id()), None)
    }
    pub fn gap(&mut self) {
        self.enabled = false;
        self.decision = None;
        self.logic.gap();
    }
}
impl CrossoverActor {
    fn process_quote(&mut self, q: &QuoteTick) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.quotes += 1;
        match self.logic.on_quote(q) {
            Ok(Some(signal)) => {
                self.decision = Some((signal, *q));
            }
            Ok(None) => {}
            Err(_) => {
                self.fatal = Some("Native crossover quote failed".into());
            }
        }
        Ok(())
    }
}
impl DataActor for CrossoverActor {
    fn on_quote(&mut self, q: &QuoteTick) -> Result<()> {
        if self.full_mode {
            return Ok(());
        }
        self.process_quote(q)
    }
    fn on_data(&mut self, data: &nautilus_model::data::CustomData) -> Result<()> {
        if let Some(full) = data
            .data
            .as_any()
            .downcast_ref::<kite_adapter::data::full_tick::KiteFullTick>()
        {
            if !self.enabled {
                return Ok(());
            }
            self.full_ticks += 1;
            self.last_full_tick = Some(full.clone());
            self.process_quote(&full.quote)?;
        }
        Ok(())
    }
}
nautilus_strategy!(CrossoverActor, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        match self.pending.take() {
            Some((id, signal, _)) if id == event.client_order_id => {
                if self.logic.filled(signal).is_err() {
                    self.fatal = Some("Native fill state mismatch".into());
                }
                self.fills += 1;
            }
            _ => self.fatal = Some("Unexpected native strategy fill".into()),
        }
    }
    fn on_order_canceled(&mut self, event: &OrderCanceled) {
        if self.pending.is_some_and(|p| p.0 == event.client_order_id) {
            self.pending = None;
            self.logic.cancelled();
            self.cancels += 1;
        }
    }
    fn on_order_denied(&mut self, event: OrderDenied) {
        eprintln!("Paper risk denial: {}", event.reason);
        self.pending = None;
        self.decision = None;
        self.logic.cancelled();
        self.denied += 1;
    }
    fn on_order_rejected(&mut self, _: OrderRejected) {
        self.pending = None;
        self.logic.cancelled();
        self.fatal = Some("Paper order rejected".into());
    }
});
