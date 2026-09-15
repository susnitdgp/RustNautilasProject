use crate::config::Config;
use anyhow::{Result, ensure};
use nautilus_model::data::QuoteTick;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Strategy {
    pub config: Config,
    pub mids: VecDeque<Decimal>,
    pub previous: Option<i8>,
    pub position: u32,
    pub pending: bool,
    pub entries: u32,
    pub last_ts: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum Signal {
    Buy,
    Sell,
}
impl Strategy {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            mids: VecDeque::new(),
            previous: None,
            position: 0,
            pending: false,
            entries: 0,
            last_ts: 0,
        })
    }
    pub fn gap(&mut self) {
        self.mids.clear();
        self.previous = None;
    }
    pub fn on_quote(&mut self, q: &QuoteTick) -> Result<Option<Signal>> {
        ensure!(
            q.instrument_id
                == nautilus_model::identifiers::InstrumentId::from("CRUDEOIL26SEPFUT.MCX"),
            "Strategy instrument mismatch"
        );
        let bid = q.bid_price.as_decimal();
        let ask = q.ask_price.as_decimal();
        let ts = q.ts_event.as_u64();
        let invalid = ts == 0
            || ts < self.last_ts
            || bid <= Decimal::ZERO
            || ask < bid
            || bid.fract() != Decimal::ZERO
            || ask.fract() != Decimal::ZERO
            || ask - bid > Decimal::from(self.config.max_spread_rupees)
            || q.bid_size.as_decimal() < Decimal::ONE
            || q.ask_size.as_decimal() < Decimal::ONE
            || q.ts_init.as_u64().saturating_sub(ts)
                > u64::from(self.config.max_age_seconds) * 1_000_000_000
            || ts.saturating_sub(q.ts_init.as_u64()) > 2_000_000_000;
        if invalid {
            self.gap();
            return Ok(None);
        }
        self.last_ts = ts;
        self.mids.push_back((bid + ask) / Decimal::from(2));
        if self.mids.len() > self.config.slow {
            self.mids.pop_front();
        }
        if self.mids.len() < self.config.slow {
            return Ok(None);
        }
        let fast = self
            .mids
            .iter()
            .rev()
            .take(self.config.fast)
            .copied()
            .sum::<Decimal>()
            / Decimal::from(self.config.fast as u32);
        let slow =
            self.mids.iter().copied().sum::<Decimal>() / Decimal::from(self.config.slow as u32);
        let relation = if fast > slow {
            1
        } else if fast < slow {
            -1
        } else {
            0
        };
        let previous = self.previous.replace(relation);
        if self.pending {
            return Ok(None);
        }
        let signal = match previous {
            Some(p)
                if p <= 0
                    && relation > 0
                    && self.position == 0
                    && self.entries < self.config.max_entries =>
            {
                Some(Signal::Buy)
            }
            Some(p) if p >= 0 && relation < 0 && self.position == 1 => Some(Signal::Sell),
            _ => None,
        };
        if signal.is_some() {
            self.pending = true;
        }
        Ok(signal)
    }
    pub fn filled(&mut self, signal: Signal) -> Result<()> {
        ensure!(self.pending, "Unexpected strategy fill");
        match signal {
            Signal::Buy => {
                ensure!(self.position == 0, "Position cap exceeded");
                self.position = 1;
                self.entries += 1;
            }
            Signal::Sell => {
                ensure!(self.position == 1, "No strategy position to exit");
                self.position = 0;
            }
        }
        self.pending = false;
        Ok(())
    }
    pub fn cancelled(&mut self) {
        self.pending = false;
        self.gap();
    }
}
