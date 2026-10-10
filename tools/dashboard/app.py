# app.py v1.0.0
"""Live web dashboard for the Nautilus Kite bot (Streamlit).

Reads the dashboard Redis the bot publishes to (doc/LIVE_DASHBOARD.md): per slot
`<prefix>:v1:{<slot>}:dash:state` (hash, `snapshot` JSON) and `:events` (stream).
Read-only: it never writes to Redis and never talks to the trading Redis or Kite.

  tools/dashboard/run.sh            # http://127.0.0.1:8501 (open via an SSH tunnel)

The Redis URL comes from config/dashboard.json (server side only; never sent to the
browser). Prefix: `kite-prod` (the bot) or `kite-demo` (tools/dash_demo.py).
"""
import json
import os
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import altair as alt
import pandas as pd
import redis
import streamlit as st

VERSION = "1.0.0"
IST = timezone(timedelta(hours=5, minutes=30))
ROOT = Path(__file__).resolve().parents[2]
CONFIG = Path(os.environ.get("DASHBOARD_CONFIG", ROOT / "config" / "dashboard.json"))
PREFIXES = ["kite-prod", "kite-demo"]
STALE_S = 5  # the bot writes at least once a second
TRAIL_POINTS = 900  # price points kept per slot in this browser session

st.set_page_config(page_title="Kite bot · live", page_icon="📈", layout="wide")

st.markdown(
    """
<style>
  .block-container {padding-top: 3.2rem; padding-bottom: 1rem;}
  .kv {display:grid; grid-template-columns: 7.5rem 1fr; row-gap:.25rem; font-size:.92rem;}
  .kv .k {color: var(--text-color); opacity:.6;}
  .kv .v {font-variant-numeric: tabular-nums;}
  .ev {font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size:.85rem;}
  div[data-testid="stMetricValue"] {font-variant-numeric: tabular-nums;}
</style>
""",
    unsafe_allow_html=True,
)


# ── data ───────────────────────────────────────────────────────────────────────

@st.cache_resource
def client() -> redis.Redis:
    url = json.loads(CONFIG.read_text())["redis_dashboard_url"]
    return redis.Redis.from_url(url, decode_responses=True, socket_timeout=3, socket_connect_timeout=3,
                                health_check_interval=30)


def keys(prefix: str, slot: str) -> tuple[str, str]:
    base = f"{prefix}:v1:{{{slot}}}:dash"
    return f"{base}:state", f"{base}:events"


@st.cache_data(ttl=10, show_spinner=False)
def slots(prefix: str) -> list[str]:
    found = set()
    for k in client().scan_iter(match=f"{prefix}:v1:*:dash:state", count=200):
        if "{" in k and "}" in k:
            found.add(k[k.index("{") + 1:k.index("}")])
    # main strategy (Sniper) first, then the rest by name
    return sorted(found, key=lambda x: (0 if "sniper" in x else 1, x))


def snapshot(prefix: str, slot: str) -> dict | None:
    raw = client().hget(keys(prefix, slot)[0], "snapshot")
    return json.loads(raw) if raw else None


def events(prefix: str, slot: str, count: int) -> pd.DataFrame:
    rows = client().xrevrange(keys(prefix, slot)[1], count=count)
    data = [
        (datetime.fromtimestamp(int(f.get("ts_ms", i.split("-")[0])) / 1000, IST), f.get("text", ""))
        for i, f in rows
    ]
    return pd.DataFrame(data, columns=["Time", "Event"])


# ── formatting ─────────────────────────────────────────────────────────────────

def px(v) -> str:
    return "—" if v is None else f"{v:,.0f}"


def money(pts: float | None, pv: float) -> str:
    if pts is None:
        return "—"
    return f"{pts:+,.1f} pts · ₹{pts * pv:+,.0f}"


def unrealised(b: dict) -> float | None:
    if not b.get("position") or b.get("last_price") is None or b.get("entry_avg") is None:
        return None
    return b["position"] * (b["last_price"] - b["entry_avg"])


def ago(ms: int) -> float:
    return time.time() - ms / 1000


def badge(text: str, kind: str) -> str:
    # icon + label, never colour alone
    color, icon = {
        "good": ("green", "●"), "warn": ("orange", "◐"), "bad": ("red", "■"), "info": ("blue", "◆"), "muted": ("gray", "○"),
    }[kind]
    return f":{color}-background[{icon} {text}]"


def status_kind(s: str) -> str:
    return {"RUNNING": "good", "STARTING": "warn", "STOPPING": "warn", "SQUARED OFF": "muted"}.get(s, "bad")


def side(b: dict) -> str:
    p = b.get("position") or 0
    return f"LONG {p:.0f} lot" if p > 0 else f"SHORT {-p:.0f} lot" if p < 0 else "FLAT"


def next_bar(bar_ns: int) -> str:
    if not bar_ns:
        return "—"
    now_ns = time.time_ns()
    nxt = (now_ns // bar_ns + 1) * bar_ns
    left = (nxt - now_ns) // 1_000_000_000
    return f"{datetime.fromtimestamp(nxt / 1e9, IST):%H:%M}  (in {left // 60}:{left % 60:02d})"


def square_off_in(hhmm: str) -> str:
    try:
        h, m = map(int, hhmm.split(":"))
    except ValueError:
        return hhmm or "—"
    now = datetime.now(IST)
    t = now.replace(hour=h, minute=m, second=0, microsecond=0)
    left = int((t - now).total_seconds())
    return f"{hhmm} IST · in {left // 3600}h {left % 3600 // 60:02d}m" if left > 0 else f"{hhmm} IST · reached"


def kv(rows: list[tuple[str, str]]) -> str:
    cells = "".join(f'<div class="k">{k}</div><div class="v">{v}</div>' for k, v in rows)
    return f'<div class="kv">{cells}</div>'


# ── price trail (kept in this browser session) ─────────────────────────────────

def remember_price(slot_id: str, b: dict, updated_ms: int) -> pd.DataFrame:
    trail = st.session_state.setdefault("trail", {}).setdefault(slot_id, [])
    if b.get("last_price") is not None and (not trail or trail[-1][0] != updated_ms):
        trail.append((updated_ms, b["last_price"]))
        del trail[:-TRAIL_POINTS]
    return pd.DataFrame(
        [(datetime.fromtimestamp(ms / 1000, IST).replace(tzinfo=None), p) for ms, p in trail], columns=["Time", "Price"]
    )


def price_chart(df: pd.DataFrame, b: dict):
    levels = []
    if b.get("position"):
        if b.get("entry_avg") is not None:
            levels.append(("Entry", b["entry_avg"], "#a1a1aa"))
        if b.get("sl") is not None:
            levels.append(("Stop", b["sl"], "#fb923c"))
        for i, tp in enumerate(b.get("tps") or [], 1):
            levels.append((f"TP{i}", tp, "#4ade80"))
    # levels at the same price share one line and one label ("Entry = Stop 5,862")
    merged: dict[float, list] = {}
    for name, v, color in levels:
        if v in merged:
            merged[v][0] += f" = {name}"
            if name == "Stop":
                merged[v][1] = color  # a line that is also the stop keeps the stop colour
        else:
            merged[v] = [name, color]
    levels = [(name, v, color) for v, (name, color) in merged.items()]
    prices = list(df["Price"]) + [v for _, v, _ in levels]
    lo, hi = min(prices), max(prices)
    pad = max(2.0, (hi - lo) * 0.08)
    y = alt.Y("Price:Q", scale=alt.Scale(domain=[lo - pad, hi + pad], zero=False), title=None,
              axis=alt.Axis(grid=True, gridOpacity=0.15, format=",.0f"))
    base = alt.Chart(df).encode(x=alt.X("Time:T", title=None, axis=alt.Axis(format="%H:%M:%S", gridOpacity=0, tickCount=6, labelOverlap=True)))
    line = base.mark_line(strokeWidth=2, color="#60a5fa").encode(y=y)
    hover = alt.selection_point(fields=["Time"], nearest=True, on="pointerover", empty=False)
    dots = base.mark_circle(size=64, color="#60a5fa").encode(
        y=y, opacity=alt.condition(hover, alt.value(1), alt.value(0)),
        tooltip=[alt.Tooltip("Time:T", format="%H:%M:%S"), alt.Tooltip("Price:Q", format=",.0f")],
    ).add_params(hover)
    layers = [line, dots]
    if levels:
        ldf = pd.DataFrame(levels, columns=["Level", "Price", "Color"])
        rules = alt.Chart(ldf).mark_rule(strokeDash=[4, 4], strokeWidth=1.5).encode(
            y="Price:Q", color=alt.Color("Color:N", scale=None), tooltip=["Level", alt.Tooltip("Price:Q", format=",.0f")]
        )
        labels = alt.Chart(ldf).mark_text(align="left", dx=4, dy=-6, fontSize=11).encode(
            y="Price:Q", x=alt.value(0), text=alt.Text("label:N"), color=alt.Color("Color:N", scale=None)
        ).transform_calculate(label="datum.Level + ' ' + format(datum.Price, ',.0f')")
        layers += [rules, labels]
    return alt.layer(*layers).properties(height=260)


# ── views ──────────────────────────────────────────────────────────────────────

def overview(prefix: str, names: list[str]):
    cols = st.columns(max(1, len(names)))
    for col, name in zip(cols, names):
        snap = snapshot(prefix, name)
        with col.container(border=True):
            if not snap:
                st.markdown(f"**{name}**  \n{badge('no data', 'muted')}")
                continue
            b, age = snap["board"], ago(snap["updated_at_ms"])
            fresh = badge(f"live {age:.0f}s", "good") if age < STALE_S else badge(f"stale {age:,.0f}s", "bad")
            st.markdown(f"**{name}**  \n{badge(b['status'], status_kind(b['status']))} {fresh}")
            u = unrealised(b)
            st.markdown(kv([
                ("Position", side(b)),
                ("Unrealised", money(u, b["point_value"])),
                ("Realised", money(b["realized_points"], b["point_value"])),
            ]), unsafe_allow_html=True)


def slot_view(prefix: str, name: str, event_count: int):
    snap = snapshot(prefix, name)
    if not snap:
        st.info(f"No state for {name} under `{prefix}` yet.")
        return
    b, age = snap["board"], ago(snap["updated_at_ms"])
    pv = b["point_value"]
    title = b["title"] or "SATS"

    st.markdown(
        f"### {title}  \n"
        f"{b['instrument']} · `{b['slot']}`   "
        f"{badge(b['mode'].split(' ')[0], 'bad' if b['mode'].startswith('LIVE') else 'info')} "
        f"{badge(b['status'], status_kind(b['status']))} "
        f"{badge(f'updated {age:.0f}s ago', 'good' if age < STALE_S else 'bad')}"
    )
    if age >= STALE_S:
        st.warning(f"No update for {age:,.0f} s: the bot is stopped, or cannot reach the dashboard Redis.", icon="⏸️")
    if b.get("halted"):
        st.error(f"HALTED: {b['halted']}", icon="🛑")
    if b.get("feed_fault"):
        st.warning(f"Feed fault: {b['feed_fault']}", icon="📡")
    if b.get("dropped"):
        st.caption(f"Dashboard dropped {b['dropped']} updates (queue full)")

    u = unrealised(b)
    m = st.columns(5)
    m[0].metric("Price", px(b.get("last_price")))
    m[1].metric("Position", side(b), f"entry {px(b.get('entry_avg'))}" if b.get("position") else None,
                delta_color="off", delta_arrow="off")
    m[2].metric("Unrealised ₹", "—" if u is None else f"{u * pv:+,.0f}", None if u is None else f"{u:+,.1f} pts")
    m[3].metric("Realised ₹", f"{b['realized_points'] * pv:+,.0f}", f"{b['realized_points']:+,.1f} pts")
    m[4].metric("Round trips", b["round_trips"], f"{b['fills']} fills", delta_color="off", delta_arrow="off")

    left, right = st.columns([3, 2])
    with left:
        df = remember_price(f"{prefix}/{name}", b, snap["updated_at_ms"])
        with st.container(border=True):
            st.markdown("**Price** · this session" + ("  ·  dashed: entry, stop, targets" if b.get("position") else ""))
            if len(df) >= 2:
                st.altair_chart(price_chart(df, b), width="stretch")
            else:
                st.caption("Collecting prices…")
    with right:
        with st.container(border=True):
            st.markdown("**Market**")
            bar = b.get("last_bar")
            st.markdown(kv([
                ("Last bar", f"{bar[0]} · close {px(bar[1])}" if bar else "—"),
                ("Next bar", next_bar(b.get("bar_ns", 0))),
                ("Bars", f"history {b['history_bars']:,} · live {b['live_bars']:,}"),
                ("Feed", "⚠ " + b["feed_fault"] if b.get("feed_fault") else "OK"),
            ]), unsafe_allow_html=True)
        with st.container(border=True):
            st.markdown(f"**{b.get('model_title') or 'SATS'}**")
            trend = {1: "▲ BULLISH", -1: "▼ BEARISH"}.get(b.get("trend"), "— neutral")
            rows = [("Trend", trend)]
            if b.get("model_rows"):
                rows += [(k, v) for k, v in b["model_rows"]]
            else:
                rows += [("SuperTrend", px(b.get("supertrend"))), ("TQI", f"{b.get('tqi', 0):.2f} · {b.get('regime') or '—'}")]
            rows += [("Exit", b.get("exit_rule") or "—"), ("Warmed", "yes" if b.get("warmed") else "no")]
            st.markdown(kv(rows), unsafe_allow_html=True)

    c1, c2 = st.columns([2, 3])
    with c1:
        with st.container(border=True):
            st.markdown("**Position**")
            tps = b.get("tps") or [None, None, None]
            st.markdown(kv([
                ("Side", side(b)),
                ("Entry", px(b.get("entry_avg"))),
                ("Stop", px(b.get("sl")) + (f" · SL-M {px(b['exchange_stop'])} at Zerodha" if b.get("exchange_stop") else "")),
                ("Targets", " / ".join(px(t) for t in tps)),
                ("Unrealised", money(u, pv)),
                ("Realised", money(b["realized_points"], pv)),
            ]), unsafe_allow_html=True)
        with st.container(border=True):
            st.markdown("**Session**")
            st.markdown(kv([
                ("Square-off", square_off_in(b.get("square_off", ""))),
                ("Lots", str(b["lots"])),
                ("Run", f"<code>{b.get('redis_namespace', '')}</code>"),
            ]), unsafe_allow_html=True)
    with c2:
        with st.container(border=True):
            st.markdown("**Events** · newest first")
            ev = events(prefix, name, event_count)
            if ev.empty:
                st.caption("none yet")
            else:
                ev["Time"] = ev["Time"].dt.strftime("%H:%M:%S")
                st.dataframe(ev, hide_index=True, width="stretch", height=300,
                             column_config={"Time": st.column_config.TextColumn(width="small")})


# ── page ───────────────────────────────────────────────────────────────────────

with st.sidebar:
    st.markdown(f"**Kite bot · live** v{VERSION}")
    prefix = st.selectbox("Key prefix", PREFIXES, index=PREFIXES.index(os.environ.get("DASH_PREFIX", "kite-prod"))
                          if os.environ.get("DASH_PREFIX", "kite-prod") in PREFIXES else 0,
                          help="kite-prod = the bot · kite-demo = tools/dash_demo.py")
    refresh = st.select_slider("Refresh", options=[1, 2, 5, 10], value=1, format_func=lambda s: f"{s} s")
    event_count = st.select_slider("Events shown", options=[20, 50, 100, 250], value=50)
    st.caption("Read-only view of the dashboard Redis. The Redis URL stays on the server.")

try:
    names = slots(prefix)
except (redis.RedisError, OSError, KeyError, json.JSONDecodeError) as e:
    st.error(f"Cannot read the dashboard Redis ({type(e).__name__}). Check {CONFIG}.")
    st.stop()

if not names:
    st.info(f"No slots publishing under `{prefix}` yet. For demo data run `python3 tools/dash_demo.py` and pick `kite-demo`.")
    st.stop()


@st.fragment(run_every=refresh)
def live():
    try:
        overview(prefix, names)
        tabs = st.tabs(names)
        for tab, name in zip(tabs, names):
            with tab:
                slot_view(prefix, name, event_count)
    except (redis.RedisError, OSError) as e:
        st.warning(f"Dashboard Redis unreachable ({type(e).__name__}); retrying…", icon="🔌")
    st.caption(f"{datetime.now(IST):%d %b %Y %H:%M:%S} IST · prefix `{prefix}`")


live()
