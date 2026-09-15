use crate::{events, outbox::Outbox};
use anyhow::{Result, anyhow, ensure};
use kite_execution::{
    coordinator,
    mock::{MockBroker, Outcome},
    rate_limit::{Limiter, policy::Policy},
};
use kite_journal::{
    model::{Event, Intent, Product, Side},
    store::Journal,
};
use kite_strategy::checkpoint::{Snapshot, Store};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    enums::OrderSide,
    events::{OrderEventAny, OrderInitialized},
    identifiers::VenueOrderId,
};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub enum Request {
    Submit(Box<OrderInitialized>),
    Quote(QuoteTick),
    Cancel(nautilus_model::identifiers::ClientOrderId, UnixNanos),
    Checkpoint(Snapshot),
}
pub struct Handle {
    pub(crate) tx: SyncSender<Request>,
    rx: Receiver<Result<Vec<OrderEventAny>>>,
    pub(crate) alive: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}
struct Pending {
    init: OrderInitialized,
    venue: VenueOrderId,
}
impl Handle {
    pub fn start(
        url: String,
        namespace: String,
        config: kite_strategy::config::Config,
    ) -> Result<Self> {
        config.validate()?;
        let (tx, input) = mpsc::sync_channel(16);
        let (output, rx) = mpsc::sync_channel(16);
        let (ready, initialized) = mpsc::sync_channel(1);
        let alive = Arc::new(AtomicBool::new(true));
        let running = alive.clone();
        let join = thread::Builder::new()
            .name("kite-paper-redis".into())
            .spawn(move || {
                let state = (|| {
                    Ok::<_, anyhow::Error>((
                        Journal::create_at(&url, &namespace)?,
                        Limiter::create_at(&url, &namespace, Policy::default())?,
                        Outbox::create_at(&url, &namespace)?,
                        Store::create_at(&url, &namespace)?,
                    ))
                })();
                let (mut journal, mut limiter, mut outbox, mut checkpoint) = match state {
                    Ok(s) => {
                        let _ = ready.send(Ok(()));
                        s
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        running.store(false, Ordering::Release);
                        return;
                    }
                };
                let mut pending: Option<Pending> = None;
                let mut last_quote_ts = 0;
                while running.load(Ordering::Acquire) {
                    let request = match input.recv_timeout(Duration::from_millis(100)) {
                        Ok(r) => r,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    };
                    let result = (|| -> Result<Vec<OrderEventAny>> {
                        let mut batch = vec![];
                        match request {
                            Request::Submit(init) => {
                                crate::validation::order(&init)?;
                                let net: i64 = journal
                                    .state()
                                    .orders()
                                    .map(|o| {
                                        if matches!(o.intent.side, Side::Buy) {
                                            i64::from(o.filled)
                                        } else {
                                            -i64::from(o.filled)
                                        }
                                    })
                                    .sum();
                                ensure!(
                                    (init.order_side == OrderSide::Buy && net == 0)
                                        || (init.order_side == OrderSide::Sell && net == 1),
                                    "Paper supports one long contract only"
                                );
                                ensure!(pending.is_none(), "Only one paper order may be pending");
                                let id = init.client_order_id.to_string();
                                let venue = VenueOrderId::from(format!("P{id}").as_str());
                                let limit = (init
                                    .price
                                    .ok_or_else(|| anyhow!("Missing paper limit"))?
                                    .as_decimal()
                                    * Decimal::from(100))
                                .to_i64()
                                .ok_or_else(|| anyhow!("Invalid paper limit"))?;
                                journal.append(Event::Intent {
                                    intent: Intent {
                                        id: id.clone(),
                                        symbol: "CRUDEOIL26SEPFUT".into(),
                                        side: if init.order_side == OrderSide::Buy {
                                            Side::Buy
                                        } else {
                                            Side::Sell
                                        },
                                        product: Product::Nrml,
                                        quantity: 1,
                                        limit_price_paise: limit,
                                    },
                                })?;
                                let mut mock =
                                    MockBroker::new(Outcome::Accepted(venue.to_string()));
                                coordinator::submit(&mut journal, &mut mock, &id, &mut limiter)?;
                                ensure!(
                                    journal
                                        .state()
                                        .orders()
                                        .any(|o| o.intent.id == id && o.broker_id.is_some()),
                                    "Paper budget deferred; review prepared intent"
                                );
                                batch.push(OrderEventAny::Initialized((*init).clone()));
                                batch.extend(events::accepted(&init, venue, init.ts_init));
                                pending = Some(Pending { init: *init, venue });
                            }
                            Request::Quote(q) => {
                                crate::validation::quote(&q, &config, last_quote_ts)?;
                                last_quote_ts = q.ts_event.as_u64();
                                if let Some(p) = &pending {
                                    ensure!(
                                        q.instrument_id == p.init.instrument_id,
                                        "Paper instrument mismatch"
                                    );
                                    let price = if p.init.order_side == OrderSide::Buy {
                                        q.ask_price
                                    } else {
                                        q.bid_price
                                    };
                                    let limit =
                                        p.init.price.ok_or_else(|| anyhow!("Missing limit"))?;
                                    if q.ts_event > p.init.ts_init
                                        && ((p.init.order_side == OrderSide::Buy && price <= limit)
                                            || (p.init.order_side == OrderSide::Sell
                                                && price >= limit))
                                    {
                                        let paise = (price.as_decimal() * Decimal::from(100))
                                            .to_i64()
                                            .ok_or_else(|| anyhow!("Fill price out of range"))?;
                                        journal.append(Event::Fill {
                                            id: p.init.client_order_id.to_string(),
                                            broker_id: p.venue.to_string(),
                                            trade_id: format!("T{}", p.init.client_order_id),
                                            quantity: 1,
                                            price_paise: paise,
                                        })?;
                                        batch.push(events::filled(
                                            &p.init, p.venue, price, q.ts_event,
                                        ));
                                        pending = None;
                                    }
                                }
                            }
                            Request::Cancel(id, ts) => {
                                let p = pending
                                    .as_ref()
                                    .ok_or_else(|| anyhow!("No pending paper order"))?;
                                ensure!(
                                    p.init.client_order_id == id,
                                    "Paper cancel identity mismatch"
                                );
                                journal.append(Event::Cancelled {
                                    id: id.to_string(),
                                    broker_id: p.venue.to_string(),
                                })?;
                                batch.push(events::cancelled(&p.init, p.venue, ts));
                                pending = None;
                            }
                            Request::Checkpoint(snapshot) => {
                                checkpoint.save(&snapshot)?;
                                ensure!(
                                    checkpoint.read()? == snapshot,
                                    "Checkpoint verification failed"
                                );
                            }
                        }
                        if !batch.is_empty() {
                            outbox.append(&batch)?;
                        }
                        Ok(batch)
                    })();
                    let failed = result.is_err();
                    if output.try_send(result).is_err() || failed {
                        break;
                    }
                }
                running.store(false, Ordering::Release);
            })?;
        let startup = initialized
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| anyhow!("Paper worker startup timed out"))
            .and_then(|r| r);
        if let Err(error) = startup {
            alive.store(false, Ordering::Release);
            let _ = join.join();
            return Err(error);
        }
        Ok(Self {
            tx,
            rx,
            alive,
            join: Some(join),
        })
    }
    pub fn send(&self, request: Request) -> Result<()> {
        ensure!(self.alive.load(Ordering::Acquire), "Paper worker stopped");
        self.tx
            .try_send(request)
            .map_err(|_| anyhow!("Paper command queue unavailable"))
    }
    pub fn receive(&self) -> Result<Vec<OrderEventAny>> {
        self.rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| anyhow!("Paper worker response unavailable"))?
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
