# SATS — Self-Aware Trend System: strategy reference

Rust port of **Self-Aware Trend System (SATS) v1.13.1** (WillyAlgoTrader, TradingView Pine),
set up for **CRUDEOILM** intraday (MIS). SATS is kept in the project as a second strategy;
Precision Sniper is the main one. In `config/portfolio-production.json` the SATS slot
`crudeoilm-sats-202610` is currently `enabled: false`.

| Item | Where |
|---|---|
| Signal and trade model (pure, no I/O) | `crates/sats` (`params`, `indicators`, `quality`, `supertrend`, `trade`, `learn`, `engine`) |
| Slot settings, execution modes, entry window and filter | `apps/kite-node/src/native_node/sats_config.rs` |
| Live strategy (orders, intrabar exits, safety) | `apps/kite-node/src/native_node/sats_strategy.rs`, `sats_trail.rs` |
| Live / paper runner | `apps/kite-node/src/native_node/sats_live.rs` |
| Backtest | `apps/kite-node/src/native_node/sats_backtest.rs` |
| Shipped config | `config/sats-crudeoilm.json` |
| Launch scripts | `deploy/run-sats-paper.sh`, `deploy/run-sats-live.sh` |

Written for kite-node 2.16.0; `tp_through_ticks` and bid/ask paper fills since 2.26.0.

---

## 1. The idea in one paragraph

SATS is an **adaptive SuperTrend**. Two ATR bands follow price; a close beyond the opposite
band **flips the trend**, and every flip is a trade signal (BUY on an up-flip, SELL on a
down-flip). What makes it "self-aware" is that the band width is not fixed: it adapts to a
**Trend Quality Index (TQI)** built from efficiency, volatility regime, structure and
momentum. Clean trends get a tighter trailing band, choppy markets a wider one. A trend can
also flip on a **quality collapse** (character flip) before price breaks the band. Each signal
gets a 0–100 **score**, and the trade has a pivot-based stop and three R-multiple targets that can
**scale with trend quality** (Dynamic TP).

---

## 2. Bar by bar: what the engine does

All on **closed bars** (`bar_minutes`, 5 minutes in the shipped config):

1. **Base values**: ATR (`atr_length`) and its baseline (SMA over `atr_baseline_length`),
   volatility ratio = ATR / baseline, RSI, efficiency ratio (ER) over `efficiency_window`.
   With `efficiency_weighted_atr`, the ATR used for bands and stops is ATR × (0.5 + 0.5 × ER):
   up to half as wide in choppy markets.
2. **TQI** (section 4).
3. **Adaptive SuperTrend** (section 3): band multipliers, bands with ratchet, price flip
   or character flip.
4. **Dynamic TP** multiples (section 6.3), pivots, **signal score** (section 5).
5. **Settle the old trade first** (stop, TP1, TP2, TP3, flip exit, timeout).
6. **Open a new trade** on a flip, if no model trade is open, the entry window allows it,
   and score / TQI clear the minimums.

---

## 3. The adaptive SuperTrend

### 3.1 Band multiplier

Start from the preset's base multiplier (section 7), then:

* **Legacy ER adaptation** (`use_adaptive`): × (1 + `adaptation_strength` × (0.5 − ER)).
  ER above 0.5 (efficient trend) narrows the bands, below 0.5 (chop) widens them.
* **TQI adaptation** (`use_tqi`): with Q = `quality_influence`,
  multiplier × (1 − Q + Q × (0.6 + 0.8 × (1 − TQI)^`quality_curve_power`)).
  High TQI narrows the bands, low TQI widens them: with Q = 1 the factor runs from 0.6× (TQI 1)
  to 1.4× (TQI 0); with the shipped Q = 0.4 it runs from 0.84× to 1.16×.
  `quality_curve_power` > 1 makes the narrowing kick in only at really high quality.
* **Asymmetric bands** (`asymmetric_bands`): the *active* band (the trailing stop side of the
  current trend) is tightened by up to 30 % × `asymmetry_strength` × TQI; the *passive* band
  (the one price must break to flip) is widened by up to 40 % × `asymmetry_strength` × TQI.
  In a high-quality trend, the trail hugs price and a flip needs a bigger move.
* **Smoothing** (`smooth_multipliers`): multipliers move only 15 % toward their new value each
  bar, so bands do not jump.

### 3.2 Bands, ratchet and flips

* Lower band = source − lower multiplier × ATR; upper band = source + upper multiplier × ATR
  (`source` = close in the shipped config).
* **Ratchet**: while price stays above it, the lower band can only rise (the upper band can only
  fall while price stays below it).
* **Price flip**: in a downtrend, a close above the previous upper band flips up (mirror for
  down).
* **Character flip** (`character_flip`, needs `use_tqi`): the trend is at least
  `char_flip_min_age` bars old, TQI was above `char_flip_high_tqi` within that window, TQI is now
  below `char_flip_low_tqi`, and price has moved against the trend over that window. This flips
  the trend on a **quality collapse** before the band is broken.

The signal reason in the logs says "Price band break", "Quality collapse" or both.

---

## 4. Trend Quality Index (TQI, 0–1)

A weighted average of four components, each 0–1:

| Component | Weight input (shipped) | How it is measured |
|---|---|---|
| Efficiency | `weight_efficiency` (0.35) | ER: net move / sum of bar-to-bar moves over `efficiency_window` |
| Volatility factor | `weight_vol_factor` (0.20) | `tqi_volatility_factor` "ATR regime": ATR / baseline mapped 0.6→0 … 1.8→1. "Volume activity": volume z-score −1→0 … 2→1 (weight dropped if no volume) |
| Structure | `weight_structure` (0.25) | Where the close sits in the `structure_window` high–low range: 1 at either extreme, 0 in the middle |
| Momentum | `weight_momentum` (0.20) | Share of up bars (in an up move) or down bars (in a down move) over `momentum_window` |

TQI drives: the band multipliers (section 3.1), the character flip (section 3.2), Dynamic TP
(section 6.3), and the optional entry filter `entry_filter.min_tqi`. With `use_tqi` off, TQI is a
constant 0.5 and these features are neutral. Market regime is shown as Trending (ER ≥ 0.5),
Mixed (≥ 0.25) or Choppy.

**Impact.** Raising a weight makes that aspect dominate the quality reading. For example,
more `weight_efficiency` rewards straight-line moves; more `weight_structure` rewards price
pressing range extremes. Longer windows give a slower, smoother TQI.

---

## 5. Signal score (0–100)

Computed for every flip; it **does not block entries** unless `entry_filter.min_score` > 0.
Points (normalised by the maximum available):

| Part | Max | Measures |
|---|---|---|
| Momentum | 17 | Move over the last 3 bars in the signal direction, 0.3–2.0 ATR |
| Efficiency | 17 | ER 0.15–0.70 |
| Volume | 17 | Volume z-score 0–3 (`score_use_volume`, needs volume) |
| RSI depth | 17 | How far RSI went beyond `rsi_oversold` (buy) / `rsi_overbought` (sell) within `rsi_memory_bars`: a flip after a real oversold dip scores higher (`score_use_rsi`) |
| Structure | 16 | Distance from the last valid pivot, in ATR: closer scores higher (`score_use_structure`, needs a pivot younger than `max_pivot_age_bars`) |
| Break depth | 16 | How far the close broke the old band, 0–1 ATR |

Pivots use `pivot_strength` bars on each side.

---

## 6. The trade

### 6.1 Entry

At the flip bar's close (± `slippage_ticks` in the model). Only one model trade at a time: a
flip while a trade is open first closes it (flip exit), then opens the new one on the same bar.

### 6.2 Stop

* Base: the last **valid pivot** low (long) / high (short), if one is younger than
  `max_pivot_age_bars` and on the right side of price; otherwise the signal bar's low / high.
* Stop = base − SL buffer × ATR, and at least SL buffer × ATR from the entry
  (SL buffer = the preset's SL multiplier; `sl_buffer_atr` only with "Custom").
* **Cap**: never further than max(`max_sl_distance_atr`, SL buffer) × ATR from the entry.
* Rejected if the stop is already breached at the signal close, or risk < `min_risk_ticks`.

### 6.3 Targets: Fixed or Dynamic

* `tp_mode` "Fixed": TP1 / TP2 / TP3 = entry ± R × `tp1_r` / `tp2_r` / `tp3_r` (sorted).
* `tp_mode` "Dynamic" (shipped): the R multiples are scaled by trend quality and volatility:
  scale = `dyn_tp_min_scale` + q × (`dyn_tp_max_scale` − `dyn_tp_min_scale`), where q is a
  weighted mix of TQI (`dyn_tp_tqi_influence`) and the volatility ratio (`dyn_tp_vol_influence`).
  Each multiple is then clamped between a floor (TP1: `dyn_tp1_floor_r`; TP2 / TP3 the same
  floor scaled by their fixed ratio) and `dyn_tp_ceiling_r`.
  Shipped: scale 0.5–2.0, so TP1 is anywhere from 0.5 R in poor conditions to 2 R in strong,
  volatile trends.

### 6.4 Exits (model)

Checked on each closed bar after the entry bar, **stop first**:

| Exit | Rule | Fill |
|---|---|---|
| SL | Bar touches the stop (whole remaining position) | Stop, or the open if gapped through |
| TP1 / TP2 | Touched: ⅓ of the position each | Target |
| TP3 | Touched: the rest | Target |
| Flip exit | Trend turned against the trade | Bar close |
| Timeout | Trade open `trade_timeout_bars` bars | Bar close |

The SATS model does **not** move its stop after TP1 / TP2 (no step stop). The slot's
`execution` settings decide how much of this is actually traded (section 8).

### 6.5 Self-learning (experimental, `auto_calibration`, shipped off)

When on, SATS tunes its own `quality_influence`. Closed trades are grouped into epochs of
`calibration_window` trades. If an epoch's average net R is below `calibration_bad_r`, Q moves
by `calibration_quality_step` (hill-climbing, reversing direction if the last move made things
worse), within `calibration_quality_floor` … `calibration_quality_ceiling`. A good epoch
(> `calibration_good_r`) holds Q. Leave it off for live trading: behaviour then depends on
the trade history in the warm-up window.

---

## 7. Presets: which JSON values are actually used

`params.preset` overrides five inputs: ATR length, base band multiplier, efficiency window,
RSI length and the SL buffer. **The JSON values `atr_length`, `base_band_width`,
`efficiency_window`, `rsi_length`, `sl_buffer_atr` are only used with `"Custom"`.**

| Preset | ATR | Base band × ATR | ER window | RSI | SL buffer × ATR |
|---|---|---|---|---|---|
| Scalping | 10 | 1.5 | 14 | 9 | 1.0 |
| Default | 14 | 2.0 | 20 | 14 | 1.5 |
| Swing | 21 | 2.5 | 30 | 21 | 2.0 |
| Crypto 24/7 | 14 | 2.8 | 20 | 14 | 2.5 |
| Custom | JSON | JSON | JSON | JSON | JSON |

`"Auto"` (shipped) picks by `bar_minutes`: up to 5 min → **Scalping**, up to 240 min →
Default, otherwise Swing. The shipped 5-minute config therefore runs **Scalping**: ATR 10,
band 1.5, ER 14, RSI 9, SL buffer 1.0. The JSON's 13 / 2.0 / 20 / 14 / 1.5 are ignored.
To use your own values, set `"preset": "Custom"`.

**Warm-up** = max(50, ATR + `atr_baseline_length` − 2, RSI + `rsi_memory_bars` − 1,
ER window, `momentum_window`, `structure_window`) bars. Shipped: 10 + 100 − 2 = **108
five-minute bars**. No signal is taken before that.

**Impact.**
* A wider base band gives fewer flips: fewer, longer trades and later exits.
* A larger SL buffer gives a wider stop, a bigger R and further targets.
* A longer ATR or ER window reacts more slowly.

---

## 8. From model events to orders (`execution`)

The model always thinks in thirds; `execution` decides what is traded for the slot's `lots`.

| `exit_mode` | Behaviour | Constraints |
|---|---|---|
| `"single"` (shipped) | The whole position exits at `single_exit_at` (TP1 / TP2 / TP3), or earlier on SL, flip or timeout | Any lots |
| `"thirds"` | ⅓ of the lots at TP1, ⅓ at TP2, the rest at TP3 / SL / flip / timeout, as in the script | `lots` divisible by 3; no `exchange_stop_loss` |
| `"trail"` | No profit target. At TP1 the stop moves to breakeven (actual fill ± `trail.breakeven_offset_points`) and then, with `trail.supertrend_trail`, follows the SuperTrend line at each bar close (the tighter of the two). Exits on the stop, a trend flip, a timeout or the square-off | Any lots |

Other `execution` inputs:

| Key | Shipped | Effect |
|---|---|---|
| `single_exit_at` | TP1 | Target for single mode. TP1 gives a high hit rate and small wins; TP3 gives few hits and large wins |
| `intrabar_exits` | true | Single mode only. True: SL and target are checked on every tick (bid for long, ask for short) and exit immediately. False: checked on the closed bar's high / low and exited at the bar close |
| `exchange_stop_loss` | false | Rests a reduce-only SL-M at Zerodha at SATS's stop after the entry fills, so the position is protected if the program dies. Any other exit first cancels it (confirmed), then sends the market exit. Not allowed with thirds. Off by choice: all exits come from the program |
| `trail.breakeven_offset_points` | 3.0 | Trail mode: breakeven stop this many points in your favour (covers costs) |
| `trail.supertrend_trail` | true | Trail mode: also trail on the SuperTrend line after TP1 |
| `round_trip_cost_points` | 2.0 | Backtest costs per lot per round trip, in points |
| `slippage_points_per_side` | 0.5 | Backtest slippage per order |

**Note (single mode).** After the position exits at TP1, SATS's own model trade keeps running
until its TP3, SL, flip or timeout. A new entry needs a new flip, and a flip always closes the
model trade first, so in practice no signal is lost.

---

## 9. Entry window and entry filter (slot level)

| Key | Shipped | Effect |
|---|---|---|
| `entry_window` | 15:00–23:00 | New entries only on signal bars closing at or after `from` and before `to` (IST). Omit for the whole session. `to` must not be after `live.square_off`. Exits are never blocked |
| `entry_filter.min_score` | 0 | Minimum signal score (0–100) for a new entry. 0 = off |
| `entry_filter.min_tqi` | 0 | Minimum TQI (0–1) for a new entry. 0 = off |
| `live.square_off` | 23:15 | Flattens on that bar and blocks entries for the rest of the day. Must fall on a `bar_minutes` boundary |

**Impact.** The window cuts the quiet morning session (crude moves mostly in the
European / US hours). The filters trade fewer, higher-quality flips; tune them with the backtest's
score and TQI columns.

---

## 10. Live execution

* **Start checks** are the same as Sniper: session day open, at least 5 minutes before the
  square-off, contract not expired, Kite instrument master matches the slot, and enough
  broker-finalised warm-up history.
* **Orders**: MARKET with Kite's automatic market protection, MIS, DAY. Entries open `lots`;
  exits close what `execution` says (section 8).
* **Flips use two orders**: the exit first, then, once the position is flat, the opposite entry.
  If the exit is not filled within **30 s**, the entry is cancelled and the strategy halts.
  (Sniper uses a single flip order; SATS does not.)
* **Warm-up bars never trade.** Exits of a model trade this run did not open are ignored.
* **Intrabar** (single mode with `intrabar_exits`): stop and target are checked on every quote.
  Otherwise SATS is bar-close only.
* **Late bars**: a bar delivered more than 90 s after its close does not open a trade.
* **Halts** (no new entries, flatten): order rejected or denied, a position the strategy did not
  expect, the flip exit not filled in 30 s, an SL-M cancel not confirmed in 15 s
  (`exchange_stop_loss` only), a feed fault or a stop request.
* The adapter refuses orders if the account holds any position or open order it does not own
  (see `PORTFOLIO_SLOTS.md`).

---

## 11. Backtest

`native-sats-backtest PORTFOLIO SLOT FROM TO` uses the slot's JSON and Kite history at the
`candle_sources` interval for `bar_minutes`. Results go to `backtest_results/sats/…`:
`trades.csv` has, per trade, the model and executed entry and exit, the exit reason, **score**,
**TQI**, bars held, R, points and ₹ after `round_trip_cost_points` and
`slippage_points_per_side`.

`candle_sources` maps a candle size to the Kite history interval it is built from (for example,
6-minute candles from 3-minute history). Live candles are always built from WebSocket ticks at
`bar_minutes`.

The model's own `commission_pct_per_fill` and `slippage_ticks` (shipped 0) only change SATS's
internal R accounting and the self-learning; real costs are modelled by the `execution` cost
inputs.

`tp_through_ticks` (shipped 1; the script's rule is 0) makes a take-profit count only when the
bar trades that many ticks **through** it: high above a long's target, low below a short's. A
plain touch at the last-traded price is often not reachable at the bid/ask. On 9 Oct (live) the
5m bar's low touched TP1 8855 while the ask never got there; the model counted TP1 at the bar
close and the market exit filled 8863, 8 points worse. Live, a target is still taken intrabar
as soon as the bid (long) or ask (short) reaches it.

---

## 12. Remaining `params` reference

Inputs not covered above (shipped values):

| Key | Shipped | Meaning |
|---|---|---|
| `source` | close | Price fed to the bands (open, high, low, close, hl2, hlc3, ohlc4, hlcc4) |
| `use_adaptive`, `adaptation_strength` | true, 0.5 | Legacy ER band adaptation (section 3.1) |
| `atr_baseline_length` | 100 | Baseline for the volatility ratio; also sets the warm-up |
| `use_tqi`, `quality_influence`, `quality_curve_power` | true, 0.4, 1.5 | TQI band adaptation (section 3.1). With Q = 0.4 the TQI factor runs from 0.84× (TQI 1) to 1.16× (TQI 0) |
| `smooth_multipliers` | true | 15 % per bar smoothing of the multipliers |
| `asymmetric_bands`, `asymmetry_strength` | true, 0.5 | Active band up to 15 % tighter, passive up to 20 % wider at TQI 1 |
| `efficiency_weighted_atr` | true | ATR × (0.5 + 0.5 × ER) for bands and stops |
| `character_flip`, `char_flip_min_age`, `char_flip_high_tqi`, `char_flip_low_tqi` | true, 5, 0.55, 0.25 | Quality-collapse flips (section 3.2). Lower `char_flip_low_tqi` or higher `char_flip_high_tqi` gives fewer early flips |
| `weight_*`, `structure_window`, `momentum_window`, `tqi_volatility_factor` | 0.35 / 0.2 / 0.25 / 0.2, 20, 10, ATR regime | TQI (section 4) |
| `score_use_structure`, `pivot_strength`, `max_pivot_age_bars` | true, 3, 100 | Pivots for the score and the stop |
| `score_use_rsi`, `rsi_overbought`, `rsi_oversold`, `rsi_memory_bars` | true, 70, 30, 20 | RSI-depth score |
| `score_use_volume`, `volume_z_window` | true, 20 | Volume score and the volume z-score |
| `max_sl_distance_atr` | 4.0 | Stop distance cap in ATR |
| `tp_mode`, `tp1_r` … `tp3_r` | Dynamic, 1 / 2 / 3 | Section 6.3 |
| `trade_timeout_bars` | 100 | Model trade closes at the bar close after this many bars (100 × 5 min ≈ 8 h, so on 5 min the square-off usually comes first) |
| `dyn_tp_*` | 0.6, 0.4, 0.5, 2.0, 0.5, 8.0 | Dynamic TP influences, scale range, TP1 floor, ceiling |
| `auto_calibration`, `calibration_*` | false, … | Section 6.5 |
| `min_risk_ticks` | 2 | Minimum risk in ticks |

All ranges are validated at load time with the script's own limits (for example
`base_band_width` 0.5–5.0, `tp*_r` 0.5–10); an out-of-range value refuses to start.

---

## 13. Limitations

* **Ignored JSON values under a preset.** With "Auto" or a named preset, five inputs come from
  the preset table (section 7). Easy to miss when tuning.
* **Bar-close entries.** Entries only at a flip bar's close (first tick of the next bar).
* **Two-order flip.** A fill wait sits between the exit and the new entry, and it costs one
  more brokerage than Sniper's flip.
* **No step stop in the model.** In single and thirds modes the stop stays at the initial level
  until exit; only trail mode moves it.
* **Trend-following in chop.** SuperTrend systems whipsaw in ranges; TQI widens the bands but
  does not stop flips. Use `entry_window` and `entry_filter` to trade less in chop.
* **Self-learning depends on history.** Keep it off live.
* **No broker-side stop by default** (`exchange_stop_loss` false): protection depends on the
  program running.
* **Currently disabled** in the production portfolio. Backtests and paper runs work as is; a
  live run needs the slot enabled and rolled to the current contract (see `PORTFOLIO_SLOTS.md`),
  and cannot run live alongside Sniper on the same account.

---

## 14. Running

| Task | Command |
|---|---|
| Backtest | `./target/release/kite-node native-sats-backtest config/portfolio-production.json crudeoilm-sats-202610 2026-09-10 2026-10-09` |
| Paper | `deploy/run-sats-paper.sh` |
| Live preflight | `./target/release/kite-node native-sats-live-preflight config/portfolio-production.json crudeoilm-sats-202610 config/kite-production.json` |
| Live (types LIVE) | `deploy/run-sats-live.sh` |
| Redis status | `redis-utility/sats-redis-status.sh` |
