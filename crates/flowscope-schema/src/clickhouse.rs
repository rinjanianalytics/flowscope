// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! ClickHouse helpers for the schema. Port of the DDL part of
//! `common/schema/clickhouse.go`. The output must match the Go version byte for
//! byte: the raw table name is derived from a hash of it.

use crate::definition::{Column, Schema};
use crate::key::ColumnKey;

/// Options altering `clickhouse_create_table()` and `clickhouse_select_columns()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableOption {
    /// Skip the columns for the main flows table only.
    SkipMainOnlyColumns,
    /// Skip the columns with a GenerateFrom value.
    SkipGeneratedColumns,
    /// Skip the columns with an Alias value.
    SkipAliasedColumns,
    /// Skip the TimeReceived column.
    SkipTimeReceived,
    /// Use "<generate> AS <name>" for generated columns.
    SubstituteGenerates,
}

impl Column {
    /// Declaration of the column for ClickHouse.
    pub fn clickhouse_definition(&self) -> String {
        let mut result = vec![format!("`{}`", self.name), self.clickhouse_type.clone()];
        if !self.clickhouse_codec.is_empty() {
            result.push(format!("CODEC({})", self.clickhouse_codec));
        }
        if !self.clickhouse_alias.is_empty() {
            result.push(format!("ALIAS {}", self.clickhouse_alias));
        }
        result.join(" ")
    }
}

impl Schema {
    fn clickhouse_iterate(&self, options: &[TableOption], mut f: impl FnMut(Column)) {
        for column in self.columns() {
            if options.contains(&TableOption::SkipTimeReceived) && column.key == ColumnKey::TimeReceived {
                continue;
            }
            if options.contains(&TableOption::SkipMainOnlyColumns) && column.clickhouse_main_only {
                continue;
            }
            if options.contains(&TableOption::SkipGeneratedColumns) && !column.clickhouse_generate_from.is_empty() {
                continue;
            }
            if options.contains(&TableOption::SkipAliasedColumns) && !column.clickhouse_alias.is_empty() {
                continue;
            }
            let mut column = column.clone();
            if options.contains(&TableOption::SubstituteGenerates) && !column.clickhouse_generate_from.is_empty() {
                column.name = format!("{} AS {}", column.clickhouse_generate_from, column.name);
            }
            f(column);
        }
    }

    /// Columns for the CREATE TABLE clause.
    pub fn clickhouse_create_table(&self, options: &[TableOption]) -> String {
        let mut lines = Vec::new();
        self.clickhouse_iterate(options, |c| lines.push(c.clickhouse_definition()));
        lines.join(",\n")
    }

    /// Column names for use in SELECT.
    pub fn clickhouse_select_columns(&self, options: &[TableOption]) -> Vec<String> {
        let mut cols = Vec::new();
        self.clickhouse_iterate(options, |c| cols.push(c.name));
        cols
    }

    /// Sorting keys, prefixed by the primary keys.
    pub fn clickhouse_sorting_keys(&self) -> Vec<String> {
        let mut cols = self.clickhouse_primary_keys();
        for column in self.columns() {
            if column.clickhouse_not_sorting_key || column.clickhouse_main_only {
                continue;
            }
            if !cols.contains(&column.name) {
                cols.push(column.name.clone());
            }
        }
        cols
    }

    /// Primary keys.
    pub fn clickhouse_primary_keys(&self) -> Vec<String> {
        self.clickhouse_primary_keys.iter().map(|k| k.name()).collect()
    }

    /// Hash of the input table in ClickHouse.
    pub fn clickhouse_hash(&self) -> String {
        let create =
            self.clickhouse_create_table(&[TableOption::SkipGeneratedColumns, TableOption::SkipAliasedColumns]);
        format!("{}v5", hash_string(create.as_bytes()))
    }
}

/// FNV-1 (not FNV-1a) 128-bit hash, like Go's `hash/fnv.New128()`.
pub(crate) struct Fnv128(u128);

impl Fnv128 {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;

    pub(crate) fn new() -> Self {
        Fnv128(Self::OFFSET)
    }

    pub(crate) fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.0 = self.0.wrapping_mul(Self::PRIME);
            self.0 ^= b as u128;
        }
    }

    pub(crate) fn sum(&self) -> [u8; 16] {
        self.0.to_be_bytes()
    }
}

/// Standard base32 without padding.
pub(crate) fn base32_nopad(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u64 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | b as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

pub(crate) fn hash_string(data: &[u8]) -> String {
    let mut h = Fnv128::new();
    h.write(data);
    base32_nopad(&h.sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv128() {
        // Reference values computed with Go's hash/fnv New128().
        let mut h = Fnv128::new();
        h.write(b"");
        assert_eq!(hex(&h.sum()), "6c62272e07bb014262b821756295c58d");
        let mut h = Fnv128::new();
        h.write(b"a");
        assert_eq!(hex(&h.sum()), "d228cb69101a8caf78912b704e4a141e");
    }

    #[test]
    fn base32() {
        assert_eq!(base32_nopad(b""), "");
        assert_eq!(base32_nopad(b"f"), "MY");
        assert_eq!(base32_nopad(b"fo"), "MZXQ");
        assert_eq!(base32_nopad(b"foo"), "MZXW6");
        assert_eq!(base32_nopad(b"foob"), "MZXW6YQ");
        assert_eq!(base32_nopad(b"fooba"), "MZXW6YTB");
        assert_eq!(base32_nopad(b"foobar"), "MZXW6YTBOI");
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
