//! Offline replay of recorded KiteFullTick catalogs through the v2.10 Ribbon engine.
//!
//! This command never constructs an execution client and never submits orders.
use anyhow::{Context, Result, ensure};
use nautilus_model::data::HasTsInit;
use std::path::Path;

#[derive(Debug, serde::Serialize)]
struct ReplaySummary {
    mode: &'static str,
    packets: usize,
    instrument_token: u32,
    first_event_ns: u64,
    last_event_ns: u64,
    first_receive_ns: u64,
    last_receive_ns: u64,
    duration_ms: u64,
    source_fresh_packets: usize,
    price_min: f64,
    price_max: f64,
    bucket_changes: usize,
    previews: usize,
    trusted_previews: usize,
    events_emitted: usize,
}

#[derive(Debug, serde::Serialize)]
struct RecoverySummary {
    boundary_ns: u64,
    packets_before_boundary: usize,
    packets_after_boundary: usize,
    trusted_before_canonical: usize,
    trusted_after_canonical: usize,
    resumed_after_canonical: bool,
    events_emitted: usize,
}

fn trade_ns(tick: &kite_adapter::data::full_tick::KiteFullTick) -> Result<u64> {
    let raw = tick
        .snapshot
        .raw
        .as_ref()
        .context("recorded full tick is missing raw packet")?;
    let seconds = raw
        .full
        .as_ref()
        .and_then(|f| f.last_trade_timestamp)
        .context("recorded full tick is missing last-trade timestamp")?;
    Ok(u64::from(seconds) * 1_000_000_000)
}

fn price(tick: &kite_adapter::data::full_tick::KiteFullTick) -> Result<f64> {
    let raw = tick
        .snapshot
        .raw
        .as_ref()
        .context("recorded full tick is missing raw packet")?;
    ensure!(raw.ltp_paise > 0, "recorded LTP must be positive");
    Ok(f64::from(raw.ltp_paise) / 100.0)
}

fn validate_ticks(
    ticks: &[kite_adapter::data::full_tick::KiteFullTick],
) -> Result<(u32, f64, f64, usize)> {
    ensure!(
        !ticks.is_empty(),
        "catalog contains no KiteFullTick packets"
    );
    let token = ticks[0].snapshot.instrument_token;
    ensure!(token > 0, "recorded instrument token is zero");
    let mut previous_receive = 0;
    let mut previous_event = 0;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut fresh = 0;
    for tick in ticks {
        ensure!(
            tick.snapshot.instrument_token == token,
            "catalog mixes instrument tokens"
        );
        let receive = tick.ts_init().as_u64();
        let event = trade_ns(tick)?;
        ensure!(
            previous_receive == 0 || receive >= previous_receive,
            "catalog receive timestamps are out of order"
        );
        ensure!(
            previous_event == 0 || event >= previous_event,
            "catalog last-trade timestamps are out of order"
        );
        previous_receive = receive;
        previous_event = event;
        let p = price(tick)?;
        min = min.min(p);
        max = max.max(p);
        fresh += usize::from(tick.snapshot.source_fresh);
    }
    Ok((token, min, max, fresh))
}

fn seed(
    engine: &mut super::trend_ribbon_realtime::RealtimeRibbon,
    last_close_ns: u64,
    bar_ns: u64,
    bars: usize,
    base: f64,
) -> Result<(f64, f64, f64)> {
    ensure!(bars >= 60, "replay warmup requires at least 60 bars");
    let mut last = base;
    let mut last_high = base + 2.0;
    let mut last_low = base - 2.0;
    for i in 0..bars {
        let wave = ((i as f64) * 0.37).sin() * 4.0;
        let drift = (i as f64 - bars as f64) * 0.015;
        let close = base + wave + drift;
        let high = close + 2.0;
        let low = close - 2.0;
        let close_ns = last_close_ns - (bars - 1 - i) as u64 * bar_ns;
        engine.on_confirmed_bar(high, low, close, close_ns)?;
        last = close;
        last_high = high;
        last_low = low;
    }
    Ok((last_high, last_low, last))
}

fn as_recorded(
    settings: &super::trend_ribbon::Settings,
    bar_ns: u64,
    ticks: &[kite_adapter::data::full_tick::KiteFullTick],
    token: u32,
    min: f64,
    max: f64,
    fresh: usize,
) -> Result<ReplaySummary> {
    let first = &ticks[0];
    let first_event = trade_ns(first)?;
    let first_receive = first.ts_init().as_u64();
    let first_bucket = first_event / bar_ns * bar_ns;
    let mut engine = super::trend_ribbon_realtime::RealtimeRibbon::new(settings, bar_ns)?;
    seed(&mut engine, first_bucket, bar_ns, 80, price(first)?)?;

    let mut previews = 0;
    let mut trusted = 0;
    let mut events = 0;
    let mut buckets = 0;
    let mut previous_bucket = None;
    for tick in ticks {
        let event_ns = trade_ns(tick)?;
        let bucket = event_ns / bar_ns * bar_ns;
        if previous_bucket.is_some_and(|v| v != bucket) {
            buckets += 1;
        }
        previous_bucket = Some(bucket);
        let (snapshot, event) = engine.on_tick(
            price(tick)?,
            event_ns,
            tick.ts_init().as_u64(),
            0,
            true,
            false,
        )?;
        if let Some(snapshot) = snapshot {
            previews += 1;
            trusted += usize::from(snapshot.trusted);
        }
        events += usize::from(event.is_some());
    }
    let last = ticks.last().expect("non-empty");
    Ok(ReplaySummary {
        mode: "as_recorded_startup_gate",
        packets: ticks.len(),
        instrument_token: token,
        first_event_ns: first_event,
        last_event_ns: trade_ns(last)?,
        first_receive_ns: first_receive,
        last_receive_ns: last.ts_init().as_u64(),
        duration_ms: last.ts_init().as_u64().saturating_sub(first_receive) / 1_000_000,
        source_fresh_packets: fresh,
        price_min: min,
        price_max: max,
        bucket_changes: buckets,
        previews,
        trusted_previews: trusted,
        events_emitted: events,
    })
}

fn boundary_primed(
    settings: &super::trend_ribbon::Settings,
    bar_ns: u64,
    ticks: &[kite_adapter::data::full_tick::KiteFullTick],
    token: u32,
    min: f64,
    max: f64,
    fresh: usize,
) -> Result<ReplaySummary> {
    let first = &ticks[0];
    let first_event = trade_ns(first)?;
    let first_receive = first.ts_init().as_u64();
    // A deterministic future-safe epoch aligned to the configured interval.
    let base_open = 2_000_000_000_000_000_000u64 / bar_ns * bar_ns;
    let mut engine = super::trend_ribbon_realtime::RealtimeRibbon::new(settings, bar_ns)?;
    let base = price(first)?;
    let (prior_high, prior_low, prior_close) =
        seed(&mut engine, base_open - bar_ns, bar_ns, 79, base)?;

    // First observed LTP is explicitly treated as the constructed candle open.
    let _ = engine.on_tick(base, base_open, base_open, 0, true, false)?;
    engine.on_confirmed_bar(prior_high, prior_low, prior_close, base_open)?;

    let mut previews = 0;
    let mut trusted = 0;
    let mut events = 0;
    let mut buckets = 0;
    let mut previous_bucket = None;
    for tick in ticks {
        let event_offset = trade_ns(tick)?.saturating_sub(first_event);
        let receive_offset = tick.ts_init().as_u64().saturating_sub(first_receive);
        let event_ns = base_open + event_offset;
        let now_ns = base_open + receive_offset.max(event_offset);
        let bucket = event_ns / bar_ns * bar_ns;
        if previous_bucket.is_some_and(|v| v != bucket) {
            buckets += 1;
        }
        previous_bucket = Some(bucket);
        let (snapshot, event) = engine.on_tick(price(tick)?, event_ns, now_ns, 0, true, false)?;
        if let Some(snapshot) = snapshot {
            previews += 1;
            trusted += usize::from(snapshot.trusted);
        }
        events += usize::from(event.is_some());
    }
    let last = ticks.last().expect("non-empty");
    Ok(ReplaySummary {
        mode: "boundary_primed_same_ltp_path",
        packets: ticks.len(),
        instrument_token: token,
        first_event_ns: base_open,
        last_event_ns: base_open + trade_ns(last)?.saturating_sub(first_event),
        first_receive_ns: base_open,
        last_receive_ns: base_open + last.ts_init().as_u64().saturating_sub(first_receive),
        duration_ms: last.ts_init().as_u64().saturating_sub(first_receive) / 1_000_000,
        source_fresh_packets: fresh,
        price_min: min,
        price_max: max,
        bucket_changes: buckets,
        previews,
        trusted_previews: trusted,
        events_emitted: events,
    })
}

fn boundary_recovery(
    settings: &super::trend_ribbon::Settings,
    bar_ns: u64,
    ticks: &[kite_adapter::data::full_tick::KiteFullTick],
) -> Result<Option<RecoverySummary>> {
    let first_event = trade_ns(&ticks[0])?;
    let first_bucket = first_event / bar_ns * bar_ns;
    let Some(cross_index) = ticks
        .iter()
        .position(|tick| trade_ns(tick).is_ok_and(|ts| ts / bar_ns * bar_ns > first_bucket))
    else {
        return Ok(None);
    };
    ensure!(
        cross_index > 0,
        "boundary recovery requires pre-boundary packets"
    );

    let mut engine = super::trend_ribbon_realtime::RealtimeRibbon::new(settings, bar_ns)?;
    seed(&mut engine, first_bucket, bar_ns, 80, price(&ticks[0])?)?;

    let mut trusted_before = 0;
    let mut trusted_after = 0;
    let mut events = 0;
    let mut prior_high = f64::NEG_INFINITY;
    let mut prior_low = f64::INFINITY;
    let mut prior_close = price(&ticks[0])?;

    for (index, tick) in ticks.iter().enumerate() {
        let event_ns = trade_ns(tick)?;
        let p = price(tick)?;
        if index < cross_index {
            prior_high = prior_high.max(p);
            prior_low = prior_low.min(p);
            prior_close = p;
        }
        let (snapshot, event) =
            engine.on_tick(p, event_ns, tick.ts_init().as_u64(), 0, true, false)?;
        if index == cross_index {
            // The first tick of the new candle must still be gated. This mirrors
            // live operation before the canonical completed bar arrives.
            ensure!(
                snapshot.is_some_and(|v| !v.trusted),
                "new candle became trusted before canonical prior bar arrived"
            );
            let boundary = event_ns / bar_ns * bar_ns;
            engine.on_confirmed_bar(
                prior_high.max(prior_close),
                prior_low.min(prior_close),
                prior_close,
                boundary,
            )?;
            continue;
        }
        if let Some(snapshot) = snapshot {
            if index < cross_index {
                trusted_before += usize::from(snapshot.trusted);
            } else {
                trusted_after += usize::from(snapshot.trusted);
            }
        }
        events += usize::from(event.is_some());
    }

    let boundary = trade_ns(&ticks[cross_index])? / bar_ns * bar_ns;
    Ok(Some(RecoverySummary {
        boundary_ns: boundary,
        packets_before_boundary: cross_index,
        packets_after_boundary: ticks.len() - cross_index,
        trusted_before_canonical: trusted_before,
        trusted_after_canonical: trusted_after,
        resumed_after_canonical: trusted_after > 0,
        events_emitted: events,
    }))
}

pub fn run(config: &str, catalog: &str) -> Result<()> {
    let selection = super::production::Selection::load(config)?;
    let settings = selection
        .trend_ribbon
        .as_ref()
        .context("replay requires a trend_ribbon_boswaves selection")?;
    let ticks = super::catalog::read_full(Path::new(catalog))?;
    let (token, min, max, fresh) = validate_ticks(&ticks)?;
    let bar_ns = selection.bar_ns();
    let recorded = as_recorded(settings, bar_ns, &ticks, token, min, max, fresh)?;
    let primed = boundary_primed(settings, bar_ns, &ticks, token, min, max, fresh)?;
    let recovery = boundary_recovery(settings, bar_ns, &ticks)?;

    if let Some(recovery) = &recovery {
        ensure!(
            recovery.trusted_before_canonical == 0
                && recovery.resumed_after_canonical
                && recovery.events_emitted == 0,
            "canonical-boundary recovery validation failed"
        );
    }

    ensure!(
        recorded.events_emitted == 0 && primed.events_emitted == 0,
        "validation replay unexpectedly emitted trading events"
    );
    ensure!(
        primed.trusted_previews == primed.previews && primed.previews == ticks.len(),
        "boundary-primed replay did not trust every recorded LTP update"
    );
    ensure!(
        recorded.trusted_previews == 0,
        "as-recorded mid-candle startup should remain gated for these short captures"
    );

    println!(
        "{}",
        serde_json::json!({
            "event":"trend_ribbon_v210_catalog_replay",
            "catalog":catalog,
            "interval":selection.interval_name(),
            "recorded":recorded,
            "boundary_primed":primed,
            "canonical_boundary_recovery":recovery,
            "synthetic_warmup":true,
            "trading_events_enabled":false,
            "broker_accessed":false,
            "live_orders_enabled":false,
            "purpose":"validate native KiteFullTick decoding, LTP forming-candle updates, startup gating and trusted intrabar indicator previews"
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_ordered_and_initializes_preview_math() {
        let settings: super::super::trend_ribbon::Settings =
            serde_json::from_value(serde_json::json!({
                "alma_length":34,"alma_offset":0.85,"alma_sigma":6.0,
                "deviation_length":34,"deviation_multiplier":0.65,
                "slope_length":3,"minimum_slope":0.08,"atr_length":14,
                "session":{"start":"09:00:00","end":"23:15:00","days":"23456","reset_daily":true},
                "realtime":{"enabled":true}
            }))
            .unwrap();
        let bar_ns = 300_000_000_000;
        let open = 2_000_000_000_000_000_000u64 / bar_ns * bar_ns;
        let mut engine =
            super::super::trend_ribbon_realtime::RealtimeRibbon::new(&settings, bar_ns).unwrap();
        let prior = seed(&mut engine, open - bar_ns, bar_ns, 80, 6000.0).unwrap();
        engine.on_tick(6001.0, open, open, 0, true, false).unwrap();
        engine
            .on_confirmed_bar(prior.0, prior.1, prior.2, open)
            .unwrap();
        let (snapshot, event) = engine
            .on_tick(
                6002.0,
                open + 1_000_000_000,
                open + 1_000_000_000,
                0,
                true,
                false,
            )
            .unwrap();
        assert!(snapshot.unwrap().trusted);
        assert!(event.is_none());
    }
}
