use super::events::AdapterEvent;
use crate::credentials::KiteCredentials;
use nautilus_common::factories::ClientConfig;
use nautilus_model::instruments::FuturesContract;
use std::{any::Any, sync::Arc};
use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub struct KiteDataClientConfig {
    pub instrument: FuturesContract,
    pub instrument_token: u32,
    pub duration_seconds: u64,
    pub credentials: Arc<KiteCredentials>,
    pub events: Sender<AdapterEvent>,
}
impl ClientConfig for KiteDataClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
