#!/usr/bin/env python3
# dash_demo.py v1.0.1
"""Demo data for designing the live web dashboard (doc/LIVE_DASHBOARD.md).

Writes the same keys, hash fields, JSON shape and event stream the bot writes, under a
separate prefix (default `kite-demo`, never the bot's `kite-prod`), to the dashboard
Redis in config/dashboard.json. No Python packages needed: commands go through
`redis-cli --pipe`.

  python3 tools/dash_demo.py                 # load two demo slots (Sniper LONG, SATS flat)
  python3 tools/dash_demo.py --live 300      # ... then tick prices for 300 s, publishing on :live
  python3 tools/dash_demo.py --clear         # delete the demo keys

Keys: kite-demo:v1:{<slot>}:dash:state | :events | :live  (expire after 7 days)
"""
import argparse, json, random, subprocess, sys, time
from datetime import datetime, timedelta, timezone

IST = timezone(timedelta(hours=5, minutes=30))
TTL = 7 * 86_400


def resp(*args):
    out = [f"*{len(args)}\r\n".encode()]
    for a in args:
        b = a if isinstance(a, bytes) else str(a).encode()
        out.append(f"${len(b)}\r\n".encode() + b + b"\r\n")
    return b"".join(out)


class Redis:
    def __init__(self, url):
        self.url = url

    def pipe(self, cmds):
        data = b"".join(resp(*c) for c in cmds)
        r = subprocess.run(["redis-cli", "-u", self.url, "--no-auth-warning", "--pipe"], input=data, capture_output=True)
        out = r.stdout.decode(errors="replace")
        if r.returncode != 0 or "errors: 0" not in out:
            sys.exit(f"redis-cli --pipe failed: {out.strip()} {r.stderr.decode(errors='replace').strip()}")


def keys(prefix, slot):
    base = f"{prefix}:v1:{{{slot}}}:dash"
    return f"{base}:state", f"{base}:events", f"{base}:live"


def stamp(ms):
    return datetime.fromtimestamp(ms / 1000, IST).strftime("%H:%M:%S")


class Slot:
    """One demo slot: a board (same fields as the Rust `Board`) plus its event log."""

    def __init__(self, board, timeline):
        self.board = board
        self.events = []  # (ms, text), oldest first
        for ms, text in timeline:
            self.event(ms, text)
        self.seq = 0

    def event(self, ms, text):
        self.events.append((ms, text))
        self.board["events"] = [f"{stamp(m)}  {t}" for m, t in reversed(self.events[-8:])]

    def snapshot(self, now_ms):
        self.seq += 1
        return json.dumps({"seq": self.seq, "updated_at_ms": now_ms, "board": self.board}, ensure_ascii=False)

    def write_cmds(self, prefix, now_ms, new_events):
        state, ev, live = keys(prefix, self.board["slot"])
        cmds = [
            ["HSET", state, "snapshot", self.snapshot(now_ms), "updated_at_ms", now_ms, "seq", self.seq, "status", self.board["status"]],
            ["EXPIRE", state, TTL],
        ]
        for ms, text in new_events:
            cmds.append(["XADD", ev, "MAXLEN", "~", 1000, f"{ms}-*", "ts_ms", ms, "text", text])
        if new_events:
            cmds.append(["EXPIRE", ev, TTL])
        cmds.append(["PUBLISH", live, self.seq])
        return cmds


def ms_at(day, hh, mm, ss=0):
    return int(datetime(day.year, day.month, day.day, hh, mm, ss, tzinfo=IST).timestamp() * 1000)


def sniper_slot(day):
    t = lambda h, m, s=0: ms_at(day, h, m, s)
    board = {
        "mode": "LIVE (real Zerodha orders)", "slot": "crudeoilm-sniper-202610", "instrument": "CRUDEOILM26OCTFUT.MCX",
        "square_off": "23:15", "exit_rule": "TP1 1 / TP2 1 / TP3 1 lot, step stop",
        "redis_namespace": "kite-demo:v1:{crudeoilm-sniper-202610}:commands:20261010-5f2a9c1e",
        "point_value": 10.0, "lots": 3, "title": "SNIPER v2.1.0 · 3m Conservative", "model_title": "Precision Sniper",
        "model_rows": [["EMA 12/26", "5871 / 5862  (trend 5848)"], ["Score", "bull 6 · bear 1 of 7"], ["ADX / RSI", "27 / 61  Trend"]],
        "last_price": 5884.0, "last_bar": ["10 Oct 14:30", 5882.0], "live_bars": 96, "history_bars": 1297,
        "bar_ns": 180_000_000_000, "feed_fault": None, "warmed": True, "trend": 1,
        "supertrend": None, "tqi": 0.0, "regime": "", "next_r": [0.0, 0.0, 0.0],
        "position": 2.0, "entry_avg": 5862.0, "sl": 5862.0, "exchange_stop": None, "tps": [5880.0, 5898.0, 5916.0],
        "realized_points": 59.0, "round_trips": 2, "fills": 7, "status": "RUNNING", "halted": None, "dropped": 0, "events": [],
    }
    timeline = [
        (t(9, 5, 2), "Warm-up done on 1297 bars · trend BULLISH · ready yes · trading from this bar"),
        (t(10, 12, 1), "LONG A 3 lot @ 5838  SL 5826  TP 5850/5862/5874  (target set)"),
        (t(10, 12, 1), "ORDER Buy 3 lot (position 0 → target 3)"),
        (t(10, 12, 2), "FILL Buy 3 @ 5839"),
        (t(10, 31, 44), "TP1 5850: close 1 lot"),
        (t(10, 31, 44), "ORDER Sell 1 lot reduce-only (position 3 → target 2)"),
        (t(10, 31, 45), "FILL Sell 1 @ 5850"),
        (t(10, 52, 10), "TP2 5862: close 1 lot"),
        (t(10, 52, 11), "FILL Sell 1 @ 5862"),
        (t(11, 18, 3), "EXIT Step stop @ 5850  (model +1.00 R)"),
        (t(11, 18, 4), "FILL Sell 1 @ 5849"),
        (t(12, 3, 0), "SHORT B 3 lot @ 5841  SL 5853  TP 5829/5817/5805  (target set)"),
        (t(12, 3, 1), "FILL Sell 3 @ 5840"),
        (t(12, 27, 0), "EXIT SL @ 5853  (model -1.00 R)"),
        (t(12, 27, 1), "FILL Buy 3 @ 5854"),
        (t(14, 0, 1), "LONG A+ 3 lot @ 5862  SL 5844  TP 5880/5898/5916  (target set)"),
        (t(14, 0, 1), "ORDER Buy 3 lot (position 0 → target 3)"),
        (t(14, 0, 2), "FILL Buy 3 @ 5862"),
        (t(14, 22, 37), "TP1 5880: close 1 lot"),
        (t(14, 22, 38), "FILL Sell 1 @ 5880"),
    ]
    return Slot(board, timeline)


def sats_slot(day):
    t = lambda h, m, s=0: ms_at(day, h, m, s)
    board = {
        "mode": "PAPER (Kite mock execution)", "slot": "crudeoilm-sats-202610", "instrument": "CRUDEOILM26OCTFUT.MCX",
        "square_off": "23:15", "exit_rule": "Trail: BE@TP1 + SuperTrend",
        "redis_namespace": "kite-demo:v1:{crudeoilm-sats-202610}:commands:20261010-0b7d33aa",
        "point_value": 10.0, "lots": 1, "title": "", "model_title": "", "model_rows": [],
        "last_price": 5884.0, "last_bar": ["10 Oct 14:30", 5882.0], "live_bars": 57, "history_bars": 1104,
        "bar_ns": 300_000_000_000, "feed_fault": None, "warmed": True, "trend": 1,
        "supertrend": 5851.0, "tqi": 0.64, "regime": "Trending", "next_r": [1.0, 2.0, 3.0],
        "position": 0.0, "entry_avg": None, "sl": None, "exchange_stop": None, "tps": None,
        "realized_points": -12.0, "round_trips": 2, "fills": 4, "status": "RUNNING", "halted": None, "dropped": 0, "events": [],
    }
    timeline = [
        (t(9, 5, 4), "Warm-up done on 1104 history bars · trend BULLISH · warmed yes · bar closing 09:10 is backfilled from Kite history ~45s later"),
        (t(9, 10, 48), "Backfilled bar 09:10 from Kite history (48s after close)"),
        (t(10, 15, 1), "BUY Buy 1 lot  (model 5843, SL 5822, TP1 5864)"),
        (t(10, 15, 2), "FILL Buy 1 @ 5843"),
        (t(10, 41, 9), "TRAIL breakeven 5843"),
        (t(11, 20, 3), "FLATTEN Sell 1 lot (Breakeven stop hit at 5842 (stop 5843))"),
        (t(11, 20, 4), "FILL Sell 1 @ 5842"),
        (t(12, 5, 1), "SELL Sell 1 lot  (model 5840, SL 5859, TP1 5821)"),
        (t(12, 5, 2), "FILL Sell 1 @ 5840"),
        (t(12, 35, 0), "FLATTEN Buy 1 lot (trend flip (BUY))"),
        (t(12, 35, 1), "FILL Buy 1 @ 5851"),
        (t(12, 35, 2), "BUY waiting for exit fill before entering"),
        (t(12, 35, 3), "BUY signal blocked (cut-off/stopping/halted)"),
    ]
    return Slot(board, timeline)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", default="config/dashboard.json")
    ap.add_argument("--prefix", default="kite-demo")
    ap.add_argument("--live", type=int, default=0, metavar="SECONDS", help="tick prices and publish for this long")
    ap.add_argument("--clear", action="store_true", help="delete the demo keys and exit")
    a = ap.parse_args()
    if a.prefix == "kite-prod":
        sys.exit("Refusing to write demo data under the bot's own prefix")
    url = json.load(open(a.config))["redis_dashboard_url"]
    r = Redis(url)
    now = datetime.now(IST)
    slots = [sniper_slot(now.date()), sats_slot(now.date())]
    now_ms = int(time.time() * 1000)
    for s in slots:
        # slide the day's timeline so its last event is 5 minutes ago (never in the future)
        shift = now_ms - 5 * 60_000 - s.events[-1][0]
        timeline, s.events = [(m + shift, t) for m, t in s.events], []
        for m, t in timeline:
            s.event(m, t)
        bar_ms = s.board["bar_ns"] // 1_000_000
        last_close = now_ms // bar_ms * bar_ms
        s.board["last_bar"][0] = datetime.fromtimestamp(last_close / 1000, IST).strftime("%d %b %H:%M")
    all_keys = [k for s in slots for k in keys(a.prefix, s.board["slot"])[:2]]
    if a.clear:
        r.pipe([["DEL", *all_keys]])
        print("Deleted:", *all_keys, sep="\n  ")
        return
    cmds = [["DEL", *all_keys]]
    for s in slots:
        cmds += s.write_cmds(a.prefix, now_ms, s.events)
    r.pipe(cmds)
    for s in slots:
        state, ev, live = keys(a.prefix, s.board["slot"])
        print(f"{s.board['slot']}: {len(s.events)} events\n  {state}\n  {ev}\n  {live}")

    # live mode: random-walk price each second, a new bar every bar_ns, publish each write
    end = time.time() + a.live
    price = slots[0].board["last_price"]
    while time.time() < end:
        time.sleep(1)
        price = round(price + random.choice([-2, -1, -1, 0, 0, 1, 1, 2]), 0)
        now_ms = int(time.time() * 1000)
        cmds = []
        for s in slots:
            b, new = s.board, []
            b["last_price"] = price
            bar_ms = b["bar_ns"] // 1_000_000
            if now_ms // bar_ms != (now_ms - 1000) // bar_ms:  # a bar just closed
                b["live_bars"] += 1
                b["last_bar"] = [datetime.fromtimestamp(now_ms / 1000, IST).strftime("%d %b %H:%M"), price]
            if b["position"] and b.get("tps") and price >= b["tps"][1] and b["position"] == 2.0:
                b["position"], b["fills"] = 1.0, b["fills"] + 1
                b["realized_points"] += b["tps"][1] - b["entry_avg"]
                b["sl"] = b["tps"][0]
                for text in (f"TP2 {b['tps'][1]:.0f}: close 1 lot", f"FILL Sell 1 @ {b['tps'][1]:.0f}"):
                    s.event(now_ms, text)
                    new.append((now_ms, text))
            cmds += s.write_cmds(a.prefix, now_ms, new)
        r.pipe(cmds)
        print(f"\r{datetime.now(IST):%H:%M:%S}  price {price:.0f}  ", end="", flush=True)
    if a.live:
        print()


if __name__ == "__main__":
    main()
