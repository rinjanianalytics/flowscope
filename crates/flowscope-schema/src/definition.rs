// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Data schema for flows. Port of `common/schema/definition.go`. The order of
//! columns and the way `finalize()` expands them is part of the wire contract:
//! it drives the ClickHouse DDL (and therefore the raw table hash) and the
//! protobuf field numbers.

use crate::key::{self, ColumnKey};
use std::fmt;

/// Whether an interface faces inside or outside the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum InterfaceBoundary {
    #[default]
    Undefined = 0,
    External = 1,
    Internal = 2,
}

impl InterfaceBoundary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::External => "external",
            Self::Internal => "internal",
        }
    }
}

impl std::str::FromStr for InterfaceBoundary {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "" | "undefined" => Ok(Self::Undefined),
            "external" => Ok(Self::External),
            "internal" => Ok(Self::Internal),
            _ => Err("unknown interface boundary".into()),
        }
    }
}

impl fmt::Display for InterfaceBoundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Direction of a flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Direction {
    #[default]
    Undefined = 0,
    Ingress = 1,
    Egress = 2,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Ingress => "ingress",
            Self::Egress => "egress",
        }
    }
}

impl std::str::FromStr for Direction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "" | "undefined" => Ok(Self::Undefined),
            "ingress" => Ok(Self::Ingress),
            "egress" => Ok(Self::Egress),
            _ => Err("unknown direction".into()),
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

macro_rules! serde_str_enum {
    ($t:ty) => {
        impl serde::Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}
serde_str_enum!(InterfaceBoundary);
serde_str_enum!(Direction);

/// Name of the asns ClickHouse dictionary.
pub const DICTIONARY_ASNS: &str = "asns";
/// Name of the protocols ClickHouse dictionary.
pub const DICTIONARY_PROTOCOLS: &str = "protocols";
/// Name of the icmp ClickHouse dictionary.
pub const DICTIONARY_ICMP: &str = "icmp";
/// Name of the TCP ClickHouse dictionary.
pub const DICTIONARY_TCP: &str = "tcp";
/// Name of the UDP ClickHouse dictionary.
pub const DICTIONARY_UDP: &str = "udp";

/// Group of columns that can be disabled together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ColumnGroup(pub u32);

impl ColumnGroup {
    pub const NONE: ColumnGroup = ColumnGroup(0);
    pub const L2: ColumnGroup = ColumnGroup(1);
    pub const NAT: ColumnGroup = ColumnGroup(2);
    pub const L3L4: ColumnGroup = ColumnGroup(3);
    pub const LAST: ColumnGroup = ColumnGroup(4);
}

/// Wire kind of a column in the optional protobuf encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProtobufKind {
    #[default]
    Unset,
    String,
    Uint64,
    Uint32,
    Bytes,
}

impl ProtobufKind {
    pub fn type_name(self) -> &'static str {
        match self {
            Self::Unset => "",
            Self::String => "string",
            Self::Uint64 => "uint64",
            Self::Uint32 => "uint32",
            Self::Bytes => "bytes",
        }
    }
}

/// A column of data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Column {
    pub key: ColumnKey,
    pub name: String,
    pub disabled: bool,
    pub no_disable: bool,
    pub group: ColumnGroup,
    pub depends: Vec<ColumnKey>,

    /// For the filter parser.
    pub parser_type: &'static str,

    /// ClickHouse type for the column.
    pub clickhouse_type: String,
    /// ClickHouse type when materialization is requested.
    pub clickhouse_materialized_type: String,
    /// Compression codec.
    pub clickhouse_codec: String,
    /// Alias expression.
    pub clickhouse_alias: String,
    /// For columns derived from another column.
    pub clickhouse_not_sorting_key: bool,
    /// Compute the column from an SQL expression instead of the flow.
    pub clickhouse_generate_from: String,
    /// Only include this column in the main table.
    pub clickhouse_main_only: bool,
    /// The column was materialized (it is not by default).
    pub clickhouse_materialized: bool,

    pub console_not_dimension: bool,
    pub console_truncate_ip: bool,

    /// Protobuf field number. -1 means the column is not exported.
    pub protobuf_index: i32,
    pub protobuf_type: ProtobufKind,
    pub protobuf_repeated: bool,
}

impl Column {
    /// Whether a value for this column is sent to ClickHouse.
    pub fn should_provide_value(&self) -> bool {
        self.clickhouse_generate_from.is_empty() && self.clickhouse_alias.is_empty()
    }
}

/// The data schema.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub(crate) columns: Vec<Column>,
    /// Index in `columns` for each key.
    pub(crate) column_index: Vec<Option<usize>>,
    /// Bit i is set when group i is disabled.
    pub(crate) disabled_groups: u64,
    /// Number of columns generated at runtime after `ColumnKey::Last`.
    pub(crate) dynamic_columns: u32,
    /// Primary keys of the aggregated tables, in order.
    pub(crate) clickhouse_primary_keys: Vec<ColumnKey>,
}

fn col(key: ColumnKey) -> Column {
    Column { key, ..Default::default() }
}

fn lc_string(key: ColumnKey) -> Column {
    Column { parser_type: "string", clickhouse_type: "LowCardinality(String)".into(), ..col(key) }
}

fn lc_string_nsk(key: ColumnKey) -> Column {
    Column { clickhouse_not_sorting_key: true, ..lc_string(key) }
}

fn typed(key: ColumnKey, parser: &'static str, ch: &str) -> Column {
    Column { parser_type: parser, clickhouse_type: ch.into(), ..col(key) }
}

fn l3l4(key: ColumnKey, ch: &str) -> Column {
    Column { disabled: true, group: ColumnGroup::L3L4, ..typed(key, "uint", ch) }
}

fn packet_size_bucket_alias() -> String {
    let boundaries = [64, 128, 256, 512, 768, 1024, 1280, 1501, 2048, 3072, 4096, 8192, 10240, 16384, 32768, 65536];
    let mut conditions = Vec::new();
    let mut last = 0;
    for b in boundaries {
        conditions.push(format!("PacketSize < {b}, '{last}-{}'", b - 1));
        last = b;
    }
    conditions.push(format!("'{last}-Inf'"));
    format!("multiIf({})", conditions.join(", "))
}

fn icmp_alias(proto: u8, version: &str) -> String {
    format!(
        "if(Proto = {proto}, dictGetOrDefault('{DICTIONARY_ICMP}', 'name', tuple(Proto, ICMP{version}Type, ICMP{version}Code), concat(toString(ICMP{version}Type), '/', toString(ICMP{version}Code))), '')"
    )
}

/// The flows schema before configuration is applied. Any column starting with
/// Src/InIf is duplicated as Dst/OutIf by `finalize()`.
pub(crate) fn flows() -> Schema {
    use ColumnKey as K;
    let columns = vec![
        Column {
            no_disable: true,
            clickhouse_type: "DateTime".into(),
            clickhouse_codec: "DoubleDelta, LZ4".into(),
            console_not_dimension: true,
            ..col(K::TimeReceived)
        },
        Column {
            no_disable: true,
            clickhouse_type: "UInt64".into(),
            console_not_dimension: true,
            ..col(K::SamplingRate)
        },
        typed(K::ExporterAddress, "ip", "LowCardinality(IPv6)"),
        lc_string_nsk(K::ExporterName),
        lc_string_nsk(K::ExporterGroup),
        lc_string_nsk(K::ExporterRole),
        lc_string_nsk(K::ExporterSite),
        lc_string_nsk(K::ExporterRegion),
        lc_string_nsk(K::ExporterTenant),
        Column {
            clickhouse_main_only: true,
            clickhouse_codec: "ZSTD(1)".into(),
            console_truncate_ip: true,
            ..typed(K::SrcAddr, "ip", "IPv6")
        },
        Column {
            clickhouse_main_only: true,
            clickhouse_type: "UInt8".into(),
            console_not_dimension: true,
            ..col(K::SrcNetMask)
        },
        Column {
            clickhouse_main_only: true,
            clickhouse_materialized_type: "LowCardinality(String)".into(),
            clickhouse_alias: "CASE
 WHEN EType = 0x800 THEN concat(replaceRegexpOne(IPv6CIDRToRange(SrcAddr, (96 + SrcNetMask)::UInt8).1::String, '^::ffff:', ''), '/', SrcNetMask::String)
 WHEN EType = 0x86dd THEN concat(IPv6CIDRToRange(SrcAddr, SrcNetMask).1::String, '/', SrcNetMask::String)
 ELSE ''
END"
            .into(),
            ..typed(K::SrcNetPrefix, "prefix", "String")
        },
        typed(K::SrcAS, "asn", "UInt32"),
        lc_string(K::SrcNetName),
        lc_string(K::SrcNetRole),
        lc_string(K::SrcNetSite),
        lc_string(K::SrcNetRegion),
        lc_string(K::SrcNetTenant),
        Column { disabled: true, group: ColumnGroup::L2, ..typed(K::SrcVlan, "uint", "UInt16") },
        typed(K::SrcCountry, "string", "FixedString(2)"),
        lc_string(K::SrcGeoCity),
        lc_string(K::SrcGeoState),
        Column { clickhouse_main_only: true, ..typed(K::DstASPath, "aspath", "Array(UInt32)") },
        Column {
            depends: vec![K::DstASPath],
            clickhouse_generate_from: "c_DstASPath[1]".into(),
            ..typed(K::Dst1stAS, "asn", "UInt32")
        },
        Column {
            depends: vec![K::DstASPath],
            clickhouse_generate_from: "c_DstASPath[2]".into(),
            ..typed(K::Dst2ndAS, "asn", "UInt32")
        },
        Column {
            depends: vec![K::DstASPath],
            clickhouse_generate_from: "c_DstASPath[3]".into(),
            ..typed(K::Dst3rdAS, "asn", "UInt32")
        },
        Column {
            clickhouse_main_only: true,
            disabled: true,
            ..typed(K::SrcCommunities, "community", "Array(UInt32)")
        },
        Column { clickhouse_main_only: true, ..typed(K::DstCommunities, "community", "Array(UInt32)") },
        Column {
            clickhouse_main_only: true,
            clickhouse_type: "Array(UInt128)".into(),
            console_not_dimension: true,
            disabled: true,
            ..col(K::SrcLargeCommunities)
        },
        Column {
            clickhouse_main_only: true,
            clickhouse_type: "Array(UInt128)".into(),
            console_not_dimension: true,
            ..col(K::DstLargeCommunities)
        },
        lc_string(K::InIfName),
        lc_string_nsk(K::InIfDescription),
        Column { clickhouse_not_sorting_key: true, ..typed(K::InIfSpeed, "uint", "UInt32") },
        lc_string_nsk(K::InIfConnectivity),
        lc_string_nsk(K::InIfProvider),
        Column {
            clickhouse_not_sorting_key: true,
            ..typed(
                K::InIfBoundary,
                "boundary",
                "Enum8('undefined' = 0, 'external' = 1, 'internal' = 2)",
            )
        },
        typed(K::EType, "etype", "UInt32"),
        typed(K::Proto, "proto", "UInt32"),
        Column { clickhouse_main_only: true, ..typed(K::SrcPort, "uint", "UInt16") },
        Column {
            no_disable: true,
            clickhouse_type: "UInt64".into(),
            clickhouse_codec: "T64, LZ4".into(),
            clickhouse_not_sorting_key: true,
            console_not_dimension: true,
            ..col(K::Bytes)
        },
        Column {
            no_disable: true,
            clickhouse_type: "UInt64".into(),
            clickhouse_codec: "T64, LZ4".into(),
            clickhouse_not_sorting_key: true,
            console_not_dimension: true,
            ..col(K::Packets)
        },
        Column {
            depends: vec![K::Bytes, K::Packets],
            clickhouse_alias: "intDiv(Bytes, Packets)".into(),
            ..typed(K::PacketSize, "uint", "UInt64")
        },
        Column {
            depends: vec![K::PacketSize],
            clickhouse_type: "LowCardinality(String)".into(),
            clickhouse_alias: packet_size_bucket_alias(),
            ..col(K::PacketSizeBucket)
        },
        typed(K::ForwardingStatus, "uint", "UInt32"),
        typed(
            K::FlowDirection,
            "direction",
            "Enum8('undefined' = 0, 'ingress' = 1, 'egress' = 2)",
        ),
        Column {
            disabled: true,
            group: ColumnGroup::NAT,
            clickhouse_main_only: true,
            console_truncate_ip: true,
            ..typed(K::SrcAddrNAT, "ip", "IPv6")
        },
        Column {
            disabled: true,
            group: ColumnGroup::NAT,
            clickhouse_main_only: true,
            ..typed(K::SrcPortNAT, "uint", "UInt16")
        },
        Column { disabled: true, group: ColumnGroup::L2, ..typed(K::SrcMAC, "mac", "UInt64") },
        l3l4(K::IPTTL, "UInt8"),
        l3l4(K::IPTos, "UInt8"),
        l3l4(K::IPFragmentID, "UInt32"),
        l3l4(K::IPFragmentOffset, "UInt16"),
        l3l4(K::IPv6FlowLabel, "UInt32"),
        l3l4(K::TCPFlags, "UInt16"),
        l3l4(K::ICMPv4Type, "UInt8"),
        l3l4(K::ICMPv4Code, "UInt8"),
        l3l4(K::ICMPv6Type, "UInt8"),
        l3l4(K::ICMPv6Code, "UInt8"),
        Column {
            depends: vec![K::Proto, K::ICMPv4Type, K::ICMPv4Code],
            disabled: true,
            group: ColumnGroup::L3L4,
            clickhouse_alias: icmp_alias(1, "v4"),
            ..typed(K::ICMPv4, "string", "LowCardinality(String)")
        },
        Column {
            depends: vec![K::Proto, K::ICMPv6Type, K::ICMPv6Code],
            disabled: true,
            group: ColumnGroup::L3L4,
            clickhouse_alias: icmp_alias(58, "v6"),
            ..typed(K::ICMPv6, "string", "LowCardinality(String)")
        },
        Column {
            disabled: true,
            clickhouse_codec: "ZSTD(1)".into(),
            ..typed(K::NextHop, "ip", "LowCardinality(IPv6)")
        },
        Column {
            disabled: true,
            clickhouse_main_only: true,
            ..typed(K::MPLSLabels, "array(uint)", "Array(UInt32)")
        },
        mpls_label(K::MPLS1stLabel, 1),
        mpls_label(K::MPLS2ndLabel, 2),
        mpls_label(K::MPLS3rdLabel, 3),
        mpls_label(K::MPLS4thLabel, 4),
        Column { disabled: true, ..typed(K::IngressVRFID, "uint", "UInt32") },
        Column { disabled: true, ..typed(K::EgressVRFID, "uint", "UInt32") },
    ];
    Schema {
        clickhouse_primary_keys: vec![
            K::TimeReceived,
            K::ExporterAddress,
            K::EType,
            K::Proto,
            K::InIfName,
            K::SrcAS,
            K::ForwardingStatus,
            K::OutIfName,
            K::DstAS,
            K::SamplingRate,
        ],
        columns,
        ..Default::default()
    }
    .finalize()
}

fn mpls_label(key: ColumnKey, n: u32) -> Column {
    Column {
        disabled: true,
        depends: vec![ColumnKey::MPLSLabels],
        clickhouse_main_only: true,
        clickhouse_alias: format!("MPLSLabels[{n}]"),
        ..typed(key, "uint", "UInt32")
    }
}

impl Schema {
    pub(crate) fn finalize(mut self) -> Schema {
        let original_keys: Vec<ColumnKey> = self.columns.iter().map(|c| c.key).collect();
        let already_exists = |name: &str| ColumnKey::from_name(name).is_some_and(|k| original_keys.contains(&k));
        let mut ncolumns = Vec::with_capacity(self.columns.len() * 2);
        for mut column in std::mem::take(&mut self.columns) {
            let name = column.key.name();
            if name.is_empty() {
                panic!("missing name mapping for {}", column.key.0);
            }
            if column.name.is_empty() {
                column.name = name;
            }
            // Non-main columns with an alias are not sorting keys.
            if !column.clickhouse_main_only && !column.clickhouse_alias.is_empty() {
                column.clickhouse_not_sorting_key = true;
            }
            column.depends.sort();
            column.depends.dedup();

            ncolumns.push(column.clone());

            // Expand Src → Dst and InIf → OutIf. Like in Go, the second swap
            // operates on the (possibly) renamed column.
            for (source, target) in [("Src", "Dst"), ("InIf", "OutIf")] {
                if let Some(rest) = column.name.strip_prefix(source) {
                    column.name = format!("{target}{rest}");
                    if !already_exists(&column.name) {
                        column.key = ColumnKey::from_name(&column.name)
                            .unwrap_or_else(|| panic!("missing name mapping for {:?}", column.name));
                        column.clickhouse_alias = column.clickhouse_alias.replace(source, target);
                        column.clickhouse_generate_from = column.clickhouse_generate_from.replace(source, target);
                        ncolumns.push(column.clone());
                    }
                }
            }
        }
        self.columns = ncolumns;

        // Protobuf field numbers and wire types, used only by the optional
        // outlet Kafka output.
        let mut protobuf_index = 1;
        for column in self.columns.iter_mut() {
            if !column.should_provide_value() {
                column.protobuf_index = -1;
                continue;
            }
            if column.protobuf_index == 0 {
                column.protobuf_index = protobuf_index;
                protobuf_index += 1;
            }
            if column.protobuf_type == ProtobufKind::Unset {
                match column.clickhouse_type.as_str() {
                    "String" | "LowCardinality(String)" | "FixedString(2)" => {
                        column.protobuf_type = ProtobufKind::String
                    }
                    "UInt64" => column.protobuf_type = ProtobufKind::Uint64,
                    "UInt32" | "UInt16" | "UInt8" | "DateTime" => column.protobuf_type = ProtobufKind::Uint32,
                    "IPv6" | "LowCardinality(IPv6)" => column.protobuf_type = ProtobufKind::Bytes,
                    "Array(UInt32)" => {
                        column.protobuf_type = ProtobufKind::Uint32;
                        column.protobuf_repeated = true;
                    }
                    "Array(UInt128)" => {
                        column.protobuf_type = ProtobufKind::Bytes;
                        column.protobuf_repeated = true;
                    }
                    t if t.starts_with("Enum8") => column.protobuf_type = ProtobufKind::Uint32,
                    _ => column.protobuf_index = -1,
                }
            }
        }

        // Column index.
        let max_key = self.columns.iter().map(|c| c.key.0).max().unwrap_or(0).max(ColumnKey::TimeReceived.0);
        self.column_index = vec![None; max_key as usize + 1];
        for (i, column) in self.columns.iter().enumerate() {
            self.column_index[column.key.0 as usize] = Some(i);
        }

        // Disabled groups. Group 0 is "no group" and is disabled only when
        // every column is disabled, like in Go.
        self.disabled_groups = 0;
        for group in 0..ColumnGroup::LAST.0 {
            if !self.columns.iter().any(|c| !c.disabled && c.group.0 == group) {
                self.disabled_groups |= 1 << group;
            }
        }
        self
    }

    pub fn lookup_column_by_key(&self, key: ColumnKey) -> Option<&Column> {
        self.column_index.get(key.0 as usize).copied().flatten().map(|i| &self.columns[i])
    }

    pub(crate) fn lookup_column_by_key_mut(&mut self, key: ColumnKey) -> Option<&mut Column> {
        let idx = self.column_index.get(key.0 as usize).copied().flatten()?;
        Some(&mut self.columns[idx])
    }

    pub fn lookup_column_by_name(&self, name: &str) -> Option<&Column> {
        ColumnKey::from_name(name).and_then(|k| self.lookup_column_by_key(k))
    }

    /// Reverse the direction of a column (Src ↔ Dst, In ↔ Out) when the other
    /// column exists and is enabled. Otherwise, return the same key.
    pub fn reverse_column_direction(&self, key: ColumnKey) -> ColumnKey {
        let name = key.name();
        let mut candidate = String::new();
        if let Some(rest) = name.strip_prefix("Src") {
            candidate = format!("Dst{rest}");
        }
        if let Some(rest) = name.strip_prefix("Dst") {
            candidate = format!("Src{rest}");
        }
        if let Some(rest) = name.strip_prefix("In") {
            candidate = format!("Out{rest}");
        }
        if let Some(rest) = name.strip_prefix("Out") {
            candidate = format!("In{rest}");
        }
        if let Some(k) = ColumnKey::from_name(&candidate)
            && self.lookup_column_by_key(k).is_some_and(|c| !c.disabled)
        {
            return k;
        }
        key
    }

    /// Enabled columns, in order.
    pub fn columns(&self) -> impl Iterator<Item = &Column> {
        self.columns.iter().filter(|c| !c.disabled)
    }

    /// All columns, including disabled ones.
    pub fn all_columns(&self) -> &[Column] {
        &self.columns
    }

    /// Whether a column group is disabled.
    pub fn is_disabled(&self, group: ColumnGroup) -> bool {
        self.disabled_groups & (1 << group.0) != 0
    }

    /// Enable every column. Only meant for tests.
    pub fn enable_all_columns(mut self) -> Schema {
        for c in self.columns.iter_mut() {
            c.disabled = false;
        }
        self.finalize()
    }

    /// Highest column key in use.
    pub(crate) fn max_key(&self) -> u32 {
        self.columns.iter().map(|c| c.key.0).max().unwrap_or(0)
    }
}

/// Register the name of a dynamic column.
pub(crate) fn register_dynamic(key: ColumnKey, name: &str) {
    key::register(key, name);
}
