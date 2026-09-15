use kite_recorder::{records::Record, replay::replay, writer::Recorder};
use nautilus_model::{
    data::QuoteTick,
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol},
    instruments::FuturesContract,
    types::{Currency, Price, Quantity},
};
use std::str::FromStr;

fn instrument() -> FuturesContract {
    FuturesContract::builder()
        .instrument_id(InstrumentId::from_as_ref("TEST.MCX").unwrap())
        .raw_symbol(Symbol::new("TEST"))
        .asset_class(AssetClass::Commodity)
        .underlying("TEST".into())
        .activation_ns(1u64.into())
        .expiration_ns(10u64.into())
        .currency(Currency::from_str("INR").unwrap())
        .price_precision(0)
        .price_increment(Price::from_str("1").unwrap())
        .multiplier(Quantity::from(100))
        .lot_size(Quantity::from(1))
        .ts_event(1u64.into())
        .ts_init(1u64.into())
        .build()
        .unwrap()
}
fn header() -> Record {
    Record::Header {
        schema_version: 1,
        instrument: Box::new(instrument()),
        instrument_token: 123,
    }
}
fn quote() -> QuoteTick {
    QuoteTick::new_checked(
        instrument().id,
        Price::from_str("99").unwrap(),
        Price::from_str("100").unwrap(),
        Quantity::from(2),
        Quantity::from(3),
        2u64.into(),
        3u64.into(),
    )
    .unwrap()
}
#[test]
fn round_trip_preserves_quotes_and_gap_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticks.parquet");
    let writer = Recorder::create(&path).unwrap();
    for record in [
        header(),
        Record::Connected { generation: 1 },
        Record::Quote {
            quote: quote(),
            generation: 1,
        },
        Record::Gap { generation: 1 },
        Record::Connected { generation: 2 },
        Record::Quote {
            quote: quote(),
            generation: 2,
        },
        Record::End { quotes: 2 },
    ] {
        writer.record(record).unwrap();
    }
    assert_eq!(writer.finish().unwrap(), 7);
    let mut quotes = Vec::new();
    let report = replay(&path, |record| {
        if let Record::Quote { quote, .. } = record {
            quotes.push(quote);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(report.quotes, 2);
    assert_eq!(report.gaps, 1);
    assert_eq!(quotes, vec![quote(), quote()]);
}
#[test]
fn refuses_overwriting_an_existing_capture() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticks.parquet");
    std::fs::write(&path, "keep").unwrap();
    assert!(Recorder::create(&path).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
}
#[test]
fn unfinished_capture_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticks.parquet");
    let writer = Recorder::create(&path).unwrap();
    writer.record(header()).unwrap();
    writer.finish().unwrap();
    assert!(
        replay(&path, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("incomplete")
    );
}
#[test]
fn wrong_generation_counts_version_and_trailing_records_are_rejected() {
    let cases = vec![
        vec![
            header(),
            Record::Connected { generation: 2 },
            Record::End { quotes: 0 },
        ],
        vec![
            header(),
            Record::Connected { generation: 1 },
            Record::Quote {
                quote: quote(),
                generation: 2,
            },
            Record::End { quotes: 1 },
        ],
        vec![header(), Record::End { quotes: 1 }],
        vec![
            header(),
            Record::End { quotes: 0 },
            Record::Connected { generation: 1 },
        ],
        vec![
            Record::Header {
                schema_version: 2,
                instrument: Box::new(instrument()),
                instrument_token: 123,
            },
            Record::End { quotes: 0 },
        ],
    ];
    for records in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ticks.parquet");
        let writer = Recorder::create(&path).unwrap();
        for record in records {
            writer.record(record).unwrap();
        }
        writer.finish().unwrap();
        assert!(replay(&path, |_| Ok(())).is_err());
    }
}
#[test]
fn multiple_row_groups_replay_in_sequence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticks.parquet");
    let writer = Recorder::create(&path).unwrap();
    writer.record(header()).unwrap();
    writer.record(Record::Connected { generation: 1 }).unwrap();
    for _ in 0..600 {
        writer
            .record(Record::Quote {
                quote: quote(),
                generation: 1,
            })
            .unwrap();
    }
    writer.record(Record::End { quotes: 600 }).unwrap();
    writer.finish().unwrap();
    assert_eq!(replay(&path, |_| Ok(())).unwrap().quotes, 600);
}
