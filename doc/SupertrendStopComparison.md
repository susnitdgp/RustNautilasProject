# Supertrend confirmation strategy: fixed ATR stop comparison

## Fixed rules

Same 17 August–15 September 2026 window, 22 sessions and 3,732 five-minute bars.
One CRUDEOIL September contract per position (100-barrel multiplier).
Same immutable input and 870 earlier warmup candles as the previous comparison.

Baseline: Supertrend(7,2), MACD(12,26,9), session HLC3 VWAP. Enter long when
Supertrend is bullish, close > VWAP and MACD > signal; short reverses these.
Wait for confirmation if initially absent. Signal on completed bar, fill next open.
Exit on completed-bar Supertrend reversal or session close.

New variant adds a fixed native simulated stop-market:
- Native Wilder ATR(14), evaluated at the completed entry-signal candle.
- Long trigger = floor(actual entry price - 1.5 * ATR).
- Short trigger = ceil(actual entry price + 1.5 * ATR).
- No trailing, no target and no widening after entry.
- Stop competes with existing Supertrend/EOD exits; unused stops are canceled.
- Gap beyond trigger fills at the worse open, not the better trigger price.
- After stop-out, no entry until a fresh Supertrend direction change. MACD/VWAP
  must still confirm. The block is carried across sessions in a unique Redis key.
- Native Redis order persistence remains enabled. No real broker client is used.

This tests the proposed stop plus re-entry restriction as one variant.
No parameter sweep or strategy-default change was made.

## Thirty-day results

| Metric | Current strategy | With fixed 1.5 ATR stop |
|---|---:|---:|
| Trades | 143 | 143 |
| Long / short | 74 / 69 | 74 / 69 |
| Wins | 54 | 43 |
| Win rate | 37.8% | 30.1% |
| Gross P&L INR | 108,100 | 50,700 |
| Closed-trade drawdown INR | 46,200 | 85,800 |
| Worst trade INR | -14,900 | -12,500 |
| Average losing trade INR | -3,294.25 | -2,660.20 |
| Sum of winning trades INR | 394,700 | 311,400 |
| Sum of losing trades INR | -286,600 | -260,700 |
| Fixed stop-outs | 0 | 70 |

The stop reduced individual loss size but cut aggregate profit by INR 57,400
and increased drawdown by INR 39,600. It does not improve this sample overall.
Both variants entered on exactly the same candles, in the same directions and
at the same prices. Synthetic timestamps may differ by one nanosecond when
the baseline must close a position before reversing and the stopped variant
is already flat. 45 trades improved, 24 worsened and 74 were unchanged.
Despite more improved trades, forgone gains outweighed reduced losses.

Fees, spread, slippage and liquidity constraints are excluded. Drawdown measures
closed-trade equity only. Five-minute OHLC cannot establish exact intrabar timing;
stops use the native OHLC matching model. This is not forward validation.
The default strategy is unchanged and real orders remain disabled.

## Review and reproduction

Modules: supertrend_stop_policy.rs, supertrend_stop_actor.rs,
supertrend_stop_backtest.rs and supertrend_stop_batch.rs.
The policy computes signals and the persistent block; the actor manages native
orders/stops. Backtest runner reconciles fills against native P&L; batch isolates
each daily engine and transfers the block only through Redis.

    cargo run --locked -p kite-node -- native-supertrend-stop-compare 2026-08-17 2026-09-15 backtest_results/supertrend_macd_vwap_2026-08-17_to_2026-09-15_2c404264-451e-4255-8a6a-20d0dbaad733/historical_input.json

Saved reports:
backtest_results/supertrend_stop_comparison_2026-08-17_to_2026-09-15_cbc3cbb1-97fc-4165-91f9-c4b3f894922e/

Root comparison.json, verification.json and README.md (including daily P&L).
confirmed/ and atr_stop/ contain daily native results, candles, indicators,
signals, fills and trades. The comparison records its unique Redis cooldown key.

Baseline trades reproduce the prior run exactly. All 143 stop-variant trades
were audited for entry confirmation, entry/exit prices, one-lot quantities,
ATR trigger arithmetic, gap prices, reversal/EOD exits, and no re-entry before a
fresh direction change, including across sessions. P&L and drawdown reconcile.


Validation: 197 workspace tests passed, zero failed, one existing ignored fixture.
Clippy with warnings denied, formatting and diff checks passed.
New tests cover both stop directions, actual gap-open fills, flat shutdown,
same-trend re-entry blocking and keeping the block during next-day warmup.
Changes are uncommitted.
