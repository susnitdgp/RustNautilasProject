//! Read-only terminal dashboard for ILRC Nautilus execution.
use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug, Default)]
pub struct State {
    pub bars: usize,
    pub last_bar: String,
    pub last_price: f64,
    pub position: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
    pub setup: String,
    pub event: String,
    pub fault: Option<String>,
}
pub type Shared = Arc<Mutex<State>>;
pub fn shared() -> Shared {
    Arc::new(Mutex::new(State::default()))
}
pub fn render(state: &State, instrument: &str, mode: &str, session_seconds: u64) {
    let tty = io::stdout().is_terminal();
    if tty {
        print!("\x1b[2J\x1b[H");
    }
    let side = if state.position > 0.0 {
        "LONG"
    } else if state.position < 0.0 {
        "SHORT"
    } else {
        "FLAT"
    };
    println!("┌──────────────────────────────────────────────────────────────┐");
    println!("│ ILRC COMBINED — NAUTILUS / KITE  {:<26}│", mode);
    println!("├──────────────────────────────────────────────────────────────┤");
    println!("  Instrument     {instrument}");
    println!(
        "  Time (IST)     {}",
        chrono::Utc::now()
            .with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"))
            .format("%d-%m-%Y %H:%M:%S")
    );
    println!("  Session        {session_seconds} seconds configured");
    println!(
        "  Last candle    {}     Bars {}",
        if state.last_bar.is_empty() {
            "waiting for first bar"
        } else {
            &state.last_bar
        },
        state.bars
    );
    println!(
        "  Market price   {}",
        if state.bars > 0 {
            format!("{:.2}", state.last_price)
        } else {
            "waiting".into()
        }
    );
    println!(
        "  Position       {side} ({:.0} contract)",
        state.position.abs()
    );
    println!(
        "  Protective SL  {}",
        state.stop.map_or("not confirmed".into(), |v| format!(
            "{v:.2} (strategy tracked)"
        ))
    );
    println!(
        "  Target         {}",
        state.target.map_or("n/a".into(), |v| format!("{v:.2}"))
    );
    println!(
        "  Setup          {}",
        if state.setup.is_empty() {
            "scanning"
        } else {
            &state.setup
        }
    );
    println!(
        "  Last event     {}",
        if state.event.is_empty() {
            "waiting"
        } else {
            &state.event
        }
    );
    println!(
        "  Risk alert     {}",
        state.fault.as_deref().unwrap_or("none reported")
    );
    println!("└──────────────────────────────────────────────────────────────┘");
    println!("  Dashboard displays strategy observations; verify actual broker orders in Kite.");
    let _ = io::stdout().flush();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_state_is_flat_and_not_protected() {
        let s = State::default();
        assert!(s.stop.is_none());
        assert_eq!(s.position, 0.0);
    }
}
