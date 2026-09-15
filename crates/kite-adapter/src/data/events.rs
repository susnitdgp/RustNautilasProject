use crate::websocket::supervisor::{FeedEvent, Summary};
use nautilus_model::data::QuoteTick;

#[derive(Debug)]
pub enum AdapterEvent {
    Quote { quote: QuoteTick, generation: u32 },
    Feed(FeedEvent),
    Complete(Summary),
    Failed,
}
