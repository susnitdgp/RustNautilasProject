use kite_adapter::websocket::{models::BinaryFrame, parser};

fn packet(len: usize) -> Vec<u8> {
    let mut p = vec![0; len];
    p[..4].copy_from_slice(&144870151u32.to_be_bytes());
    p[4..8].copy_from_slice(&612345i32.to_be_bytes());
    p
}
fn frame(packets: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = (packets.len() as u16).to_be_bytes().to_vec();
    for p in packets {
        bytes.extend_from_slice(&(p.len() as u16).to_be_bytes());
        bytes.extend(p);
    }
    bytes
}
#[test]
fn heartbeat_and_empty_batch() {
    assert!(matches!(
        parser::parse(&[0]).unwrap(),
        BinaryFrame::Heartbeat
    ));
    assert!(matches!(parser::parse(&[0, 0]).unwrap(), BinaryFrame::Ticks(v) if v.is_empty()));
}
#[test]
fn decodes_ltp_and_quote_modes_without_fabricating_depth() {
    let mut quote = packet(44);
    quote[8..12].copy_from_slice(&7u32.to_be_bytes());
    quote[16..20].copy_from_slice(&1234u32.to_be_bytes());
    let BinaryFrame::Ticks(ticks) = parser::parse(&frame(&[packet(8), quote])).unwrap() else {
        panic!()
    };
    assert_eq!(ticks.len(), 2);
    assert_eq!(ticks[0].ltp_paise, 612345);
    assert!(ticks[0].cumulative_volume.is_none());
    assert_eq!(ticks[1].last_quantity, Some(7));
    assert_eq!(ticks[1].cumulative_volume, Some(1234));
    assert!(ticks.iter().all(|t| t.full.is_none()));
}
#[test]
fn full_depth_timestamp_and_oi_offsets() {
    let mut p = packet(184);
    p[44..48].copy_from_slice(&1000u32.to_be_bytes());
    p[48..52].copy_from_slice(&999u32.to_be_bytes());
    p[60..64].copy_from_slice(&1001u32.to_be_bytes());
    for i in 0..10 {
        let offset = 64 + i * 12;
        p[offset..offset + 4].copy_from_slice(&(10 + i as u32).to_be_bytes());
        p[offset + 4..offset + 8].copy_from_slice(&(600000 + i as i32).to_be_bytes());
        p[offset + 8..offset + 10].copy_from_slice(&(20 + i as u16).to_be_bytes());
        p[offset + 10..offset + 12].copy_from_slice(&[255, 255]); // padding must be ignored
    }
    let BinaryFrame::Ticks(ticks) = parser::parse(&frame(&[p])).unwrap() else {
        panic!()
    };
    let full = ticks[0].full.as_ref().unwrap();
    assert_eq!(full.exchange_timestamp, Some(1001));
    assert_eq!(full.last_trade_timestamp, Some(1000));
    assert_eq!(full.open_interest, 999);
    assert_eq!(full.bids[4].price_paise, 600004);
    assert_eq!(full.asks[0].quantity, 15);
    assert_eq!(full.asks[4].orders, 29);
}
#[test]
fn zero_timestamps_are_missing() {
    let BinaryFrame::Ticks(ticks) = parser::parse(&frame(&[packet(184)])).unwrap() else {
        panic!()
    };
    assert!(ticks[0].full.as_ref().unwrap().exchange_timestamp.is_none());
}
#[test]
fn rejects_truncation_trailing_data_bad_lengths_and_zero_token() {
    let good = frame(&[packet(184)]);
    for end in 2..good.len() {
        assert!(parser::parse(&good[..end]).is_err());
    }
    assert!(parser::parse(&[]).is_err());
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(parser::parse(&trailing).is_err());
    for len in [28, 32, 43, 183, 185] {
        assert!(parser::parse(&frame(&[packet(len)])).is_err());
    }
    let mut zero = packet(8);
    zero[..4].fill(0);
    assert!(parser::parse(&frame(&[zero])).is_err());
}
#[test]
fn enforces_packet_count() {
    let mut bytes = frame(&[packet(8)]);
    bytes[..2].copy_from_slice(&2u16.to_be_bytes());
    assert!(parser::parse(&bytes).is_err());
    assert!(parser::parse(&[255, 255]).is_err());
}
