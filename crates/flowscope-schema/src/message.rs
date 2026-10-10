// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Flow message and columnar batch. Port of `common/schema/message.go` and the
//! append part of `common/schema/clickhouse.go`.
//!
//! A `FlowMessage` holds the fields enrichment needs, plus a columnar batch
//! of finalized flows ready to be sent to ClickHouse in Native format.

use crate::definition::{ColumnGroup, Schema};
use crate::key::ColumnKey;
use std::collections::BTreeMap;
use std::net::Ipv6Addr;
use std::sync::Arc;

/// Data of one column of the batch.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnData {
    UInt64(Vec<u64>),
    UInt32(Vec<u32>),
    UInt16(Vec<u16>),
    UInt8(Vec<u8>),
    DateTime(Vec<u32>),
    Enum8(Vec<i8>),
    /// String or LowCardinality(String). Strings are stored contiguously.
    String {
        data: Vec<u8>,
        offsets: Vec<usize>,
    },
    FixedString {
        size: usize,
        data: Vec<u8>,
    },
    /// IPv6 or LowCardinality(IPv6), in network order.
    IPv6(Vec<[u8; 16]>),
    ArrayUInt32 {
        offsets: Vec<u64>,
        data: Vec<u32>,
    },
    ArrayUInt128 {
        offsets: Vec<u64>,
        data: Vec<u128>,
    },
}

impl ColumnData {
    /// Build an empty column for a ClickHouse type.
    pub fn for_type(t: &str) -> Option<ColumnData> {
        let inner = t.strip_prefix("LowCardinality(").and_then(|s| s.strip_suffix(')')).unwrap_or(t);
        Some(match inner {
            "UInt64" => ColumnData::UInt64(vec![]),
            "UInt32" => ColumnData::UInt32(vec![]),
            "UInt16" => ColumnData::UInt16(vec![]),
            "UInt8" => ColumnData::UInt8(vec![]),
            "DateTime" => ColumnData::DateTime(vec![]),
            "String" => ColumnData::String { data: vec![], offsets: vec![] },
            "IPv6" => ColumnData::IPv6(vec![]),
            "Array(UInt32)" => ColumnData::ArrayUInt32 { offsets: vec![], data: vec![] },
            "Array(UInt128)" => ColumnData::ArrayUInt128 { offsets: vec![], data: vec![] },
            _ if inner.starts_with("Enum8(") => ColumnData::Enum8(vec![]),
            _ if inner.starts_with("FixedString(") => {
                let size: usize = inner["FixedString(".len()..inner.len() - 1].parse().ok()?;
                if size == 0 {
                    return None;
                }
                ColumnData::FixedString { size, data: vec![] }
            }
            _ => return None,
        })
    }

    /// Number of rows.
    pub fn rows(&self) -> usize {
        match self {
            ColumnData::UInt64(v) => v.len(),
            ColumnData::UInt32(v) | ColumnData::DateTime(v) => v.len(),
            ColumnData::UInt16(v) => v.len(),
            ColumnData::UInt8(v) => v.len(),
            ColumnData::Enum8(v) => v.len(),
            ColumnData::String { offsets, .. } => offsets.len(),
            ColumnData::FixedString { size, data } => data.len() / size,
            ColumnData::IPv6(v) => v.len(),
            ColumnData::ArrayUInt32 { offsets, .. } | ColumnData::ArrayUInt128 { offsets, .. } => offsets.len(),
        }
    }

    fn clear(&mut self) {
        match self {
            ColumnData::UInt64(v) => v.clear(),
            ColumnData::UInt32(v) | ColumnData::DateTime(v) => v.clear(),
            ColumnData::UInt16(v) => v.clear(),
            ColumnData::UInt8(v) => v.clear(),
            ColumnData::Enum8(v) => v.clear(),
            ColumnData::String { data, offsets } => {
                data.clear();
                offsets.clear();
            }
            ColumnData::FixedString { data, .. } => data.clear(),
            ColumnData::IPv6(v) => v.clear(),
            ColumnData::ArrayUInt32 { offsets, data } => {
                offsets.clear();
                data.clear();
            }
            ColumnData::ArrayUInt128 { offsets, data } => {
                offsets.clear();
                data.clear();
            }
        }
    }

    fn push_default(&mut self) {
        match self {
            ColumnData::UInt64(v) => v.push(0),
            ColumnData::UInt32(v) | ColumnData::DateTime(v) => v.push(0),
            ColumnData::UInt16(v) => v.push(0),
            ColumnData::UInt8(v) => v.push(0),
            ColumnData::Enum8(v) => v.push(0),
            ColumnData::String { data, offsets } => offsets.push(data.len()),
            ColumnData::FixedString { size, data } => data.resize(data.len() + *size, 0),
            ColumnData::IPv6(v) => v.push([0; 16]),
            ColumnData::ArrayUInt32 { offsets, data } => offsets.push(data.len() as u64),
            ColumnData::ArrayUInt128 { offsets, data } => offsets.push(data.len() as u64),
        }
    }

    fn pop(&mut self) {
        match self {
            ColumnData::UInt64(v) => _ = v.pop(),
            ColumnData::UInt32(v) | ColumnData::DateTime(v) => _ = v.pop(),
            ColumnData::UInt16(v) => _ = v.pop(),
            ColumnData::UInt8(v) => _ = v.pop(),
            ColumnData::Enum8(v) => _ = v.pop(),
            ColumnData::String { data, offsets } => {
                offsets.pop();
                data.truncate(offsets.last().copied().unwrap_or(0));
            }
            ColumnData::FixedString { size, data } => data.truncate(data.len().saturating_sub(*size)),
            ColumnData::IPv6(v) => _ = v.pop(),
            ColumnData::ArrayUInt32 { offsets, data } => {
                offsets.pop();
                data.truncate(offsets.last().copied().unwrap_or(0) as usize);
            }
            ColumnData::ArrayUInt128 { offsets, data } => {
                offsets.pop();
                data.truncate(offsets.last().copied().unwrap_or(0) as usize);
            }
        }
    }

    /// Get string at row `i` (String columns only).
    pub fn string_at(&self, i: usize) -> Option<&[u8]> {
        match self {
            ColumnData::String { data, offsets } => {
                let start = if i == 0 { 0 } else { offsets[i - 1] };
                Some(&data[start..offsets[i]])
            }
            ColumnData::FixedString { size, data } => Some(&data[i * size..(i + 1) * size]),
            _ => None,
        }
    }
}

/// A value recorded in debug mode, used by tests to inspect a flow.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    U64(u64),
    U32(u32),
    U16(u16),
    U8(u8),
    String(String),
    IP(Ipv6Addr),
    ArrayU32(Vec<u32>),
    ArrayU128(Vec<u128>),
}

impl Value {
    /// JSON representation used by golden tests (integers as numbers, IPs as
    /// strings, UInt128 as "high:low").
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Value::U64(v) => json!(v),
            Value::U32(v) => json!(v),
            Value::U16(v) => json!(v),
            Value::U8(v) => json!(v),
            Value::String(v) => json!(v),
            Value::IP(v) => json!(crate::message::format_ip(v)),
            Value::ArrayU32(v) => json!(v),
            Value::ArrayU128(v) => {
                json!(v.iter().map(|x| format!("{}:{}", x >> 64, *x as u64)).collect::<Vec<_>>())
            }
        }
    }
}

/// Format an IP like Go's netip: IPv4-mapped addresses are shown as "::ffff:a.b.c.d".
pub fn format_ip(ip: &Ipv6Addr) -> String {
    match ip.to_ipv4_mapped() {
        Some(v4) => format!("::ffff:{v4}"),
        None => ip.to_string(),
    }
}

/// A column of the batch, with its metadata.
#[derive(Clone, Debug)]
pub struct BatchColumn {
    pub key: ColumnKey,
    pub name: String,
    pub clickhouse_type: String,
    pub data: ColumnData,
}

/// The columnar batch, embedded in a flow message.
#[derive(Clone, Debug, Default)]
struct MessageBatch {
    /// Columns, in insertion order.
    columns: Vec<BatchColumn>,
    /// Position in `columns` for each key.
    positions: Vec<Option<usize>>,
    /// Columns set for the current flow, indexed by key.
    column_set: Vec<bool>,
    row_count: usize,

    protobuf_enabled: bool,
    protobuf: Vec<u8>,
    protobuf_message: Vec<u8>,
}

/// Abstract representation of a flow through the various subsystems.
#[derive(Clone, Debug)]
pub struct FlowMessage {
    pub time_received: u32,
    pub sampling_rate: u64,

    /// For exporter classifier.
    pub exporter_address: Option<Ipv6Addr>,

    /// For interface classifier.
    pub in_if: u32,
    pub out_if: u32,
    pub src_vlan: u16,
    pub dst_vlan: u16,

    /// For routing component.
    pub src_addr: Option<Ipv6Addr>,
    pub dst_addr: Option<Ipv6Addr>,
    pub next_hop: Option<Ipv6Addr>,

    /// The core component may override them.
    pub src_as: u32,
    pub dst_as: u32,
    pub src_net_mask: u8,
    pub dst_net_mask: u8,

    /// Values appended for the current flow. Only filled in debug mode.
    pub other_columns: BTreeMap<ColumnKey, Value>,
    /// Record appended values in `other_columns`.
    pub debug: bool,

    reversed: bool,
    batch: MessageBatch,
    schema: Arc<Schema>,
}

impl Schema {
    /// Create a new flow message with an empty batch.
    pub fn new_flow_message(self: &Arc<Self>) -> FlowMessage {
        let max_key = self.max_key() as usize;
        let mut batch = MessageBatch {
            positions: vec![None; max_key + 1],
            column_set: vec![false; max_key + 1],
            ..Default::default()
        };
        for column in self.all_columns() {
            if column.disabled || !column.should_provide_value() {
                continue;
            }
            let data = ColumnData::for_type(&column.clickhouse_type)
                .unwrap_or_else(|| panic!("unhandled ClickHouse type {:?}", column.clickhouse_type));
            batch.positions[column.key.0 as usize] = Some(batch.columns.len());
            batch.columns.push(BatchColumn {
                key: column.key,
                name: column.name.clone(),
                clickhouse_type: column.clickhouse_type.clone(),
                data,
            });
        }
        FlowMessage {
            time_received: 0,
            sampling_rate: 0,
            exporter_address: None,
            in_if: 0,
            out_if: 0,
            src_vlan: 0,
            dst_vlan: 0,
            src_addr: None,
            dst_addr: None,
            next_hop: None,
            src_as: 0,
            dst_as: 0,
            src_net_mask: 0,
            dst_net_mask: 0,
            other_columns: BTreeMap::new(),
            debug: cfg!(test),
            reversed: false,
            batch,
            schema: self.clone(),
        }
    }
}

impl FlowMessage {
    /// Reset public fields. The batch is left untouched.
    fn reset(&mut self) {
        self.time_received = 0;
        self.sampling_rate = 0;
        self.exporter_address = None;
        self.in_if = 0;
        self.out_if = 0;
        self.src_vlan = 0;
        self.dst_vlan = 0;
        self.src_addr = None;
        self.dst_addr = None;
        self.next_hop = None;
        self.src_as = 0;
        self.dst_as = 0;
        self.src_net_mask = 0;
        self.dst_net_mask = 0;
        self.other_columns.clear();
        self.reversed = false;
        self.batch.column_set.iter_mut().for_each(|b| *b = false);
        self.batch.protobuf.clear();
    }

    /// Clear all column data.
    pub fn clear(&mut self) {
        self.reset();
        for c in self.batch.columns.iter_mut() {
            c.data.clear();
        }
        self.batch.row_count = 0;
    }

    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }

    /// Turn on protobuf encoding of enriched flows.
    pub fn enable_protobuf(&mut self) {
        self.batch.protobuf_enabled = true;
    }

    /// Protobuf encoding of the last finalized flow. Empty when disabled.
    pub fn protobuf_message(&self) -> &[u8] {
        &self.batch.protobuf_message
    }

    /// Number of flows batched.
    pub fn flow_count(&self) -> usize {
        self.batch.row_count
    }

    /// Columns of the batch.
    pub fn batch_columns(&self) -> &[BatchColumn] {
        &self.batch.columns
    }

    /// Reverse the direction of the next calls to `append_*()`.
    pub fn reverse(&mut self) {
        self.reversed = !self.reversed;
        std::mem::swap(&mut self.in_if, &mut self.out_if);
        std::mem::swap(&mut self.src_vlan, &mut self.dst_vlan);
        std::mem::swap(&mut self.src_addr, &mut self.dst_addr);
        std::mem::swap(&mut self.src_as, &mut self.dst_as);
        std::mem::swap(&mut self.src_net_mask, &mut self.dst_net_mask);
    }

    /// Column to append to, if it exists and is not already set.
    #[inline]
    fn target(&mut self, key: ColumnKey) -> Option<(ColumnKey, usize)> {
        let key = if self.reversed { key.reverse_direction() } else { key };
        let k = key.0 as usize;
        let pos = (*self.batch.positions.get(k)?)?;
        if self.batch.column_set[k] {
            return None;
        }
        Some((key, pos))
    }

    #[inline]
    fn mark(&mut self, key: ColumnKey, value: impl FnOnce() -> Value) {
        self.batch.column_set[key.0 as usize] = true;
        if self.debug {
            self.other_columns.insert(key, value());
        }
    }

    /// Append a DateTime value.
    pub fn append_date_time(&mut self, key: ColumnKey, value: u32) {
        if value == 0 {
            return;
        }
        let Some((key, pos)) = self.target(key) else { return };
        match &mut self.batch.columns[pos].data {
            ColumnData::DateTime(v) => v.push(value),
            other => panic!("unhandled datetime type {other:?}"),
        }
        self.mark(key, || Value::U32(value));
        self.protobuf_append_uint(key, value as u64);
    }

    /// Append an UInt64/32/16/8 or Enum8 value.
    pub fn append_uint(&mut self, key: ColumnKey, value: u64) {
        if value == 0 {
            return;
        }
        let Some((key, pos)) = self.target(key) else { return };
        let debug = match &mut self.batch.columns[pos].data {
            ColumnData::UInt64(v) => {
                v.push(value);
                Value::U64(value)
            }
            ColumnData::UInt32(v) => {
                v.push(value as u32);
                Value::U32(value as u32)
            }
            ColumnData::UInt16(v) => {
                v.push(value as u16);
                Value::U16(value as u16)
            }
            ColumnData::UInt8(v) => {
                v.push(value as u8);
                Value::U8(value as u8)
            }
            ColumnData::Enum8(v) => {
                v.push(value as i8);
                Value::U8(value as u8)
            }
            other => panic!("unhandled uint type {other:?}"),
        };
        self.mark(key, || debug);
        self.protobuf_append_uint(key, value);
    }

    /// Append a String value. FixedString values are padded or cut.
    pub fn append_string(&mut self, key: ColumnKey, value: &str) {
        if value.is_empty() {
            return;
        }
        let Some((key, pos)) = self.target(key) else { return };
        match &mut self.batch.columns[pos].data {
            ColumnData::String { data, offsets } => {
                data.extend_from_slice(value.as_bytes());
                offsets.push(data.len());
            }
            ColumnData::FixedString { size, data } => {
                let bytes = value.as_bytes();
                let n = bytes.len().min(*size);
                data.extend_from_slice(&bytes[..n]);
                data.resize(data.len() + (*size - n), 0);
            }
            other => panic!("unhandled string type {other:?}"),
        }
        self.mark(key, || Value::String(value.to_string()));
        self.protobuf_append_string(key, value);
    }

    /// Append an IPv6 value.
    pub fn append_ipv6(&mut self, key: ColumnKey, value: Option<Ipv6Addr>) {
        let Some(value) = value else { return };
        let Some((key, pos)) = self.target(key) else { return };
        match &mut self.batch.columns[pos].data {
            ColumnData::IPv6(v) => v.push(value.octets()),
            other => panic!("unhandled IPv6 type {other:?}"),
        }
        self.mark(key, || Value::IP(value));
        self.protobuf_append_ip(key, &value);
    }

    /// Append an Array(UInt32) value.
    pub fn append_array_uint32(&mut self, key: ColumnKey, value: &[u32]) {
        if value.is_empty() {
            return;
        }
        let Some((key, pos)) = self.target(key) else { return };
        match &mut self.batch.columns[pos].data {
            ColumnData::ArrayUInt32 { offsets, data } => {
                data.extend_from_slice(value);
                offsets.push(data.len() as u64);
            }
            other => panic!("unhandled array type {other:?}"),
        }
        self.mark(key, || Value::ArrayU32(value.to_vec()));
        self.protobuf_append_array_uint32(key, value);
    }

    /// Append an Array(UInt128) value.
    pub fn append_array_uint128(&mut self, key: ColumnKey, value: &[u128]) {
        if value.is_empty() {
            return;
        }
        let Some((key, pos)) = self.target(key) else { return };
        match &mut self.batch.columns[pos].data {
            ColumnData::ArrayUInt128 { offsets, data } => {
                data.extend_from_slice(value);
                offsets.push(data.len() as u64);
            }
            other => panic!("unhandled array type {other:?}"),
        }
        self.mark(key, || Value::ArrayU128(value.to_vec()));
        self.protobuf_append_array_uint128(key, value);
    }

    fn append_default_values(&mut self) {
        for c in self.batch.columns.iter_mut() {
            if !self.batch.column_set[c.key.0 as usize] {
                c.data.push_default();
            }
        }
    }

    /// Revert the changes made since the last finalization.
    pub fn undo(&mut self) {
        for c in self.batch.columns.iter_mut() {
            if self.batch.column_set[c.key.0 as usize] {
                c.data.pop();
            }
        }
        self.reset();
    }

    /// Finalize the current flow. The message can then be reused for the next
    /// one.
    pub fn finalize(&mut self) {
        self.append_date_time(ColumnKey::TimeReceived, self.time_received);
        self.append_uint(ColumnKey::SamplingRate, self.sampling_rate);
        self.append_ipv6(ColumnKey::ExporterAddress, self.exporter_address);
        self.append_uint(ColumnKey::SrcAS, self.src_as as u64);
        self.append_uint(ColumnKey::DstAS, self.dst_as as u64);
        self.append_uint(ColumnKey::SrcNetMask, self.src_net_mask as u64);
        self.append_uint(ColumnKey::DstNetMask, self.dst_net_mask as u64);
        self.append_ipv6(ColumnKey::SrcAddr, self.src_addr);
        self.append_ipv6(ColumnKey::DstAddr, self.dst_addr);
        self.append_ipv6(ColumnKey::NextHop, self.next_hop);
        if !self.schema.is_disabled(ColumnGroup::L2) {
            self.append_uint(ColumnKey::SrcVlan, self.src_vlan as u64);
            self.append_uint(ColumnKey::DstVlan, self.dst_vlan as u64);
        }
        if self.batch.protobuf_enabled {
            self.batch.protobuf_message = self.batch.protobuf.clone();
        }
        self.batch.row_count += 1;
        self.append_default_values();
        self.reset();
        self.check();
    }

    fn check(&self) {
        if cfg!(debug_assertions) {
            for c in &self.batch.columns {
                assert_eq!(
                    c.data.rows(),
                    self.batch.row_count,
                    "column {} has a count of {} instead of {}",
                    c.name,
                    c.data.rows(),
                    self.batch.row_count
                );
            }
        }
    }

    /// Data for a column of the batch.
    pub fn column_data(&self, key: ColumnKey) -> Option<&ColumnData> {
        let pos = (*self.batch.positions.get(key.0 as usize)?)?;
        Some(&self.batch.columns[pos].data)
    }

    // Protobuf encoding. Each column is appended at most once per flow, as the
    // append functions check `column_set` first.

    fn protobuf_field(&self, key: ColumnKey) -> Option<u32> {
        if !self.batch.protobuf_enabled {
            return None;
        }
        let column = self.schema.lookup_column_by_key(key)?;
        (column.protobuf_index > 0).then_some(column.protobuf_index as u32)
    }

    fn protobuf_append_uint(&mut self, key: ColumnKey, value: u64) {
        if let Some(field) = self.protobuf_field(key) {
            crate::protobuf::put_tag(&mut self.batch.protobuf, field, 0);
            crate::protobuf::put_varint(&mut self.batch.protobuf, value);
        }
    }

    fn protobuf_append_string(&mut self, key: ColumnKey, value: &str) {
        if let Some(field) = self.protobuf_field(key) {
            crate::protobuf::put_bytes(&mut self.batch.protobuf, field, value.as_bytes());
        }
    }

    fn protobuf_append_ip(&mut self, key: ColumnKey, value: &Ipv6Addr) {
        if let Some(field) = self.protobuf_field(key) {
            crate::protobuf::put_bytes(&mut self.batch.protobuf, field, &value.octets());
        }
    }

    fn protobuf_append_array_uint32(&mut self, key: ColumnKey, values: &[u32]) {
        if let Some(field) = self.protobuf_field(key) {
            for &v in values {
                crate::protobuf::put_tag(&mut self.batch.protobuf, field, 0);
                crate::protobuf::put_varint(&mut self.batch.protobuf, v as u64);
            }
        }
    }

    fn protobuf_append_array_uint128(&mut self, key: ColumnKey, values: &[u128]) {
        if let Some(field) = self.protobuf_field(key) {
            for &v in values {
                // High then low, big-endian: this is just the u128 in big-endian.
                crate::protobuf::put_bytes(&mut self.batch.protobuf, field, &v.to_be_bytes());
            }
        }
    }

    /// Encode the columns of the batch for the ClickHouse native protocol.
    pub fn encode_columns(&self, with_custom: bool, out: &mut Vec<u8>) {
        crate::native::encode_columns(&self.batch.columns, self.batch.row_count, with_custom, out);
    }

    /// JSON representation, with the same shape as the Go version
    /// (`json.Marshal` of the struct). Invalid addresses are empty strings and
    /// `OtherColumns` is null unless in debug mode.
    pub fn to_json(&self) -> serde_json::Value {
        let ip = |ip: &Option<Ipv6Addr>| ip.as_ref().map(format_ip).unwrap_or_default();
        let mut m = serde_json::Map::new();
        m.insert("TimeReceived".into(), self.time_received.into());
        m.insert("SamplingRate".into(), self.sampling_rate.into());
        m.insert("ExporterAddress".into(), ip(&self.exporter_address).into());
        m.insert("InIf".into(), self.in_if.into());
        m.insert("OutIf".into(), self.out_if.into());
        m.insert("SrcVlan".into(), self.src_vlan.into());
        m.insert("DstVlan".into(), self.dst_vlan.into());
        m.insert("SrcAddr".into(), ip(&self.src_addr).into());
        m.insert("DstAddr".into(), ip(&self.dst_addr).into());
        m.insert("NextHop".into(), ip(&self.next_hop).into());
        m.insert("SrcAS".into(), self.src_as.into());
        m.insert("DstAS".into(), self.dst_as.into());
        m.insert("SrcNetMask".into(), self.src_net_mask.into());
        m.insert("DstNetMask".into(), self.dst_net_mask.into());
        let other = if self.debug {
            let mut o: Vec<(String, serde_json::Value)> =
                self.other_columns.iter().map(|(k, v)| (k.name(), v.to_json())).collect();
            // Go sorts map keys when encoding.
            o.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(o.into_iter().collect())
        } else {
            serde_json::Value::Null
        };
        m.insert("OtherColumns".into(), other);
        serde_json::Value::Object(m)
    }

    /// The INSERT statement for the batch.
    pub fn insert_query(&self, table: &str) -> String {
        let cols: Vec<String> =
            self.batch.columns.iter().map(|c| flowscope_clickhouse::quote_identifier(&c.name)).collect();
        format!("INSERT INTO {} ({}) VALUES", flowscope_clickhouse::quote_identifier(table), cols.join(", "))
    }
}
