use super::{fill, identity, order};
use anyhow::{Result, ensure};
use kite_journal::{model::Product, store::Journal};
use nautilus_core::UnixNanos;
use nautilus_model::{
    identifiers::{ClientId, Venue},
    reports::ExecutionMassStatus,
};
use std::collections::{BTreeMap, BTreeSet};
pub struct Batch {
    pub native: ExecutionMassStatus,
    pub issues: BTreeSet<&'static str>,
    pub product_by_client: BTreeMap<String, Product>,
    pub skipped_orders: usize,
}
pub fn build(journal: &Journal) -> Result<Batch> {
    ensure!(
        !journal.write_uncertain(),
        "Journal outcome uncertain; reopen before reporting"
    );
    let history = journal.history();
    let ts_init = history.last().map_or(0, |r| r.recorded_at_ns);
    ensure!(
        history
            .windows(2)
            .all(|w| w[0].recorded_at_ns <= w[1].recorded_at_ns),
        "Journal clock moved backward; report mapping requires review"
    );
    let generation = journal.generation();
    let mut native = ExecutionMassStatus::new(
        ClientId::from("KITE-SIM"),
        identity::account(),
        Venue::from("MCX"),
        UnixNanos::from(ts_init),
        Some(identity::report_id(
            generation,
            "mass",
            journal.record_count(),
        )),
    );
    // Never let a partial journal clear engine positions or imply a complete account snapshot.
    native.set_report_window(
        history.first().map(|r| UnixNanos::from(r.recorded_at_ns)),
        false,
    );
    let mut issues = BTreeSet::from([
        "simulation_local_timestamps",
        "simulation_zero_commission",
        "account_positions_not_reported",
        "venue_acceptance_time_unavailable",
    ]);
    let mut product_by_client = BTreeMap::new();
    let mut skipped_orders = 0;
    let mut orders = Vec::new();
    let mut fills = Vec::new();
    for o in journal.state().orders() {
        product_by_client.insert(o.intent.id.clone(), o.intent.product);
        match order::map(o, history, generation, ts_init)? {
            Some(report) => orders.push(report),
            None => {
                skipped_orders += 1;
                issues.insert("order_requires_reconciliation");
            }
        }
    }
    for (index, record) in history.iter().enumerate() {
        if let Some(report) = fill::map(
            record,
            journal.state().order(record.event.id())?,
            generation,
            index + 1,
            ts_init,
        )? {
            fills.push(report);
        }
    }
    native.add_order_reports(orders);
    native.add_fill_reports(fills);
    Ok(Batch {
        native,
        issues,
        product_by_client,
        skipped_orders,
    })
}
