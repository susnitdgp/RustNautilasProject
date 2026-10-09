# VCE-Mojo v1.6 on MCX CRUDEOILM (mini) — development research

Instrument: CRUDEOILM26OCTFUT.MCX, Kite instrument token **145894663**, October expiry **2026-10-19**. Read-only historical API, no orders, no modification to production or enabled strategy manifest.

Source: user-supplied VCE-Mojo Pine v6. Existing Python research approximation is parameterized via VCE_DATASET to run on mini candles. This is a **research implementation**, not an integrated Rust strategy and NOT exact TradingView parity. It models Balanced sensitivity and the core coil/watch/rejection/ATR-stop/target mechanics, but signal-close vs next-open execution, EOD mechanics, ATR seeding, range state and ambiguous candle ordering need independent parity validation.

Data: Kite historical October CRUDEOILM mini futures: 1m 41,280, 3m 13,760, 5m 8,256 complete candles through Oct 8, 2026 (August used as warmup). Scored September 2026 and October 1–8 separately. Next-candle open reference fills, 2 points completed round trip, 0.5-point adverse slippage per side. P&L **price points**, not rupees; need verify contract multiplier/charges for rupee P&L.

| Timeframe | Target | Sep trades | Sep wins | Sep net pts | Sep PF | Oct trades | Oct wins | Oct net pts | Oct PF |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1m | TP1 1R | 443 | 198 | -1593.10 | 0.670 | 80 | 42 | -144.39 | 0.820 |
| 1m | TP2 1.5R | 404 | 142 | -1580.25 | 0.696 | 73 | 27 | -336.26 | 0.653 |
| 1m | TP3 2R | 369 | 105 | -1704.18 | 0.675 | 66 | 20 | -271.95 | 0.711 |
| 3m | TP1 1R | 109 | 47 | -758.06 | 0.650 | 27 | 9 | -252.85 | 0.550 |
| 3m | TP2 1.5R | 99 | 41 | -317.71 | 0.839 | 21 | 9 | +103.46 | 1.295 |
| 3m | TP3 2R | 93 | 30 | -451.40 | 0.780 | 21 | 8 | +129.49 | 1.342 |
| 5m | TP1 1R | 59 | 32 | -79.79 | 0.930 | 16 | 9 | +74.12 | 1.233 |
| 5m | TP2 1.5R | 56 | 24 | -91.19 | 0.929 | 16 | 9 | +281.37 | 1.882 |
| 5m | TP3 2R | 54 | 22 | +26.04 | 1.020 | 14 | 5 | -16.69 | 0.960 |

**Decision:** No tested combination profitable in both periods. 5m TP3 September +26.04 points PF 1.02 is near flat and October negative. Positive 5m TP2 October has only 16 trades and negative September. Do not deploy.

Data export: `./target/release/kite-node native-amd-export 145894663 1 > /tmp/vce_crudeoilm_1m.json` (replace 1 with 3 or 5). Run `VCE_DATASET=vce_crudeoilm python3 -B research/vce_mojo_pnl.py`. Local data files are not tracked. Trading remains disabled.
