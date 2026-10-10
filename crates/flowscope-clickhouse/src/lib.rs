// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! ClickHouse client using the native TCP protocol, like the Go version
//! (ch-go and clickhouse-go). Configurations point to the native port
//! (9000), so the Rust version speaks the same protocol.

pub mod block;
mod compress;
mod conn;
pub mod io;
pub mod types;

pub use block::{Block, Column, Value, Values};
pub use conn::{Connection, Error, Options, QueryResult, REVISION, ServerInfo, Setting};
pub use types::DataType;

/// Quote an identifier with backquotes.
pub fn quote_identifier(s: &str) -> String {
    format!("`{}`", s.replace('\\', "\\\\").replace('`', "\\`"))
}

/// Quote a string literal.
pub fn quote_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}
