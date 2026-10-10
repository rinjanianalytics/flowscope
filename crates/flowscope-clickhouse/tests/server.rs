// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Tests against a real ClickHouse.
//! Needs `docker compose -f tests/interop/docker-compose.yml up -d`.

use flowscope_clickhouse::{Block, Column, Connection, DataType, Options, Setting, Value, Values};

fn options(compression: bool) -> Options {
    Options {
        address: std::env::var("FLOWSCOPE_TEST_CLICKHOUSE").unwrap_or_else(|_| "127.0.0.1:19000".into()),
        compression,
        ..Default::default()
    }
}

#[test]
#[ignore = "needs ClickHouse"]
fn query_types() {
    for compression in [false, true] {
        let mut c = Connection::connect(&options(compression)).unwrap();
        c.ping().unwrap();
        assert!(c.server().version.0 >= 24, "{:?}", c.server());
        let r = c
            .query(
                "SELECT number, toString(number) AS s, toLowCardinality(toString(number % 3)) AS lc, \
                 [number, number + 1] AS arr, toIPv6('::ffff:192.0.2.1') AS ip, \
                 CAST(number % 2 AS Enum8('even' = 0, 'odd' = 1)) AS e, \
                 if(number = 1, NULL, number) AS n, toDateTime(1700000000 + number, 'UTC') AS t, \
                 map('a', number) AS m, (number, 'x') AS tup, toFloat64(number) / 2 AS f \
                 FROM numbers(5000)",
                &[],
            )
            .unwrap();
        assert_eq!(r.rows(), 5000);
        let rows: Vec<Vec<Value>> = r.iter_rows().collect();
        assert_eq!(
            rows[1],
            vec![
                Value::UInt(1),
                Value::String("1".into()),
                Value::String("1".into()),
                Value::Array(vec![Value::UInt(1), Value::UInt(2)]),
                Value::IPv6("::ffff:192.0.2.1".parse().unwrap()),
                Value::String("odd".into()),
                Value::Null,
                Value::DateTime(1_700_000_001),
                Value::Map(vec![(Value::String("a".into()), Value::UInt(1))]),
                Value::Tuple(vec![Value::UInt(1), Value::String("x".into())]),
                Value::Float(0.5),
            ]
        );
        assert_eq!(rows[4999][0], Value::UInt(4999));
    }
}

#[test]
#[ignore = "needs ClickHouse"]
fn errors() {
    let mut c = Connection::connect(&options(true)).unwrap();
    let err = c.query("SELECT nope", &[]).unwrap_err();
    assert_eq!(err.code(), Some(47), "{err}");
    // The connection is still usable.
    let r = c.query("SELECT 1", &[]).unwrap();
    assert_eq!(r.iter_rows().next().unwrap(), vec![Value::UInt(1)]);
    let bad = Options { password: "wrong".into(), ..options(true) };
    assert!(Connection::connect(&bad).is_err());
}

#[test]
#[ignore = "needs ClickHouse"]
fn insert() {
    for compression in [false, true] {
        let mut c = Connection::connect(&options(compression)).unwrap();
        let table = format!("test_insert_{}_{compression}", std::process::id());
        c.execute(&format!("DROP TABLE IF EXISTS {table}"), &[]).unwrap();
        c.execute(
            &format!(
                "CREATE TABLE {table} (a UInt64, b LowCardinality(String), c Array(UInt32), \
                 d LowCardinality(IPv6), e Enum8('x' = 1, 'y' = 2), f FixedString(2), g Array(UInt128), \
                 h LowCardinality(Nullable(String))) ENGINE = Memory"
            ),
            &[Setting::new("allow_suspicious_low_cardinality_types", 1)],
        )
        .unwrap();
        let n = 1000;
        let block = Block {
            rows: n,
            columns: vec![
                Column {
                    name: "a".into(),
                    data_type: DataType::UInt64,
                    values: Values::UInt64((0..n as u64).collect()),
                },
                Column {
                    name: "b".into(),
                    data_type: DataType::parse("LowCardinality(String)").unwrap(),
                    values: Values::String((0..n).map(|i| format!("v{}", i % 7).into_bytes()).collect()),
                },
                Column {
                    name: "c".into(),
                    data_type: DataType::parse("Array(UInt32)").unwrap(),
                    values: Values::Array((1..=n as u64).collect(), Box::new(Values::UInt32((0..n as u32).collect()))),
                },
                Column {
                    name: "d".into(),
                    data_type: DataType::parse("LowCardinality(IPv6)").unwrap(),
                    values: Values::Fixed(16, (0..n).flat_map(|i| [i as u8; 16]).collect()),
                },
                Column {
                    name: "e".into(),
                    data_type: DataType::parse("Enum8('x' = 1, 'y' = 2)").unwrap(),
                    values: Values::Int8((0..n).map(|i| 1 + (i % 2) as i8).collect()),
                },
                Column {
                    name: "f".into(),
                    data_type: DataType::FixedString(2),
                    values: Values::Fixed(2, (0..n).flat_map(|_| *b"FR").collect()),
                },
                Column {
                    name: "g".into(),
                    data_type: DataType::parse("Array(UInt128)").unwrap(),
                    values: Values::Array(
                        vec![1; n]
                            .iter()
                            .scan(0, |s, x| {
                                *s += x;
                                Some(*s)
                            })
                            .collect(),
                        Box::new(Values::UInt128((0..n as u128).map(|x| x << 64 | 7).collect())),
                    ),
                },
                Column {
                    name: "h".into(),
                    data_type: DataType::parse("LowCardinality(Nullable(String))").unwrap(),
                    values: Values::Nullable(
                        (0..n).map(|i| i % 3 == 0).collect(),
                        Box::new(Values::String((0..n).map(|i| format!("n{}", i % 2).into_bytes()).collect())),
                    ),
                },
            ],
        };
        c.insert(
            &format!("INSERT INTO {table} (a, b, c, d, e, f, g, h) VALUES"),
            &[Setting::important("async_insert", 0)],
            &block,
        )
        .unwrap();
        let r = c
            .query(&format!("SELECT count(), sum(a), uniqExact(b), sum(arraySum(c)), countIf(e = 'y'), any(f), sum(bitShiftRight(g[1], 64)), countIf(h IS NULL), countIf(h = 'n1') FROM {table}"), &[])
            .unwrap();
        let row = r.iter_rows().next().unwrap();
        assert_eq!(row[0], Value::UInt(1000));
        assert_eq!(row[1], Value::UInt(499_500));
        assert_eq!(row[2], Value::UInt(7));
        assert_eq!(row[3], Value::UInt(499_500));
        assert_eq!(row[4], Value::UInt(500));
        assert_eq!(row[5], Value::String("FR".into()));
        assert_eq!(row[6].as_f64(), Some(499_500.0));
        assert_eq!(row[7], Value::UInt(334));
        // Not null and odd: 1000 - 334 nulls, half of the rest... count directly.
        let expected = (0..1000).filter(|i| i % 3 != 0 && i % 2 == 1).count() as u64;
        assert_eq!(row[8], Value::UInt(expected));
        let r = c.query(&format!("SELECT d FROM {table} WHERE a = 5"), &[]).unwrap();
        assert_eq!(r.iter_rows().next().unwrap()[0], Value::IPv6(std::net::Ipv6Addr::from([5u8; 16])));
        c.execute(&format!("DROP TABLE {table}"), &[]).unwrap();
    }
}
