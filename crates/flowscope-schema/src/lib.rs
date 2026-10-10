// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Data schema for flows used by FlowScope. Port of `common/schema`.
//!
//! One schema drives the ClickHouse DDL, the columnar batches sent to
//! ClickHouse, the optional protobuf encoding of enriched flows and the
//! console dimensions.

mod clickhouse;
mod config;
mod definition;
mod key;
mod keys_gen;
mod message;
pub mod native;
mod protobuf;
mod root;

pub use clickhouse::TableOption;
pub use config::{
    Configuration, CustomDict, CustomDictAttribute, CustomDictKey, CustomDictLayout, SkipIndexType, default_indexes,
    format_g,
};
pub use definition::{
    Column, ColumnGroup, DICTIONARY_ASNS, DICTIONARY_ICMP, DICTIONARY_PROTOCOLS, DICTIONARY_TCP, DICTIONARY_UDP,
    Direction, InterfaceBoundary, ProtobufKind, Schema,
};
pub use key::ColumnKey;
pub use message::{BatchColumn, ColumnData, FlowMessage, Value, format_ip};
pub use root::Component;
