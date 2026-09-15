use crate::{
    checkpoint::{Snapshot, Store},
    config::Config,
    crossover::{Signal, Strategy},
};
use anyhow::{Result, ensure};
use kite_execution::{
    coordinator,
    mock::{MockBroker, Outcome},
    rate_limit::{Limiter, policy::Policy},
};
use kite_journal::{
    connection,
    model::{Event, Intent, Product, Side},
    store::Journal,
};
use kite_recorder::records::Record;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    identifiers::InstrumentId,
    types::{Price, Quantity},
};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Serialize;
use std::path::Path;
#[derive(Serialize)]
pub struct Summary {
    pub event: &'static str,
    pub quotes: usize,
    pub signals: usize,
    pub paper_fills: usize,
    pub cancelled: usize,
    pub open_contracts: u32,
    pub checkpoint_verified: bool,
    pub broker_accessed: bool,
    pub live_orders_enabled: bool,
}
struct Pending {
    id: String,
    broker: String,
    signal: Signal,
    limit: i64,
    age: u32,
}
pub fn synthetic(config: Config, namespace: &str) -> Result<Summary> {
    let prices = [
        6008, 6006, 6004, 6002, 6000, 6002, 6004, 6006, 6005, 6005, 6003, 6003, 6001, 5999, 6000,
    ];
    let records = prices
        .iter()
        .enumerate()
        .map(|(i, p)| Record::Quote {
            quote: QuoteTick::new(
                InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
                Price::new(*p as f64, 0),
                Price::new((*p + 1) as f64, 0),
                Quantity::from(10),
                Quantity::from(10),
                UnixNanos::from(1_789_450_000_000_000_000_u64 + i as u64 * 1_000_000_000),
                UnixNanos::from(1_789_450_000_000_000_000_u64 + i as u64 * 1_000_000_000),
            ),
            generation: 1,
        })
        .collect::<Vec<_>>();
    run_at(&connection::url_from_env()?, config, namespace, records)
}
pub fn replay(config: Config, namespace: &str, path: &Path) -> Result<Summary> {
    let mut records = Vec::new();
    kite_recorder::replay::replay(path, |record| {
        ensure!(records.len() < 2000, "Paper replay limited to 2000 records");
        records.push(record);
        Ok(())
    })?;
    run_at(&connection::url_from_env()?, config, namespace, records)
}
pub fn run_at(url: &str, config: Config, namespace: &str, records: Vec<Record>) -> Result<Summary> {
    let mut strategy = Strategy::new(config)?;
    let mut journal = Journal::create_at(url, namespace)?;
    let mut limiter = Limiter::create_at(url, namespace, Policy::default())?;
    let mut store = Store::create_at(url, namespace)?;
    let mut pending: Option<Pending> = None;
    let mut quotes = 0;
    let mut signals = 0;
    let mut fills = 0;
    let mut cancelled = 0;
    let cursor = records.len();
    for (index, record) in records.into_iter().enumerate() {
        match record {
            Record::Gap { .. } => {
                if let Some(p) = pending.take() {
                    journal.append(Event::Cancelled {
                        id: p.id,
                        broker_id: p.broker,
                    })?;
                    strategy.cancelled();
                    cancelled += 1;
                }
                strategy.gap();
            }
            Record::Quote { quote, .. } => {
                quotes += 1;
                // Validate quality before a paper fill or a new signal.
                let signal = strategy.on_quote(&quote)?;
                if let Some(mut p) = pending.take() {
                    let price = match p.signal {
                        Signal::Buy => quote.ask_price.as_decimal(),
                        Signal::Sell => quote.bid_price.as_decimal(),
                    };
                    let paise = (price * Decimal::from(100))
                        .to_i64()
                        .ok_or_else(|| anyhow::anyhow!("Paper price out of range"))?;
                    let crossed = match p.signal {
                        Signal::Buy => paise <= p.limit,
                        Signal::Sell => paise >= p.limit,
                    };
                    if !strategy.mids.is_empty() && crossed {
                        journal.append(Event::Fill {
                            id: p.id.clone(),
                            broker_id: p.broker.clone(),
                            trade_id: format!("T{}", fills + 1),
                            quantity: 1,
                            price_paise: paise,
                        })?;
                        strategy.filled(p.signal)?;
                        fills += 1;
                    } else {
                        p.age += 1;
                        if p.age >= 3 || strategy.mids.is_empty() {
                            journal.append(Event::Cancelled {
                                id: p.id,
                                broker_id: p.broker,
                            })?;
                            strategy.cancelled();
                            cancelled += 1;
                        } else {
                            pending = Some(p);
                        }
                    }
                }
                if let Some(signal) = signal {
                    signals += 1;
                    let id = format!("S{signals}");
                    let broker = format!("P{signals}");
                    let limit = (match signal {
                        Signal::Buy => quote.ask_price.as_decimal(),
                        Signal::Sell => quote.bid_price.as_decimal(),
                    } * Decimal::from(100))
                    .to_i64()
                    .ok_or_else(|| anyhow::anyhow!("Signal price out of range"))?;
                    // Checkpoint the pending decision before journal/dispatch; restart is review-only.
                    store.save(&Snapshot {
                        version: 1,
                        cursor: index,
                        strategy: strategy.clone(),
                        finished: false,
                    })?;
                    journal.append(Event::Intent {
                        intent: Intent {
                            id: id.clone(),
                            symbol: "CRUDEOIL26SEPFUT".into(),
                            side: match signal {
                                Signal::Buy => Side::Buy,
                                Signal::Sell => Side::Sell,
                            },
                            product: Product::Nrml,
                            quantity: 1,
                            limit_price_paise: limit,
                        },
                    })?;
                    let mut mock = MockBroker::new(Outcome::Accepted(broker.clone()));
                    coordinator::submit(&mut journal, &mut mock, &id, &mut limiter)?;
                    pending = Some(Pending {
                        id,
                        broker,
                        signal,
                        limit,
                        age: 0,
                    });
                }
            }
            _ => {}
        }
        store.save(&Snapshot {
            version: 1,
            cursor: index + 1,
            strategy: strategy.clone(),
            finished: false,
        })?;
    }
    if let Some(p) = pending {
        journal.append(Event::Cancelled {
            id: p.id,
            broker_id: p.broker,
        })?;
        strategy.cancelled();
        cancelled += 1;
    }
    let snapshot = Snapshot {
        version: 1,
        cursor,
        strategy: strategy.clone(),
        finished: true,
    };
    store.save(&snapshot)?;
    let checkpoint_verified = store.read()? == snapshot;
    let recovered = Journal::open_at(url, namespace)?;
    let net: i64 = recovered
        .state()
        .orders()
        .map(|o| match o.intent.side {
            Side::Buy => i64::from(o.filled),
            Side::Sell => -i64::from(o.filled),
        })
        .sum();
    ensure!(
        checkpoint_verified && net == i64::from(strategy.position),
        "Strategy/journal position mismatch"
    );
    Ok(Summary {
        event: "strategy_paper_complete",
        quotes,
        signals,
        paper_fills: fills,
        cancelled,
        open_contracts: strategy.position,
        checkpoint_verified,
        broker_accessed: false,
        live_orders_enabled: false,
    })
}
