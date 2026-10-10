// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Query columns and query filters. They need a schema to be validated.

use crate::filter;
use crate::sqlbuilder::{self as sb, Expr};
use flowscope_schema::{ColumnKey, DICTIONARY_ASNS, DICTIONARY_PROTOCOLS, DICTIONARY_TCP, DICTIONARY_UDP, Schema};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const ETYPE_IPV4: u64 = 0x800;
const ETYPE_IPV6: u64 = 0x86dd;
const PROTO_TCP: u64 = 6;
const PROTO_UDP: u64 = 17;

/// A query column. It has to be validated before use.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct Column {
    name: String,
    key: Option<ColumnKey>,
}

impl PartialEq for Column {
    /// Two columns are equal when they have the same name.
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Column {}

impl From<String> for Column {
    fn from(name: String) -> Self {
        Column { name, key: None }
    }
}

impl From<Column> for String {
    fn from(c: Column) -> Self {
        c.name
    }
}

impl std::fmt::Display for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

impl Column {
    pub fn new(name: &str) -> Self {
        Column { name: name.into(), key: None }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The key of the column. Panics when the column was not validated.
    pub fn key(&self) -> ColumnKey {
        self.key.expect("query column not validated")
    }

    /// Validate the column against the schema.
    pub fn validate(&mut self, schema: &Schema) -> Result<(), String> {
        match schema.lookup_column_by_name(&self.name) {
            Some(c) if !c.console_not_dimension && !c.disabled => {
                self.key = Some(c.key);
                Ok(())
            }
            _ => Err(format!("unknown column name {}", self.name)),
        }
    }

    /// Reverse the direction of the column, when the reversed column is a
    /// valid one.
    pub fn reverse(&mut self, schema: &Schema) {
        let mut reversed = Column::new(&schema.reverse_column_direction(self.key()).name());
        if reversed.validate(schema).is_ok() {
            *self = reversed;
        }
    }

    /// The expression to use in SELECT. The database is the one holding the
    /// dictionaries.
    pub fn to_sql_select(&self, schema: &Schema, database: &str) -> Expr {
        let key = self.key();
        let me = sb::column(&self.name);
        match key {
            ColumnKey::SrcAS | ColumnKey::DstAS | ColumnKey::Dst1stAS | ColumnKey::Dst2ndAS | ColumnKey::Dst3rdAS => {
                sb::function(
                    "concat",
                    vec![
                        sb::function("toString", vec![me.clone()]),
                        sb::string(": "),
                        dictionary_lookup(database, DICTIONARY_ASNS, me, "???"),
                    ],
                )
            }
            ColumnKey::InIfBoundary | ColumnKey::OutIfBoundary => sb::function("toString", vec![me]),
            ColumnKey::EType => {
                let etype = sb::column("EType");
                sb::function(
                    "if",
                    vec![
                        sb::op(etype.clone(), "=", sb::uint(ETYPE_IPV4)),
                        sb::string("IPv4"),
                        sb::function(
                            "if",
                            vec![sb::op(etype, "=", sb::uint(ETYPE_IPV6)), sb::string("IPv6"), sb::string("???")],
                        ),
                    ],
                )
            }
            ColumnKey::Proto => dictionary_lookup(database, DICTIONARY_PROTOCOLS, me, "???"),
            ColumnKey::MPLSLabels | ColumnKey::DstASPath => {
                sb::function("arrayStringConcat", vec![me, sb::string(" ")])
            }
            ColumnKey::SrcCommunities | ColumnKey::DstCommunities => {
                let direction = &self.name[..3];
                sb::function(
                    "arrayStringConcat",
                    vec![
                        sb::function(
                            "arrayConcat",
                            vec![
                                communities_to_strings(&format!("{direction}Communities")),
                                large_communities_to_strings(&format!("{direction}LargeCommunities")),
                            ],
                        ),
                        sb::string(" "),
                    ],
                )
            }
            ColumnKey::SrcMAC | ColumnKey::DstMAC => sb::function("MACNumToString", vec![me]),
            ColumnKey::TCPFlags => {
                let bits = ["FIN", "SYN", "RST", "PSH", ".ACK", "URG", "ECE", "CWR", "NS"];
                let flags = bits
                    .iter()
                    .enumerate()
                    .map(|(bit, name)| {
                        sb::function(
                            "if",
                            vec![
                                sb::op(
                                    sb::function("bitTest", vec![me.clone(), sb::uint(bit as u64)]),
                                    "=",
                                    sb::uint(1),
                                ),
                                sb::string(&name[..1]),
                                sb::string(""),
                            ],
                        )
                    })
                    .collect();
                sb::function("arrayStringConcat", vec![sb::array(flags), sb::string("")])
            }
            ColumnKey::DstPort | ColumnKey::SrcPort => {
                let proto = sb::column("Proto");
                let named = |dictionary: &str| {
                    sb::function(
                        "concat",
                        vec![
                            sb::function("toString", vec![me.clone()]),
                            sb::string("/"),
                            dictionary_lookup(database, dictionary, me.clone(), ""),
                        ],
                    )
                };
                // The port may have no name in the dictionary, in which case
                // the trailing slash is dropped.
                sb::function(
                    "replaceRegexpOne",
                    vec![
                        sb::function(
                            "multiIf",
                            vec![
                                sb::op(proto.clone(), "=", sb::uint(PROTO_TCP)),
                                named(DICTIONARY_TCP),
                                sb::op(proto, "=", sb::uint(PROTO_UDP)),
                                named(DICTIONARY_UDP),
                                sb::function("toString", vec![me.clone()]),
                            ],
                        ),
                        sb::string("/$"),
                        sb::string(""),
                    ],
                )
            }
            _ => match schema.lookup_column_by_key(key) {
                Some(c) if c.clickhouse_type.starts_with("UInt") => sb::function("toString", vec![me]),
                Some(c) if c.clickhouse_type == "IPv6" || c.clickhouse_type == "LowCardinality(IPv6)" => {
                    address_to_string(me)
                }
                _ => me,
            },
        }
    }
}

/// Validate a list of columns.
pub fn validate_columns(columns: &mut [Column], schema: &Schema) -> Result<(), String> {
    columns.iter_mut().try_for_each(|c| c.validate(schema))
}

/// Reverse the direction of a list of columns.
pub fn reverse_columns(columns: &mut [Column], schema: &Schema) {
    columns.iter_mut().for_each(|c| c.reverse(schema));
}

/// Look up the name matching a key in a dictionary. The fallback is used for
/// an unknown key. The dictionary is qualified with its database, as the
/// remote nodes of a cluster do not resolve a bare name.
pub fn dictionary_lookup(database: &str, dictionary: &str, key: Expr, fallback: &str) -> Expr {
    sb::function(
        "dictGetOrDefault",
        vec![sb::string(&sb::table(dictionary, Some(database))), sb::string("name"), key, sb::string(fallback)],
    )
}

/// Turn an address into a string, without the IPv4-mapped prefix.
pub fn address_to_string(addr: Expr) -> Expr {
    sb::function(
        "replaceRegexpOne",
        vec![sb::function("IPv6NumToString", vec![addr]), sb::string("^::ffff:"), sb::string("")],
    )
}

/// Turn a BGP community into "AS:value".
pub fn community_to_string(community: Expr) -> Expr {
    sb::function(
        "concat",
        vec![
            sb::function("toString", vec![sb::function("bitShiftRight", vec![community.clone(), sb::uint(16)])]),
            sb::string(":"),
            sb::function("toString", vec![sb::function("bitAnd", vec![community, sb::number("0xffff")])]),
        ],
    )
}

/// Turn an array of BGP communities into an array of strings.
pub fn communities_to_strings(column: &str) -> Expr {
    sb::function("arrayMap", vec![sb::lambda("c", community_to_string(sb::column("c"))), sb::column(column)])
}

/// Turn a large BGP community into "AS:value:value".
pub fn large_community_to_string(community: Expr) -> Expr {
    let part = |shift: u64| {
        let value = if shift > 0 {
            sb::function("bitShiftRight", vec![community.clone(), sb::uint(shift)])
        } else {
            community.clone()
        };
        sb::function("toString", vec![sb::function("bitAnd", vec![value, sb::number("0xffffffff")])])
    };
    sb::function("concat", vec![part(64), sb::string(":"), part(32), sb::string(":"), part(0)])
}

/// Turn an array of large BGP communities into an array of strings.
pub fn large_communities_to_strings(column: &str) -> Expr {
    sb::function("arrayMap", vec![sb::lambda("c", large_community_to_string(sb::column("c"))), sb::column(column)])
}

/// A query filter. It has to be validated before use.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct Filter {
    filter: String,
    validated: bool,
    direct: Option<Expr>,
    reverse: Option<Expr>,
    main_table_required: bool,
}

impl From<String> for Filter {
    fn from(s: String) -> Self {
        Filter::new(s.trim())
    }
}

impl From<Filter> for String {
    fn from(f: Filter) -> Self {
        f.filter
    }
}

impl PartialEq for Filter {
    /// Once validated, two filters written differently but compiling to the
    /// same conditions are equal.
    fn eq(&self, other: &Self) -> bool {
        if self.validated != other.validated {
            return false;
        }
        if !self.validated {
            return self.filter == other.filter;
        }
        self.direct == other.direct && self.reverse == other.reverse
    }
}

impl std::fmt::Display for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.filter)
    }
}

impl Filter {
    pub fn new(input: &str) -> Self {
        Filter { filter: input.into(), ..Filter::default() }
    }

    /// Validate the filter. The database is the one holding the
    /// dictionaries a filter may need.
    pub fn validate(&mut self, schema: &Arc<Schema>, database: &str) -> Result<(), String> {
        if self.filter.is_empty() {
            self.validated = true;
            return Ok(());
        }
        let mut meta = filter::Meta { schema: schema.clone(), database: database.into(), reverse_direction: false };
        let direct = filter::parse(&self.filter, &meta).map_err(|e| format!("cannot parse filter: {}", e.human()))?;
        meta.reverse_direction = true;
        let reverse =
            filter::parse(&self.filter, &meta).map_err(|e| format!("cannot parse reverse filter: {}", e.human()))?;
        self.direct = direct.expr;
        self.reverse = reverse.expr;
        self.main_table_required = direct.main_table_required || reverse.main_table_required;
        self.validated = true;
        Ok(())
    }

    fn check(&self) {
        assert!(self.validated, "query filter not validated");
    }

    pub fn main_table_required(&self) -> bool {
        self.check();
        self.main_table_required
    }

    pub fn direct(&self) -> Option<Expr> {
        self.check();
        self.direct.clone()
    }

    pub fn reverse(&self) -> Option<Expr> {
        self.check();
        self.reverse.clone()
    }

    /// Swap the direct and the reverse filters.
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.direct, &mut self.reverse);
    }
}
