use chrono::NaiveDate;
use kite_adapter::http::historical::Candle;

pub fn bounds(date: NaiveDate) -> (u64, u64) {
    let parse = |time: &str| {
        chrono::DateTime::parse_from_rfc3339(&format!("{date}T{time}+05:30"))
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap() as u64
    };
    (parse("09:00:00"), parse("23:30:00"))
}

pub fn two_day_candles() -> Vec<Candle> {
    let mut candles = Vec::new();
    for day in [14, 15] {
        let date = NaiveDate::from_ymd_opt(2026, 9, day).unwrap();
        for i in 0..174 {
            let minute = 540 + i * 5;
            let p = 6000.
                + if (i / 20) % 2 == 0 {
                    (i % 20) as f64 * 10.
                } else {
                    200. - (i % 20) as f64 * 10.
                };
            candles.push(Candle {
                timestamp: format!("{date}T{:02}:{:02}:00+05:30", minute / 60, minute % 60),
                open: p,
                high: p + 5.,
                low: p - 5.,
                close: p + 1.,
                volume: 100,
                oi: 1000,
            });
        }
    }
    candles
}
