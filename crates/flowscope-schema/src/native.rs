// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Encoding of a batch of flows for the ClickHouse native protocol.

use crate::message::{BatchColumn, ColumnData};
use flowscope_clickhouse::block::{write_column_header, write_low_cardinality_ipv6, write_low_cardinality_strings};
use flowscope_clickhouse::io::put_str;

fn is_low_cardinality(t: &str) -> bool {
    t.starts_with("LowCardinality(")
}

/// Encode the columns (headers and data) of a batch. The block header is
/// written by the caller.
pub fn encode_columns(columns: &[BatchColumn], rows: usize, with_custom: bool, out: &mut Vec<u8>) {
    for c in columns {
        write_column_header(out, &c.name, &c.clickhouse_type, with_custom);
        if rows == 0 {
            continue;
        }
        let lc = is_low_cardinality(&c.clickhouse_type);
        match &c.data {
            ColumnData::UInt64(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ColumnData::UInt32(v) | ColumnData::DateTime(v) => {
                v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()))
            }
            ColumnData::UInt16(v) => v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ColumnData::UInt8(v) => out.extend_from_slice(v),
            ColumnData::Enum8(v) => out.extend(v.iter().map(|&x| x as u8)),
            ColumnData::String { data, offsets } if lc => write_low_cardinality_strings(out, data, offsets),
            ColumnData::String { data, offsets } => {
                let mut start = 0;
                for &end in offsets {
                    put_str(out, &data[start..end]);
                    start = end;
                }
            }
            ColumnData::FixedString { data, .. } => out.extend_from_slice(data),
            ColumnData::IPv6(v) if lc => write_low_cardinality_ipv6(out, v),
            ColumnData::IPv6(v) => v.iter().for_each(|x| out.extend_from_slice(x)),
            ColumnData::ArrayUInt32 { offsets, data } => {
                offsets.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
                data.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
            }
            ColumnData::ArrayUInt128 { offsets, data } => {
                offsets.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
                data.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
            }
        }
    }
}
