# Precision Sniper — strategy reference

Main live strategy of this project. Rust port of **Precision Sniper v2.1.0** (WillyAlgoTrader,
TradingView Pine), traded on **CRUDEOILM** (MCX crude oil mini futures) as an intraday MIS
strategy through the native Kite execution client.

| Item | Where |
|---|---|
| Signal and trade model (pure, no I/O) | `crates/sniper` (`engine.rs`, `params.rs`, `nt.rs`) |
| Indicators | `crates/sniper/src/nt.rs` on `nautilus-indicators` 0.64; `ta.rs` = old Pine-exact port, kept as a backup file (not used) |
| Slot settings, entry window, costs | `apps/kite-node/src/native_node/sniper_config.rs` |
| Live strategy (orders, intrabar exits, safety) | `apps/kite-node/src/native_node/sniper_strategy.rs` |
| Live / paper runner | `apps/kite-node/src/native_node/sniper_live.rs` |
| Backtest | `apps/kite-node/src/native_node/sniper_backtest.rs` |
| Shipped config | `config/sniper-crudeoilm.json` |
| Launch scripts | `deploy/run-sniper-paper.sh`, `deploy/run-sniper-live.sh` |

Written for kite-node 2.22.0 / kite-adapter 0.5.0 / sniper 2.1.0+3.

---

## 1. The idea in one paragraph

A trade starts when the **fast EMA crosses the slow EMA** and price is already on the
crossing side of **both** EMAs (momentum is mandatory). The cross is only a *candidate*: it
must also collect enough **evidence points** (trend EMA, RSI band, MACD histogram and slope,
ADX/DMI, volume spike, session VWAP), must not happen in **high volatility**, and must not be
at an **RSI extreme**. The entry is at the **close of the signal bar**. The stop is the wider
of an **ATR stop** and a **swing-structure stop** (capped). Three targets sit at fixed
multiples of the initial risk (R). Lots are scaled out at TP1 and TP2, the rest at TP3, and a
**step stop** ratchets the stop up behind each target hit. An accepted **opposite signal
reverses** the position.

---

## 2. Bar by bar: what the engine does

Everything is evaluated on **closed bars** (`bar_minutes`, 3 minutes in the shipped config).
Within one bar the order is fixed:

1. **Indicators update**: EMA fast / slow / trend, ATR, the 42-bar mean of ATR, RSI,
   MACD histogram (12/26/9), DMI/ADX (14/14), 20-bar volume mean, session VWAP (resets at
   each IST calendar day).
2. **Candidate lifecycle**: a fresh cross creates a pending candidate in its direction.
   The candidate is checked by the **gate** (section 4). It is accepted, kept waiting
   (only if `confirm_window` > 0), or dropped with a reject reason.
3. **Open trade management** (only on bars after the entry bar), in this order:
   * **stop first**, using the stop that was in force at the start of the bar. If the bar's
     low (long) / high (short) touches it, the trade closes at the stop, or at the open if the
     bar gapped through. If a target was also touched on that bar, the trade is flagged
     *ambiguous*, and the stop still wins;
   * otherwise **targets**: TP1, TP2, TP3 touched on this bar are applied in order (partials);
     TP3 closes the rest when `full_tp3` is on;
   * otherwise a **reversal**: an accepted opposite signal on this bar closes the trade at
     the bar close;
   * otherwise the **step stop** moves for the *next* bar (section 6).
4. **New entry** at the bar close, if a signal was accepted and no trade is open (a reversal
   frees the slot on the same bar, so exit and new entry happen together).

Live trading adds tick-level checks between bar closes (section 8).

---

### 2.1 Indicators

All indicators (EMA fast/slow/trend, ATR and its 42-bar mean, RSI, MACD 12/26/9 histogram,
DMI/ADX 14/14, 20-bar volume mean, session VWAP) come from **`nautilus-indicators` 0.64**
through thin streaming wrappers in `crates/sniper/src/nt.rs`: EMA, SMA, RSI (Wilder),
MACD (EMA), ATR (Wilder) and VWAP are the library's. ADX is not in the library, so it is
built from its `DirectionalMovement`, `AverageTrueRange` and `WilderMovingAverage` with
Pine's `fixnan`. VWAP gets the bar time shifted to IST so it resets at the IST day.

The earlier hand-written Pine `ta.*` port stays in `crates/sniper/src/ta.rs` as a backup
file; the engine does not use it. Against that port (and Pine) the library differs only in
the warm-up: EMA / Wilder averages seed with the first value instead of the SMA of the first
`length` values, and RSI counts the first bar as a zero change. The gap shrinks every bar and
is gone well before the engine's own warm-up (trend EMA × `warmup_mult` bars) ends. The test
`nautilus_backend_converges_to_the_pine_port` checks every indicator against the backup.

## 3. Presets: what really sets the lengths

`params.preset` decides the EMA lengths, RSI / ATR lengths, the minimum score and the ATR
stop multiplier. The `ema_fast`, `ema_slow`, `ema_trend`, `rsi_len`, `atr_len`,
`min_score`, `sl_mult` fields in the JSON are used **only with `"Custom"`**; with any
other preset they are ignored.

| Preset | EMA fast / slow / trend | RSI | ATR | Min score (of 10) | SL × ATR |
|---|---|---|---|---|---|
| Scalping | 5 / 13 / 34 | 8 | 10 | 4 | 0.8 |
| Aggressive | 8 / 18 / 50 | 11 | 12 | 3 | 1.2 |
| Default | 9 / 21 / 55 | 13 | 14 | 5 | 1.5 |
| **Conservative** (shipped) | **12 / 26 / 89** | **14** | **14** | **7** | **2.0** |
| Swing | 13 / 34 / 89 | 21 | 20 | 6 | 2.5 |
| Crypto 24/7 | 9 / 21 / 55 | 14 | 20 | 5 | 2.0 |
| Custom | from the JSON fields | | | | |

`"Auto"` picks by `timeframe_minutes`: up to 5 min → Scalping, up to 60 min → Default,
under 4 h → Conservative, otherwise Swing. The shipped config sets Conservative explicitly,
so on 3-minute bars it does **not** get Scalping.

**Impact.** Shorter EMAs cross more often: more trades, more whipsaws, earlier entries.
A lower minimum score accepts weaker evidence. A smaller SL multiplier means a tighter stop,
so a smaller risk (R). Because the targets are multiples of R, they come closer too, which
gives a higher hit rate but smaller wins and more stop-outs from noise.

Validation: fast < slow < trend must hold after resolution.

---

## 4. Entry gate: when a crossover becomes a trade

A candidate is checked in this order; the first failing check is the reject reason (shown in
the backtest summary as "rejected" counts).

| # | Check | Reject reason |
|---|---|---|
| 1 | Model warmed up and every indicator has a value | Data not ready / warmup |
| 2 | Entries allowed for this bar (entry window, blackouts, halt, square-off) | Outside entry window |
| 4 | Not high volatility, when `vol_mode` = "Skip Signals" | High volatility blocks entry |
| 5 | **Momentum**: long needs close > fast EMA and > slow EMA (short: below both) | Price momentum lost |
| 6 | RSI not extreme: long needs RSI < 75, short needs RSI > 25 | RSI extreme |
| 8 | **Evidence score** ≥ required ratio × maximum score | Insufficient score |
| 9 | Stop / target geometry valid (risk ≥ `min_risk_ticks`, targets ordered, prices > 0) | Invalid tick/risk/target geometry |
| 10 | Structure stop not over the cap, when `structure_policy` = "Skip entry" | Structure exceeds cap |
| 11 | Close within `max_extension` × ATR of the fast EMA (if `max_extension` > 0) | Too far from fast EMA |
| 12 | No trade already open in the same direction | Same direction already open |

### 4.1 Evidence score

One point each, for a long (a short mirrors every condition):

1. close above the **trend EMA**;
2. **RSI between 50 and 75**;
3. **MACD histogram > 0**;
4. MACD histogram **rising** (higher than the previous bar);
5. **ADX ≥ 20 and DI+ > DI−** (a real trend in the trade's direction);
6. **volume > 1.2 ×** its 20-bar average (only if the feed has volume);
7. close above the **session VWAP** (only if `vwap` is on and the feed has volume).

Maximum score = 5 + 1 (volume available) + 1 (VWAP on). For CRUDEOILM with volume, that's 7.

**Required ratio** = the largest of:
* the preset's minimum score / 10 (Conservative: 0.7);
* 0.5 when `hide_c` is on;
* 0.65 for `grade_filter` "A+ and A", 0.8 for "A+ Only", 0 for "All".

Shipped config: 0.7 × 7 = 4.9, so **a signal needs 5 of the 7 points**.

**Grade** (logged and shown on the dashboard, used by `grade_filter`): score / max ≥ 0.8 is A+,
≥ 0.65 is A, ≥ 0.5 is B, otherwise C. With the shipped settings every accepted trade is A or A+.

### 4.2 Candidate waiting: `confirm_window`

* `0` (shipped): the cross bar itself must pass the gate, otherwise the candidate dies.
* `N > 0`: a candidate that fails any check may wait up to N more bars for all checks to
  pass. Losing momentum (check 5), a closed entry window, or a trade already open in the same
  direction kills it immediately.

**Impact.** A larger window catches crosses that confirm a bar or two later, giving more
trades at worse (later) prices.

---

## 5. Stop and targets

All prices are rounded to `tick_size` (1.0 for CRUDEOILM). Entry = signal bar close.

### 5.1 Stop

* **ATR stop**: entry ∓ ATR × SL multiplier (× `vol_widen` in high volatility when
  `vol_mode` = "Widen SL").
* **Structure stop** (`structure` on): the lowest low (long) / highest high (short) of the
  last `swing_lookback` + 1 bars, ∓ 0.2 × ATR.
* With structure on, the **wider** of the two is used: the stop sits beyond the swing
  *and* at least an ATR stop away.
* **Cap**: if that stop is further than 1.5 × the ATR-stop distance, it is either
  pulled in to the cap and marked "ATR capped" (`structure_policy` "Cap and flag",
  shipped), or the entry is refused ("Skip entry").

**Risk (R)** = |entry − stop|. It must be at least `min_risk_ticks` ticks.

### 5.2 Targets

TP1 / TP2 / TP3 = entry ± R × `tp1_r` / `tp2_r` / `tp3_r` (shipped 1 / 2 / 3).
They must satisfy TP1 < TP2 < TP3.

### 5.3 Impact of the stop and target inputs

| Input | Shipped | Raising it | Lowering it |
|---|---|---|---|
| preset SL × ATR | 2.0 | Wider stop, larger R, targets further away; fewer stop-outs, smaller hit rate on targets, larger ₹ loss per stop | Tighter stop, more noise stop-outs, targets closer |
| `structure` | true | (on) stop also respects the recent swing: usually wider, fewer stop-hunts | (off) pure ATR stop, tighter and more mechanical |
| `swing_lookback` | 10 | Swing taken over more bars, usually further away, so wider stops (until the cap) | Nearer swing, tighter stop |
| `structure_policy` | Cap and flag | "Skip entry" refuses trades whose swing stop would be beyond the cap: fewer, tighter-risk trades | — |
| `vol_mode` | Skip Signals | "Widen SL": trades in high volatility with stop × `vol_widen`; "Off": volatility ignored | — |
| `vol_threshold` | 1.3 | Fewer bars count as high volatility, so more trades are allowed | More bars are blocked or widened |
| `vol_widen` | 1.5 | Wider stops in volatile bars (Widen SL mode only) | — |
| `tp1_r` / `tp2_r` / `tp3_r` | 1 / 2 / 3 | Targets further away: bigger wins, fewer hits, more trades end on stop or reversal | Closer targets: more partial profits, less upside captured |
| `min_risk_ticks` | 2 | Refuses trades with a very small risk (protects against near-zero R) | — |
| `positive_only` | true | Refuses plans with any level ≤ 0 (safety for odd prices) | — |

---

## 6. Position management: scale-out, step stop, reversal

### 6.1 Scale-out with whole lots

`lots`, `tp1_lots`, `tp2_lots` (slot config, not `params`) decide the size:

| Event | Shipped (3 / 1 / 1) | Position after |
|---|---|---|
| Entry | buy or sell 3 lots | 3 |
| TP1 touched | close 1 lot | 2 |
| TP2 touched | close 1 lot | 1 |
| TP3 touched (`full_tp3` on) | close the rest | 0 |
| Stop, step stop, reversal, square-off | close whatever is open | 0 |

`tp1_lots + tp2_lots` must leave at least one lot for TP3. `tp1_lots = tp2_lots = 0` is the
whole-position model: one entry order and one exit order per trade. (`params.tp1_close_fraction`
/ `tp2_close_fraction` must stay 0; the fractions are derived from the lots.)

If price jumps past TP1 and TP2 on the same tick, the strategy sends **one** order for both
tranches (for example, SELL 2 from +3).

**Impact.** Larger early tranches lock profit sooner and reduce variance, but leave less
size for the TP3 runner. Every tranche is a separate order, so each costs one brokerage.

### 6.2 Step stop (`step_stop`, shipped on)

At each bar close the stop ratchets behind the targets already reached:

| Reached | Stop moves to |
|---|---|
| TP1 | entry (breakeven) |
| TP2 | TP1 |
| TP3 (only when `full_tp3` is off) | TP2 |

The stop only moves in the trade's favour, and only at a **bar close**. A TP hit in the middle
of a bar does not move the stop until that bar closes. A stop hit after a move is reported as
"Step stop" instead of "SL".

**Impact.** On, it protects partial profits: a trade that reached TP1 can no longer lose
on the remaining lots (apart from slippage). Off, the remaining lots keep the original stop
until TP3, the reversal or the square-off: bigger runners, but winners can turn into losers.

### 6.3 `full_tp3`

* on (shipped): TP3 closes everything that is still open;
* off: TP3 only counts as reached (and the step stop moves to TP2); the last lots keep
  running until the stop, a reversal or the square-off.

### 6.4 Reversal

An accepted opposite signal (it must pass the full gate, including the entry window) closes
the open trade at the bar close and opens the new one on the same bar. A same-direction
signal while a trade is open is refused.

Live, the reversal is **one flip order** sized "lots still open + new lots":

| Open when the opposite signal arrives | Order |
|---|---|
| +3 (no TP hit) | SELL 6 → −3 |
| +2 (after TP1) | SELL 5 → −3 |
| +1 (after TP2) | SELL 4 → −3 |

That saves one brokerage and the wait for a separate exit fill. If Kite fills only part of a
flip and the position ends **smaller on the new side**, the strategy does not top it up; it
halts and flattens. If the part-fill leaves you still on the old side, the next order is another
flip for the rest.

Because a reversal needs an *accepted* signal, an opposite cross during a blackout, after
`entries_until`, or while halted does **not** close the trade; the stop, targets and square-off
still do.

---

## 7. Time and session inputs (slot config)

| Input | Shipped | Effect |
|---|---|---|
| `bar_minutes` | 3 | Candle size. Allowed 3, 5, 10, 15, 30. Must equal `params.timeframe_minutes`. Live bars are built from WebSocket ticks; history comes from the matching Kite interval. |
| `entries_until` | 23:00 | No new entries on signal bars closing at or after this time (IST). Exits continue. |
| `square_off` | 23:15 | On the bar closing at or after this time: flatten everything, no new entries for the rest of the day. Must be before the MCX session close. |
| `entry_blackouts` | 17:30–19:30 | IST windows `[from, to)` with no new entries, by signal-bar close. The shipped window covers the 08:30 ET US releases (CPI, NFP, jobless claims): 18:00 IST while the US is on daylight time, 19:00 IST in winter. It does **not** cover the weekly EIA crude inventory (10:30 ET = 20:00 / 21:00 IST); add a window if you want that too. Exits are never blocked. |
| (fixed) | 09:00 | No entries before 09:00 IST. |

**Impact.** Wider windows give more trades, including news spikes. Each blackout removes the
most whipsaw-prone hours; the backtest prints the hour breakdown (`by_hour_ist`) to tune them.

---

## 8. Live execution

### 8.1 Start of day and warm-up

The runner (`native-sniper-live` / `native-sniper-paper`) refuses to start unless:
* today is an MCX session day in `config/mcx-session-calendar.json` and the session is open;
* at least 5 minutes remain before `square_off`;
* the slot's contract has not expired (`rollover.expected_expiry`) and the Kite instrument
  master matches the slot's symbol, token and expiry;
* enough broker-finalised history is available: max(trend EMA × `warmup_mult`, ATR + 42) + 20
  bars. Shipped: 89 × 3 + 20 = **287 three-minute bars**, about one MCX session, fetched from
  up to 10 days of Kite history.

Warm-up bars only feed the model, never orders. A model trade still open when the live stream
starts is closed in the model ("Live start"), so the first live trade always starts from flat.

### 8.2 Orders

* Every order is a **MARKET** order with Kite's automatic market protection, product **MIS**,
  validity DAY.
* **One order at a time** moves the position toward the model's target. The next order waits
  for the previous one to be resolved (filled, rejected or cancelled).
* Kinds: entry from flat, reduce-only exit (partial or full), or one flip order (section 6.4).
  It never adds to an open position.
* There is **no resting stop order at Zerodha**. All exits are sent by the program.

### 8.3 Intrabar exits (on every quote)

Between bar closes, every quote is checked on the **executable** price: bid for a long, ask
for a short.
1. **Stop first**: if the price is at or through the current stop, the whole position is closed
   ("SL", or "Step stop" when it has moved).
2. Otherwise the **highest target** reached (TP1 / TP2 / TP3) is applied, closing its tranche.

The model is updated at the same moment, so model and broker position never disagree. The step
stop still moves only at the bar close, exactly as in the Pine script.

### 8.4 Bar delivery

* A bar delivered more than 10 s after its close came from the startup backfill (shown on the
  dashboard).
* A bar delivered more than **90 s** late never opens a trade ("exits only").

### 8.5 Safety stops (halt and flatten)

Any of these halts new entries, sets the target to flat and flattens:
* an order **rejected** by Kite or **denied** by the adapter's pre-order check;
* an order not resolved by the broker within **30 s** (the run is not reported flat while that
  order may still be live at Kite);
* the model wanting more lots on the same side than are open (for example, a partly filled flip);
* an exit order cancelled by Kite 5 times in a row;
* a run fault or a stop request (Ctrl+C, SIGTERM, SIGHUP). Run faults: a feed gap or invalid
  packet, the market-data feed stopping with an error or going stale, or the Kite order client
  stopping itself (order stream lost, session expired, failed reconciliation). On a fault the
  runner flattens, waits up to 60 s for flat and **stops the process** (2.22.0; before that a
  faulted run sat idle, holding the slot lock, until the square-off window passed).

Flattening never gives up (2.22.0): the first 3 attempts go out on consecutive guard ticks,
then one every 10 s until flat. Until 2.22.0 it stopped after 3 attempts for the rest of the
day, which denials caused by an unresolved earlier order could use up in about 3 s.

**Orders cancelled by Kite** (for example when market protection cannot be met), 2.22.0:
* an entry or flip is **not** re-sent: that trade is abandoned and anything that filled is
  flattened; later signals still trade;
* an exit is re-sent on the next tick, up to 5 times in a row, then the strategy halts and
  keeps flattening. Until 2.22.0 any cancelled order was silently re-sent on every tick.

The adapter also refuses any order if the Zerodha account holds another position or an open
order it does not own (see `PORTFOLIO_SLOTS.md`).

---

## 9. Costs and the backtest

`native-sniper-backtest CONFIG FROM TO` replays Kite history bar by bar and writes
`backtest_results/sniper/<symbol>/<timestamp>/` (`summary.json`, `summary.txt`, `trades.csv`).

| Input | Shipped | Used for |
|---|---|---|
| `point_value` | 10 | ₹ per 1.0 price move per lot (CRUDEOILM) |
| `round_trip_cost_points` | 2.0 | Per lot per round trip, excluding brokerage (exchange, STT/CTT, stamp, GST), in points |
| `brokerage_per_order` | 20 | ₹ per executed order. A trade pays one per order: entry, each partial, final exit |
| `slippage_points_per_side` | 1.0 | Adverse points on every fill (entry and each exit) |

How the backtest fills:
* entries at the signal-bar close ± slippage;
* exits on bar OHLC, stop first; targets fill at the target ± slippage;
* a reversal's flip order is charged once, to the trade it closes (the next trade shows
  `flip_entry = true` and one order fewer);
* the square-off closes at the bar close.

It reports trades, win rate, net ₹, profit factor, average win / loss, max drawdown, model R,
breakdowns by day, side, exit reason, grade and IST hour, candidate count and reject reasons.

**Backtest vs live.** Live exits are on ticks, not bar OHLC: a stop or target touched and
reversed inside a bar exits live but may not in the backtest, and fills are at the market price
(market protection), not exactly at the level. Treat backtest numbers as a model; compare with
paper and live logs.

---

## 10. Full input reference (`config/sniper-crudeoilm.json`)

### 10.1 Slot level

| Key | Shipped | Meaning |
|---|---|---|
| `strategy` | "sniper" | Must be "sniper" (guards against the wrong file) |
| `symbol`, `instrument_token` | CRUDEOILM26OCTFUT, 145894663 | Must match the portfolio slot exactly; change both at rollover |
| `bar_minutes` | 3 | Candle size (3, 5, 10, 15, 30) |
| `lots` | 3 | Position size per trade (1..100; the broker settings `max_lots` must be ≥ this) |
| `tp1_lots`, `tp2_lots` | 1, 1 | Lots closed at TP1 / TP2 |
| `point_value` | 10 | ₹ per point per lot (backtest P&L) |
| `round_trip_cost_points` | 2.0 | Backtest costs per lot per round trip |
| `brokerage_per_order` | 20 | Backtest brokerage per order |
| `slippage_points_per_side` | 1.0 | Backtest slippage per fill |
| `live.session_calendar` | config/mcx-session-calendar.json | Holidays and special sessions |
| `live.product` | MIS | Must be MIS |
| `entries_until`, `square_off`, `entry_blackouts` | 23:00, 23:15, 17:30–19:30 | Section 7 |

### 10.2 `params` (the Pine inputs)

| Key | Shipped | Meaning and impact |
|---|---|---|
| `preset` | Conservative | Lengths, minimum score, SL multiplier (section 3) |
| `timeframe_minutes` | 3 | Must equal `bar_minutes`; also drives "Auto" |
| `vwap` | true | Adds the VWAP point to the score (maximum 7 instead of 6). Off: one point less to collect, but also a lower maximum |
| `warmup_mult` | 3 | Warm-up = trend EMA × this (2..10). Higher: steadier EMAs before trading, longer history needed |
| `ema_fast` … `sl_mult` | (unused) | Only read with preset "Custom" |
| `grade_filter` | All | "A+ and A" (≥ 65 %) or "A+ Only" (≥ 80 %). It only matters when it is above the preset's minimum: with Conservative (70 %) "A+ and A" changes nothing, "A+ Only" needs 6 of 7 points |
| `hide_c` | true | Refuse grade C (< 50 %). Matters only if the preset minimum is below 0.5 |
| `vol_mode` | Skip Signals | High volatility = ATR > `vol_threshold` × its 42-bar mean. Skip, Widen SL, or Off |
| `vol_threshold` | 1.3 | Section 5.3 |
| `vol_widen` | 1.5 | Section 5.3 |
| `confirm_window` | 0 | Section 4.2 |
| `max_extension` | 0.0 | Off. E.g. 1.5 refuses entries more than 1.5 ATR away from the fast EMA (avoids chasing) |
| `tp1_r`, `tp2_r`, `tp3_r` | 1, 2, 3 | Target R multiples (TP1 < TP2 < TP3) |
| `step_stop` | true | Section 6.2 |
| `full_tp3` | true | Section 6.3 |
| `structure` | true | Section 5.1 |
| `swing_lookback` | 10 | Bars for the swing stop (≥ 3) |
| `structure_policy` | Cap and flag | Section 5.1 |
| `min_risk_ticks` | 2 | Minimum risk in ticks |
| `tick_size` | 1.0 | CRUDEOILM tick (₹1) |
| `positive_only` | true | Refuse plans with a non-positive price level |

Fixed constants (not configurable): ADX trend level 20, ATR mean 42 bars, volume mean 20 bars,
volume spike 1.2×, structure buffer 0.2 ATR, structure cap 1.5×, RSI band 25 / 75, MACD 12/26/9,
DMI 14/14.

---

## 11. Limitations

* **No broker-side stop.** If the program, the box or the network dies with a position open,
  nothing protects it until the program is back. (The adapter has a resting SL-M feature; it is
  deliberately left disabled for this strategy.)
* **Market orders only.** Fills are at market with Kite's automatic protection; in a fast move
  the fill can be several points from the level that triggered it.
* **Bar-close entries.** Entries happen only at a bar close; a move inside the signal bar is
  missed. The bar close is detected on the first tick of the next bar.
* **Step stop at bar close only.** After TP1 is hit mid-bar, the breakeven stop is not active
  until that bar closes.
* **Partly filled flip halts the day.** Rare for 3–6 lots of CRUDEOILM, but possible.
* **One contract per slot.** Rollover is manual: a new slot ID, symbol and token each month.
* **VWAP resets at the IST calendar day.** MCX evening hours are in the same day, so this
  matches the session, but it is not a rolling VWAP.
* **Not ported from the Pine script:** higher-timeframe bias, session filter, dashboard,
  journal, alerts, position calculator.
* **Backtest uses bar OHLC.** Intrabar sequence is unknown; an ambiguous bar (stop and target
  both touched) is booked as a stop.

---

## 12. Running

| Task | Command |
|---|---|
| Backtest | `./target/release/kite-node native-sniper-backtest config/sniper-crudeoilm.json 2026-09-10 2026-10-09` |
| Paper (live data, mock fills) | `deploy/run-sniper-paper.sh` |
| Live preflight only | `./target/release/kite-node native-sniper-live-preflight config/portfolio-production.json crudeoilm-sniper-202610 config/kite-production.json` |
| Live (real orders, asks you to type LIVE) | `deploy/run-sniper-live.sh` |
| Slot status (lock, order budget, today's logs) | `redis-utility/sniper-redis-status.sh` |

Tuning loop: change one input in the JSON, backtest the same date range, compare
`summary.txt` (net ₹, profit factor, drawdown, reject counts), then paper-trade before live.
