# Rust NautilusTrader / Zerodha Kite Research Workspace

Reusable Rust Kite integration, NautilusTrader components and read-only multi-asset portfolio validation. The discarded ILRC strategy, its runners and its research files were removed from the active tree. Older Git history remains available. No strategy is approved for live trading.

## SATS v1.13.1 (Rust port)

`crates/sats` is a bar-for-bar port of the Self-Aware Trend System v1.13.1 (WillyAlgoTrader): presets, adaptive
asymmetric SuperTrend with Trend Quality Index and character-flip, score, dynamic TPs, the script's single model
position (SL-first, thirds at TP1/TP2, TP3, flip and timeout exits, tick rounding, fees, slippage) and the
experimental self-learning calibration. All inputs live in the slot's own JSON (`config/sats-crudeoilm.json`).

Backtest a slot (read-only, Kite historical candles, no orders):

    ./target/release/kite-node native-sats-backtest config/portfolio-development.example.json crudeoilm-sats-202610 2026-09-01 2026-10-08

Writes `backtest_results/sats/<slot>/<run>/summary.json` and `trades.csv`: the `model_*` columns reproduce the
script's own R accounting (TradingView trader card); the `exec_*` columns apply the slot's `execution` settings
(lots, thirds or single exit, next-open market fills, slippage, round-trip cost).

