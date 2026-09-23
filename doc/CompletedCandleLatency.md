# Completed-candle latency changes

These changes are a candidate, not an instruction to start or restart production.
The live binary is not replaced by building into the separate candidate target directory.
The strategy still trades completed candles, not an intrabar momentum detector.
Position size remains one contract; reversal orders remain separate exit and entry.

## Polling policy

The two-second completion grace is unchanged and now has one named constant.
For three-minute bars the first scheduled request is at 09:03:02, 09:06:02, etc.
Five-minute bars use the corresponding five-minute boundaries.
The schedule has no independent ten-second phase offset after a candle close.
Routine full seven-day historical audits continue about every ten seconds,
except that an audit immediately before a scheduled boundary is deferred to that boundary.
All completed warm-up sessions and current-session continuity remain validated.

The historical reader retains its HTTP connection pool across requests.
Credentials are still refreshed from Redis before each read; they are never logged.
Every response is filtered and validated using the same request-start timestamp.
A response crossing a candle boundary cannot make the new candle eligible prematurely.

If a successful historical response has only the newly closed tail missing,
the reader retries at least 500 milliseconds after that response completes,
for at most 15 seconds after the completion grace. No entries are allowed on the old bar.
Older/internal gaps still fail validation, and no partial history is committed.
After publication-wait exhaustion, normal pause/recovery handling applies.
HTTP errors do not use the fast retry cadence. Authentication and rate-limit
rejections stop the bar feed rather than hammering the endpoint. Other failures
retain ten-second retries and the six-failure limit.

Kite documents a historical endpoint limit of three requests per second:
https://kite.trade/docs/connect/v3/exceptions/
The bounded publication retry path is at most two sequential reads per second;
other clients using the same API key must also respect the shared broker limit.
Historical responses are not guaranteed to become final exactly two seconds after close.
Later price corrections still trigger a pause and indicator rebuild.

## Avoid unnecessary Trend Ribbon rebuilds

Trend Ribbon uses price, not volume or open interest. Its history now merges
volume-only corrections without discarding indicator state or replaying history.
Any OHLC change still requires a full validated rebuild. Other strategies keep
the existing volume-sensitive policy. Open-interest-only changes remain ignored.
This does not prove the cause of older runs' rebuilds: they had no per-cause audit.

## Diagnostics

The dashboard now names the completed candle's opening AND closing time.
For example: 20:57 -> 21:00 IST is one three-minute candle, not a clock mismatch.
Refresh is once per second, independent of order execution.
New fields show request duration, first validated close-to-data arrival delay,
price and volume-only correction counts, ignored volume corrections, and rebuild cause.
An unchanged historical re-audit cannot overwrite the original arrival delay.

Completed-run reports include bar-feed.json and the same metrics in summary.json.
Recovery records include their available cause, and signals include bar_close_ns
and bar_to_signal_ms. These are diagnostics, not order-enabling settings.

## Validation and limitations

Regression tests cover both intervals, candle completion boundaries, delayed
publication, gaps, atomic history updates, duplicate suppression, volume-only
corrections, price corrections, connection reuse, credential rotation, and
explicit opening/closing labels. Mock integration uses disposable Redis instances.
Never run these tests against the production Redis database.

Scheduling and local mock tests cannot establish a guaranteed live fill time.
Network latency, historical-data availability, quote arrival, broker checks,
and separate reversal fills still contribute to latency. The first read is
scheduled at close plus two seconds; this is not a two-second fill guarantee.
Exact OHLC parity against a chart requires that chart's timestamp and OHLC data.

Activation requires separate user authorization, a clean supervised shutdown,
and fresh broker/Redis reconciliation. Do not run a second production node.
