//! Native lifecycle/order adapter around the user strategy.
use super::{status::FeedStatus, strategy::UserStrategy};
use anyhow::Result;
use kite_adapter::data::full_tick::KiteFullTick;
use kite_strategy::config::Config;
use nautilus_common::{
    actor::{DataActor, DataActorNative},
    cache::Cache,
};
use nautilus_model::{
    data::{CustomData, DataType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderCanceled, OrderDenied, OrderFilled, OrderRejected},
    identifiers::{ClientOrderId, InstrumentId},
    orders::Order,
    types::Quantity,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Default, Debug)]
pub struct State {
    pub cache: Option<Rc<RefCell<Cache>>>,
    pub ticks: u64,
    pub signals: u64,
    pub fills: u64,
    pub cancelled: u64,
    pub denied: u64,
    pub started: bool,
    pub stopped: bool,
    pub position: f64,
    pub entries: u32,
    pub errors: Vec<String>,
    pub rejected: std::collections::BTreeMap<String, u64>,
    pub last_full: Option<KiteFullTick>,
    pub history: Vec<KiteFullTick>,
}
pub struct NativeStrategy {
    core: StrategyCore,
    config: Config,
    policy: UserStrategy,
    instrument: InstrumentId,
    full: bool,
    state: Rc<RefCell<State>>,
    done: Arc<AtomicBool>,
    pending: Option<ClientOrderId>,
    last_source: u64,
    last_received: u64,
    generation: u32,
    enabled: bool,
}
impl std::fmt::Debug for NativeStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeStrategy").finish_non_exhaustive()
    }
}
impl NativeStrategy {
    pub fn new(
        config: Config,
        instrument: InstrumentId,
        full: bool,
        state: Rc<RefCell<State>>,
        done: Arc<AtomicBool>,
    ) -> Self {
        let policy = UserStrategy::new(&config);
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("CROSSOVER-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            config,
            policy,
            instrument,
            full,
            state,
            done,
            pending: None,
            last_source: 0,
            last_received: 0,
            generation: 0,
            enabled: true,
        }
    }
    fn reject(&mut self, reason: &str) {
        *self
            .state
            .borrow_mut()
            .rejected
            .entry(reason.into())
            .or_default() += 1;
        self.policy.reset();
    }
    fn position(&self) -> f64 {
        self.cache()
            .positions_open(
                None,
                Some(&self.instrument),
                self.strategy_id().as_ref(),
                None,
                None,
            )
            .iter()
            .map(|p| p.signed_qty)
            .sum()
    }
    fn process(&mut self, q: &QuoteTick, full: Option<&KiteFullTick>) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let now = self.clock().timestamp_ns().as_u64();
        if let Some(reason) = crate::paper_flow::diagnostics::quote_reason(
            q,
            &self.config,
            self.last_source,
            self.last_received,
            now,
        ) {
            self.reject(reason);
            if let Some(id) = self.pending {
                self.cancel_order(id, None, None)?;
            }
            return Ok(());
        }
        self.last_source = q.ts_event.as_u64();
        self.last_received = q.ts_init.as_u64();
        let position = self.position();
        {
            let mut state = self.state.borrow_mut();
            state.ticks += 1;
            state.position = position;
        }
        let signal = match full {
            Some(tick) => self.policy.on_full_tick(tick, position)?,
            None => self.policy.on_quote(q, position)?,
        };
        if self.pending.is_some() {
            return Ok(());
        }
        let Some(side) = signal else {
            return Ok(());
        };
        if side == OrderSide::Buy && self.state.borrow().entries >= self.config.max_entries {
            return Ok(());
        }
        if !((side == OrderSide::Buy && position == 0.0)
            || (side == OrderSide::Sell && position == 1.0))
        {
            self.reject("position_limit");
            return Ok(());
        }
        let price = if side == OrderSide::Buy {
            q.ask_price
        } else {
            q.bid_price
        };
        let order = self.order().limit(
            self.instrument,
            side,
            Quantity::from(1),
            price,
            Some(TimeInForce::Day),
            None,
            None,
            Some(side == OrderSide::Sell),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        self.pending = Some(order.client_order_id());
        self.state.borrow_mut().signals += 1;
        if side == OrderSide::Buy {
            self.state.borrow_mut().entries += 1;
        }
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}
impl DataActor for NativeStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.state.borrow_mut().started = true;
        self.state.borrow_mut().cache = Some(DataActorNative::cache_rc(self));
        if self.full {
            self.subscribe_data(
                DataType::new("KiteFeedStatus", None, None),
                Some("KITE".into()),
                None,
            );
            self.subscribe_data(
                DataType::new("KiteFullTick", None, None),
                Some("KITE".into()),
                None,
            );
        } else {
            self.subscribe_quotes(self.instrument, None, None);
        }
        Ok(())
    }
    fn on_stop(&mut self) -> Result<()> {
        self.enabled = false;
        self.cancel_all_orders(self.instrument, None, None, true, None)?;
        self.state.borrow_mut().stopped = true;
        Ok(())
    }
    fn on_quote(&mut self, q: &QuoteTick) -> Result<()> {
        if !self.full {
            self.process(q, None)?;
        }
        Ok(())
    }
    fn on_data(&mut self, data: &CustomData) -> Result<()> {
        if let Some(status) = data.data.as_any().downcast_ref::<FeedStatus>() {
            match status.kind.as_str() {
                "connected" => {
                    self.generation = status.generation;
                    self.enabled = true;
                    self.policy.reset();
                }
                "gap" => {
                    self.enabled = false;
                    self.policy.reset();
                    self.cancel_all_orders(self.instrument, None, None, true, None)?;
                }
                "complete" => {
                    self.enabled = false;
                    self.done.store(true, Ordering::Release);
                }
                "failed" => {
                    self.enabled = false;
                    self.state
                        .borrow_mut()
                        .errors
                        .push("Kite feed failed".into());
                    self.done.store(true, Ordering::Release);
                }
                other => self.reject(other),
            }
        }
        if let Some(full) = data.data.as_any().downcast_ref::<KiteFullTick>() {
            self.state.borrow_mut().last_full = Some(full.clone());
            self.state.borrow_mut().history.push(full.clone());
            if full.snapshot.connection_generation != self.generation {
                self.reject("obsolete_generation");
                return Ok(());
            }
            self.process(&full.quote, Some(full))?;
        }
        Ok(())
    }
    fn on_save(&self) -> Result<indexmap::IndexMap<String, Vec<u8>>> {
        let s = self.state.borrow();
        let value = serde_json::json!({"ticks":s.ticks,"signals":s.signals,"fills":s.fills,"entries":s.entries,"automatic_resume_enabled":false});
        Ok(indexmap::IndexMap::from([(
            "summary".into(),
            serde_json::to_vec(&value)?,
        )]))
    }
}
nautilus_strategy!(NativeStrategy, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        if self.pending == Some(event.client_order_id) {
            self.pending = None;
        }
        self.state.borrow_mut().fills += 1;
    }
    fn on_order_canceled(&mut self, event: &OrderCanceled) {
        if self.pending == Some(event.client_order_id) {
            self.pending = None;
        }
        self.state.borrow_mut().cancelled += 1;
    }
    fn on_order_denied(&mut self, _: OrderDenied) {
        self.pending = None;
        self.state.borrow_mut().denied += 1;
    }
    fn on_order_rejected(&mut self, _: OrderRejected) {
        self.pending = None;
        self.state
            .borrow_mut()
            .errors
            .push("Native order rejected".into());
    }
});
