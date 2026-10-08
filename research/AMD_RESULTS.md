# AMD Po3 v2.4.0 exploratory P&L — CRUDEOIL26OCTFUT.MCX

Historical Kite 5m candles aggregated into complete 15m candles, with August as indicator warmup; evaluate the September 1–30 and October 1–8, 2026 sessions. Indicator baseline: default accumulation width percentile (25), 20-bar range, pivot strength 3, max 6 bars sweep return, default 1.5 fib target, 0.4 ATR stop buffer, 64-bar timeout, filters disabled. These are **independent approximations**, not a line-by-line Pine engine: percentile/range-tail and confirmed-pivot behavior are simplified, ATR is anchored differently, signal session treatment differs, stop/target intrabar order is unknown and 5m aggregation is not original native TradingView 15m OHLC.

Trades assume execution at the next complete 15m bar's open (0.5 adverse points slippage on each side), 2 points round trip, stop-first OHLC exit, 23:15 end-of-entry-window flat rule. This is research, not Kite broker fills. Output on available data: Sept 9 model trades, one winning trade, -401 points, profit factor 0.249; Oct 1–8 six model trades, three winning trades, +80 points, profit factor 1.584. Slippage sensitivity (0/0.5/1/2 per side): September -392/-401/-410/-428 points; October +86/+80/+74/+62 points. Thus September fails and October sample is too small to establish an edge. No reliable profitability established. **Do not confuse this approximation with measured performance of the user's supplied TradingView indicator.** A faithful implementation or external TradingView Strategy Tester trade log is required before validating the original Pine logic.

## 3-minute and 5-minute comparison (same approximation)

October 2026 standard CRUDEOIL futures, complete 3m or 5m candles through October 8; August warmup; September 1–30 and October 1–8 scored. Next-bar-open reference fills, 2 points round-trip cost, 0.5 points adverse slippage per side. Same default AMD Po3 approximate rules across timeframes. This is not an exact Pine Script execution.

| Timeframe | September trades | September net points | September profit factor | October trades | October net points | October profit factor |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 3m | 52 | -98 | 0.895 | 11 | -170 | 0.363 |
| 5m | 43 | -167 | 0.838 | 8 | -74 | 0.677 |
| 15m | 9 | -401 | 0.249 | 6 | +80 | 1.584 |

3m and 5m versions are net negative in **both months** under these assumptions. 15m has a small positive October window only. All tested outcomes reflect a simplified approximation, not the verified original indicator. No reliable edge demonstrated.

To refresh the data: `./target/release/kite-node native-amd-export 145894407 3 > /tmp/amd_crudeoil_3m.json` or use `5` for `/tmp/amd_crudeoil_5m.json`. To test: `AMD_MINUTES=3 python3 research/amd_po3_test.py`, `AMD_MINUTES=5 ...`, or `AMD_MINUTES=15 ...` (15m aggregates 5m bars). Data export only, never sends orders.
