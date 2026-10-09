# Rust NautilusTrader / Zerodha Kite Research Workspace

Reusable Rust Kite integration, NautilusTrader components and read-only multi-asset portfolio validation. The discarded ILRC strategy, its runners and its research files were removed from the active tree. Older Git history remains available. No strategy is approved for live trading.

## VCE-Mojo v1.6 (Rust port)

`crates/vce-mojo` is a bar-for-bar port of the VCE-Mojo v1.6 Pine script (BullByte Volatility Coil Edge with
AlgoMojo BUY/SELL/SHORT/COVER events): coil detection, watched coil, bar-close triggers, first-touch SL/target
exits (SL wins ties) and the IST EOD square-off. Any portfolio slot can select it; see `doc/MULTI_STRATEGY.md`.

Backtest a slot (read-only, Kite historical candles, no orders):

    ./target/release/kite-node native-vce-backtest PORTFOLIO.json SLOT_ID 2026-09-01 2026-10-08

Writes `backtest_results/vce-mojo/<slot>/<run>/summary.json` and `trades.csv`. The `pine_*` columns reproduce
TradingView's chart (signal-close entry, level exits, no costs) for trade-by-trade parity checks; the
`fill_*`/`net_*` columns use next-open entries, gap-aware exits, slippage and round-trip costs from the slot config.
