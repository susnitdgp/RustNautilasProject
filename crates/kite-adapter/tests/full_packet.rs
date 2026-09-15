use kite_adapter::{
    mapping::market_data,
    websocket::{models::BinaryFrame, parser},
};
fn packet(len: usize) -> Vec<u8> {
    let mut p = vec![0u8; len];
    p[0..4].copy_from_slice(&144870151u32.to_be_bytes());
    p[4..8].copy_from_slice(&600000u32.to_be_bytes());
    if len >= 44 {
        for (offset, value) in [
            (8, 7u32),
            (12, 599900),
            (16, 12345),
            (20, 111),
            (24, 222),
            (28, 598000),
            (32, 610000),
            (36, 590000),
            (40, 595000),
        ] {
            p[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
    }
    if len == 184 {
        for (offset, value) in [
            (44, 1789450000u32),
            (48, 500),
            (52, 600),
            (56, 400),
            (60, 1789450001),
        ] {
            p[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        for i in 0..10 {
            let o = 64 + i * 12;
            p[o..o + 4].copy_from_slice(&(100 + i as u32).to_be_bytes());
            p[o + 4..o + 8].copy_from_slice(&(600000 + i as u32 * 100).to_be_bytes());
            p[o + 8..o + 10].copy_from_slice(&(10 + i as u16).to_be_bytes());
            p[o + 10..o + 12].copy_from_slice(&65535u16.to_be_bytes());
        }
    }
    let mut frame = vec![0, 1];
    frame.extend_from_slice(&(len as u16).to_be_bytes());
    frame.extend(p);
    frame
}
#[test]
fn full_packet_retains_every_documented_field_and_ten_depth_entries() {
    let BinaryFrame::Ticks(ticks) = parser::parse(&packet(184)).unwrap() else {
        panic!()
    };
    let tick = &ticks[0];
    let q = tick.quote_fields.as_ref().unwrap();
    let f = tick.full.as_ref().unwrap();
    assert_eq!(tick.last_quantity, Some(7));
    assert_eq!(tick.cumulative_volume, Some(12345));
    assert_eq!(
        (
            q.average_price_paise,
            q.total_buy_quantity,
            q.total_sell_quantity
        ),
        (599900, 111, 222)
    );
    assert_eq!(
        (q.open_paise, q.high_paise, q.low_paise, q.close_paise),
        (598000, 610000, 590000, 595000)
    );
    assert_eq!(
        (
            f.open_interest,
            f.open_interest_day_high,
            f.open_interest_day_low
        ),
        (500, 600, 400)
    );
    assert_eq!(f.last_trade_timestamp, Some(1789450000));
    assert_eq!(f.exchange_timestamp, Some(1789450001));
    for (i, d) in f.bids.iter().chain(f.asks.iter()).enumerate() {
        assert_eq!(d.quantity, 100 + i as u32);
        assert_eq!(d.price_paise, 600000 + i as i32 * 100);
        assert_eq!(d.orders, 10 + i as u16);
    }
    let snapshot = market_data::snapshot(
        tick,
        chrono::DateTime::from_timestamp(1789450002, 0).unwrap(),
        3,
    );
    assert_eq!(snapshot.raw.as_ref(), Some(tick));
}
#[test]
fn short_packets_are_not_marked_full() {
    for len in [8, 44] {
        let BinaryFrame::Ticks(ticks) = parser::parse(&packet(len)).unwrap() else {
            panic!()
        };
        assert!(ticks[0].full.is_none());
        assert_eq!(ticks[0].quote_fields.is_some(), len == 44);
    }
}
#[test]
fn native_full_custom_data_roundtrip_keeps_depth() {
    use kite_adapter::data::full_tick::KiteFullTick;
    use nautilus_model::data::{CustomData, QuoteTick, custom::ensure_custom_data_json_registered};
    let BinaryFrame::Ticks(ticks) = parser::parse(&packet(184)).unwrap() else {
        panic!()
    };
    let snapshot = market_data::snapshot(
        &ticks[0],
        chrono::DateTime::from_timestamp(1789450002, 0).unwrap(),
        1,
    );
    let q = QuoteTick::new(
        "CRUDEOIL26SEPFUT.MCX".into(),
        nautilus_model::types::Price::new(6000.0, 0),
        nautilus_model::types::Price::new(6005.0, 0),
        100u32.into(),
        105u32.into(),
        1789450001000000000u64.into(),
        1789450002000000000u64.into(),
    );
    ensure_custom_data_json_registered::<KiteFullTick>().unwrap();
    let custom = CustomData::from_arc(std::sync::Arc::new(KiteFullTick { snapshot, quote: q }));
    let encoded = serde_json::to_vec(&custom).unwrap();
    let decoded = CustomData::from_json_bytes(&encoded).unwrap();
    assert_eq!(decoded, custom);
}
