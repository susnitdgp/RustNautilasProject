# RustNautilasProject
## Current Engineering Design and Strategies

**As-built technical review | Revision 1.0 | 23 September 2026 (IST)**

Repository: `/home/ubuntu/RustNautilasProject`
Host: `ip-172-31-36-59`
Source baseline: `5566a6270ed5e7c4e4e7e9489664a25defe18883`
Commit: **Add production Pivot and Trend Ribbon strategies**

**Status: implementation documented; live qualification remains incomplete.** This is a description of the inspected implementation, not deployment approval. The supplied Pine source, the Rust implementation, and the execution safety policy are distinguished wherever they differ.

Prepared for manual engineering review. No strategy was started, no broker order was submitted, and no trading source or private configuration was changed while preparing this document.

**Configuration-layout update (23 September 2026):** Optional JSON presets were relocated to `config/backup/` after this as-built review. Command examples and configuration-file references below use their new locations. The reviewed trading logic, source baseline, release-binary fingerprint and outstanding findings are unchanged.

## Document control and reading guide

The repository and source files were inspected on 23 September 2026. The initial host snapshot was taken at **00:58:37 IST**. `main` and the local `origin/main` reference both pointed to `5566a62`; only `config/kite-production.json` and `config/kite-sandbox.toml` were modified before documentation work. Neither private file's contents are reproduced here. No `kite-node` process was observed in that snapshot. This does not establish the broker account's current position. [V1]

The installed release executable had SHA-256:

```text
4bbf0bc5fe8ae7c7d21ec9608207dcee5db7c70e90af212b6cbe50289811acdc
```

**Evidence convention.** References such as [S04] identify inspected source files and line ranges in the source register. [P1] identifies the uploaded Pine source. [V1–V3] distinguish current inspection/test results from earlier session evidence. Code is authoritative for implementation behavior; comments, old documentation and earlier chat descriptions are not treated as proof when executable code differs.

| Section | Review focus |
|---|---|
| 1–3 | System scope, workspace, runtime and concurrency |
| 4–5 | Market data, timestamps, configuration and sessions |
| 6–8 | Trend Ribbon, Pivot Point SuperTrend and baseline strategy |
| 9–11 | Orders, reconciliation, persistence and recovery |
| 12–13 | Operation, verification evidence and limitations |
| 14–15 | Open engineering items and acceptance checklist |
| Appendix | Source register and reproducibility notes |

<!-- page -->

# 1. Executive engineering summary

The selected production path is a **single-process Rust application hosting a Nautilus LiveNode**, with a project-specific Kite data adapter, a project-specific Kite execution adapter, and Redis-backed state. Separate command-line selections choose **Trend Ribbon [BOSWaves]**, **Pivot Point SuperTrend**, or the original **Supertrend + MACD + session VWAP** strategy. These selections reuse the same `BarStrategy` actor and execution infrastructure; they are not three independent order engines. [S01, S02, S03]

The configured production scope is one standard CRUDEOIL October 2026 futures contract, completed five-minute candles, and MIS protected MARKET/DAY orders. Production startup requires explicit activation gates, valid routing identity, usable credentials, clean ownership and a flat account with no open broker orders. New positions are not silently adopted from a previous process. [S03, S04, S08, S09, S15]

Three states must not be conflated: **indicator direction** is a calculated trend; **target** is the actor's desired exposure; **position** is exposure represented by confirmed execution events in the native cache and checked against the broker. A trend can be bullish while the process intentionally remains flat. [S03, S10]

## Important findings from this documentation review

**Trend Ribbon is not fully equivalent to the uploaded Pine session logic.** Rust retains its direction across sessions. The uploaded Pine has `doSquareOff=true` by default and explicitly sets `trend := 0` on its qualifying first out-of-session bar. Earlier statements that the two always carry yesterday's trend identically were too broad. The existing 17-flip regression protects current Rust behavior, not every behavior of the original Pine source. [S10, S18, P1]

**The dedicated square-off timer is wired for Pivot only.** The shared actor registers its 250 ms clock callback only when a Pivot engine is selected. Trend Ribbon still has the run deadline, quote-driven exits and final reconciliation, but does not have that independent timer. If fresh quotes stop, a reducing Ribbon exit is not guaranteed by the timer path. This needs explicit review before unattended live use. [S03:194–239, S02:292–303, S09:402–435]

**A successful build or configuration check is not live certification.** Real broker fills, externally exported TradingView numerical parity, and Ribbon-specific outage-at-cutoff qualification remain unverified. Reconciliation deliberately stops on uncertainty; it cannot promise that a broker or network error will never occur. [S07, S12, V2, V3]

# 2. Scope and workspace structure

The workspace declares Rust edition **2024**, minimum Rust **1.98.0**, and Nautilus dependencies pinned to **0.63.0**. Two local Cargo patches replace the execution and trading crates; the vendor notes describe an accepted-state ordering correction for immediate LIMIT matching and a TWAP lifecycle forwarding correction. These are integration patches, not a separate fork of the full runtime. [S01]

| Component | Implemented responsibility |
|---|---|
| `apps/kite-node` | CLI, selected-strategy LiveNode assembly, actors, session controls, reports, backtests and operational commands |
| `crates/kite-adapter` | Instrument/credential handling, Kite historical and WebSocket data, native order translation, broker transport and reconciliation |
| `crates/kite-journal` | Redis connection and durability helpers; other journal/state workflows |
| `crates/kite-execution` | Simulation/coordinator utilities and shared rate-limit machinery; real broker transport is in `kite-adapter` |
| `crates/kite-strategy` | Crossover/paper/checkpoint components; the current Ribbon and Pivot engines reside in `apps/kite-node/src/native_node` |
| `crates/kite-paper` | Isolated synthetic execution and outbox/worker components |
| `crates/kite-recorder` | Recording/replay components for separate capture workflows |

The selected live runner does not activate every crate capability. The existence of recorder, TWAP, emulator or experimental backtest modules does not imply their use by the Trend Ribbon production launcher. [S02, S22]

<!-- page -->

# 3. Runtime architecture and concurrency

## 3.1 Production event flow

| Stage | Data or action | Principal implementation |
|---|---|---|
| Manual launcher | Selects command, strategy JSON and private broker JSON | `deploy/run-trend-ribbon-live.sh` |
| Selection/startup | Validates settings, calendar, contract, history and ownership | `production.rs`, `supertrend_live_runner.rs` |
| `STBARS` client | Publishes completed external five-minute bars | `supertrend_live_data.rs` |
| `KITE` client | Publishes quotes and feed-status events | `data.rs` |
| `BarStrategy` | Updates indicator on bars; attempts target transitions on eligible quotes | `supertrend_actor.rs` |
| Nautilus risk/execution | Routes native orders and execution events | LiveNode configured in the runner |
| Native Kite dispatcher | Validates, persists intent, submits once, reconciles observations | `native_client/dispatch.rs` |
| Redis and reports | Ownership, native cache, command journal, final diagnostic files | Persistence, ledger and report modules |

The LiveNode environment is `Live` only for the production backend and `Sandbox` otherwise. Native cache state is saved, but automatic load is disabled (`save_state=true`, `load_state=false`). A unique run UUID scopes state and reports. `shutdown_on_error=true` is configured. [S02:262–289]

**Reconciliation ownership is important:** the runner explicitly sets Nautilus execution-engine reconciliation to `false`. Broker reconciliation is implemented by the project's native Kite client and dispatcher. This is not an absence of reconciliation; it is a different owner of that responsibility. [S02:267, S09, S11, S12]

## 3.2 Task and state model

The actor owns mutable strategy state through ordinary Rust fields and `Rc<RefCell<...>>` cache/report references. Cross-task control uses `Arc`, atomics and mutex-protected rebuild/fault slots. Bar retrieval, quote supervision, execution submissions, order-stream monitoring, deadline watching and terminal refresh use Tokio tasks/futures. The production path does not declare a dedicated, CPU-pinned operating-system order-executor thread. [S02, S03, S05, S09]

The execution client spawns submission/cancellation tasks, but they share one `tokio::sync::Mutex<Dispatcher>`. This serializes broker command and reconciliation state. A task-admission guard limits the retained task list to 256. The strategy additionally allows only one pending order. These are implementation guards, not a claim of fully bounded queues or hard real-time latency. [S03:105–155, S09:437–580]

The ownership monitor moves its Redis check to `spawn_blocking`. Other Redis persistence calls are synchronous inside their calling paths. No measured end-to-end latency target or high-availability failover guarantee is established by the inspected code. [S13, S14]

## 3.3 Design trade-offs

Reusing one actor/adapter path reduces duplicated order logic, but makes changes to shared session and recovery code affect several strategies. Serialized dispatch simplifies ownership and avoids overlapping unresolved orders, but can delay command handling while broker reads retry. Polling broker candles favors a common historical data source over immediate tick-derived bar construction, at the cost of polling latency and repeated history retrieval. [S02–S05, S11–S12]

<!-- page -->

# 4. Market-data design and timestamp semantics

## 4.1 Two separate feeds

`KITE` supplies executable bid/ask quotes and feed-status events from the market-data WebSocket. `STBARS` supplies the indicator's five-minute OHLCV bars from the **Kite historical API**, not by locally aggregating the quote stream. Historical candles also retain OI, although OI does not enter these selected indicators. [S04, S05, S06]

Before node operation, the runner fetches seven calendar days of history ending on the current IST date. It removes unfinished candles and validates coverage against the configured calendar. For default Ribbon parameters, the general minimum is 100 completed warm-up bars; the formula also raises this requirement for larger configured lengths. Warm-up rebuilds calculations but does not authorize a historical Ribbon/Pivot entry. [S02:189–221, S03:278–318]

The running bar task sleeps ten seconds between retrieval cycles and re-fetches the seven-day window. OHLCV revisions or a paused feed cause a validated history rebuild; OI-only changes do not independently increment the revision count. Six consecutive history/update failures terminate that feed path after pausing admission. [S04:145–195, S06]

## 4.2 Completed bars and decision time

Kite candle timestamps are treated as **bar-open timestamps**. The native bar's `ts_event` is the candle open plus 300 seconds; `ts_init` records receipt. A candle becomes eligible only when its close is at least two seconds behind the current clock. Bars must be ordered, five-minute aligned, and have valid positive OHLC values. Calendar validation rejects missing required bars instead of inventing them. [S05]

**Example:** the regression label `12:35 SHORT` identifies the **12:35–12:40 candle**, not an order sent at 12:35. The strategy can evaluate that completed candle only after 12:40, subject to the two-second allowance, history polling, an eligible later quote and execution checks. Actual fill time and price are separate facts. [S05:11–38, S03, S18]

## 4.3 Admission and recovery checks

For real data, both quote event and receive timestamps must be non-future and no more than five seconds old. Bid must be positive and ask must not be below bid. Ordinary entry processing also requires the expected latest completed bar and a quote newer than that bar. These tests can delay an entry; they must not be represented as guaranteed zero-latency signal execution. [S05, S03:341–439]

A quote-feed gap marks the feed offline and pauses processing. Rebuild requests carry a generation/epoch. The actor accepts a rebuild only when its epoch is current and the feed is online. Historical bars are recalculated, but any reconstructed entry target is then restricted: an already open position may be retained only when it is nonzero, inside session, and agrees with the rebuilt direction. Otherwise target becomes flat. Both `0.0` and `-0.0` are treated as flat. [S03:241–259, 357–406]

# 5. Configuration, contract identity and session policy

The selected JSON is the routing source of truth: instrument ID, symbol, token, expected expiry, strategy type and calendar. When loading broker settings, its token replaces the broker JSON token. Product, expected account identity and broker activation remain private broker-file responsibilities. [S08]

The inspected selections use `CRUDEOIL26OCTFUT.MCX`, token `145894407`, expected expiry `2026-10-19`, one contract and five-minute bars. These are **configured values**, not a new exchange-master validation performed for this document. Instrument construction requires broker lot size 1 and tick size 1, and sets a Nautilus economic multiplier of 100. Order quantity 1 is not multiplied into 100 broker order units by the translation code. [S08, S15, S16]

The calendar uses `Asia/Kolkata`, covers `2026-08-17` through `2026-10-19`, and configures regular hours 09:00–23:30. It includes an evening-only override for 14 September and a closed override for 2 October. These are repository calendar inputs, not independently researched market notices. Weekends and dates outside coverage are rejected; weekend special sessions are unsupported. [S08, S16]

For Ribbon and Pivot production, effective end is the earlier of the strategy-session end and the configured market close minus 1,800 seconds. With the present files, production end is **23:00 IST**, while Ribbon/Pivot paper sessions may run through **23:15 IST**. Startup requires at least fifteen seconds before the effective live end. These are exit-request/admission deadlines, not guarantees that a broker has filled the exit at exactly that instant. [S08, S16, S17]

<!-- page -->

# 6. Trend Ribbon [BOSWaves]: current Rust design

Strategy selector: `trend_ribbon_boswaves`. Engine: `trend_ribbon.rs`. The implementation processes completed candles and uses **close** as the source. It does not implement TradingView display gradients, candle coloring or chart labels. [S10]

| Parameter | Default | Implemented meaning |
|---|---:|---|
| ALMA length | 34 | Window of closing prices |
| ALMA offset / sigma | 0.85 / 6.0 | Gaussian weighting center and width |
| Deviation length / multiplier | 34 / 0.65 | Population standard deviation and confirmation-band distance |
| Slope length / minimum | 3 / 0.08 | ALMA change over three bars, divided by ATR |
| ATR length | 14 | SMA-seeded Wilder/RMA true range |
| Session | 09:00–23:15 IST | Indicator/session window; live execution is capped earlier |
| `reset_daily` | false | Accepted in shared session settings; Ribbon does not read it to reset its engine |

The defaults above are read from the current Ribbon JSON; the engine validation does not reproduce every Pine input upper bound. It accepts no configurable price-source field. [S10:9–48, S16]

## 6.1 Calculation pipeline

For an oldest-to-newest window `x[i]`, the implemented ALMA uses an unrounded center:

```text
m = offset * (N - 1); s = N / sigma
w[i] = exp(-(i - m)^2 / (2 * s^2))
ALMA = sum(x[i] * w[i]) / sum(w[i])
Deviation = sqrt(sum((x[i] - mean)^2) / N)
```

True range uses `high-low` on the first input and thereafter the maximum of that range and the absolute gaps from previous close. ATR is seeded with the arithmetic mean of the first 14 true ranges, then updated using `(13 * previous_ATR + current_TR) / 14`. ALMA, deviation and ATR continue through the received historical sequence; they are not cleared at each session open. [S10:51–158, 178–210]

```text
slope_score = (ALMA[t] - ALMA[t-3]) / ATR[t]
upper_confirm = ALMA + 0.65 * Deviation
lower_confirm = ALMA - 0.65 * Deviation
```

Bullish setup requires an in-session bar, `slope_score > 0.08`, and `close > upper_confirm`. Bearish setup requires `slope_score < -0.08` and `close < lower_confirm`. All comparisons are strict. Direction changes only when the opposing/currently absent setup qualifies; loss of setup alone does not reset direction. A transition from initial neutral to either direction can emit a Ribbon signal. [S10:204–234]

Both the bar-open and bar-close times must be inside the engine's session to emit its entry signal. Thus a candle opening at 23:10 but completing at 23:15 cannot create a new paper entry through this signal gate. The outer production actor additionally blocks execution at its 23:00 end even though the Ribbon indicator session remains configured through 23:15. [S10:229–234, S03:415–451]

## 6.2 Indicator direction versus an actual position

The actor starts with target zero. Historical warm-up can establish Ribbon direction, but only a nonzero signal on a bar closing after the recorded startup boundary changes the trading target. An opposite target closes current exposure first; an entry on the new side waits until that exit is confirmed and another eligible quote arrives. No repeated same-direction entry is intended while the existing position already matches target. [S03:55–155, 278–296]

The release engine therefore retains prior calculated trend, but **does not automatically re-open a prior-day or pre-start position**. A process restart is a new run, not an automatic continuation of a cached trading position. [S02:264, S03, S09:322–333]

<!-- page -->

# 7. Pine equivalence and the pinned market regression

## 7.1 What the uploaded Pine actually says

The supplied script is a Pine v6 **indicator**, not a `strategy()` order simulator. It defines four alert conditions: long entry, short entry, long exit and short exit. Its trend calculations use ALMA, standard deviation and ATR-normalized slope. It defaults to source `close`, session `0900-2315`, timezone `Asia/Kolkata`, and `doSquareOff=true`. [P1:7–9, 21–55, 85–111, 121–191, 345–379]

Although Pine declares a persistent `var int trend`, it also contains this conditional session-end reset:

```pine
bool sqOffBar = doSquareOff and inSession[1] and not inSession and prevTrend != 0
if sqOffBar
    trend := 0
```

That reset occurs when the specified transition is present in the chart's bars. It is not an unconditional reset at 09:00. If no qualifying out-of-session bar occurs, the condition need not fire. Rust has no equivalent square-off reset inside `TrendRibbon::update`. Consequently, a subsequent session can have different initial direction and first-entry behavior. Matching this Pine behavior requires an explicit decision about this condition, rather than assuming that retaining `var` state proves parity. [P1:161–191, S10:178–234]

| Area | Uploaded Pine | Current Rust |
|---|---|---|
| Default calculations | ALMA, deviation and ATR slope | Matching calculation form and defaults |
| Session-end direction | Conditional `trend := 0` when square-off fires | Direction remains unchanged; trading target/exposure handled separately |
| Entry eligibility at boundary | Setup uses bar-open `time(...)` session test | Signal additionally requires bar close inside session; production adds 23:00 cutoff |
| Price source | Configurable `input.source` | Close only |
| Zero ATR | Slope expression returns 0.0 | Slope remains `None`; initialization flag can differ |
| Evaluation | No `barstate.isconfirmed` gate in supplied source | Only completed historical bars accepted |
| Exit representation | Explicit four Boolean alert series | Direction/signal observations plus position-based actor order intents |

These are source-level distinctions, not all demonstrated trading losses or proven live incidents. In particular, screenshot comparisons cannot establish realtime alert-frequency behavior or exact numerical equality. [P1, S03, S10]

## 7.2 What the automated regression protects

The fixture `apps/kite-node/tests/fixtures/trend_ribbon_sep18_21_22.json` pins **602 Kite historical candles**, starting 17 September at 16:50 IST, including 80 warm-up candles before the three reviewed dates. The test runs the actual Rust engine, converts candle-open timestamps to native close timestamps, and compares the resulting nonzero signals against a hard-coded list. [S18, V2]

| Date, 2026 | Pinned candle-open labels, IST | Count |
|---|---|---:|
| 18 September | 09:05 SHORT; 13:35 LONG; 14:45 SHORT; 15:05 LONG; 16:10 SHORT; 16:55 LONG; 20:20 SHORT | 7 |
| 21 September | 14:30 LONG; 15:20 SHORT; 17:00 LONG; 18:10 SHORT; 23:00 LONG | 5 |
| 22 September | 12:35 SHORT; 16:45 LONG; 18:40 SHORT; 19:15 LONG; 22:20 SHORT | 5 |

Those labels were derived from historical calculations and visually compared with the supplied screenshots. They are **not an exported TradingView numeric reference**. The regression does not compare every Pine long/short exit, square-off reset, indicator value, realtime update or broker fill. The 21 September 23:00 candle completes at 23:05 and is not eligible for production entry under the 23:00 cutoff. [S18, V3]

<!-- page -->

# 8. Other implemented strategy selections

## 8.1 Pivot Point SuperTrend

Selector: `pivot_point_supertrend`. Defaults are pivot period **2**, ATR period **10**, ATR factor **3**, weekday session 09:00–23:15 for paper and 09:00–23:00 for the current production selection. Daily session reset is enabled. MACD/VWAP confirmation is disabled for this selection. [S16, S19]

A pivot candidate waits for `n` completed bars on each side, so the default confirmation window is five bars. Equal extremes select the rightmost candidate by allowing equality on the left and requiring strict comparisons on the right. If a high and low are confirmed together, the high is selected for the center update. [S19:179–208]

The first confirmed pivot initializes the center. Later confirmed pivot values update it as `(2 * previous_center + pivot) / 3`. Basic trailing bands are center minus/plus ATR × factor; the previous close and bands control trailing continuity. A close above the prior down band sets bullish direction; below the prior up band sets bearish direction. [S19:206–230]

Unlike Ribbon, Pivot emits an entry only on **-1 to +1** or **+1 to -1** transitions. Establishing the first direction from neutral is not itself an entry. At the first in-session bar of a new day, its configured reset clears the pivot window, center, bands, local previous close, direction and support/resistance, while ATR remains continuous. [S19:134–175, 232–240]

The shared actor has a dedicated 250 ms timer for this selection. At run end or stopping, it targets zero and attempts a reducing order even without waiting for the next quote callback, subject to pending-order and downstream execution guards. This is a mechanism for attempting square-off, not a guarantee of a completed broker fill. [S03:194–239]

## 8.2 Original Supertrend + MACD + session VWAP

Selector: `supertrend_macd_vwap`. Supertrend uses Nautilus ATR(7), Wilder smoothing and multiplier 2 around `(high+low)/2`. Entry confirmation uses EMA-based MACD(12,26), EMA signal(9), and session VWAP calculated from typical price `(high+low+close)/3` and volume. [S20]

A long confirmation requires `close > VWAP` and `MACD > signal`; a short requires both inverse comparisons. It does not require MACD to be on a particular side of zero. VWAP and accumulated volume reset on the IST date boundary; MACD history persists. Loss of confirmation does not block an exit caused by the opposite Supertrend target. [S20, S03:116–120]

The baseline actor updates target from current direction on each completed bar. Therefore, its entry behavior is **not the same fresh-flip-only policy** as the Ribbon/Pivot branches: an aligned, newly completed post-start bar can authorize entry while the trend remains unchanged. [S03:320–337]

The baseline selection also retains legacy activation semantics: its strategy JSON is validated with `live_orders_enabled=false`, while the dedicated broker-production path separately requires the live build and enabled broker settings. Do not generalize the new two-JSON activation rule to this legacy selector without reviewing `Selection::validate` and `broker_settings`. [S08:88–103, 168–176]

## 8.3 Features not enabled by these selections

The selected production strategies do not add ATR stop-loss orders, profit targets, brackets, pyramiding, automatic position scaling or automatic contract rollover. Existing stop-policy experiments, backtest variants and execution-algorithm demonstrations are separate entry points, not implied production features. The selected runner explicitly reports `atr_stop_enabled=false`, and selection validation rejects enabling that field. [S02:367–372, S08:151–161, S22]

<!-- page -->

# 9. Order execution, risk and reversal sequencing

## 9.1 Production admission

The native production settings require the `kite-adapter/live-orders` build feature, broker `live_orders_enabled=true`, an explicitly configured expected account, MIS product, a nonzero token, and `market_protection=-1`. Ribbon and Pivot additionally require their selected strategy JSON's activation flag. These gates are checked before the live backend runs. [S07, S08]

Startup verifies the broker account identity and opens the dedicated order stream before the initial account snapshot. Production refuses startup when **any returned account position is nonzero** or any broker order is not terminal. This is an account-wide guard, not merely a check that the selected contract is flat. Mixing another trading process or manual positions in that account conflicts with the implemented admission policy. [S09:297–342]

For every order, the dispatcher validates command identity, ownership, stream readiness for new entries, absence of unresolved native orders, agreement between native and broker position, supported exposure, and reducing-exit-before-new-entry policy. Unmanaged exposure or an unowned open order stops admission/reconciliation rather than being silently adopted. [S09:437–452, S11:149–249, 352–398]

## 9.2 Native order and durable submission

The actor creates a quantity-one native MARKET order with DAY time-in-force. An exit sets reduce-only; translation verifies that its side opposes existing exposure and cannot increase or reverse it. It then constructs a protected-market Kite command with the stored correlation tag and `market_protection=-1`. The reducing policy is checked by the application; it should not be described as an independently proven broker-native reduce-only facility. [S03:105–154, S15]

Before the broker mutation, the dispatcher reserves an account command budget, creates the submitted event, persists a command record including the unique tag, publishes the submitted event, and records dispatch health. Broker submission has a six-second timeout. An uncertain result is classified as unknown; the order is not submitted again merely because an acknowledgement was absent. [S11:252–313, S14]

Redis account command budgets are **5 per second, 100 per minute and 1,000 per day** in the code. They include cancellations and failed requests. These are conservative application limits, not claims about the broker's published limits. The LiveNode also configures a per-order notional ceiling of 2,000,000 for the selected instrument, while the dispatcher enforces one-contract exposure. [S02:268, S13:90–100, S11:218–228]

## 9.3 A reversal is two orders, not a two-contract jump

| Step | Example: long to short |
|---|---|
| Signal | Completed bar changes trading target to -1 |
| Exit | Existing +1 position causes a SELL reducing order of 1 |
| Pending | Further target dispatch is blocked while that order is pending |
| Confirmation | Broker-backed execution events update the native position |
| Entry | A subsequent eligible quote can submit a SELL entry of 1 from flat |

An order acknowledgement is not a fill and must not advance this sequence by itself. A fill deadline of ten seconds is tracked by the actor; the runner watches it every 100 ms and requests failure/stop if exceeded. Broker snapshot retries do not automatically extend that deadline. [S03:149–154, 460–486; S02:292–303; S11–S12]

There is no supported promise that every protected-market order will fill or that every operational failure leaves the account flat. The fail-closed design blocks uncertain new exposure; it does not replace checking the real broker state after a fault. [S09, S11]

<!-- page -->

# 10. Broker reconciliation design

## 10.1 Authoritative observations

The dedicated Kite order WebSocket is a notification channel. Its order messages trigger REST snapshots of orders, trades and positions. HTTP submission acknowledgements and WebSocket notifications do not directly create confirmed fills. The snapshot must be internally consistent with owned command records before native events are published. [S09, S11, S12]

Order identity includes the client-order record, persisted broker tag, broker order ID, product, token and selected instrument. The reconciler rejects ambiguous tags, changed ownership, unowned open orders, excessive exposure and inconsistent records. Missing visibility immediately after acknowledgement can remain unresolved and be checked again; it is not an instruction to resubmit. [S11:352–455]

Observation preparation is separated from publication. The dispatcher validates the snapshot and proposed events first, then saves updated records and emits native order events. Write or event-delivery failures leave the dispatcher requiring recovery. This is not a distributed exactly-once guarantee across Kite, Redis and the local cache. [S11:315–350, S14]

## 10.2 Trigger and retry policy

| Mechanism | Current code default |
|---|---|
| Order notification | Triggers reconciliation; bursts can coalesce |
| REST fallback | Every 15 seconds |
| Unresolved-order check | Every 5 seconds; heartbeat only when none are unresolved |
| Quiet order stream | Ten-second idle observation is not itself a reconnect failure |
| Read/snapshot retry | At most three attempts for classified transient errors or observation lag |
| Snapshot deadline | Twelve seconds per attempt |
| Retry delays | 250 ms, then 500 ms |
| Actual stream reconnect | Bounded generation count; 500 ms scaled delay; handshake failure is terminal |

Authentication failures, rate limits and structural/identity errors are not blindly retried. Rate-limit outcomes can persist a cooldown and stop the run. The same bounded read machinery is used during reconciliation; none of these read retries authorize repeating an uncertain order mutation. [S11, S12]

## 10.3 Recovery of the order stream

A stream disconnect marks entry admission unready. After reconnection, readiness is restored only after a fresh reconciliation for the same connection generation. A snapshot crossing a disconnect/reconnect boundary cannot reopen entry admission. Reducing orders are exempt from the stream-readiness entry gate, but still depend on client activity, valid positions, ownership, persistence and broker access. [S11:163–173, S12:170–238]

If monitoring/reconciliation fails, the client marks itself inactive, signals the run to stop and faults the dispatcher. Final shutdown drains submitted tasks, performs final checks where possible, and reports `ReviewRequired` unless clean flatness and resolved orders can be established. Final reconciliation **verifies** exposure; it does not itself create a missing closing order. [S09:353–435, S11:63–80]

## 10.4 Operational interpretation

A reconciliation error may mean the system detected a genuine account mismatch, a delayed/inconsistent snapshot, stale ownership, authentication trouble or an unresolved command. Suppressing the message, resetting the namespace or repeatedly relaunching does not repair its cause. Inspection must relate the run UUID and persisted command records to the broker's actual orders, trades and positions. [S11–S14, S23]

<!-- page -->

# 11. Persistence, ownership, recovery and observability

## 11.1 Persistence layers

| Layer | Scope and intended use |
|---|---|
| Native Nautilus Redis cache | Run-instance-scoped native orders, positions and saved actor state |
| Native command ledger | Per-run owned command records, broker IDs/tags, outcomes and event history |
| Account coordination | Durable account owner, state, unresolved count, position, heartbeat and command attempts |
| Strategy ownership | Prevents conflicting selected live runs for a symbol |
| Local run reports | Human-readable indicators, order intents, fills, recovery events and final summary |

Native cache configuration uses `use_instance_id=true`, `flush_on_start=false`, and `save_market_data=false`. The selected cache requires Redis TCP database 0 and AOF enabled. Journal connections require `noeviction`; durable journal writes use `WAITAOF 1 0 2000` to confirm local AOF persistence. This is local durability confirmation, not proof of replicated disaster recovery. [S13–S14]

Production strategy ownership uses `kite:production:supertrend:<symbol>:owner`, including Ribbon and Pivot despite the historical name. Account coordination is separate and persistent. Ownership does not expire into an automatic takeover: an unclean run retains a review barrier. Clean release is conditional on matching the same owner. [S13]

## 11.2 Credentials and private configuration

The connection is selected through `KITE_REDIS_URL`, defaulting to local Redis database 0. The credential module reads the API key and access token from named Redis keys; it does not renew the token. The existing credential design uses redacted debug output and zeroizing secret containers. No secret values, account ID values, private webhook URLs or private configuration bodies are included in this document. [S14, S21]

Both private files remain intentionally outside this documentation change: `config/kite-production.json` and `config/kite-sandbox.toml`. Their presence as local Git modifications is not evidence that they were included in the committed production release. [V1]

## 11.3 Restart and shutdown

A failed process is not resumed automatically from saved state. The new LiveNode uses a new UUID and does not load the prior run state. Account-startup checks examine retained ownership before acquiring a new strategy lease. Manual review is required after an uncertain dispatch, retained owner or crash; clearing Redis broadly would destroy evidence and bypass the intended protection. [S02:239–265, S13, S23]

Ctrl-C/SIGTERM is handled by the lifecycle watcher. It marks stopping, waits briefly for flatness, then stops the node. The actor cancels outstanding orders on stop. Execution-client disconnect drains tasks and reconciles before the runner snapshots final counts. Clean status requires a successful run, zero position, zero pending orders, no stored fault/errors, and a stopped actor. Unknown counts are reported as `null` when initialization or reconciliation did not establish them. [S02:292–377, S03:208–216, S09:402–435, S17]

The Pivot clock exit and Ribbon quote-dependent limitation described in Sections 1 and 14 remain relevant during this sequence. Neither a stop request nor a printed cutoff establishes that the broker account is flat.

## 11.4 Reports and recording boundary

Selected runs write `indicators.json`, `signals.json`, `fills.json`, `recoveries.json` and `summary.json` under `data/supertrend-live/<RUN_UUID>/`. `signals.json` records attempted order intents, not the four original Pine alert series. The dashboard refreshes every five seconds; optional Slack reporting is a separate notification facility. Historical names such as `supertrend_live_complete` also cover Ribbon/Pivot runs, so always inspect the explicit `strategy` field. [S02, S03]

This selected runner is **not a continuous raw-tick/Parquet recorder**: native market-data saving is disabled and no recorder actor is attached by this launcher. The separate recorder/catalog commands must be evaluated independently. A running strategy does not by itself confirm that every raw tick is being archived. [S02, S13, S22]

<!-- page -->

# 12. Build, check and manual operation

These commands document the current interfaces. They were not used to start trading during this review. Build/validation commands do not grant permission to launch production. The engineering gaps in Section 14 remain open.

## 12.1 Build without launching a node

```bash
cd /home/ubuntu/RustNautilasProject
cargo build --locked --release -p kite-node \
  --features kite-adapter/live-orders -j 3
sha256sum target/release/kite-node
```

The prior build in this session used the live-orders feature and produced the installed executable hash recorded in Document Control. A release binary is an artifact separate from Git; rebuilding after a source change is necessary before expecting that behavior in the launcher. The launcher checks that the executable exists, not that its hash matches a particular commit. [S01, S16, V1, V3]

## 12.2 Offline production-configuration checks

```bash
./target/release/kite-node native-trend-ribbon-production-check \
  config/production-trend-ribbon.json config/kite-production.json

./target/release/kite-node native-pivot-production-check \
  config/backup/production-pivot-supertrend.json config/kite-production.json
```

These parse and validate configuration/build gates and report effective session bounds. They do **not** load account credentials, connect a trading node, query account exposure, validate the instrument master, or send broker orders. `configuration_valid=true` can coexist with `can_start_now_by_calendar=false`. [S07]

## 12.3 Separate public contract validation

```bash
./target/release/kite-node native-contract-check \
  config/production-trend-ribbon.json
```

This command downloads instrument metadata and checks the selected contract/calendar. It is not an account, margin or full readiness check. No new contract download was performed for this documentation task. Rollover remains an explicit configuration-and-review activity. [S08:207–237, S15]

## 12.4 Simulation and paper interfaces

```bash
# Synthetic data with Nautilus Sandbox execution; Redis is still used.
./target/debug/kite-node native-trend-ribbon-sim \
  config/backup/trend-ribbon-boswaves.json

# Synthetic data with native Kite mock execution.
./target/debug/kite-node native-trend-ribbon-kite-mock \
  config/backup/trend-ribbon-boswaves.json

# Live market data, paper orders, 300-second bounded run.
./target/debug/kite-node native-trend-ribbon-paper \
  config/backup/trend-ribbon-boswaves.json 300
```

Build a default debug executable first when required. Paper mode still needs valid market-data credentials, contract metadata and the current session. The Ribbon CLI has a bounded `paper` command; no dedicated `native-trend-ribbon-session-paper` command is currently declared. [S02, S24]

## 12.5 Manual production interfaces — real orders possible

```bash
# Run only the deliberately selected strategy, after review.
./deploy/run-trend-ribbon-live.sh
# Alternative selections, not simultaneous instructions:
# ./deploy/run-pivot-live.sh
# ./deploy/run-supertrend-live.sh
```

The current regular-session gate rejects launch before 09:00 IST and near/after the effective end. A manual early-session startup can miss an initial signal if initialization crosses the first candle close; the implementation does not promise a fixed initialization duration. The error still saying “Start Pivot production...” is shared wording and is not proof that the Ribbon launcher selected Pivot. Keep activation and calendar settings intact rather than bypassing a time-gate failure. [S02:103–113, 189–221, S08:77–86]

The repository's systemd unit is for the original **paper** strategy and specifies `Restart=no`; it is not evidence of an installed production scheduler or automatic daily relaunch. [S25]

<!-- page -->

# 13. Verification evidence and its limits

## 13.1 Evidence available for this revision

| Evidence | Status and interpretation |
|---|---|
| Current repository/process snapshot | Baseline `5566a62`; two private config modifications; no `kite-node` process observed. No broker account assertion. [V1] |
| Installed release binary | SHA-256 rechecked against the preceding production build. It was not rebuilt during documentation. [V1, V3] |
| Current Ribbon tests | Re-run during documentation: **4 passed, 0 failed**, including the real-market regression and deterministic rebuild test. [V2] |
| Earlier application suite | Earlier session output: **78 unit tests + 14 native integration tests + 3 production integration tests passed** before the added real-market regression. Not a fresh complete suite in this documentation pass. [V3] |
| Later real-market regression | Separately added and passed; included again in the four current Ribbon tests. Do not add repeated runs as unique test counts. [S18, V2, V3] |
| Earlier static/build checks | Clippy with warnings denied, format/diff checks and release compilation succeeded in the preceding work. [V3] |
| Earlier synthetic Ribbon run | 174 bars, 4 order intents/fills, flat final cache and Clean report using simulated data and Sandbox execution. Not live fills. [V3] |
| Earlier production-config checks | Both Ribbon and Pivot passed offline checks, reported 23:00 cutoff, and explicitly reported no engine/account/master check or orders. [V3] |

The current focused test command was:

```bash
cargo test --locked -p kite-node --bin kite-node \
  native_node::trend_ribbon::tests -- --nocapture
```

## 13.2 What remains unproved

The screenshots provide a visual reference, not exact exported Pine calculation series. No automated TradingView execution of the supplied source was performed. The pinned regression has no independent Pine reference for square-off or the four alert Booleans. Nor does a synthetic run establish broker fill latency, slippage, full-session reliability, profitability, or behavior under every market-data/order-stream failure. [P1, S18, V3]

The fixture starts with a limited prior-history slice. Matching its expected flip labels does not establish that ATR or every indicator value is identical to a chart initialized from a much longer series. Actual TradingView parity requires identical contract, candle data, source selection, settings, session treatment, timestamp conventions and evaluation mode, with explicit numerical and signal comparisons. This is a validation requirement, not a completed result. [S10, S18, P1]

Production integration tests and mock reconciliation coverage do not establish that all Ribbon-specific paths received the same testing as Pivot. In particular, the quote-outage-at-cutoff and Pine square-off/next-session paths need dedicated acceptance tests. [S03, S18, V3]

No checked-in `.github` CI workflow was found in the inspected tracked-file inventory. The regression is runnable with Cargo; automatic execution on remote pushes should not be assumed from the existence of the test alone. External CI configuration was not inspected. [V1]

<!-- page -->

# 14. Open engineering items

These items describe the inspected baseline. They are **not fixes applied by this documentation task**.

## E1 — Trend Ribbon session-reset parity

**Finding:** the supplied Pine resets direction when its mandatory square-off condition fires; current Rust preserves direction and never reads `session.reset_daily` to reset Ribbon state. This can affect first-session entry signals and invalidates an unconditional “same overnight behavior” claim. **Required review:** decide and document the intended contract, then add tests covering a qualifying out-of-session bar, a missing outside-session transition, next-session setups and startup warm-up. A confirmed Pine-reference fixture should test entries and exits, not just reversals. [S10, S18, P1]

## E2 — Trend Ribbon quote-independent square-off

**Finding:** the clock callback is registered only for Pivot. Ribbon uses quotes plus a run watcher; `on_stop` cancels orders, and adapter finalization verifies rather than originates a missing exit. **Risk:** during stale/absent quotes, a stop/cutoff need not send the intended reducing Ribbon order. **Required review:** qualify a deliberate quote-independent exit design and test cutoff, Ctrl-C, pending orders, quote outage and reconciliation failure with an existing Ribbon position. Until then, do not treat unattended Ribbon square-off as proven. [S03, S02, S09]

## E3 — Exact parity and realtime scope

**Finding:** close-only source, completed-bar processing, zero-ATR initialization and stricter close-time entry gating differ from configurable/realtime Pine behavior. **Required review:** declare these supported differences explicitly and obtain an exported reference dataset for the agreed operating mode. Do not rename a current-Rust regression into proof of the full Pine runtime. [S10, S18, P1]

## E4 — Shared naming and selector consistency

**Finding:** messages, report paths, strategy ID and Redis ownership keys retain “Supertrend” or “Pivot” names across selections. The original Supertrend CLI guard checks absence of Pivot rather than explicitly excluding Ribbon, and the legacy strategy activation flag has different semantics. **Required review:** improve labels and selector tests without weakening current routing/account gates. Existing launcher paths do choose their named configurations. [S03:55–62, S08, S13, S16, S24]

## E5 — Operational qualification and deadline interaction

**Finding:** snapshot reads can use three twelve-second attempts, while the actor's fill deadline is ten seconds and shutdown drains have their own bounds. The intended response is review/stop on uncertainty, not an unlimited wait or order retry. **Required review:** test deadline interactions with realistic delayed broker observations, keep broker exposure visible during failures and verify the final reducing fill before declaring Clean. [S02, S09, S11, S12]

## E6 — Recording, release provenance and review hygiene

**Finding:** selected runs do not archive all raw ticks; release executables are not commit-stamped by the launcher; test names/comments and source notices can overstate compatibility or retain historical wording. **Required review:** separately specify recording/retention, bind releases to source/configuration evidence, and preserve original-source attribution. The uploaded Ribbon Pine includes a BOSWaves/MPL-2.0 notice, while the inspected Rust file begins with a generic compatibility comment. This document is not a legal license assessment. [S10:1, S13, S16, S22, P1:1–3]

# 15. Manual acceptance checklist

Before considering the implementation qualified, record a decision for E1 and E2, verify the chosen bar/alert timing semantics, and test all four entry/exit intents including cutoff and recovery. Re-run application and adapter tests, preserve the actual test logs, and distinguish unit, mock, paper and real-broker evidence.

For any intended live session, independently confirm the selected contract/calendar, fresh credentials, account-wide flatness, absence of open orders, correct binary/configuration identity and clean ownership. Keep a broker-side view of orders, trades and positions available during the session. A final review must reconcile the broker, native cache, command ledger and summary; a printed shutdown request alone is insufficient. These review steps follow the application's implemented guard and recovery model. [S07–S17, S23]

**Sign-off record:** reviewer __________  date __________  source commit __________  configuration revision __________  E1 decision __________  E2 evidence __________  remaining restrictions __________.

<!-- page -->

# Appendix A. Source register

All repository paths below are relative to `/home/ubuntu/RustNautilasProject` and were inspected at `5566a6270ed5e7c4e4e7e9489664a25defe18883`. Line numbers refer to that baseline. The private configuration bodies were not used as document content.

**[S01] Workspace and dependencies.** `Cargo.toml:1–55`; `vendor/README.md:1–23`; workspace crate manifests. Declared Rust/Nautilus versions, workspace members and documented local patches.

**[S02] Selected live runner.** `apps/kite-node/src/native_node/supertrend_live_runner.rs:94–114,151–379`. Startup, warm-up, backend selection, native node configuration, risk ceiling, stop watcher and final report generation.

**[S03] Shared strategy actor.** `apps/kite-node/src/native_node/supertrend_actor.rs:40–172,174–239,241–451,460–497`. Target dispatch, timer registration, bar/quote callbacks, recovery and order-event handling.

**[S04] External bar and quote clients.** `apps/kite-node/src/native_node/supertrend_live_data.rs:75–197`; `apps/kite-node/src/native_node/data.rs:85–198,239–276`. Historical polling, data-event publication, quotes and feed-status integration.

**[S05] Timestamp, freshness and coverage rules.** `apps/kite-node/src/native_node/supertrend_live_bars.rs:11–39,105–135`; `apps/kite-node/src/native_node/supertrend_live_control.rs:23–66`. Native close timestamps, two-second allowance, calendar coverage and quote-age checks.

**[S06] Corrected-history merger.** `apps/kite-node/src/native_node/supertrend_revision.rs:15–61`. Atomic validation of merged history and OHLCV revision detection.

**[S07] Offline configuration check and native production gates.** `apps/kite-node/src/native_node/pivot_production.rs:1–40`; `crates/kite-adapter/src/execution/native_client/production.rs:30–145`.

**[S08] Strategy selection and contract/calendar checks.** `apps/kite-node/src/native_node/production.rs:8–206,207–259`. Session bounds, activation rules, configuration validation, contract validation and legacy readiness/replay paths.

**[S09] Native execution client.** `crates/kite-adapter/src/execution/native_client/mod.rs:104–172,247–435,437–580`. Native client structure, netting OMS, account-wide startup gates, stream task, submit path and task drains.

**[S10] Trend Ribbon implementation.** `apps/kite-node/src/native_node/trend_ribbon.rs:1–254`. Settings, ALMA, deviation, ATR, state transitions and signal/initialization semantics.

**[S11] Serialized command dispatcher.** `crates/kite-adapter/src/execution/native_client/dispatch.rs:39–110,141–313,315–491`. Admission, durable pre-submission record, unknown outcomes, observation preparation/publication and shutdown health.

**[S12] Order stream and outage policy.** `crates/kite-adapter/src/execution/native_client/order_stream.rs:19–35,45–90,104–245`; `crates/kite-adapter/src/execution/native_client/outage.rs:1–52`; `crates/kite-adapter/src/execution/native_client/shutdown.rs:1–42`.

**[S13] Native persistence and ownership.** `apps/kite-node/src/native_node/persistence.rs:1–51`; `supertrend_live_lease.rs:1–63` and `supertrend_owner_monitor.rs:1–35` in the same directory; `crates/kite-adapter/src/execution/native_client/coordination.rs:14–188`.

**[S14] Command ledger and durable connection.** `crates/kite-adapter/src/execution/native_client/ledger.rs:6–141`; `crates/kite-journal/src/connection.rs:1–47`. Command records, compare-and-set writes, connection configuration, noeviction and WAITAOF.

**[S15] Contract and order-unit representation.** `crates/kite-adapter/src/instruments/contract.rs:16–99`; `crates/kite-adapter/src/execution/native.rs:14–107`. Instrument assumptions, lot/multiplier fields and reducing/protected-market translation.

**[S16] Selected configuration and launcher files.** `config/production-trend-ribbon.json:1–29`; `config/backup/trend-ribbon-boswaves.json:1–29`; `config/backup/production-pivot-supertrend.json:1–38`; `config/backup/production-supertrend.json:1–33`; `deploy/run-trend-ribbon-live.sh:1–20`; the corresponding Pivot/Supertrend launchers.

**[S17] Session and process lifecycle.** `apps/kite-node/src/native_node/pivot_session.rs:7–69`; `session_calendar.rs:7–118`; `supertrend_session.rs:1–23`; `lifecycle.rs:1–36`, all in the same native-node directory.

**[S18] Ribbon tests and fixture.** `apps/kite-node/src/native_node/trend_ribbon.rs:256–376`; `apps/kite-node/tests/fixtures/trend_ribbon_sep18_21_22.json`. Four focused tests; hard-coded open-time signal labels; pinned Kite candle snapshot.

**[S19] Pivot engine.** `apps/kite-node/src/native_node/pivot_point.rs:1–262`. Pivot confirmation, center/band evolution, daily state reset, ATR continuity and strict reversal signals.

**[S20] Baseline calculations.** `apps/kite-node/src/native_node/supertrend.rs:1–55`; `supertrend_confirmation.rs:1–74` in the same directory. Supertrend and entry-only MACD/VWAP confirmation.

**[S21] Credential design.** `doc/RedisCredentials.md:3–29`, supported by the native production factory's credential load at `crates/kite-adapter/src/execution/native_client/production.rs:111–126`. Module design only; no stored secrets were copied into this document.

**[S22] Other workspace capabilities.** `crates/kite-strategy/src/lib.rs`; `crates/kite-execution/src/lib.rs`; `crates/kite-paper/src/lib.rs`; `crates/kite-recorder/src/lib.rs`; `crates/kite-journal/src/lib.rs`, together with the selected runner's actual assembly in [S02].

**[S23] Maintained operational recovery guidance.** `README.md:108–127`, read alongside the implementation in [S09], [S11], [S13] and [S14]. Earlier README overview descriptions are not substituted for the newer Ribbon code.

**[S24] Native CLI.** `apps/kite-node/src/native_node/cli.rs:8–66,180–208`. Ribbon/Pivot commands, checks, and selector guards.

**[S25] Paper service template.** `deploy/kite-supertrend-paper.service:1–22`. This is a repository file, not an inspection of installed systemd activation state.

**[P1] Uploaded Pine source.** `Pasted markdown.md`, supplied in this conversation. Source notice at lines 1–3; indicator declaration 7–9; trend inputs 21–55; session inputs 85–111; calculations 121–139; state and square-off 141–191; alert conditions 345–379. The original source was read as supplied; its session reset was not silently removed from the analysis.

# Appendix B. Evidence and reproducibility record

**[V1] Current inspection.** Desktop Commander on `ip-172-31-36-59`, repository snapshot at 00:58:37 IST, 23 September 2026: `git status --short --branch`, full HEAD, release SHA-256, private-file hashes for preservation checking and process names. Subsequent source inventory found no tracked `.github` workflow. The hashes of private files are retained only for verification, not reproduced as document content.

**[V2] Focused current test.** During this document preparation, `cargo test --locked -p kite-node --bin kite-node native_node::trend_ribbon::tests -- --nocapture` completed with 4 passed, 0 failed and 75 filtered out. This is a unit-test execution, not a broker, paper-session or production launch.

**[V3] Earlier session evidence.** Tool results in the preceding implementation work show the 78-unit/17-integration application run, later separate real-market regression, Clippy/diff checks, live-capable release build, synthetic Ribbon run and offline production checks. These are historical results from this conversation, explicitly separated from the focused current test. Exact TradingView series and real broker fills were not verified by those commands.

For repeatable engineering review, retain the full source commit, selected non-secret JSON, binary checksum, test command/output, fixture version and all relevant run reports. Preserve private configuration locally and review changes before staging. Changes to strategy session semantics require new expected-result evidence; changing a test's expected timestamps alone is not validation.

**End of document.**
