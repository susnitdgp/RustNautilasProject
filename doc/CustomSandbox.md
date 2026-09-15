# User-selected sandbox compatibility

User-selected endpoint: http://94.136.191.37:3000/.
Checked 15 September 2026. The landing page identifies this as the Nordible
Zerodha APIs Test environment and links to its open-source mock repository.
This is a separately hosted API mock, not the official sandbox.kite.trade service.

## Connection and command

The endpoint is recorded in config/kite-custom-sandbox.toml.

    cargo run --locked -p kite-node -- native-kite-custom-preflight config/kite-custom-sandbox.toml

This performs bounded GET requests with no credentials or cookies and no redirects.
It does not read Redis credential keys, send orders or attach an execution client.
Incompatible reads produce structured per-route findings and a nonzero exit status.
Passing read shapes alone would still not authorize execution: stateful order/fill/
position behavior and fresh full-tick delivery would need separate verification.

## Observed incompatibilities

| Check | Observed behavior |
| --- | --- |
| Landing page | HTTP 200, Nordible mock identification |
| /user/profile | HTTP 404 |
| /oms/user/profile | HTTP 404; official sandbox prefix is not available |
| /user/margins/commodity | HTTP 200 with marker indicating missing segment handling |
| /trades | HTTP 200 with marker indicating incomplete request handling |
| /orders | Fixed rejected ACC sample dated 2015; marker says request-body handling is incomplete |
| /portfolio/positions | Fixed nonzero NIFTY option sample from 2015 |
| CRUDEOIL quote request | Returns unrelated instruments and 2018 snapshots; requested contract absent |
| /instruments/MCX | Returns quote JSON instead of an MCX instruments CSV |

These responses cannot establish the configured CRUDEOIL account, quote identity,
current exposure or native order lifecycle. Substituting them into reconciliation
would require ignoring the project's identity and freshness checks; those checks
remain enforced. No orders were sent to this server and no credentials were sent.

The new endpoint is available for HTTP compatibility checks only. It is NOT selected
as the runnable strategy execution backend. The official sandbox configuration
remains separate, and real broker order execution remains disabled.

To use this host for the complete strategy test, the service needs compatible
profile/commodity-margin reads, a current CRUDEOIL instruments/quote feed, stateful
LIMIT placement/cancellation, consistent orders/trades/positions and a documented
WebSocket full-tick endpoint. Server-side changes have not been made in this task.

Implementation: crates/kite-adapter/src/execution/native_client/custom_sandbox.rs.
Tests cover rejection of static/unrelated responses, unauthorized endpoint settings,
and credential-free GET requests with redirect rejection.
Evidence: /tmp/kite-custom-preflight.log, /tmp/kite-custom-workspace-tests.log,
/tmp/kite-custom-clippy.log.

Server-linked source: https://github.com/nordible/zerodha-sandbox

Verification: 176 workspace tests passed, zero failed; Clippy all-targets with
-D warnings, rustfmt and diff checks passed. One existing ignored child fixture
is exercised by its passing parent test. The actual custom preflight failed all
six compatibility checks as expected; no orders or credentials were sent.
