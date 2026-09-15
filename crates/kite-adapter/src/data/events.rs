use crate::mapping::market_data::Snapshot;
use crate::websocket::supervisor::{FeedEvent, Summary};

#[derive(Debug)]
pub enum AdapterEvent {
    Full { snapshot: Box<Snapshot> },
    Feed(FeedEvent),
    Complete(Summary),
    Failed,
}
