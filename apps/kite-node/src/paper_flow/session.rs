use super::diagnostics::Diagnostics;
use super::{actor::CrossoverActor, risk::Controls};
use crate::{native_paper_command as native, runtime::core::Core};
use anyhow::{Result, anyhow, ensure};
use kite_adapter::{
    data::full_tick::KiteFullTick, mapping::market_data::Snapshot as MarketSnapshot,
};
use kite_paper::{
    client::PaperExecutionClient,
    events,
    worker::{Handle, Request},
};
use kite_strategy::{checkpoint::Snapshot, config::Config};
use nautilus_common::msgbus::ShareableMessageHandler;
use nautilus_common::{
    actor::{
        DataActor,
        registry::{deregister_actor, try_get_actor_unchecked},
    },
    component::{Component, deregister_component, register_component_actor},
    messages::execution::TradingCommand,
    msgbus::{self, MessagingSwitchboard, TypedHandler, TypedIntoHandler},
};
use nautilus_core::UnixNanos;
use nautilus_execution::engine::ExecutionEngine;
use nautilus_model::data::CustomData;
use nautilus_model::orders::Order;
use nautilus_model::{
    data::QuoteTick, enums::OrderStatus, events::OrderEventAny, instruments::FuturesContract,
};
use nautilus_portfolio::portfolio::Portfolio;
use nautilus_risk::engine::{RiskEngine, config::RiskEngineConfig};
use nautilus_trading::strategy::Strategy;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};
pub fn with_actor<T>(f: impl FnOnce(&mut CrossoverActor) -> T) -> T {
    let mut actor = try_get_actor_unchecked::<CrossoverActor>(&"CROSSOVER-001".into())
        .expect("paper actor registered");
    f(&mut actor)
}
pub struct Session {
    pub core: Core,
    instrument: FuturesContract,
    pub expected_token: Option<u32>,
    pub diagnostics: Diagnostics,
    latest_full: Option<MarketSnapshot>,
    full_handler: ShareableMessageHandler,
    execution: Rc<RefCell<ExecutionEngine>>,
    risk: RiskEngine,
    pub controls: Controls,
    worker: Handle,
    config: Config,
    commands: Rc<RefCell<VecDeque<TradingCommand>>>,
    dispatch: Rc<RefCell<VecDeque<TradingCommand>>>,
    order_events: Rc<RefCell<VecDeque<OrderEventAny>>>,
    overflow: Rc<Cell<bool>>,
    quote_handler: TypedHandler<QuoteTick>,
    order_handler: TypedHandler<OrderEventAny>,
    clock: Rc<RefCell<nautilus_common::clock::TestClock>>,
    transport_connected: bool,
    stopped: bool,
    pub realtime: bool,
    pub(super) match_after_ns: u64,
}
fn queue_handler(
    queue: Rc<RefCell<VecDeque<TradingCommand>>>,
    overflow: Rc<Cell<bool>>,
) -> TypedIntoHandler<TradingCommand> {
    TypedIntoHandler::from(move |cmd: TradingCommand| {
        let mut q = queue.borrow_mut();
        if q.len() >= 16 {
            overflow.set(true);
        } else {
            q.push_back(cmd);
        }
    })
}
impl Session {
    #[cfg(test)]
    pub fn native_notional_limit(
        &mut self,
        id: nautilus_model::identifiers::InstrumentId,
        limit: rust_decimal::Decimal,
    ) {
        self.risk.set_max_notional_per_order(id, limit);
    }
    pub fn new(
        url: &str,
        namespace: &str,
        config: Config,
        instrument: &FuturesContract,
        ts: UnixNanos,
    ) -> Result<Self> {
        let worker = Handle::start(url.to_owned(), namespace.to_owned(), config.clone())?;
        let clock = Rc::new(RefCell::new(nautilus_common::clock::TestClock::new()));
        clock.borrow_mut().advance_time(ts, true);
        let core = Core::with_clock(instrument, clock.clone());
        let account = native::account(ts);
        core.cache.borrow_mut().add_account(account.clone())?;
        let portfolio = Portfolio::new(core.clock.clone(), core.cache.clone(), None);
        let mut risk = RiskEngine::new(
            RiskEngineConfig::default(),
            portfolio.clone_shallow(),
            core.clock.clone(),
            core.cache.clone(),
        );
        risk.set_max_notional_per_order(instrument.id, rust_decimal::Decimal::from(2_000_000));
        risk.start();
        let execution = Rc::new(RefCell::new(ExecutionEngine::new(
            core.clock.clone(),
            core.cache.clone(),
            None,
        )));
        execution
            .borrow_mut()
            .register_client(Box::new(PaperExecutionClient::new(&worker, account)))?;
        ExecutionEngine::register_msgbus_handlers(&execution);
        execution.borrow_mut().start();
        let mut actor = CrossoverActor::new(config.clone())?;
        actor.core.register(
            msgbus::get_message_bus().borrow().trader_id,
            core.clock.clone(),
            core.cache.clone(),
            Rc::new(RefCell::new(portfolio)),
        )?;
        actor.initialize()?;
        actor.start()?;
        register_component_actor(actor);
        let commands = Rc::new(RefCell::new(VecDeque::new()));
        let dispatch = Rc::new(RefCell::new(VecDeque::new()));
        let overflow = Rc::new(Cell::new(false));
        msgbus::register_trading_command_endpoint(
            MessagingSwitchboard::risk_engine_queue_execute(),
            queue_handler(commands.clone(), overflow.clone()),
        );
        msgbus::register_trading_command_endpoint(
            MessagingSwitchboard::exec_engine_queue_execute(),
            queue_handler(dispatch.clone(), overflow.clone()),
        );
        msgbus::register_trading_command_endpoint(
            MessagingSwitchboard::exec_engine_execute(),
            queue_handler(dispatch.clone(), overflow.clone()),
        );
        let quote_handler = TypedHandler::from(move |q: &QuoteTick| {
            with_actor(|a| a.handle_quote(q));
        });
        msgbus::subscribe_quotes(
            "data.quotes.MCX.CRUDEOIL26SEPFUT".into(),
            quote_handler.clone(),
            None,
        );
        let full_handler = ShareableMessageHandler::from_typed(move |data: &CustomData| {
            if data.data.type_name() == "KiteFullTick" {
                with_actor(|a| a.handle_data(data));
            }
        });
        msgbus::subscribe_any("data.KiteFullTick*".into(), full_handler.clone(), None);
        let order_events = Rc::new(RefCell::new(VecDeque::new()));
        let events_queue = order_events.clone();
        let events_overflow = overflow.clone();
        let order_handler = TypedHandler::from(move |event: &OrderEventAny| {
            let mut queue = events_queue.borrow_mut();
            if queue.len() >= 64 {
                events_overflow.set(true);
            } else {
                queue.push_back(event.clone());
            }
        });
        msgbus::subscribe_order_events(
            "events.order.CROSSOVER-001".into(),
            order_handler.clone(),
            None,
        );
        Ok(Self {
            core,
            instrument: instrument.clone(),
            expected_token: None,
            diagnostics: Diagnostics::default(),
            latest_full: None,
            full_handler,
            execution,
            risk,
            controls: Controls::default(),
            worker,
            config,
            commands,
            dispatch,
            order_events,
            overflow,
            quote_handler,
            order_handler,
            clock,
            transport_connected: false,
            stopped: false,
            realtime: false,
            match_after_ns: 0,
        })
    }
    fn check(&self) -> Result<()> {
        ensure!(!self.overflow.get(), "Native paper queue overflow");
        ensure!(
            with_actor(|a| a.fatal.is_none()),
            "Native strategy callback failed"
        );
        Ok(())
    }
    fn callbacks(&mut self) -> Result<()> {
        loop {
            let event = self.order_events.borrow_mut().pop_front();
            let Some(event) = event else { break };
            if let OrderEventAny::Denied(denied) = &event {
                let init = self
                    .core
                    .cache
                    .borrow()
                    .order(&denied.client_order_id)
                    .map(|o| o.init_event().clone())
                    .ok_or_else(|| anyhow!("Denied order missing"))?;
                self.worker.send(Request::ExternalEvents(vec![
                    OrderEventAny::Initialized(init),
                    event.clone(),
                ]))?;
                self.worker.receive()?;
            }
            with_actor(|a| a.handle_order_event(event));
        }
        self.check()
    }
    fn checkpoint(&self, finished: bool) -> Result<()> {
        let checkpoint_started = std::time::Instant::now();
        let snapshot = with_actor(|a| Snapshot {
            version: 1,
            cursor: a.quotes as usize,
            strategy: a.logic.clone(),
            finished,
        });
        self.worker.send(Request::Checkpoint(snapshot))?;
        self.worker.receive()?;
        self.worker.send(Request::Control(serde_json::json!({"version":1,"generation":self.controls.generation,"connected":self.controls.connected,"gaps":self.controls.gaps,"blocked":self.controls.blocked,"diagnostics":self.diagnostics.json(),"last_full_tick":self.latest_full,"finished":finished,"resume_allowed":false,"open_contracts":with_actor(|a|a.logic.position)})))?;
        self.worker.receive()?;
        let elapsed = checkpoint_started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        self.diagnostics
            .max_checkpoint_ms
            .set(self.diagnostics.max_checkpoint_ms.get().max(elapsed));
        Ok(())
    }
    pub fn connected(&mut self, generation: u64) -> Result<()> {
        self.controls.connect(generation)?;
        self.diagnostics.connections += 1;
        self.transport_connected = true;
        with_actor(|a| {
            a.logic.gap();
            a.enabled = true;
        });
        self.checkpoint(false)
    }
    pub fn gap(&mut self) -> Result<()> {
        self.diagnostics.socket_gaps += 1;
        self.transport_connected = false;
        self.suspend()
    }
    fn suspend(&mut self) -> Result<()> {
        if !self.controls.connected {
            return Ok(());
        }
        self.controls.gap();
        with_actor(|a| a.gap());
        self.checkpoint(false)?;
        if with_actor(|a| a.pending.is_some()) {
            self.cancel_pending()?;
            self.checkpoint(false)?;
        }
        Ok(())
    }
    fn cancel_pending(&mut self) -> Result<()> {
        let id = with_actor(|a| a.pending.map(|p| p.0));
        if let Some(id) = id {
            with_actor(|a| a.cancel_order(id, Some(events::client_id()), None))?;
            self.drain()?;
        }
        Ok(())
    }

    fn reject_update(&mut self, reason: &str) -> Result<()> {
        self.diagnostics.reject(reason);
        self.controls.blocked += 1;
        if self.controls.connected {
            self.diagnostics.quality_suspensions += 1;
            self.suspend()?;
        }
        Ok(())
    }
    pub fn full(&mut self, snapshot: MarketSnapshot, now: u64) -> Result<()> {
        self.diagnostics.received_packets += 1;
        if self.expected_token != Some(snapshot.instrument_token)
            || snapshot
                .raw
                .as_ref()
                .is_some_and(|t| t.instrument_token != snapshot.instrument_token)
        {
            return self.reject_update("wrong_instrument_token");
        }
        let received = u64::try_from(
            snapshot
                .received_at_utc
                .timestamp_nanos_opt()
                .ok_or_else(|| anyhow!("Invalid receive timestamp"))?,
        )?;
        self.diagnostics.observe(
            snapshot
                .exchange_timestamp
                .map(|s| u64::from(s) * 1_000_000_000),
            received,
            now,
        );
        let complete = snapshot
            .raw
            .as_ref()
            .is_some_and(|t| t.full.is_some() && t.quote_fields.is_some());
        if !complete {
            return self.reject_update("incomplete_full_packet");
        }
        self.diagnostics.full_packets += 1;
        self.latest_full = Some(snapshot.clone());
        if snapshot.exchange_timestamp.is_none() {
            return self.reject_update("missing_exchange_timestamp");
        }
        if !snapshot.source_fresh {
            return self.reject_update("adapter_source_not_fresh");
        }
        if snapshot.bid_size == Some(0) || snapshot.ask_size == Some(0) {
            return self.reject_update("insufficient_top_depth");
        }
        if snapshot.bid.is_none() || snapshot.ask.is_none() {
            return self.reject_update("missing_top_depth");
        }
        let generation = u64::from(snapshot.connection_generation);
        let mapped = kite_adapter::mapping::quotes::map(&snapshot, &self.instrument);
        let q = match mapped {
            Ok(Some(q)) => q,
            Ok(None) => return self.reject_update("missing_usable_top_quote"),
            Err(_) => return self.reject_update("quote_mapping_failed"),
        };
        let full = KiteFullTick { snapshot, quote: q };
        self.quote_input(q, generation, now, Some(full))
    }
    #[cfg(test)]
    pub fn quote(&mut self, q: QuoteTick, generation: u64, now: u64) -> Result<()> {
        self.diagnostics
            .observe(Some(q.ts_event.as_u64()), q.ts_init.as_u64(), now);
        self.quote_input(q, generation, now, None)
    }
    fn quote_input(
        &mut self,
        q: QuoteTick,
        generation: u64,
        now: u64,
        full: Option<KiteFullTick>,
    ) -> Result<()> {
        if generation != self.controls.generation {
            self.diagnostics.reject("obsolete_generation");
            self.controls.blocked += 1;
            return Ok(());
        }
        if !self.transport_connected {
            self.diagnostics.reject("disconnected");
            self.controls.blocked += 1;
            return Ok(());
        }
        if let Some(reason) = super::diagnostics::quote_reason(
            &q,
            &self.config,
            self.controls.last_source,
            self.controls.last_received,
            now,
        ) {
            return self.reject_update(reason);
        }
        if !self.controls.connected {
            self.controls.connected = true;
            with_actor(|a| {
                a.logic.gap();
                a.enabled = true;
            });
            self.checkpoint(false)?;
        }
        if with_actor(|a| a.pending.is_some()) && q.ts_init.as_u64() <= self.match_after_ns {
            self.diagnostics.buffered_before_acceptance += 1;
            return Ok(());
        }
        self.advance(now)?;
        if let Err(reason) = self.controls.quote(&q, generation, now, &self.config) {
            return self.reject_update(reason);
        }
        // Match existing orders before the actor sees this quote. A new order cannot fill on its signal quote.
        if with_actor(|a| a.pending.is_some()) {
            self.worker.send(Request::Quote(q))?;
            let batch = self.worker.receive()?;
            let changed = !batch.is_empty();
            native::apply(&self.core, &mut self.execution.borrow_mut(), batch)?;
            self.callbacks()?;
            if changed {
                self.checkpoint(false)?;
            }
        }
        if full.is_some() {
            with_actor(|a| a.full_mode = true);
        }
        self.core.quote(q);
        if let Some(full) = full {
            self.core
                .engine
                .process(&CustomData::from_arc(std::sync::Arc::new(full)));
        }
        self.check()?;
        if with_actor(|a| a.pending.is_some()) {
            let expired = with_actor(|a| {
                let p = a.pending.as_mut().unwrap();
                p.2 += 1;
                p.2 >= 3 || a.logic.mids.is_empty()
            });
            if expired {
                self.cancel_pending()?;
            }
        }
        if with_actor(|a| a.decision.is_some()) {
            self.checkpoint(false)?;
            let ts = if self.realtime {
                u64::try_from(
                    chrono::Utc::now()
                        .timestamp_nanos_opt()
                        .ok_or_else(|| anyhow!("Clock out of range"))?,
                )?
            } else {
                q.ts_init.as_u64()
            };
            with_actor(|a| a.dispatch(ts.into()))?;
            self.drain()?;
            self.checkpoint(false)?;
        }
        Ok(())
    }
    pub fn advance(&mut self, now: u64) -> Result<()> {
        use nautilus_common::clock::Clock;
        let current = self.clock.borrow().timestamp_ns().as_u64();
        let events = self
            .clock
            .borrow_mut()
            .advance_time(now.max(current).into(), true);
        let handlers = self.clock.borrow().match_handlers(events);
        for handler in handlers {
            handler.run();
        }
        self.check()
    }
    pub fn stale(&mut self) -> Result<()> {
        self.diagnostics.watchdog_timeouts += 1;
        if self.controls.connected {
            self.diagnostics.quality_suspensions += 1;
        }
        self.suspend()
    }
    pub fn shutdown(&mut self) -> Result<()> {
        self.transport_connected = false;
        self.suspend()
    }
    fn drain(&mut self) -> Result<()> {
        loop {
            let cmd = self.commands.borrow_mut().pop_front();
            if cmd.is_none() && self.dispatch.borrow().is_empty() {
                break;
            }
            if let Some(cmd) = cmd {
                if let TradingCommand::SubmitOrder(s) = &cmd {
                    let now = if self.realtime {
                        u64::try_from(
                            chrono::Utc::now()
                                .timestamp_nanos_opt()
                                .ok_or_else(|| anyhow!("Clock out of range"))?,
                        )?
                    } else {
                        self.controls.last_received
                    };
                    ensure!(
                        self.core
                            .cache
                            .borrow()
                            .order(&s.client_order_id)
                            .and_then(|o| o.account_id())
                            == Some(events::account_id()),
                        "Paper risk account binding missing"
                    );
                    self.controls.order(
                        &s.order_init,
                        with_actor(|a| a.logic.position),
                        now,
                        &self.config,
                    )?;
                }
                self.risk.execute(cmd);
                self.callbacks()?;
            }
            loop {
                let cmd = self.dispatch.borrow_mut().pop_front();
                let Some(cmd) = cmd else { break };
                let id = match &cmd {
                    TradingCommand::SubmitOrder(s) => s.client_order_id,
                    TradingCommand::CancelOrder(c) => c.client_order_id,
                    _ => return Err(anyhow!("Unsupported native paper command")),
                };
                self.execution.borrow().execute(cmd);
                // Client guards may deny synchronously; never wait for a nonexistent worker request.
                if self
                    .core
                    .cache
                    .borrow()
                    .order(&id)
                    .is_some_and(|o| o.status() == OrderStatus::Denied)
                {
                    return Err(anyhow!("Native paper client denied command"));
                }
                let batch = self.worker.receive()?;
                if batch
                    .iter()
                    .any(|e| matches!(e, OrderEventAny::Accepted(_)))
                {
                    self.match_after_ns = if self.realtime {
                        u64::try_from(
                            chrono::Utc::now()
                                .timestamp_nanos_opt()
                                .ok_or_else(|| anyhow!("Clock out of range"))?,
                        )?
                    } else {
                        self.controls.last_received
                    };
                }
                native::apply(&self.core, &mut self.execution.borrow_mut(), batch)?;
                self.callbacks()?;
            }
        }
        self.check()
    }
    pub fn finish(&mut self) -> Result<serde_json::Value> {
        self.controls.connected = false;
        with_actor(|a| a.enabled = false);
        self.cancel_pending()?;
        self.checkpoint(true)?;
        let result = with_actor(
            |a| serde_json::json!({"event":"kite_paper_flow_complete","native_strategy_actor":true,"native_risk_engine":true,"quotes":a.quotes,"full_ticks":a.full_ticks,"diagnostics":self.diagnostics.json(),"last_full_tick":self.latest_full,"signals":a.signals,"paper_fills":a.fills,"cancelled":a.cancels,"denied":a.denied,"open_contracts":a.logic.position,"gaps":self.controls.gaps,"risk_blocked":self.controls.blocked,"live_orders_enabled":false,"broker_orders_accessed":false,"automatic_resume_enabled":false}),
        );
        let native_net: f64 = self
            .core
            .cache
            .borrow()
            .positions_open(None, None, None, None, None)
            .iter()
            .map(|p| p.signed_qty)
            .sum();
        ensure!(
            native_net == with_actor(|a| f64::from(a.logic.position)),
            "Native position mismatch"
        );
        self.stop();
        Ok(result)
    }
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        with_actor(|a| {
            let _ = Component::stop(a);
        });
        self.execution.borrow_mut().stop();
        self.risk.stop();
        msgbus::unsubscribe_quotes(
            "data.quotes.MCX.CRUDEOIL26SEPFUT".into(),
            &self.quote_handler,
        );
        msgbus::unsubscribe_any("data.KiteFullTick*".into(), &self.full_handler);
        msgbus::unsubscribe_order_events("events.order.CROSSOVER-001".into(), &self.order_handler);
        deregister_actor(&"CROSSOVER-001".into());
        deregister_component(&"CROSSOVER-001".into());
        self.stopped = true;
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}
