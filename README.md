# Rust NautilusTrader / Zerodha Kite Research Workspace

Reusable Rust Kite integration, NautilusTrader components and read-only multi-asset portfolio validation. The discarded ILRC strategy, its runners and its research files were removed from the active tree. Older Git history remains available. No strategy is approved for live trading.

## AMD Power of Three research

An independent 15-minute **approximation** of WillyAlgoTrader's AMD Po3 v2.4.0 indicator is available at `research/amd_po3_test.py`. This tests the standard CRUDEOIL October 2026 futures contract for September/October 2026 with modeled next-bar opening entries, 2-point round-trip costs, and configurable adverse slippage. This research is NOT an exact implementation of the TradingView Pine script; it must not be taken as evidence of its true indicator performance.

The read-only data export command is `./target/release/kite-node native-amd-export 145894407 5 > /tmp/amd_crudeoil_5m.json`, followed by `python3 research/amd_po3_test.py > /tmp/amd_results.json`. Export fetches historical 5-minute candles and Python aggregates complete 15-minute OHLC bars; it requires existing authorized Kite historical access. Scripts do not place orders or modify Redis keys.

Current research findings and limitations: `research/AMD_RESULTS.md`.
