# MCX Crude PURE Squeeze Momentum v2.28.3 — Rust / Nautilus Port

The active strategy is **MCX Crude PURE Squeeze Momentum v2.28.3**. No Trend Ribbon, ALMA, FAST/Shock, WaveTrend, Chandelier, previous-day trend, RB/RS re-entry or pre-close reversal participates in trade decisions.

## Squeeze calculation

The Rust indicator mirrors the supplied LazyBear-derived Pine calculation. Defaults are BB length 20, KC length 20, KC multiplier 1.5 and TrueRange enabled. As in the supplied Pine, BB deviation intentionally uses the KC multiplier.

Momentum is `linreg(close - avg(avg(highest(high, KC), lowest(low, KC)), sma(close, KC)), KC, 0)`.

## Confirmed-bar state machine

All persistent state and all trade actions update only on a confirmed candle close. Tick data is used only to preview the forming candle in the dashboard.

Default rules:

- BUY: positive momentum strengthening for 2 bars.
- SHORT: negative momentum strengthening downward for 2 bars.
- QLX/SELL: 2 consecutive positive weakening bars and the configured peak-to-zero retracement, or immediate zero cross. The supplied Pine baseline is 70%; the active production profile is OPT45/DB0 with 45%.
- QSX/COVER: exact inverse.
- A positive wave can produce at most one LONG trade; a negative wave can produce at most one SHORT trade. The side rearms only after momentum reaches/crosses zero.
- One action maximum per candle. An exit does not reverse on the same candle.

## Session

Default entry session is 09:00–23:15 Asia/Kolkata. `allow_entries_only_in_session=true` restricts only new entries. Normal SQZ exits remain valid outside the entry window if a position is carried.

`force_flat_at_session_end=true` uses the confirmed candle whose close time is configured by `auto_sq_off_hour=23` and `auto_sq_off_minute=15`. Exit rules have priority over day-end safety. As an explicit safety hardening, the day-end square-off candle is not eligible to open a fresh BUY/SHORT when flat; this prevents a 23:10–23:15 entry from being carried overnight. This is a deliberate divergence from the originally supplied Pine dispatcher edge case.

## JSON source of truth

All active Pine inputs are represented in `config/production-squeeze-momentum.json`, including strategy parameters, session controls and display flags. Unknown JSON fields are rejected.

## Broker actions

- BUY = enter LONG
- SELL = exit LONG
- SHORT = enter SHORT
- COVER = exit SHORT

## Dashboard

The live dashboard previews the current SQZ value and diagnostics tick-by-tick but never mutates persistent wave/peak/weak-bar state until the candle confirms. It displays position, bar status, SQZ momentum, direction/slope, squeeze state, wave used/ready, entry readiness/strengthening count, weak-bar count, retracement %, extreme, session state, day-end safety and last action/reason.

## Validation

```bash
./deploy/verify-squeeze-momentum-v2283.sh
```

Historical fixture testing and the Sandbox execution audit both use the same Rust state machine. The Sandbox verifier requires `confirmed_bar_only=true`, signal time >= bar-close time and at most one action per bar-close timestamp.


## Dynamic Wave Deadband candidate

The optional dynamic entry filter keeps exit logic unchanged. For each new SQZ sign-wave it computes `max(sqz_entry_deadband, EMA(abs(SQZ), length) * pct / 100)` from prior confirmed bars, freezes that threshold for the wave, and resets/recalculates only after SQZ reaches/crosses zero. `sqz_dynamic_deadband_pct=0.0` disables the feature. The final-test candidate uses EMA length 25 and 30%. Production remains OPT45 / DB0 with dynamic percentage 0.
