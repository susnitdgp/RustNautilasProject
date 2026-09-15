use crate::{
    events,
    worker::{Handle, Request},
};
use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use nautilus_common::{clients::ExecutionClient, messages::execution::*};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{accounts::AccountAny, enums::OmsType, identifiers::*, reports::*, types::*};
use std::sync::mpsc::SyncSender;
pub struct PaperExecutionClient {
    tx: SyncSender<Request>,
    account: AccountAny,
    started: bool,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl PaperExecutionClient {
    pub fn new(worker: &Handle, account: AccountAny) -> Self {
        Self {
            tx: worker.tx.clone(),
            account,
            started: false,
            alive: worker.alive.clone(),
        }
    }
}
#[async_trait(?Send)]
impl ExecutionClient for PaperExecutionClient {
    fn is_connected(&self) -> bool {
        self.started && self.alive.load(std::sync::atomic::Ordering::Acquire)
    }
    fn client_id(&self) -> ClientId {
        events::client_id()
    }
    fn account_id(&self) -> AccountId {
        events::account_id()
    }
    fn venue(&self) -> Venue {
        Venue::from("MCX")
    }
    fn oms_type(&self) -> OmsType {
        OmsType::Netting
    }
    fn get_account(&self) -> Option<AccountAny> {
        Some(self.account.clone())
    }
    fn provides_bulk_position_coverage(&self, _: InstrumentId) -> bool {
        false
    }
    fn generate_account_state(
        &self,
        _: Vec<AccountBalance>,
        _: Vec<MarginBalance>,
        _: bool,
        _: UnixNanos,
        _: Option<Params>,
    ) -> Result<()> {
        bail!("Synthetic account is fixed; account reporting unsupported")
    }
    fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        bail!("Paper namespaces cannot be reset")
    }
    fn dispose(&mut self) -> Result<()> {
        self.stop()
    }
    async fn connect(&mut self) -> Result<()> {
        self.start()
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.stop()
    }
    fn submit_order(&self, cmd: SubmitOrder) -> Result<()> {
        ensure!(self.is_connected(), "Paper client stopped");
        let o = cmd.order_init;
        crate::validation::order(&o)?;
        self.tx
            .try_send(Request::Submit(Box::new(o)))
            .map_err(|_| anyhow::anyhow!("Paper command queue unavailable"))
    }
    fn cancel_order(&self, cmd: CancelOrder) -> Result<()> {
        ensure!(
            self.started && cmd.instrument_id == InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
            "Invalid paper cancel"
        );
        self.tx
            .try_send(Request::Cancel(cmd.client_order_id, cmd.ts_init))
            .map_err(|_| anyhow::anyhow!("Paper command queue unavailable"))
    }
    fn modify_order(&self, _: ModifyOrder) -> Result<()> {
        bail!("Native paper modification not implemented")
    }
    fn submit_order_list(&self, _: SubmitOrderList) -> Result<()> {
        bail!("Paper order lists unsupported")
    }
    fn batch_modify_orders(&self, _: BatchModifyOrders) -> Result<()> {
        bail!("Paper batches unsupported")
    }
    fn cancel_all_orders(&self, _: CancelAllOrders) -> Result<()> {
        bail!("Use explicit paper cancel")
    }
    fn batch_cancel_orders(&self, _: BatchCancelOrders) -> Result<()> {
        bail!("Paper batches unsupported")
    }
    fn query_account(&self, _: QueryAccount) -> Result<()> {
        bail!("Paper account queries unsupported")
    }
    fn query_order(&self, _: QueryOrder) -> Result<()> {
        bail!("Use native cache and Redis replay")
    }
    async fn generate_order_status_report(
        &self,
        _: &GenerateOrderStatusReport,
    ) -> Result<Option<OrderStatusReport>> {
        bail!("Paper report query unsupported")
    }
    async fn generate_order_status_reports(
        &self,
        _: &GenerateOrderStatusReports,
    ) -> Result<Vec<OrderStatusReport>> {
        bail!("Paper report query unsupported")
    }
    async fn generate_fill_reports(&self, _: GenerateFillReports) -> Result<Vec<FillReport>> {
        bail!("Paper report query unsupported")
    }
    async fn generate_position_status_reports(
        &self,
        _: &GeneratePositionStatusReports,
    ) -> Result<Vec<PositionStatusReport>> {
        bail!("Paper position coverage unavailable")
    }
    async fn generate_mass_status(&self, _: Option<u64>) -> Result<Option<ExecutionMassStatus>> {
        bail!("Paper reconciliation is read-only Redis event replay")
    }
}
