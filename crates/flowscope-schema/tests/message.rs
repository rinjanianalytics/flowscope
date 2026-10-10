// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Port of the append, undo and protobuf tests from `common/schema`.

use flowscope_schema::{ColumnData, ColumnKey as K, Component, Value};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr};

fn ip(s: &str) -> Ipv6Addr {
    match s.parse::<IpAddr>().unwrap() {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

#[test]
fn append_default() {
    let c = Component::default_schema().enable_all_columns();
    let mut bf = c.new_flow_message();
    bf.finalize();
    assert_eq!(bf.flow_count(), 1);
    for col in bf.batch_columns() {
        assert_eq!(col.data.rows(), 1, "{}", col.name);
    }
}

#[test]
fn append_basics() {
    let c = Component::default_schema();
    let mut bf = c.new_flow_message();
    bf.debug = true;
    bf.append_date_time(K::TimeReceived, 1000);
    bf.append_uint(K::SamplingRate, 20000);
    bf.append_uint(K::DstAS, 65000);
    bf.append_uint(K::SrcAS, 0); // zero is not appended
    bf.append_uint(K::Packets, 100);
    bf.append_uint(K::Packets, 200); // duplicate is ignored
    let expected = BTreeMap::from([
        (K::TimeReceived, Value::U32(1000)),
        (K::SamplingRate, Value::U64(20000)),
        (K::DstAS, Value::U32(65000)),
        (K::Packets, Value::U64(100)),
    ]);
    assert_eq!(bf.other_columns, expected);
    bf.finalize();
    for col in bf.batch_columns() {
        assert_eq!(col.data.rows(), 1, "{}", col.name);
    }
}

#[test]
fn append_disabled_column() {
    let c = Component::default_schema();
    let mut bf = c.new_flow_message();
    bf.append_uint(K::SrcVlan, 100);
    bf.finalize();
    assert!(bf.column_data(K::SrcVlan).is_none());
}

#[test]
fn append_arrays() {
    let c = Component::default_schema();
    let mut bf = c.new_flow_message();
    bf.append_array_uint32(K::DstASPath, &[65400, 65500, 65001]);
    bf.finalize();
    bf.append_array_uint32(K::DstASPath, &[65403, 65503, 65003]);
    bf.finalize();
    assert_eq!(
        bf.column_data(K::DstASPath),
        Some(&ColumnData::ArrayUInt32 { offsets: vec![3, 6], data: vec![65400, 65500, 65001, 65403, 65503, 65003] })
    );

    let a = ((65401u128 << 32) + 100) << 64;
    bf.append_array_uint128(K::DstLargeCommunities, &[a + 200, a + 201]);
    bf.finalize();
    assert_eq!(
        bf.column_data(K::DstLargeCommunities),
        Some(&ColumnData::ArrayUInt128 { offsets: vec![0, 0, 2], data: vec![a + 200, a + 201] })
    );
}

#[test]
fn undo() {
    let c = Component::default_schema().enable_all_columns();
    let mut bf = c.new_flow_message();
    // A first complete flow.
    bf.append_uint(K::Bytes, 1);
    bf.append_array_uint32(K::DstASPath, &[1, 2]);
    bf.append_string(K::ExporterName, "first");
    bf.finalize();
    let before: Vec<_> = bf.batch_columns().iter().map(|c| c.data.clone()).collect();

    // A second one, undone.
    bf.append_uint(K::Bytes, 100);
    bf.append_uint(K::SrcAS, 65001);
    bf.append_uint(K::SrcPort, 443);
    bf.append_uint(K::IPTTL, 4);
    bf.append_uint(K::InIfBoundary, 1);
    bf.append_date_time(K::TimeReceived, 1000);
    bf.append_ipv6(K::SrcAddr, Some(ip("2001:db8::1")));
    bf.append_ipv6(K::ExporterAddress, Some(ip("192.0.2.1")));
    bf.append_string(K::ExporterName, "second");
    bf.append_string(K::SrcCountry, "FR");
    bf.append_array_uint32(K::DstASPath, &[3, 4, 5]);
    bf.append_array_uint128(K::DstLargeCommunities, &[1, 2]);
    bf.undo();
    let after: Vec<_> = bf.batch_columns().iter().map(|c| c.data.clone()).collect();
    assert_eq!(after, before);
    assert_eq!(bf.flow_count(), 1);

    // Appending again works.
    bf.append_uint(K::Bytes, 100);
    bf.finalize();
    assert_eq!(bf.column_data(K::Bytes), Some(&ColumnData::UInt64(vec![1, 100])));
}

#[test]
fn proto_input() {
    let exporter = ip("::ffff:203.0.113.14");
    let c = Component::default_schema();
    let mut bf = c.new_flow_message();
    bf.time_received = 1000;
    bf.sampling_rate = 20000;
    bf.exporter_address = Some(exporter);
    bf.append_uint(K::DstAS, 65000);
    bf.append_uint(K::Bytes, 200);
    bf.append_uint(K::Packets, 300);
    bf.finalize();
    bf.clear();

    bf.time_received = 1002;
    bf.exporter_address = Some(exporter);
    bf.append_uint(K::SrcAS, 65000);
    bf.append_uint(K::Bytes, 2000);
    bf.append_uint(K::Packets, 30);
    bf.append_uint(K::Bytes, 300); // duplicate
    bf.finalize();

    bf.time_received = 1003;
    bf.exporter_address = Some(exporter);
    bf.append_uint(K::SrcAS, 65001);
    bf.append_uint(K::Bytes, 202);
    bf.append_uint(K::Packets, 3);
    bf.finalize();

    assert_eq!(bf.column_data(K::TimeReceived), Some(&ColumnData::DateTime(vec![1002, 1003])));
    assert_eq!(bf.column_data(K::SrcAS), Some(&ColumnData::UInt32(vec![65000, 65001])));
    assert_eq!(bf.column_data(K::DstAS), Some(&ColumnData::UInt32(vec![0, 0])));
    assert_eq!(bf.column_data(K::Bytes), Some(&ColumnData::UInt64(vec![2000, 202])));
    assert_eq!(bf.column_data(K::Packets), Some(&ColumnData::UInt64(vec![30, 3])));
}

#[test]
fn fixed_string() {
    let c = Component::default_schema().enable_all_columns();
    let mut bf = c.new_flow_message();
    for country in ["FR", "F", "France", ""] {
        bf.append_string(K::SrcCountry, country);
        bf.finalize();
    }
    assert_eq!(
        bf.column_data(K::SrcCountry),
        Some(&ColumnData::FixedString { size: 2, data: b"FRF\x00Fr\x00\x00".to_vec() })
    );
}

#[test]
fn reverse() {
    let c = Component::default_schema().enable_all_columns();
    let mut bf = c.new_flow_message();
    bf.debug = true;
    bf.reverse();
    bf.append_date_time(K::TimeReceived, 1000);
    bf.append_uint(K::SamplingRate, 20000);
    bf.append_string(K::InIfName, "input");
    bf.append_string(K::OutIfName, "output");
    bf.append_array_uint32(K::DstCommunities, &[10, 11, 12]);
    bf.src_as = 65000;
    bf.dst_as = 65002;
    bf.reverse();
    let expected = BTreeMap::from([
        (K::TimeReceived, Value::U32(1000)),
        (K::SamplingRate, Value::U64(20000)),
        (K::InIfName, Value::String("output".into())),
        (K::OutIfName, Value::String("input".into())),
        (K::SrcCommunities, Value::ArrayU32(vec![10, 11, 12])),
    ]);
    assert_eq!(bf.other_columns, expected);
    assert_eq!(bf.src_as, 65002);
    assert_eq!(bf.dst_as, 65000);
}

/// Parse a flat proto3 message into field number → values.
fn parse_protobuf(mut data: &[u8]) -> BTreeMap<u32, Vec<Result<u64, Vec<u8>>>> {
    fn varint(data: &mut &[u8]) -> u64 {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = data[0];
            *data = &data[1..];
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return v;
            }
            shift += 7;
        }
    }
    let mut out: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    while !data.is_empty() {
        let tag = varint(&mut data);
        let (field, wire) = ((tag >> 3) as u32, tag & 7);
        let value = match wire {
            0 => Ok(varint(&mut data)),
            2 => {
                let len = varint(&mut data) as usize;
                let v = data[..len].to_vec();
                data = &data[len..];
                Err(v)
            }
            _ => panic!("unexpected wire type {wire}"),
        };
        out.entry(field).or_default().push(value);
    }
    out
}

#[test]
fn protobuf_encode() {
    let c = Component::default_schema();
    let mut bf = c.new_flow_message();
    bf.enable_protobuf();
    bf.time_received = 1000;
    bf.sampling_rate = 30000;
    bf.exporter_address = Some(ip("2001:db8::1"));
    bf.src_addr = Some(ip("192.0.2.1"));
    bf.dst_addr = Some(ip("198.51.100.2"));
    bf.src_as = 65001;
    bf.dst_as = 65002;
    bf.append_uint(K::Bytes, 1500);
    bf.append_uint(K::Packets, 10);
    bf.append_uint(K::SrcPort, 443);
    bf.append_uint(K::DstPort, 33000);
    bf.append_uint(K::Proto, 6);
    bf.append_uint(K::EType, 0x0800);
    bf.append_string(K::ExporterName, "router1.example.net");
    bf.append_array_uint32(K::DstASPath, &[65001, 65002, 65003]);
    bf.append_array_uint128(K::DstLargeCommunities, &[(65001u128 << 64) | 0x0000000100000002]);
    bf.finalize();

    let fields = parse_protobuf(bf.protobuf_message());
    let idx = |k: K| c.lookup_column_by_key(k).unwrap().protobuf_index as u32;
    for (k, v) in [
        (K::Bytes, 1500),
        (K::Packets, 10),
        (K::SrcPort, 443),
        (K::DstPort, 33000),
        (K::Proto, 6),
        (K::EType, 0x800),
        (K::TimeReceived, 1000),
        (K::SamplingRate, 30000),
        (K::SrcAS, 65001),
        (K::DstAS, 65002),
    ] {
        assert_eq!(fields[&idx(k)], vec![Ok(v)], "{k}");
    }
    assert_eq!(fields[&idx(K::ExporterAddress)], vec![Err(ip("2001:db8::1").octets().to_vec())]);
    assert_eq!(fields[&idx(K::SrcAddr)], vec![Err(ip("192.0.2.1").octets().to_vec())]);
    assert_eq!(fields[&idx(K::ExporterName)], vec![Err(b"router1.example.net".to_vec())]);
    assert_eq!(fields[&idx(K::DstASPath)], vec![Ok(65001), Ok(65002), Ok(65003)]);
    let mut large = vec![0, 0, 0, 0, 0, 0, 0xfd, 0xe9];
    large.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2]);
    assert_eq!(fields[&idx(K::DstLargeCommunities)], vec![Err(large)]);
}

#[test]
fn native_columns() {
    let c = Component::default_schema();
    let mut bf = c.new_flow_message();
    bf.append_uint(K::Bytes, 1);
    bf.finalize();
    let mut out = Vec::new();
    bf.encode_columns(true, &mut out);
    // First column is TimeReceived, then its custom serialization flag.
    assert_eq!(&out[0..13], b"\x0cTimeReceived");
    assert_eq!(&out[13..22], b"\x08DateTime");
    assert_eq!(out[22], 0);
    assert!(bf.insert_query("flows_x_raw").starts_with("INSERT INTO `flows_x_raw` (`TimeReceived`, `SamplingRate`"));
}
