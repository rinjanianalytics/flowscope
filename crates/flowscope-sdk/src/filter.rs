// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Build filter expressions without writing them by hand.
//!
//! The builder takes care of the syntax: quotes, escapes, parentheses. It
//! does not know the schema, so the server still decides if a column exists
//! and accepts an operator.
//!
//! ```
//! use flowscope_sdk::filter::{and, col, or};
//!
//! let filter = and([
//!     col("InIfBoundary").eq_keyword("external"),
//!     or([col("SrcAS").eq(15169), col("SrcAS").eq(2906)]),
//!     !col("ExporterName").like("lab-%"),
//! ]);
//! assert_eq!(
//!     filter.to_string(),
//!     r#"InIfBoundary = external AND (SrcAS = 15169 OR SrcAS = 2906) AND NOT ExporterName LIKE "lab-%""#
//! );
//! ```

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A value in a condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value(String);

impl Value {
    /// A word written as is: `external`, `IPv4`, `in`, `AS15169`.
    ///
    /// # Panics
    ///
    /// When the word has characters other than letters and digits.
    pub fn keyword(word: &str) -> Value {
        assert!(
            !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric()),
            "a keyword only has letters and digits: {word:?}"
        );
        Value(word.to_string())
    }

    /// A subnet (`192.0.2.0/24`), a MAC address or a community. It is
    /// written as is.
    ///
    /// # Panics
    ///
    /// When the text has characters which could end the value.
    pub fn raw(text: &str) -> Value {
        assert!(
            !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/')),
            "unexpected character in {text:?}"
        );
        Value(text.to_string())
    }

    /// Another column to compare with.
    pub fn column(name: &str) -> Value {
        Value(check_column(name).to_string())
    }
}

/// A text, written between quotes.
impl From<&str> for Value {
    fn from(text: &str) -> Value {
        // A string cannot span lines: there is no escape for a line break.
        let text = text.replace(['\r', '\n'], " ");
        let mut quoted = String::with_capacity(text.len() + 2);
        quoted.push('"');
        for c in text.chars() {
            if c == '"' || c == '\\' {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        Value(quoted)
    }
}

impl From<String> for Value {
    fn from(text: String) -> Value {
        Value::from(text.as_str())
    }
}

macro_rules! value_from {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Value {
                Value(v.to_string())
            }
        }
    )*};
}
value_from!(u8, u16, u32, u64, IpAddr, Ipv4Addr, Ipv6Addr);

/// A number written without a type, like `eq(443)`.
///
/// # Panics
///
/// When the number is negative: the filter language has no negative number.
impl From<i32> for Value {
    fn from(v: i32) -> Value {
        assert!(v >= 0, "a filter cannot hold a negative number: {v}");
        Value(v.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Or,
    And,
    /// A condition, a negation or a sub-expression.
    Unit,
}

/// A filter expression. Its text is given by `to_string()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    text: String,
    kind: Kind,
}

impl Filter {
    /// The filter matching all flows.
    pub fn all() -> Filter {
        Filter { text: String::new(), kind: Kind::Unit }
    }

    /// Tell if the filter matches all flows.
    pub fn is_all(&self) -> bool {
        self.text.is_empty()
    }

    /// An expression already written, for example by a user. It is put
    /// between parentheses when combined.
    pub fn parsed(text: &str) -> Filter {
        Filter { text: text.trim().to_string(), kind: Kind::Or }
    }

    /// Both this filter and the other one.
    pub fn and(self, other: Filter) -> Filter {
        and([self, other])
    }

    /// This filter or the other one.
    pub fn or(self, other: Filter) -> Filter {
        or([self, other])
    }

    fn within(&self, parent: Kind) -> String {
        if self.kind < parent { format!("({})", self.text) } else { self.text.clone() }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<Filter> for String {
    fn from(filter: Filter) -> String {
        filter.text
    }
}

impl std::ops::Not for Filter {
    type Output = Filter;

    /// The opposite filter. The opposite of "all flows" cannot be written
    /// and stays "all flows".
    fn not(self) -> Filter {
        if self.is_all() {
            return self;
        }
        let inner = if self.kind == Kind::Unit { self.text } else { format!("({})", self.text) };
        Filter { text: format!("NOT {inner}"), kind: Kind::Unit }
    }
}

fn combine(filters: impl IntoIterator<Item = Filter>, kind: Kind, word: &str) -> Filter {
    let mut parts: Vec<Filter> = filters.into_iter().filter(|f| !f.is_all()).collect();
    match parts.len() {
        0 => Filter::all(),
        1 => parts.remove(0),
        _ => Filter { text: parts.iter().map(|f| f.within(kind)).collect::<Vec<_>>().join(word), kind },
    }
}

/// All the filters at once. Filters matching all flows are skipped.
pub fn and(filters: impl IntoIterator<Item = Filter>) -> Filter {
    combine(filters, Kind::And, " AND ")
}

/// Any of the filters. Filters matching all flows are skipped: this helps
/// building a list from optional parts, but it is not the logic of "or".
pub fn or(filters: impl IntoIterator<Item = Filter>) -> Filter {
    combine(filters, Kind::Or, " OR ")
}

fn check_column(name: &str) -> &str {
    assert!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()),
        "a column name only has letters and digits: {name:?}"
    );
    name
}

/// A column to put a condition on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column(String);

/// A column to put a condition on.
///
/// # Panics
///
/// When the name has characters other than letters and digits.
pub fn col(name: &str) -> Column {
    Column(check_column(name).to_string())
}

impl Column {
    fn op(&self, operator: &str, value: Value) -> Filter {
        Filter { text: format!("{} {operator} {}", self.0, value.0), kind: Kind::Unit }
    }

    fn list<V: Into<Value>>(&self, operator: &str, values: impl IntoIterator<Item = V>) -> Filter {
        let values: Vec<String> = values.into_iter().map(|v| v.into().0).collect();
        assert!(!values.is_empty(), "a list needs at least one value");
        Filter { text: format!("{} {operator} ({})", self.0, values.join(", ")), kind: Kind::Unit }
    }

    /// `Column = value`
    pub fn eq(&self, value: impl Into<Value>) -> Filter {
        self.op("=", value.into())
    }

    /// `Column != value`
    pub fn ne(&self, value: impl Into<Value>) -> Filter {
        self.op("!=", value.into())
    }

    /// `Column = word`, for the columns taking a word without quotes, like
    /// `InIfBoundary = external` or `EType = IPv6`.
    pub fn eq_keyword(&self, word: &str) -> Filter {
        self.op("=", Value::keyword(word))
    }

    /// `Column != word`
    pub fn ne_keyword(&self, word: &str) -> Filter {
        self.op("!=", Value::keyword(word))
    }

    /// `Column < value`
    pub fn lt(&self, value: u64) -> Filter {
        self.op("<", value.into())
    }

    /// `Column <= value`
    pub fn le(&self, value: u64) -> Filter {
        self.op("<=", value.into())
    }

    /// `Column > value`
    pub fn gt(&self, value: u64) -> Filter {
        self.op(">", value.into())
    }

    /// `Column >= value`
    pub fn ge(&self, value: u64) -> Filter {
        self.op(">=", value.into())
    }

    /// `Column LIKE "pattern"`, with `%` for any text and `_` for any
    /// character.
    pub fn like(&self, pattern: &str) -> Filter {
        self.op("LIKE", pattern.into())
    }

    /// `Column UNLIKE "pattern"`
    pub fn unlike(&self, pattern: &str) -> Filter {
        self.op("UNLIKE", pattern.into())
    }

    /// `Column ILIKE "pattern"`: like [`Column::like`], ignoring case.
    pub fn ilike(&self, pattern: &str) -> Filter {
        self.op("ILIKE", pattern.into())
    }

    /// `Column IUNLIKE "pattern"`
    pub fn iunlike(&self, pattern: &str) -> Filter {
        self.op("IUNLIKE", pattern.into())
    }

    /// `Column IN (a, b)`
    ///
    /// # Panics
    ///
    /// When there is no value.
    pub fn is_in<V: Into<Value>>(&self, values: impl IntoIterator<Item = V>) -> Filter {
        self.list("IN", values)
    }

    /// `Column NOTIN (a, b)`
    ///
    /// # Panics
    ///
    /// When there is no value.
    pub fn not_in<V: Into<Value>>(&self, values: impl IntoIterator<Item = V>) -> Filter {
        self.list("NOTIN", values)
    }

    /// `Column << subnet`: an address inside a subnet (`192.0.2.0/24`).
    pub fn within(&self, subnet: &str) -> Filter {
        self.op("<<", Value::raw(subnet))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(col("SrcAS").eq(15169u32).to_string(), "SrcAS = 15169");
        assert_eq!(col("ExporterName").eq("edge 1").to_string(), r#"ExporterName = "edge 1""#);
        assert_eq!(col("InIfName").ne(r#"a"b\c"#).to_string(), r#"InIfName != "a\"b\\c""#);
        assert_eq!(col("InIfName").eq("a\nb").to_string(), r#"InIfName = "a b""#);
        assert_eq!(col("SrcAddr").eq("2001:db8::1".parse::<IpAddr>().unwrap()).to_string(), "SrcAddr = 2001:db8::1");
        assert_eq!(col("SrcAddr").within("192.0.2.0/24").to_string(), "SrcAddr << 192.0.2.0/24");
        assert_eq!(col("SrcAS").eq(Value::column("DstAS")).to_string(), "SrcAS = DstAS");
        assert_eq!(col("DstPort").is_in([80u16, 443]).to_string(), "DstPort IN (80, 443)");
        assert_eq!(col("Proto").not_in(["tcp", "udp"]).to_string(), r#"Proto NOTIN ("tcp", "udp")"#);
        assert_eq!(col("Bytes").ge(1500).to_string(), "Bytes >= 1500");
    }

    #[test]
    fn precedence() {
        let a = || col("SrcAS").eq(1u8);
        let b = || col("DstAS").eq(2u8);
        let c = || col("Proto").eq("tcp");
        assert_eq!(a().and(b()).and(c()).to_string(), r#"SrcAS = 1 AND DstAS = 2 AND Proto = "tcp""#);
        assert_eq!(a().or(b()).and(c()).to_string(), r#"(SrcAS = 1 OR DstAS = 2) AND Proto = "tcp""#);
        assert_eq!(a().and(b()).or(c()).to_string(), r#"SrcAS = 1 AND DstAS = 2 OR Proto = "tcp""#);
        assert_eq!((!a().and(b())).to_string(), "NOT (SrcAS = 1 AND DstAS = 2)");
        assert_eq!((!!a()).to_string(), "NOT NOT SrcAS = 1");
        assert_eq!(
            Filter::parsed("SrcAS = 1 OR DstAS = 2").and(c()).to_string(),
            r#"(SrcAS = 1 OR DstAS = 2) AND Proto = "tcp""#
        );
    }

    #[test]
    fn empty() {
        assert_eq!(and([]).to_string(), "");
        assert_eq!(and([Filter::all(), col("SrcAS").eq(1u8)]).to_string(), "SrcAS = 1");
        assert!((!Filter::all()).is_all());
    }

    #[test]
    #[should_panic]
    fn bad_column() {
        col("SrcAS = 1 OR Dst");
    }
}
