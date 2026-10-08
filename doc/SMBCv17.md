# Smart Money Breakout Channels v1.7 — Rust / NautilusTrader

Active strategy: `smart_money_breakout_channels_v17`.

The Rust implementation ports the supplied Pine channel detection and trade engine while deliberately omitting TradingView-only boxes, labels, volume gauge, and webhook alerts.

## Production behavior

- 1-minute MCX CRUDEOIL candles.
- Entries are confirmed-bar breakout decisions.
- `strong_closes_only` uses `(open + close) / 2` against channel bounds, matching Pine.
- Long and short trading are independently configurable.
- Opposite breakout mode: `Reverse`, `Exit Only`, or `Ignore`.
- Stop modes: ATR, candle wick, wick + ATR buffer.
- Target modes: R:R, ATR, or none.
- Breakeven and ATR/wick trailing ratchet only in the favorable direction.
- Intrabar stop/target checks use fresh Kite bid/ask quotes when enabled.
- Historical bar ambiguity is conservative: if both SL and TP occur in one candle, stop wins, matching the Pine ordering.
- Actual Nautilus position/fill state is authoritative.
- Reversals are two-stage: close the existing position, wait for fill, then open the new side.
- An independent timer also requests EOD flattening after the configured square-off time.

## Configuration

Source of truth: `config/production-smbc.json`.

The committed strategy live-order gate is intentionally `false` after this migration. Enable production only after historical parity and paper/sandbox verification.
