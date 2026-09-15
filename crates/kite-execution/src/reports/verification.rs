use super::batch;
use anyhow::{Result, ensure};
use kite_journal::{
    connection,
    model::{Event, Intent, Product, Side},
    store::Journal,
};
use nautilus_model::{enums::OrderStatus, reports::ExecutionMassStatus};
use serde::Serialize;
#[derive(Serialize)]
pub struct Summary {
    pub event: &'static str,
    pub native_order_reports: usize,
    pub native_fill_reports: usize,
    pub unresolved_orders: usize,
    pub oms_ack_not_venue_acceptance: bool,
    pub replay_reports_identical: bool,
    pub native_serialization_roundtrip: bool,
    pub reports_complete: bool,
    pub execution_engine_started: bool,
    pub broker_accessed: bool,
    pub live_orders_enabled: bool,
}
fn intent(id: &str, product: Product) -> Event {
    Event::Intent {
        intent: Intent {
            id: id.into(),
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: Side::Buy,
            product,
            quantity: 2,
            limit_price_paise: 600100,
        },
    }
}
fn accepted(j: &mut Journal, id: &str, broker: &str, product: Product) -> Result<()> {
    j.append(intent(id, product))?;
    j.append(Event::Dispatch { id: id.into() })?;
    j.append(Event::Acknowledged {
        id: id.into(),
        broker_id: broker.into(),
    })?;
    Ok(())
}
fn fill(j: &mut Journal, id: &str, broker: &str, trade: &str, price: i64) -> Result<()> {
    j.append(Event::Fill {
        id: id.into(),
        broker_id: broker.into(),
        trade_id: trade.into(),
        quantity: 1,
        price_paise: price,
    })?;
    Ok(())
}
pub fn run(namespace: &str) -> Result<Summary> {
    run_at(&connection::url_from_env()?, namespace)
}
pub fn run_at(url: &str, namespace: &str) -> Result<Summary> {
    let mut journal = Journal::create_at(url, namespace)?;
    accepted(&mut journal, "Filled", "B1", Product::Nrml)?;
    fill(&mut journal, "Filled", "B1", "T1", 600000)?;
    fill(&mut journal, "Filled", "B1", "T2", 600100)?;
    accepted(&mut journal, "Cancelled", "B2", Product::Mis)?;
    fill(&mut journal, "Cancelled", "B2", "T3", 599900)?;
    journal.append(Event::Cancelled {
        id: "Cancelled".into(),
        broker_id: "B2".into(),
    })?;
    accepted(&mut journal, "Submitted", "B3", Product::Nrml)?;
    journal.append(intent("Uncertain", Product::Nrml))?;
    journal.append(Event::Dispatch {
        id: "Uncertain".into(),
    })?;
    journal.append(Event::Unknown {
        id: "Uncertain".into(),
    })?;
    let first = batch::build(&journal)?;
    drop(journal);
    let journal = Journal::open_at(url, namespace)?;
    let second = batch::build(&journal)?;
    let replay_reports_identical = first.native == second.native;
    let native_serialization_roundtrip =
        serde_json::from_str::<ExecutionMassStatus>(&serde_json::to_string(&first.native)?)?
            == first.native;
    let reports = first.native.order_reports();
    let oms_ack_not_venue_acceptance = reports.values().any(|r| {
        r.client_order_id
            .is_some_and(|id| id.as_str() == "Submitted")
            && r.order_status == OrderStatus::Submitted
            && r.ts_accepted.as_u64() == 0
    });
    let native_fill_reports = first.native.fill_reports().values().map(Vec::len).sum();
    ensure!(
        reports.len() == 3
            && native_fill_reports == 3
            && first.skipped_orders == 1
            && replay_reports_identical
            && native_serialization_roundtrip
            && oms_ack_not_venue_acceptance
            && !first.native.reports_complete(),
        "Native report verification failed"
    );
    Ok(Summary {
        event: "nautilus_reports_simulation_complete",
        native_order_reports: reports.len(),
        native_fill_reports,
        unresolved_orders: first.skipped_orders,
        oms_ack_not_venue_acceptance,
        replay_reports_identical,
        native_serialization_roundtrip,
        reports_complete: first.native.reports_complete(),
        execution_engine_started: false,
        broker_accessed: false,
        live_orders_enabled: false,
    })
}
