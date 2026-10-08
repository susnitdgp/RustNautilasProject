//! Deterministic, broker-free order-state test harness. NOT a causal ILRC signal generator.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Setup {
    A,
    B,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Side {
    Buy,
    Sell,
}
#[derive(Debug, Clone, Copy)]
struct Candidate {
    id: &'static str,
    time: u64,
    setup: Setup,
    side: Side,
    entry: f64,
    stop: f64,
    target: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Exit {
    Stop,
    BreakEven,
    Target,
}
#[derive(Debug, Clone, Copy)]
struct Position {
    id: &'static str,
    setup: Setup,
    side: Side,
    entry: f64,
    stop: f64,
    target: f64,
    risk: f64,
    break_even: bool,
}
#[derive(Default)]
struct Engine {
    seen: BTreeSet<&'static str>,
    active: Option<Position>,
    opens: usize,
    exits: Vec<Exit>,
    rejected: usize,
}
impl Engine {
    fn submit_batch(&mut self, candidates: &mut [Candidate]) -> Result<()> {
        candidates.sort_by_key(|c| (c.time, if c.setup == Setup::A { 0 } else { 1 }));
        for c in candidates {
            if !self.seen.insert(c.id) {
                self.rejected += 1;
                continue;
            }
            if self.active.is_some() {
                self.rejected += 1;
                continue;
            }
            let risk = (c.entry - c.stop).abs();
            ensure!(
                c.entry.is_finite() && c.stop.is_finite() && c.target.is_finite() && risk > 0.0,
                "invalid simulated order"
            );
            let direction = if c.side == Side::Buy { 1.0 } else { -1.0 };
            ensure!(
                (c.entry - c.stop) * direction > 0.0 && (c.target - c.entry) * direction > 0.0,
                "stop/target direction invalid"
            );
            self.active = Some(Position {
                id: c.id,
                setup: c.setup,
                side: c.side,
                entry: c.entry,
                stop: c.stop,
                target: c.target,
                risk,
                break_even: false,
            });
            self.opens += 1;
        }
        Ok(())
    }
    /// Conservative simulation: adverse stop takes precedence when both stop and target touch one bar.
    fn bar(&mut self, high: f64, low: f64) -> Result<Option<Exit>> {
        ensure!(
            high.is_finite() && low.is_finite() && high >= low,
            "invalid bar"
        );
        let Some(p) = self.active.as_mut() else {
            return Ok(None);
        };
        let (stop_hit, target_hit, reached_one_r) = match p.side {
            Side::Buy => (low <= p.stop, high >= p.target, high >= p.entry + p.risk),
            Side::Sell => (high >= p.stop, low <= p.target, low <= p.entry - p.risk),
        };
        let reason = if stop_hit {
            Some(if p.break_even {
                Exit::BreakEven
            } else {
                Exit::Stop
            })
        } else if target_hit {
            Some(Exit::Target)
        } else {
            None
        };
        if let Some(reason) = reason {
            self.active = None;
            self.exits.push(reason);
            return Ok(Some(reason));
        }
        if reached_one_r {
            p.stop = p.entry;
            p.break_even = true;
        }
        Ok(None)
    }
}

pub fn run() -> Result<()> {
    let mut e = Engine::default();
    let mut entries = [
        Candidate {
            id: "b1",
            time: 1,
            setup: Setup::B,
            side: Side::Buy,
            entry: 100.0,
            stop: 95.0,
            target: 115.0,
        },
        Candidate {
            id: "a1",
            time: 1,
            setup: Setup::A,
            side: Side::Buy,
            entry: 100.0,
            stop: 95.0,
            target: 110.0,
        },
    ];
    e.submit_batch(&mut entries)?;
    ensure!(
        e.active.is_some_and(|p| p.setup == Setup::A),
        "A priority violated"
    );
    let accepted_id = e.active.expect("synthetic position").id;
    e.bar(106.0, 99.0)?;
    e.bar(108.0, 100.0)?;
    ensure!(
        e.exits == [Exit::BreakEven],
        "break-even protection violated"
    );
    e.submit_batch(&mut [Candidate {
        id: "short1",
        time: 3,
        setup: Setup::B,
        side: Side::Sell,
        entry: 100.0,
        stop: 105.0,
        target: 85.0,
    }])?;
    ensure!(
        e.bar(100.0, 84.0)? == Some(Exit::Target),
        "short 3R target violated"
    );
    println!(
        "{}",
        serde_json::json!({"event":"ilrc_mock_execution","status":"PASS","scope":"synthetic_order_state_only","causal_strategy_signals_implemented":false,"broker_orders_sent":false,"execution_client_loaded":false,"real_orders_enabled":false,"accepted_first_id":accepted_id,"opens":e.opens,"exits":e.exits,"rejected":e.rejected})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buy(id: &'static str, setup: Setup, time: u64) -> Candidate {
        Candidate {
            id,
            time,
            setup,
            side: Side::Buy,
            entry: 100.0,
            stop: 95.0,
            target: 115.0,
        }
    }
    #[test]
    fn arbitration_and_duplicate_prevention() {
        let mut e = Engine::default();
        e.submit_batch(&mut [buy("b", Setup::B, 1), buy("a", Setup::A, 1)])
            .unwrap();
        assert_eq!(e.active.unwrap().id, "a");
        assert_eq!(e.opens, 1);
        assert_eq!(e.rejected, 1);
        e.submit_batch(&mut [buy("a", Setup::A, 2)]).unwrap();
        assert_eq!(e.opens, 1);
        assert_eq!(e.rejected, 2);
    }
    #[test]
    fn break_even_after_one_r_and_conservative_intrabar_stop() {
        let mut e = Engine::default();
        e.submit_batch(&mut [buy("a", Setup::A, 1)]).unwrap();
        assert_eq!(e.bar(105.0, 96.0).unwrap(), None);
        assert!(e.active.unwrap().break_even);
        assert_eq!(e.bar(116.0, 99.0).unwrap(), Some(Exit::BreakEven));
        assert!(e.active.is_none());
    }
    #[test]
    fn stop_wins_same_bar_and_no_double_open() {
        let mut e = Engine::default();
        e.submit_batch(&mut [buy("a", Setup::A, 1)]).unwrap();
        assert_eq!(e.bar(120.0, 94.0).unwrap(), Some(Exit::Stop));
        e.submit_batch(&mut [buy("a", Setup::A, 2)]).unwrap();
        assert!(e.active.is_none());
    }
    #[test]
    fn continuation_three_r_target() {
        let mut e = Engine::default();
        e.submit_batch(&mut [buy("b", Setup::B, 1)]).unwrap();
        assert_eq!(e.bar(116.0, 100.0).unwrap(), Some(Exit::Target));
    }
}
