use nautilus_common::{
    cache::Cache,
    clock::Clock,
    live::clock::LiveClock,
    msgbus::{self, MessageBus, TypedHandler},
};
use nautilus_data::engine::DataEngine;
use nautilus_model::{
    data::{Data, QuoteTick},
    instruments::{FuturesContract, InstrumentAny},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub struct Core {
    pub engine: DataEngine,
    pub cache: Rc<RefCell<Cache>>,
    pub clock: Rc<RefCell<dyn Clock>>,
    callbacks: Rc<Cell<u64>>,
    pub last_quote: Option<QuoteTick>,
}
impl Core {
    pub fn new(instrument: &FuturesContract) -> Self {
        Self::with_clock(instrument, Rc::new(RefCell::new(LiveClock::new(None))))
    }
    pub fn with_clock(instrument: &FuturesContract, clock: Rc<RefCell<dyn Clock>>) -> Self {
        let cache = Rc::new(RefCell::new(Cache::default()));
        msgbus::set_message_bus(Rc::new(RefCell::new(MessageBus::default())));
        let callbacks = Rc::new(Cell::new(0u64));
        let count = callbacks.clone();
        msgbus::subscribe_quotes(
            "data.quotes.*".into(),
            TypedHandler::from(move |_: &QuoteTick| count.set(count.get() + 1)),
            None,
        );
        let mut engine = DataEngine::new(clock.clone(), cache.clone(), None);
        engine.process(&InstrumentAny::FuturesContract(instrument.clone()));
        Self {
            engine,
            cache,
            clock,
            callbacks,
            last_quote: None,
        }
    }
    pub fn quote(&mut self, quote: QuoteTick) {
        self.engine.process_data(Data::Quote(quote));
        self.last_quote = Some(quote);
    }
    pub fn callbacks(&self) -> u64 {
        self.callbacks.get()
    }
}
