// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! ClickHouse data types, parsed from their textual form
//! ("Array(LowCardinality(String))").

use std::fmt;

/// A ClickHouse data type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataType {
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    UInt128,
    UInt256,
    Int8,
    Int16,
    Int32,
    Int64,
    Int128,
    Int256,
    Float32,
    Float64,
    Bool,
    String,
    FixedString(usize),
    Date,
    Date32,
    /// Optional timezone.
    DateTime(Option<String>),
    /// Precision and optional timezone.
    DateTime64(u32, Option<String>),
    Uuid,
    IPv4,
    IPv6,
    /// Values and names.
    Enum8(Vec<(String, i8)>),
    Enum16(Vec<(String, i16)>),
    /// Precision and scale.
    Decimal(u32, u32),
    Nullable(Box<DataType>),
    LowCardinality(Box<DataType>),
    Array(Box<DataType>),
    /// Element types, with optional names.
    Tuple(Vec<(Option<String>, DataType)>),
    Map(Box<DataType>, Box<DataType>),
    Nothing,
}

impl DataType {
    /// Parse a type.
    pub fn parse(s: &str) -> Result<DataType, String> {
        let mut p = Parser { s: s.as_bytes(), pos: 0 };
        let t = p.data_type()?;
        p.ws();
        if p.pos != p.s.len() {
            return Err(format!("unexpected trailing data in type {s:?}"));
        }
        Ok(t)
    }

    /// Size in bytes of a fixed-size value, if any.
    pub fn fixed_size(&self) -> Option<usize> {
        Some(match self {
            DataType::UInt8 | DataType::Int8 | DataType::Bool | DataType::Enum8(_) => 1,
            DataType::UInt16 | DataType::Int16 | DataType::Date | DataType::Enum16(_) => 2,
            DataType::UInt32 | DataType::Int32 | DataType::Float32 | DataType::Date32 => 4,
            DataType::DateTime(_) | DataType::IPv4 => 4,
            DataType::UInt64 | DataType::Int64 | DataType::Float64 | DataType::DateTime64(..) => 8,
            DataType::UInt128 | DataType::Int128 | DataType::Uuid | DataType::IPv6 => 16,
            DataType::UInt256 | DataType::Int256 => 32,
            DataType::FixedString(n) => *n,
            DataType::Decimal(p, _) => match p {
                0..=9 => 4,
                10..=18 => 8,
                19..=38 => 16,
                _ => 32,
            },
            _ => return None,
        })
    }

    /// Type without LowCardinality, recursively.
    pub fn without_low_cardinality(&self) -> DataType {
        match self {
            DataType::LowCardinality(t) => t.without_low_cardinality(),
            DataType::Nullable(t) => DataType::Nullable(Box::new(t.without_low_cardinality())),
            DataType::Array(t) => DataType::Array(Box::new(t.without_low_cardinality())),
            DataType::Tuple(ts) => {
                DataType::Tuple(ts.iter().map(|(n, t)| (n.clone(), t.without_low_cardinality())).collect())
            }
            DataType::Map(k, v) => {
                DataType::Map(Box::new(k.without_low_cardinality()), Box::new(v.without_low_cardinality()))
            }
            t => t.clone(),
        }
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::UInt8 => f.write_str("UInt8"),
            DataType::UInt16 => f.write_str("UInt16"),
            DataType::UInt32 => f.write_str("UInt32"),
            DataType::UInt64 => f.write_str("UInt64"),
            DataType::UInt128 => f.write_str("UInt128"),
            DataType::UInt256 => f.write_str("UInt256"),
            DataType::Int8 => f.write_str("Int8"),
            DataType::Int16 => f.write_str("Int16"),
            DataType::Int32 => f.write_str("Int32"),
            DataType::Int64 => f.write_str("Int64"),
            DataType::Int128 => f.write_str("Int128"),
            DataType::Int256 => f.write_str("Int256"),
            DataType::Float32 => f.write_str("Float32"),
            DataType::Float64 => f.write_str("Float64"),
            DataType::Bool => f.write_str("Bool"),
            DataType::String => f.write_str("String"),
            DataType::FixedString(n) => write!(f, "FixedString({n})"),
            DataType::Date => f.write_str("Date"),
            DataType::Date32 => f.write_str("Date32"),
            DataType::DateTime(None) => f.write_str("DateTime"),
            DataType::DateTime(Some(tz)) => write!(f, "DateTime({})", quote(tz)),
            DataType::DateTime64(p, None) => write!(f, "DateTime64({p})"),
            DataType::DateTime64(p, Some(tz)) => write!(f, "DateTime64({p}, {})", quote(tz)),
            DataType::Uuid => f.write_str("UUID"),
            DataType::IPv4 => f.write_str("IPv4"),
            DataType::IPv6 => f.write_str("IPv6"),
            DataType::Enum8(v) => {
                let items: Vec<String> = v.iter().map(|(n, x)| format!("{} = {x}", quote(n))).collect();
                write!(f, "Enum8({})", items.join(", "))
            }
            DataType::Enum16(v) => {
                let items: Vec<String> = v.iter().map(|(n, x)| format!("{} = {x}", quote(n))).collect();
                write!(f, "Enum16({})", items.join(", "))
            }
            DataType::Decimal(p, s) => write!(f, "Decimal({p}, {s})"),
            DataType::Nullable(t) => write!(f, "Nullable({t})"),
            DataType::LowCardinality(t) => write!(f, "LowCardinality({t})"),
            DataType::Array(t) => write!(f, "Array({t})"),
            DataType::Tuple(ts) => {
                let items: Vec<String> = ts
                    .iter()
                    .map(|(n, t)| match n {
                        Some(n) => format!("{n} {t}"),
                        None => t.to_string(),
                    })
                    .collect();
                write!(f, "Tuple({})", items.join(", "))
            }
            DataType::Map(k, v) => write!(f, "Map({k}, {v})"),
            DataType::Nothing => f.write_str("Nothing"),
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.pos < self.s.len() && self.s[self.pos] == c {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.eat(c) { Ok(()) } else { Err(format!("expected {:?} at position {}", c as char, self.pos)) }
    }

    fn ident(&mut self) -> String {
        self.ws();
        let start = self.pos;
        while self.pos < self.s.len() && (self.s[self.pos].is_ascii_alphanumeric() || self.s[self.pos] == b'_') {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.pos]).into_owned()
    }

    fn number(&mut self) -> Result<i64, String> {
        self.ws();
        let start = self.pos;
        if self.pos < self.s.len() && self.s[self.pos] == b'-' {
            self.pos += 1;
        }
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        std::str::from_utf8(&self.s[start..self.pos])
            .unwrap()
            .parse()
            .map_err(|_| format!("expected a number at position {start}"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.ws();
        if !self.eat(b'\'') {
            return Err(format!("expected a string at position {}", self.pos));
        }
        let mut out = Vec::new();
        while self.pos < self.s.len() {
            let c = self.s[self.pos];
            self.pos += 1;
            match c {
                b'\\' if self.pos < self.s.len() => {
                    out.push(self.s[self.pos]);
                    self.pos += 1;
                }
                b'\'' => return Ok(String::from_utf8_lossy(&out).into_owned()),
                c => out.push(c),
            }
        }
        Err("unterminated string".into())
    }

    fn enum_values<T: TryFrom<i64>>(&mut self) -> Result<Vec<(String, T)>, String> {
        self.expect(b'(')?;
        let mut values = Vec::new();
        loop {
            let name = self.string()?;
            self.expect(b'=')?;
            let v = self.number()?;
            values.push((name, T::try_from(v).map_err(|_| format!("enum value {v} out of range"))?));
            if self.eat(b')') {
                return Ok(values);
            }
            self.expect(b',')?;
        }
    }

    fn data_type(&mut self) -> Result<DataType, String> {
        let name = self.ident();
        Ok(match name.as_str() {
            "UInt8" => DataType::UInt8,
            "UInt16" => DataType::UInt16,
            "UInt32" => DataType::UInt32,
            "UInt64" => DataType::UInt64,
            "UInt128" => DataType::UInt128,
            "UInt256" => DataType::UInt256,
            "Int8" => DataType::Int8,
            "Int16" => DataType::Int16,
            "Int32" => DataType::Int32,
            "Int64" => DataType::Int64,
            "Int128" => DataType::Int128,
            "Int256" => DataType::Int256,
            "Float32" => DataType::Float32,
            "Float64" => DataType::Float64,
            "Bool" => DataType::Bool,
            "String" => DataType::String,
            "UUID" => DataType::Uuid,
            "IPv4" => DataType::IPv4,
            "IPv6" => DataType::IPv6,
            "Date" => DataType::Date,
            "Date32" => DataType::Date32,
            "Nothing" => DataType::Nothing,
            "FixedString" => {
                self.expect(b'(')?;
                let n = self.number()?;
                self.expect(b')')?;
                DataType::FixedString(n as usize)
            }
            "DateTime" => {
                if self.eat(b'(') {
                    let tz = self.string()?;
                    self.expect(b')')?;
                    DataType::DateTime(Some(tz))
                } else {
                    DataType::DateTime(None)
                }
            }
            "DateTime64" => {
                self.expect(b'(')?;
                let p = self.number()? as u32;
                let tz = if self.eat(b',') { Some(self.string()?) } else { None };
                self.expect(b')')?;
                DataType::DateTime64(p, tz)
            }
            "Enum8" => DataType::Enum8(self.enum_values()?),
            "Enum16" => DataType::Enum16(self.enum_values()?),
            "Decimal" => {
                self.expect(b'(')?;
                let p = self.number()? as u32;
                self.expect(b',')?;
                let s = self.number()? as u32;
                self.expect(b')')?;
                DataType::Decimal(p, s)
            }
            "Decimal32" | "Decimal64" | "Decimal128" | "Decimal256" => {
                self.expect(b'(')?;
                let s = self.number()? as u32;
                self.expect(b')')?;
                let p = match name.as_str() {
                    "Decimal32" => 9,
                    "Decimal64" => 18,
                    "Decimal128" => 38,
                    _ => 76,
                };
                DataType::Decimal(p, s)
            }
            "Nullable" | "LowCardinality" | "Array" => {
                self.expect(b'(')?;
                let inner = Box::new(self.data_type()?);
                self.expect(b')')?;
                match name.as_str() {
                    "Nullable" => DataType::Nullable(inner),
                    "LowCardinality" => DataType::LowCardinality(inner),
                    _ => DataType::Array(inner),
                }
            }
            "Map" => {
                self.expect(b'(')?;
                let k = Box::new(self.data_type()?);
                self.expect(b',')?;
                let v = Box::new(self.data_type()?);
                self.expect(b')')?;
                DataType::Map(k, v)
            }
            "Tuple" => {
                self.expect(b'(')?;
                let mut items = Vec::new();
                loop {
                    // Named element: "name Type".
                    let save = self.pos;
                    let first = self.ident();
                    self.ws();
                    let named = !first.is_empty() && self.pos < self.s.len() && self.s[self.pos].is_ascii_alphabetic();
                    self.pos = save;
                    let n = if named { Some(self.ident()) } else { None };
                    items.push((n, self.data_type()?));
                    if self.eat(b')') {
                        break;
                    }
                    self.expect(b',')?;
                }
                DataType::Tuple(items)
            }
            "SimpleAggregateFunction" => {
                // SimpleAggregateFunction(func, Type) is stored as Type.
                self.expect(b'(')?;
                self.ident();
                self.expect(b',')?;
                let t = self.data_type()?;
                self.expect(b')')?;
                t
            }
            other => return Err(format!("unsupported type {other:?}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for t in [
            "UInt64",
            "LowCardinality(String)",
            "LowCardinality(Nullable(String))",
            "Array(UInt32)",
            "Array(UInt128)",
            "FixedString(2)",
            "DateTime",
            "DateTime('UTC')",
            "DateTime64(3, 'Europe/Paris')",
            "Enum8('undefined' = 0, 'external' = 1, 'internal' = 2)",
            "Map(String, UInt64)",
            "Tuple(String, UInt64)",
            "Tuple(a String, b Array(Nullable(IPv6)))",
            "Decimal(10, 2)",
        ] {
            assert_eq!(DataType::parse(t).unwrap().to_string(), t, "{t}");
        }
        assert_eq!(DataType::parse("SimpleAggregateFunction(max, UInt64)").unwrap(), DataType::UInt64);
        assert!(DataType::parse("Unknown").is_err());
    }
}
