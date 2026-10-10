// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Blocks of the native protocol and column values.
//!
//! A column is serialized in two parts: first the "state prefixes" of the
//! whole type tree (only LowCardinality has one), then the data. Values are
//! decoded into `Values`; LowCardinality columns are materialized.

use crate::io::*;
use crate::types::DataType;
use std::io::{self, Read};
use std::net::{Ipv4Addr, Ipv6Addr};

/// Values of a column.
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    UInt8(Vec<u8>),
    UInt16(Vec<u16>),
    UInt32(Vec<u32>),
    UInt64(Vec<u64>),
    UInt128(Vec<u128>),
    Int8(Vec<i8>),
    Int16(Vec<i16>),
    Int32(Vec<i32>),
    Int64(Vec<i64>),
    Int128(Vec<i128>),
    Float32(Vec<f32>),
    Float64(Vec<f64>),
    /// Strings, as bytes.
    String(Vec<Vec<u8>>),
    /// Fixed-size values (FixedString, IPv6, UUID, wide integers, decimals).
    Fixed(usize, Vec<u8>),
    /// Null map (true when null) and values.
    Nullable(Vec<bool>, Box<Values>),
    /// Cumulative offsets and values.
    Array(Vec<u64>, Box<Values>),
    Tuple(Vec<Values>),
    /// Cumulative offsets, keys and values.
    Map(Vec<u64>, Box<Values>, Box<Values>),
    Nothing(usize),
}

/// A single value, for convenient access to query results.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    UInt(u64),
    Int(i64),
    UInt128(u128),
    Int128(i128),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    IPv4(Ipv4Addr),
    IPv6(Ipv6Addr),
    /// Seconds since epoch.
    DateTime(u32),
    /// Ticks since epoch, with the precision.
    DateTime64(i64, u32),
    /// Days since epoch.
    Date(i32),
    Array(Vec<Value>),
    Tuple(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl Value {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt(v) => Some(*v),
            Value::Int(v) if *v >= 0 => Some(*v as u64),
            Value::DateTime(v) => Some(*v as u64),
            Value::Float(v) if *v >= 0.0 => Some(*v as u64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::UInt(v) => i64::try_from(*v).ok(),
            Value::DateTime(v) => Some(*v as i64),
            Value::Date(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::UInt(v) => Some(*v as f64),
            Value::Int(v) => Some(*v as f64),
            Value::UInt128(v) => Some(*v as f64),
            Value::Int128(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Text form, like ClickHouse prints it for simple types.
    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::UInt(v) => v.to_string(),
            Value::Int(v) => v.to_string(),
            Value::UInt128(v) => v.to_string(),
            Value::Int128(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::String(s) => s.clone(),
            Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            Value::IPv4(ip) => ip.to_string(),
            Value::IPv6(ip) => match ip.to_ipv4_mapped() {
                Some(v4) => format!("::ffff:{v4}"),
                None => ip.to_string(),
            },
            Value::DateTime(v) => v.to_string(),
            Value::DateTime64(v, _) => v.to_string(),
            Value::Date(v) => v.to_string(),
            Value::Array(a) | Value::Tuple(a) => {
                format!("[{}]", a.iter().map(|v| v.to_text()).collect::<Vec<_>>().join(","))
            }
            Value::Map(m) => format!(
                "{{{}}}",
                m.iter().map(|(k, v)| format!("{}:{}", k.to_text(), v.to_text())).collect::<Vec<_>>().join(",")
            ),
        }
    }
}

impl Values {
    /// Number of rows.
    pub fn len(&self) -> usize {
        match self {
            Values::UInt8(v) => v.len(),
            Values::UInt16(v) => v.len(),
            Values::UInt32(v) => v.len(),
            Values::UInt64(v) => v.len(),
            Values::UInt128(v) => v.len(),
            Values::Int8(v) => v.len(),
            Values::Int16(v) => v.len(),
            Values::Int32(v) => v.len(),
            Values::Int64(v) => v.len(),
            Values::Int128(v) => v.len(),
            Values::Float32(v) => v.len(),
            Values::Float64(v) => v.len(),
            Values::String(v) => v.len(),
            Values::Fixed(n, v) => v.len().checked_div(*n).unwrap_or(0),
            Values::Nullable(n, _) => n.len(),
            Values::Array(o, _) | Values::Map(o, _, _) => o.len(),
            Values::Tuple(v) => v.first().map(|x| x.len()).unwrap_or(0),
            Values::Nothing(n) => *n,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Empty values for a type.
    pub fn empty(t: &DataType) -> Values {
        match t {
            DataType::UInt8 | DataType::Bool => Values::UInt8(vec![]),
            DataType::UInt16 | DataType::Date => Values::UInt16(vec![]),
            DataType::UInt32 | DataType::DateTime(_) | DataType::IPv4 => Values::UInt32(vec![]),
            DataType::UInt64 => Values::UInt64(vec![]),
            DataType::UInt128 => Values::UInt128(vec![]),
            DataType::Int8 | DataType::Enum8(_) => Values::Int8(vec![]),
            DataType::Int16 | DataType::Enum16(_) => Values::Int16(vec![]),
            DataType::Int32 | DataType::Date32 => Values::Int32(vec![]),
            DataType::Int64 | DataType::DateTime64(..) => Values::Int64(vec![]),
            DataType::Int128 => Values::Int128(vec![]),
            DataType::Float32 => Values::Float32(vec![]),
            DataType::Float64 => Values::Float64(vec![]),
            DataType::String => Values::String(vec![]),
            DataType::Nullable(t) => Values::Nullable(vec![], Box::new(Values::empty(t))),
            DataType::LowCardinality(t) => Values::empty(t),
            DataType::Array(t) => Values::Array(vec![], Box::new(Values::empty(t))),
            DataType::Tuple(ts) => Values::Tuple(ts.iter().map(|(_, t)| Values::empty(t)).collect()),
            DataType::Map(k, v) => Values::Map(vec![], Box::new(Values::empty(k)), Box::new(Values::empty(v))),
            DataType::Nothing => Values::Nothing(0),
            t => Values::Fixed(t.fixed_size().unwrap_or(1), vec![]),
        }
    }

    /// Get one value.
    pub fn get(&self, t: &DataType, i: usize) -> Value {
        // LowCardinality columns are materialized: use the inner type.
        if let DataType::LowCardinality(inner) = t {
            return self.get(inner, i);
        }
        let start = |o: &[u64], i: usize| if i == 0 { 0 } else { o[i - 1] as usize };
        match (self, t) {
            (Values::UInt8(v), DataType::Bool) => Value::UInt(v[i] as u64),
            (Values::UInt8(v), _) => Value::UInt(v[i] as u64),
            (Values::UInt16(v), DataType::Date) => Value::Date(v[i] as i32),
            (Values::UInt16(v), _) => Value::UInt(v[i] as u64),
            (Values::UInt32(v), DataType::DateTime(_)) => Value::DateTime(v[i]),
            (Values::UInt32(v), DataType::IPv4) => Value::IPv4(Ipv4Addr::from(v[i])),
            (Values::UInt32(v), _) => Value::UInt(v[i] as u64),
            (Values::UInt64(v), _) => Value::UInt(v[i]),
            (Values::UInt128(v), _) => Value::UInt128(v[i]),
            (Values::Int8(v), DataType::Enum8(names)) => names
                .iter()
                .find(|(_, x)| *x == v[i])
                .map(|(n, _)| Value::String(n.clone()))
                .unwrap_or(Value::Int(v[i] as i64)),
            (Values::Int8(v), _) => Value::Int(v[i] as i64),
            (Values::Int16(v), DataType::Enum16(names)) => names
                .iter()
                .find(|(_, x)| *x == v[i])
                .map(|(n, _)| Value::String(n.clone()))
                .unwrap_or(Value::Int(v[i] as i64)),
            (Values::Int16(v), _) => Value::Int(v[i] as i64),
            (Values::Int32(v), DataType::Date32) => Value::Date(v[i]),
            (Values::Int32(v), _) => Value::Int(v[i] as i64),
            (Values::Int64(v), DataType::DateTime64(p, _)) => Value::DateTime64(v[i], *p),
            (Values::Int64(v), _) => Value::Int(v[i]),
            (Values::Int128(v), _) => Value::Int128(v[i]),
            (Values::Float32(v), _) => Value::Float(v[i] as f64),
            (Values::Float64(v), _) => Value::Float(v[i]),
            (Values::String(v), _) => match String::from_utf8(v[i].clone()) {
                Ok(s) => Value::String(s),
                Err(e) => Value::Bytes(e.into_bytes()),
            },
            (Values::Fixed(n, v), DataType::IPv6) => {
                Value::IPv6(Ipv6Addr::from(<[u8; 16]>::try_from(&v[i * n..(i + 1) * n]).unwrap()))
            }
            (Values::Fixed(n, v), DataType::FixedString(_)) => {
                let b = &v[i * n..(i + 1) * n];
                match std::str::from_utf8(b) {
                    Ok(s) => Value::String(s.trim_end_matches('\0').to_string()),
                    Err(_) => Value::Bytes(b.to_vec()),
                }
            }
            (Values::Fixed(n, v), _) => Value::Bytes(v[i * n..(i + 1) * n].to_vec()),
            (Values::Nullable(nulls, inner), DataType::Nullable(t)) => {
                if nulls[i] {
                    Value::Null
                } else {
                    inner.get(t, i)
                }
            }
            (Values::Array(o, inner), DataType::Array(t)) => {
                Value::Array((start(o, i)..o[i] as usize).map(|j| inner.get(t, j)).collect())
            }
            (Values::Tuple(vs), DataType::Tuple(ts)) => {
                Value::Tuple(vs.iter().zip(ts).map(|(v, (_, t))| v.get(t, i)).collect())
            }
            (Values::Map(o, ks, vs), DataType::Map(kt, vt)) => {
                Value::Map((start(o, i)..o[i] as usize).map(|j| (ks.get(kt, j), vs.get(vt, j))).collect())
            }
            _ => Value::Null,
        }
    }
}

/// A column of a block.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub values: Values,
}

/// A block of data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub columns: Vec<Column>,
    pub rows: usize,
}

impl Block {
    /// Index of a column by name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// One row as values.
    pub fn row(&self, i: usize) -> Vec<Value> {
        self.columns.iter().map(|c| c.values.get(&c.data_type, i)).collect()
    }
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Version of the LowCardinality serialization.
const LC_SHARED_DICTIONARIES_WITH_ADDITIONAL_KEYS: u64 = 1;
const LC_HAS_ADDITIONAL_KEYS: u64 = 1 << 9;
const LC_NEED_UPDATE_DICTIONARY: u64 = 1 << 10;

fn read_prefix(r: &mut dyn Read, t: &DataType) -> io::Result<()> {
    match t {
        DataType::LowCardinality(_) => {
            let version = read_u64(r)?;
            if version != LC_SHARED_DICTIONARIES_WITH_ADDITIONAL_KEYS {
                return Err(invalid(format!("unsupported LowCardinality version {version}")));
            }
            Ok(())
        }
        DataType::Nullable(t) | DataType::Array(t) => read_prefix(r, t),
        DataType::Tuple(ts) => ts.iter().try_for_each(|(_, t)| read_prefix(r, t)),
        DataType::Map(k, v) => {
            read_prefix(r, k)?;
            read_prefix(r, v)
        }
        _ => Ok(()),
    }
}

macro_rules! read_le {
    ($r:expr, $n:expr, $t:ty) => {{
        let size = std::mem::size_of::<$t>();
        let raw = read_vec($r, $n * size)?;
        raw.chunks_exact(size).map(|c| <$t>::from_le_bytes(c.try_into().unwrap())).collect::<Vec<$t>>()
    }};
}

fn read_offsets(r: &mut dyn Read, n: usize) -> io::Result<Vec<u64>> {
    let offsets = read_le!(r, n, u64);
    if offsets.windows(2).any(|w| w[0] > w[1]) {
        return Err(invalid("offsets are not increasing"));
    }
    Ok(offsets)
}

fn read_data(r: &mut dyn Read, t: &DataType, n: usize) -> io::Result<Values> {
    Ok(match t {
        DataType::UInt8 | DataType::Bool => Values::UInt8(read_vec(r, n)?),
        DataType::UInt16 | DataType::Date => Values::UInt16(read_le!(r, n, u16)),
        DataType::UInt32 | DataType::DateTime(_) | DataType::IPv4 => Values::UInt32(read_le!(r, n, u32)),
        DataType::UInt64 => Values::UInt64(read_le!(r, n, u64)),
        DataType::UInt128 => Values::UInt128(read_le!(r, n, u128)),
        DataType::Int8 | DataType::Enum8(_) => Values::Int8(read_vec(r, n)?.into_iter().map(|b| b as i8).collect()),
        DataType::Int16 | DataType::Enum16(_) => Values::Int16(read_le!(r, n, i16)),
        DataType::Int32 | DataType::Date32 => Values::Int32(read_le!(r, n, i32)),
        DataType::Int64 | DataType::DateTime64(..) => Values::Int64(read_le!(r, n, i64)),
        DataType::Int128 => Values::Int128(read_le!(r, n, i128)),
        DataType::Float32 => Values::Float32(read_le!(r, n, f32)),
        DataType::Float64 => Values::Float64(read_le!(r, n, f64)),
        DataType::String => Values::String((0..n).map(|_| read_bytes(r)).collect::<io::Result<_>>()?),
        DataType::Nothing => {
            read_vec(r, n)?;
            Values::Nothing(n)
        }
        DataType::Nullable(inner) => {
            let nulls = read_vec(r, n)?.into_iter().map(|b| b != 0).collect();
            Values::Nullable(nulls, Box::new(read_data(r, inner, n)?))
        }
        DataType::Array(inner) => {
            let offsets = read_offsets(r, n)?;
            let total = offsets.last().copied().unwrap_or(0) as usize;
            Values::Array(offsets, Box::new(read_data(r, inner, total)?))
        }
        DataType::Tuple(ts) => Values::Tuple(ts.iter().map(|(_, t)| read_data(r, t, n)).collect::<io::Result<_>>()?),
        DataType::Map(k, v) => {
            let offsets = read_offsets(r, n)?;
            let total = offsets.last().copied().unwrap_or(0) as usize;
            let keys = read_data(r, k, total)?;
            let values = read_data(r, v, total)?;
            Values::Map(offsets, Box::new(keys), Box::new(values))
        }
        DataType::LowCardinality(inner) => read_low_cardinality(r, inner, n)?,
        other => {
            let size = other.fixed_size().ok_or_else(|| invalid(format!("cannot decode {other}")))?;
            Values::Fixed(size, read_vec(r, n * size)?)
        }
    })
}

/// Pick rows from dictionary values.
fn take(dict: &Values, idx: &[usize]) -> Values {
    macro_rules! pick {
        ($v:expr, $variant:ident) => {
            Values::$variant(idx.iter().map(|&i| $v[i].clone()).collect())
        };
    }
    match dict {
        Values::UInt8(v) => pick!(v, UInt8),
        Values::UInt16(v) => pick!(v, UInt16),
        Values::UInt32(v) => pick!(v, UInt32),
        Values::UInt64(v) => pick!(v, UInt64),
        Values::UInt128(v) => pick!(v, UInt128),
        Values::Int8(v) => pick!(v, Int8),
        Values::Int16(v) => pick!(v, Int16),
        Values::Int32(v) => pick!(v, Int32),
        Values::Int64(v) => pick!(v, Int64),
        Values::Int128(v) => pick!(v, Int128),
        Values::Float32(v) => pick!(v, Float32),
        Values::Float64(v) => pick!(v, Float64),
        Values::String(v) => pick!(v, String),
        Values::Fixed(n, v) => {
            Values::Fixed(*n, idx.iter().flat_map(|&i| v[i * n..(i + 1) * n].iter().copied()).collect())
        }
        other => other.clone(),
    }
}

fn read_low_cardinality(r: &mut dyn Read, inner: &DataType, n: usize) -> io::Result<Values> {
    if n == 0 {
        return Ok(Values::empty(inner));
    }
    let flags = read_u64(r)?;
    if flags & LC_HAS_ADDITIONAL_KEYS == 0 {
        return Err(invalid("LowCardinality without additional keys is not supported"));
    }
    let key_width = 1usize << (flags & 0xff);
    // For Nullable, the dictionary has the nested type and index 0 is NULL.
    let (dict_type, nullable) = match inner {
        DataType::Nullable(t) => (t.as_ref(), true),
        t => (t, false),
    };
    let dict_size = read_u64(r)? as usize;
    let dict = read_data(r, dict_type, dict_size)?;
    let rows = read_u64(r)? as usize;
    if rows != n {
        return Err(invalid(format!("LowCardinality has {rows} rows instead of {n}")));
    }
    let raw = read_vec(r, rows * key_width)?;
    let idx: Vec<usize> = raw
        .chunks_exact(key_width)
        .map(|c| {
            let mut b = [0u8; 8];
            b[..key_width].copy_from_slice(c);
            u64::from_le_bytes(b) as usize
        })
        .collect();
    if idx.iter().any(|&i| i >= dict_size) {
        return Err(invalid("LowCardinality index out of range"));
    }
    let values = take(&dict, &idx);
    if nullable {
        let nulls = idx.iter().map(|&i| i == 0).collect();
        Ok(Values::Nullable(nulls, Box::new(values)))
    } else {
        Ok(values)
    }
}

/// Read a block (block info, columns). `with_custom` is set when the
/// revision has the custom serialization flag.
pub fn read_block(r: &mut dyn Read, with_custom: bool) -> io::Result<Block> {
    // Block info: field 1 (is_overflows), field 2 (bucket_num), 0.
    loop {
        match read_uvarint(r)? {
            0 => break,
            1 => {
                read_u8(r)?;
            }
            2 => {
                read_i32(r)?;
            }
            f => return Err(invalid(format!("unknown block info field {f}"))),
        }
    }
    let ncols = read_uvarint(r)? as usize;
    let rows = read_uvarint(r)? as usize;
    let mut columns = Vec::with_capacity(ncols);
    for _ in 0..ncols {
        let name = read_string(r)?;
        let type_name = read_string(r)?;
        let data_type = DataType::parse(&type_name).map_err(invalid)?;
        if with_custom && read_u8(r)? != 0 {
            return Err(invalid("custom serialization is not supported"));
        }
        let values = if rows > 0 {
            read_prefix(r, &data_type)?;
            read_data(r, &data_type, rows)?
        } else {
            Values::empty(&data_type)
        };
        columns.push(Column { name, data_type, values });
    }
    Ok(Block { columns, rows })
}

/// Write the block header (block info, number of columns and rows).
pub fn write_block_header(out: &mut Vec<u8>, ncols: usize, rows: usize) {
    put_uvarint(out, 1);
    out.push(0);
    put_uvarint(out, 2);
    out.extend_from_slice(&(-1i32).to_le_bytes());
    put_uvarint(out, 0);
    put_uvarint(out, ncols as u64);
    put_uvarint(out, rows as u64);
}

/// Write a column header (name, type, custom serialization flag).
pub fn write_column_header(out: &mut Vec<u8>, name: &str, data_type: &str, with_custom: bool) {
    put_str(out, name.as_bytes());
    put_str(out, data_type.as_bytes());
    if with_custom {
        out.push(0);
    }
}

fn write_prefix(out: &mut Vec<u8>, t: &DataType) {
    match t {
        DataType::LowCardinality(_) => {
            out.extend_from_slice(&LC_SHARED_DICTIONARIES_WITH_ADDITIONAL_KEYS.to_le_bytes())
        }
        DataType::Nullable(t) | DataType::Array(t) => write_prefix(out, t),
        DataType::Tuple(ts) => ts.iter().for_each(|(_, t)| write_prefix(out, t)),
        DataType::Map(k, v) => {
            write_prefix(out, k);
            write_prefix(out, v);
        }
        _ => {}
    }
}

fn write_data(out: &mut Vec<u8>, t: &DataType, v: &Values) -> Result<(), String> {
    macro_rules! le {
        ($v:expr) => {
            $v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()))
        };
    }
    match (t, v) {
        (DataType::LowCardinality(inner), v) => write_low_cardinality(out, inner, v)?,
        (_, Values::UInt8(v)) => out.extend_from_slice(v),
        (_, Values::UInt16(v)) => le!(v),
        (_, Values::UInt32(v)) => le!(v),
        (_, Values::UInt64(v)) => le!(v),
        (_, Values::UInt128(v)) => le!(v),
        (_, Values::Int8(v)) => out.extend(v.iter().map(|x| *x as u8)),
        (_, Values::Int16(v)) => le!(v),
        (_, Values::Int32(v)) => le!(v),
        (_, Values::Int64(v)) => le!(v),
        (_, Values::Int128(v)) => le!(v),
        (_, Values::Float32(v)) => le!(v),
        (_, Values::Float64(v)) => le!(v),
        (_, Values::String(v)) => v.iter().for_each(|s| put_str(out, s)),
        (_, Values::Fixed(_, v)) => out.extend_from_slice(v),
        (_, Values::Nothing(n)) => out.extend(std::iter::repeat_n(0u8, *n)),
        (DataType::Nullable(t), Values::Nullable(nulls, inner)) => {
            out.extend(nulls.iter().map(|b| *b as u8));
            write_data(out, t, inner)?;
        }
        (DataType::Array(t), Values::Array(o, inner)) => {
            le!(o);
            write_data(out, t, inner)?;
        }
        (DataType::Tuple(ts), Values::Tuple(vs)) => {
            for ((_, t), v) in ts.iter().zip(vs) {
                write_data(out, t, v)?;
            }
        }
        (DataType::Map(kt, vt), Values::Map(o, ks, vs)) => {
            le!(o);
            write_data(out, kt, ks)?;
            write_data(out, vt, vs)?;
        }
        (t, _) => return Err(format!("values do not match type {t}")),
    }
    Ok(())
}

/// Encode LowCardinality values: a dictionary of unique values, then
/// indexes. For Nullable, index 0 is NULL.
fn write_low_cardinality(out: &mut Vec<u8>, inner: &DataType, v: &Values) -> Result<(), String> {
    use std::collections::HashMap;
    let n = v.len();
    if n == 0 {
        return Ok(());
    }
    let (nulls, values, dict_type) = match (inner, v) {
        (DataType::Nullable(t), Values::Nullable(nulls, inner)) => (Some(nulls), inner.as_ref(), t.as_ref()),
        (t, v) => (None, v, t),
    };
    // Build the dictionary using the encoded form of each value as the key.
    let mut dict_keys: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut dict = Vec::new();
    let mut idx = Vec::with_capacity(n);
    let encode_one = |i: usize| -> Vec<u8> {
        let mut b = Vec::new();
        let one = take(values, &[i]);
        let _ = write_data(&mut b, dict_type, &one);
        b
    };
    if nulls.is_some() {
        // Index 0 is NULL: put a default value there.
        let mut b = Vec::new();
        let _ = write_data(&mut b, dict_type, &default_one(dict_type));
        dict_keys.insert(Vec::new(), 0);
        dict.push(b);
    }
    for i in 0..n {
        if nulls.is_some_and(|nl| nl[i]) {
            idx.push(0);
            continue;
        }
        let key = encode_one(i);
        let next = dict.len();
        let pos = *dict_keys.entry(key.clone()).or_insert(next);
        if pos == next {
            dict.push(key);
        }
        idx.push(pos);
    }
    let key_type: u64 = match dict.len() {
        0..=0xff => 0,
        0x100..=0xffff => 1,
        0x10000..=0xffff_ffff => 2,
        _ => 3,
    };
    out.extend_from_slice(&(key_type | LC_HAS_ADDITIONAL_KEYS | LC_NEED_UPDATE_DICTIONARY).to_le_bytes());
    out.extend_from_slice(&(dict.len() as u64).to_le_bytes());
    for d in &dict {
        out.extend_from_slice(d);
    }
    out.extend_from_slice(&(n as u64).to_le_bytes());
    for i in idx {
        match key_type {
            0 => out.push(i as u8),
            1 => out.extend_from_slice(&(i as u16).to_le_bytes()),
            2 => out.extend_from_slice(&(i as u32).to_le_bytes()),
            _ => out.extend_from_slice(&(i as u64).to_le_bytes()),
        }
    }
    Ok(())
}

fn default_one(t: &DataType) -> Values {
    match t {
        DataType::String => Values::String(vec![vec![]]),
        t => match Values::empty(t) {
            Values::Fixed(n, _) => Values::Fixed(n, vec![0; n]),
            Values::UInt8(_) => Values::UInt8(vec![0]),
            Values::UInt16(_) => Values::UInt16(vec![0]),
            Values::UInt32(_) => Values::UInt32(vec![0]),
            Values::UInt64(_) => Values::UInt64(vec![0]),
            Values::Int32(_) => Values::Int32(vec![0]),
            Values::Int64(_) => Values::Int64(vec![0]),
            Values::Float64(_) => Values::Float64(vec![0.0]),
            other => other,
        },
    }
}

/// Write a whole block.
pub fn write_block(out: &mut Vec<u8>, block: &Block, with_custom: bool) -> Result<(), String> {
    write_block_header(out, block.columns.len(), block.rows);
    for c in &block.columns {
        write_column_header(out, &c.name, &c.data_type.to_string(), with_custom);
        if block.rows > 0 {
            write_prefix(out, &c.data_type);
            write_data(out, &c.data_type, &c.values)?;
        }
    }
    Ok(())
}

/// Encode the data of a LowCardinality(String) column from raw strings,
/// prefix included. Used by the outlet, which keeps strings contiguous.
pub fn write_low_cardinality_strings(out: &mut Vec<u8>, data: &[u8], offsets: &[usize]) {
    use std::collections::HashMap;
    out.extend_from_slice(&LC_SHARED_DICTIONARIES_WITH_ADDITIONAL_KEYS.to_le_bytes());
    let n = offsets.len();
    if n == 0 {
        return;
    }
    let mut dict: HashMap<&[u8], usize> = HashMap::new();
    let mut order: Vec<&[u8]> = Vec::new();
    let mut idx = Vec::with_capacity(n);
    let mut start = 0;
    for &end in offsets {
        let s = &data[start..end];
        start = end;
        let next = order.len();
        let pos = *dict.entry(s).or_insert(next);
        if pos == next {
            order.push(s);
        }
        idx.push(pos);
    }
    write_lc_indexes(out, order.len(), |out| order.iter().for_each(|s| put_str(out, s)), &idx);
}

/// Encode the data of a LowCardinality(IPv6) column, prefix included.
pub fn write_low_cardinality_ipv6(out: &mut Vec<u8>, values: &[[u8; 16]]) {
    use std::collections::HashMap;
    out.extend_from_slice(&LC_SHARED_DICTIONARIES_WITH_ADDITIONAL_KEYS.to_le_bytes());
    if values.is_empty() {
        return;
    }
    let mut dict: HashMap<&[u8; 16], usize> = HashMap::new();
    let mut order: Vec<&[u8; 16]> = Vec::new();
    let mut idx = Vec::with_capacity(values.len());
    for v in values {
        let next = order.len();
        let pos = *dict.entry(v).or_insert(next);
        if pos == next {
            order.push(v);
        }
        idx.push(pos);
    }
    write_lc_indexes(out, order.len(), |out| order.iter().for_each(|v| out.extend_from_slice(*v)), &idx);
}

fn write_lc_indexes(out: &mut Vec<u8>, dict_len: usize, write_dict: impl FnOnce(&mut Vec<u8>), idx: &[usize]) {
    let key_type: u64 = match dict_len {
        0..=0xff => 0,
        0x100..=0xffff => 1,
        _ => 2,
    };
    out.extend_from_slice(&(key_type | LC_HAS_ADDITIONAL_KEYS | LC_NEED_UPDATE_DICTIONARY).to_le_bytes());
    out.extend_from_slice(&(dict_len as u64).to_le_bytes());
    write_dict(out);
    out.extend_from_slice(&(idx.len() as u64).to_le_bytes());
    for &i in idx {
        match key_type {
            0 => out.push(i as u8),
            1 => out.extend_from_slice(&(i as u16).to_le_bytes()),
            _ => out.extend_from_slice(&(i as u32).to_le_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(block: Block) {
        let mut out = Vec::new();
        write_block(&mut out, &block, true).unwrap();
        let mut r: &[u8] = &out;
        let got = read_block(&mut r, true).unwrap();
        assert_eq!(got, block);
        assert!(r.is_empty());
    }

    #[test]
    fn blocks() {
        let s = |v: &[&str]| Values::String(v.iter().map(|s| s.as_bytes().to_vec()).collect());
        roundtrip(Block {
            rows: 3,
            columns: vec![
                Column { name: "a".into(), data_type: DataType::UInt64, values: Values::UInt64(vec![1, 2, 3]) },
                Column {
                    name: "b".into(),
                    data_type: DataType::parse("LowCardinality(String)").unwrap(),
                    values: s(&["x", "y", "x"]),
                },
                Column {
                    name: "c".into(),
                    data_type: DataType::parse("Array(LowCardinality(String))").unwrap(),
                    values: Values::Array(vec![1, 1, 3], Box::new(s(&["p", "q", "p"]))),
                },
                Column {
                    name: "d".into(),
                    data_type: DataType::parse("Nullable(Int32)").unwrap(),
                    values: Values::Nullable(vec![false, true, false], Box::new(Values::Int32(vec![-1, 0, 5]))),
                },
                Column {
                    name: "e".into(),
                    data_type: DataType::parse("Map(String, UInt8)").unwrap(),
                    values: Values::Map(
                        vec![2, 2, 3],
                        Box::new(s(&["k1", "k2", "k3"])),
                        Box::new(Values::UInt8(vec![1, 2, 3])),
                    ),
                },
                Column {
                    name: "f".into(),
                    data_type: DataType::IPv6,
                    values: Values::Fixed(16, [[0u8; 16], [1u8; 16], [2u8; 16]].concat()),
                },
            ],
        });
    }

    #[test]
    fn values() {
        let t = DataType::parse("Enum8('a' = 1, 'b' = 2)").unwrap();
        assert_eq!(Values::Int8(vec![2]).get(&t, 0), Value::String("b".into()));
        let t = DataType::parse("Array(UInt32)").unwrap();
        let v = Values::Array(vec![2, 3], Box::new(Values::UInt32(vec![1, 2, 3])));
        assert_eq!(v.get(&t, 1), Value::Array(vec![Value::UInt(3)]));
    }

    #[test]
    fn lc_strings() {
        let mut a = Vec::new();
        write_low_cardinality_strings(&mut a, b"xyx", &[1, 2, 3]);
        let mut b = Vec::new();
        let t = DataType::parse("LowCardinality(String)").unwrap();
        write_prefix(&mut b, &t);
        write_data(&mut b, &t, &Values::String(vec![b"x".to_vec(), b"y".to_vec(), b"x".to_vec()])).unwrap();
        assert_eq!(a, b);
    }
}
