#!/usr/bin/env python3
"""Latency report from a kite-node run log (kite-node 2.24.1+).

Usage:  python3 tools/latency-report.py logs/sniper-paper-2026-10-12.jsonl [more.jsonl ...]

Reads the JSON lines `latency_bar`, `latency_order_sent` and `latency_fill` and prints, per
stage, count / median / p90 / max in milliseconds:

  bar      candle close -> bar handed to the strategy (`after_close_ms`), and the age of
           the tick that closed it (`tick_age_ms`, exchange time has 1 s resolution)
  strategy bar handed over -> order created by the strategy (paired with the latest bar
           of the same instrument at most 10 s earlier)
  order    created -> place call out (`strategy_to_send_ms`), admission, place call
  fill     ack -> reconciliation trigger, book read, trigger -> fill applied,
           place call out -> fill applied, by trigger (order_update, pending_timer, ...)
  total    candle close -> fill applied to the strategy

Paper runs use the simulated broker: the place call and the book read take no network
time and fills are found by a 1 s poll, so only `bar` and `strategy` are real there.
"""
import json
import statistics
import sys
from collections import defaultdict


def load(paths):
    for path in paths:
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                line = line.strip()
                if not line.startswith("{") or '"latency_' not in line:
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    continue


def pct(values, q):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(q * (len(ordered) - 1))))]


def row(name, values):
    values = [v for v in values if v is not None]
    if not values:
        print(f"  {name:<38} {'-':>6}")
        return
    print(
        f"  {name:<38} {len(values):>6} {statistics.median(values):>10.1f}"
        f" {pct(values, 0.9):>10.1f} {max(values):>10.1f}"
    )


def main(paths):
    bars, orders, fills = [], {}, []
    for event in load(paths):
        kind = event.get("event")
        if kind == "latency_bar":
            bars.append(event)
        elif kind == "latency_order_sent":
            orders[event["order"]] = event
        elif kind == "latency_fill":
            fills.append(event)
    if not (bars or orders or fills):
        sys.exit("No latency lines found (kite-node 2.24.1 or later writes them).")

    by_instrument = defaultdict(list)
    for bar in bars:
        by_instrument[bar["instrument"]].append(bar)
    for series in by_instrument.values():
        series.sort(key=lambda b: b["emitted_ms"])

    def bar_before(order):
        best = None
        for bar in by_instrument.get(order["instrument"], []):
            if bar["emitted_ms"] <= order["created_ms"]:
                best = bar
            else:
                break
        if best and order["created_ms"] - best["emitted_ms"] <= 10_000:
            return best
        return None

    print(f"{'':<40} {'count':>6} {'median':>10} {'p90':>10} {'max':>10}   (ms)")
    print("bar")
    row("candle close -> bar to strategy", [b["after_close_ms"] for b in bars])
    row("closing tick age (1 s resolution)", [b.get("tick_age_ms") for b in bars])

    print("strategy")
    paired = {oid: bar_before(o) for oid, o in orders.items()}
    row(
        "bar to strategy -> order created",
        [o["created_ms"] - paired[oid]["emitted_ms"] for oid, o in orders.items() if paired[oid]],
    )

    print("order")
    row("order created -> place call out", [o["strategy_to_send_ms"] for o in orders.values()])
    row("  of which admission (+ budget)", [o["admission_ms"] for o in orders.values()])
    row("place call (Kite round trip)", [o["place_call_ms"] for o in orders.values()])

    print("fill")
    triggers = defaultdict(list)
    for fill in fills:
        triggers[fill["trigger"]].append(fill)
    for trigger, group in sorted(triggers.items()):
        print(f"  trigger: {trigger}")
        row("  ack -> trigger", [f["trigger_after_ack_ms"] for f in group])
        row("  book read", [f["read_ms"] for f in group])
        row("  trigger -> fill applied", [f["applied_after_trigger_ms"] for f in group])
        row("  place call out -> fill applied", [f["send_to_applied_ms"] for f in group])

    print("total")
    totals = []
    for fill in fills:
        order = orders.get(fill["order"])
        bar = paired.get(fill["order"])
        if order and bar:
            applied = order["created_ms"] + fill["strategy_to_applied_ms"]
            totals.append(applied - bar["bar_close_ms"])
    row("candle close -> fill applied", totals)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
