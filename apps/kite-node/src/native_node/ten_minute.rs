//! Pairwise OHLCV aggregation of adjacent five-minute candles within a session.
use super::supertrend_input::Input;
use anyhow::{Result, ensure};
use chrono::Timelike;
pub fn aggregate(input: &Input) -> Result<Input> {
    ensure!(
        input.interval == "5minute",
        "Aggregation requires five-minute input"
    );
    kite_adapter::http::historical::validate(&input.candles)?;
    let (pairs, remainder) = input.candles.as_chunks::<2>();
    ensure!(remainder.is_empty(), "Incomplete ten-minute candle");
    let mut candles = Vec::new();
    for [a, b] in pairs {
        let ta = a.time()?;
        let tb = b.time()?;
        ensure!(
            ta.date_naive() == tb.date_naive()
                && (tb - ta).num_seconds() == 300
                && ta.minute() % 10 == 0,
            "Missing, misaligned or cross-session pair"
        );
        let mut c = a.clone();
        c.high = a.high.max(b.high);
        c.low = a.low.min(b.low);
        c.close = b.close;
        c.volume = a
            .volume
            .checked_add(b.volume)
            .ok_or_else(|| anyhow::anyhow!("Volume overflow"))?;
        c.oi = b.oi;
        candles.push(c);
    }
    Ok(Input {
        instrument_id: input.instrument_id.clone(),
        instrument_token: input.instrument_token,
        interval: "10minute".into(),
        source: format!("{}; paired five-minute aggregation", input.source),
        candles,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_model::data::{BarType, Data};
    #[test]
    fn preserves_ohlcv_and_delivers_completed_ten_minute_bars_causally() {
        let input = super::super::supertrend_input::fixture();
        let out = aggregate(&input).unwrap();
        assert_eq!(out.candles.len(), 174);
        let c = &out.candles[0];
        let a = &input.candles[0];
        let b = &input.candles[1];
        assert_eq!(c.open, a.open);
        assert_eq!(c.close, b.close);
        assert_eq!(c.high, a.high.max(b.high));
        assert_eq!(c.low, a.low.min(b.low));
        assert_eq!(c.volume, a.volume + b.volume);
        assert_eq!(c.oi, b.oi);
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        assert_eq!(
            super::super::vwap_input::validate_interval(&out, date)
                .unwrap_err()
                .to_string(),
            "Insufficient prior-session warmup"
        );
        // Repeat earlier complete sessions to provide >=100 ten-minute warmup bars.
        let mut input = super::super::supertrend_input::fixture();
        let mut earlier = input.candles[..174].to_vec();
        for c in &mut earlier {
            c.timestamp = c.timestamp.replace("2026-09-14", "2026-09-11");
        }
        earlier.extend(input.candles);
        input.candles = earlier;
        let out = aggregate(&input).unwrap();
        let bt: BarType = "CRUDEOIL26SEPFUT.MCX-10-MINUTE-LAST-EXTERNAL"
            .parse()
            .unwrap();
        let data = super::super::vwap_input::replay_interval(&out, date, bt).unwrap();
        let (start, _) = super::super::vwap_input::bounds(date).unwrap();
        let first = data
            .iter()
            .position(|d| matches!(d, Data::Quote(_)))
            .unwrap();
        assert!(matches!(data[first],Data::Quote(q) if q.ts_event.as_u64()==start+2));
        assert!(matches!(data[first+2],Data::Bar(b) if b.ts_event.as_u64()==start+600_000_000_000));
        let wrong: BarType = "CRUDEOIL26SEPFUT.MCX-5-MINUTE-LAST-EXTERNAL"
            .parse()
            .unwrap();
        assert!(super::super::vwap_input::replay_interval(&out, date, wrong).is_err());
    }
    #[test]
    fn missing_or_incomplete_pairs_are_rejected() {
        let mut input = super::super::supertrend_input::fixture();
        input.candles.remove(3);
        assert!(aggregate(&input).is_err());
        input.candles.remove(5);
        assert!(aggregate(&input).is_err());
    }
}
