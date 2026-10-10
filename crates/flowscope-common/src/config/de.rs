// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A serde deserializer over a configuration tree reproducing the behavior of
//! Go's mapstructure as configured by Akvorado:
//!
//! - struct field names match keys ignoring case, dashes and underscores
//!   (`max-batch-size`, `maxbatchsize` and `MaxBatchSize` are the same);
//! - keys starting with a dot are ignored (they hold YAML anchors);
//! - input is weakly typed: "10" is a number, 1 is true, a scalar is a list of
//!   one element, "a,b" is a list of two elements, an empty list is an empty
//!   map.
//!
//! Errors carry the path of the offending value.

use serde::de::{self, DeserializeSeed, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Error while decoding a configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub path: String,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

impl std::error::Error for Error {}

impl de::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error { path: String::new(), message: msg.to_string() }
    }

    fn unknown_field(field: &str, _expected: &'static [&'static str]) -> Self {
        Error { path: String::new(), message: format!("invalid key {field:?}") }
    }
}

/// Normalize a key for comparison with a struct field name.
pub fn normalize_key(key: &str) -> String {
    key.chars().filter(|c| *c != '-' && *c != '_').flat_map(|c| c.to_lowercase()).collect()
}

/// Deserialize a configuration value.
pub fn from_value<'de, T: de::Deserialize<'de>>(value: Value) -> Result<T, Error> {
    T::deserialize(Deserializer::new(value, String::new()))
}

/// Deserializer over a configuration value.
pub struct Deserializer {
    value: Value,
    path: String,
}

impl Deserializer {
    pub fn new(value: Value, path: String) -> Self {
        Deserializer { value, path }
    }

    fn error(&self, message: impl fmt::Display) -> Error {
        Error { path: self.path.clone(), message: message.to_string() }
    }

    fn wrap(&self, mut e: Error) -> Error {
        if e.path.is_empty() {
            e.path = self.path.clone();
        }
        e
    }

    fn type_name(&self) -> &'static str {
        match &self.value {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "list",
            Value::Object(_) => "map",
        }
    }

    fn as_i128(&self) -> Result<i128, Error> {
        match &self.value {
            Value::Number(n) => {
                if let Some(v) = n.as_i64() {
                    Ok(v as i128)
                } else if let Some(v) = n.as_u64() {
                    Ok(v as i128)
                } else {
                    let f = n.as_f64().unwrap();
                    if f.fract() == 0.0 {
                        Ok(f as i128)
                    } else {
                        Err(self.error(format!("expected an integer, got {f}")))
                    }
                }
            }
            Value::Bool(b) => Ok(*b as i128),
            Value::String(s) => parse_int(s).ok_or_else(|| self.error(format!("cannot parse {s:?} as an integer"))),
            Value::Null => Ok(0),
            _ => Err(self.error(format!("expected an integer, got a {}", self.type_name()))),
        }
    }

    fn as_f64(&self) -> Result<f64, Error> {
        match &self.value {
            Value::Number(n) => Ok(n.as_f64().unwrap()),
            Value::Bool(b) => Ok(*b as u8 as f64),
            Value::String(s) => s.trim().parse().map_err(|_| self.error(format!("cannot parse {s:?} as a number"))),
            Value::Null => Ok(0.0),
            _ => Err(self.error(format!("expected a number, got a {}", self.type_name()))),
        }
    }
}

/// Parse an integer like Go's strconv.ParseInt with base 0.
fn parse_int(s: &str) -> Option<i128> {
    let s = s.trim().replace('_', "");
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, s.strip_prefix('+').unwrap_or(&s).to_string()),
    };
    let v = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i128::from_str_radix(h, 16).ok()?
    } else if let Some(o) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
        i128::from_str_radix(o, 8).ok()?
    } else if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        i128::from_str_radix(b, 2).ok()?
    } else {
        s.parse().ok()?
    };
    Some(if neg { -v } else { v })
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" | "" => Some(false),
        _ => None,
    }
}

macro_rules! deserialize_int {
    ($name:ident, $visit:ident, $t:ty) => {
        fn $name<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            let v = self.as_i128()?;
            let v = <$t>::try_from(v).map_err(|_| self.error(format!("{v} is out of range")))?;
            visitor.$visit(v).map_err(|e| self.wrap(e))
        }
    };
}

impl<'de> de::Deserializer<'de> for Deserializer {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let path = self.path.clone();
        let r = match self.value {
            Value::Null => visitor.visit_unit(),
            Value::Bool(b) => visitor.visit_bool(b),
            Value::Number(n) => {
                if let Some(v) = n.as_u64() {
                    visitor.visit_u64(v)
                } else if let Some(v) = n.as_i64() {
                    visitor.visit_i64(v)
                } else {
                    visitor.visit_f64(n.as_f64().unwrap())
                }
            }
            Value::String(s) => visitor.visit_string(s),
            Value::Array(a) => visitor.visit_seq(SeqDe::new(a, path.clone())),
            Value::Object(m) => visitor.visit_map(MapDe::new(m, path.clone(), None)),
        };
        r.map_err(|mut e| {
            if e.path.is_empty() {
                e.path = path;
            }
            e
        })
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let b = match &self.value {
            Value::Bool(b) => *b,
            Value::Number(_) => self.as_i128()? != 0,
            Value::String(s) => parse_bool(s).ok_or_else(|| self.error(format!("cannot parse {s:?} as a boolean")))?,
            Value::Null => false,
            _ => return Err(self.error(format!("expected a boolean, got a {}", self.type_name()))),
        };
        visitor.visit_bool(b).map_err(|e| self.wrap(e))
    }

    deserialize_int!(deserialize_i8, visit_i8, i8);
    deserialize_int!(deserialize_i16, visit_i16, i16);
    deserialize_int!(deserialize_i32, visit_i32, i32);
    deserialize_int!(deserialize_i64, visit_i64, i64);
    deserialize_int!(deserialize_u8, visit_u8, u8);
    deserialize_int!(deserialize_u16, visit_u16, u16);
    deserialize_int!(deserialize_u32, visit_u32, u32);
    deserialize_int!(deserialize_u64, visit_u64, u64);

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let v = self.as_f64()?;
        visitor.visit_f32(v as f32).map_err(|e| self.wrap(e))
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let v = self.as_f64()?;
        visitor.visit_f64(v).map_err(|e| self.wrap(e))
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let s = match &self.value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Null => String::new(),
            _ => return Err(self.error(format!("expected a string, got a {}", self.type_name()))),
        };
        visitor.visit_string(s).map_err(|e| self.wrap(e))
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match &self.value {
            Value::String(s) => visitor.visit_byte_buf(s.clone().into_bytes()).map_err(|e| self.wrap(e)),
            _ => self.deserialize_seq(visitor),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.value {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let path = self.path.clone();
        let items = match self.value {
            Value::Array(a) => a,
            Value::Null => vec![],
            // StringToSliceHookFunc(","): an empty string is an empty list.
            Value::String(s) if s.is_empty() => vec![],
            Value::String(s) => s.split(',').map(|x| Value::String(x.to_string())).collect(),
            // Weak typing: a single value becomes a list.
            other => vec![other],
        };
        visitor.visit_seq(SeqDe::new(items, path.clone())).map_err(|mut e| {
            if e.path.is_empty() {
                e.path = path;
            }
            e
        })
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let path = self.path.clone();
        let map = match self.value {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            Value::Array(a) if a.is_empty() => Map::new(),
            _ => return Err(self.error(format!("expected a map, got a {}", self.type_name()))),
        };
        visitor.visit_map(MapDe::new(map, path.clone(), None)).map_err(|mut e| {
            if e.path.is_empty() {
                e.path = path;
            }
            e
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let path = self.path.clone();
        let map = match self.value {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            Value::Array(a) if a.is_empty() => Map::new(),
            _ => return Err(self.error(format!("expected a map, got a {}", self.type_name()))),
        };
        visitor.visit_map(MapDe::new(map, path.clone(), Some(fields))).map_err(|mut e| {
            if e.path.is_empty() {
                e.path = path;
            }
            e
        })
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let path = self.path.clone();
        match self.value {
            Value::String(s) => {
                // Match variants ignoring case and dashes.
                let n = normalize_key(&s);
                let s = variants.iter().find(|v| normalize_key(v) == n).map(|v| v.to_string()).unwrap_or(s);
                visitor.visit_enum(s.into_deserializer()).map_err(|e: Error| Error { path, ..e })
            }
            Value::Object(m) if m.len() == 1 => {
                let (k, v) = m.into_iter().next().unwrap();
                visitor.visit_enum(EnumDe { variant: k, value: v, path })
            }
            _ => Err(self.error("expected a string or a single-key map for an enum")),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }
}

struct SeqDe {
    iter: std::iter::Enumerate<std::vec::IntoIter<Value>>,
    path: String,
}

impl SeqDe {
    fn new(items: Vec<Value>, path: String) -> Self {
        SeqDe { iter: items.into_iter().enumerate(), path }
    }
}

impl<'de> SeqAccess<'de> for SeqDe {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        match self.iter.next() {
            Some((i, v)) => {
                let path = format!("{}[{i}]", self.path);
                seed.deserialize(Deserializer::new(v, path)).map(Some)
            }
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct MapDe {
    iter: serde_json::map::IntoIter,
    path: String,
    fields: Option<&'static [&'static str]>,
    value: Option<(String, Value)>,
}

impl MapDe {
    fn new(map: Map<String, Value>, path: String, fields: Option<&'static [&'static str]>) -> Self {
        MapDe { iter: map.into_iter(), path, fields, value: None }
    }
}

impl<'de> MapAccess<'de> for MapDe {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        loop {
            let Some((key, value)) = self.iter.next() else {
                return Ok(None);
            };
            let child = if self.path.is_empty() { key.clone() } else { format!("{}.{key}", self.path) };
            let key = match self.fields {
                Some(fields) => {
                    // Hidden keys hold YAML anchors.
                    if key.starts_with('.') {
                        continue;
                    }
                    let n = normalize_key(&key);
                    match fields.iter().find(|f| normalize_key(f) == n) {
                        Some(f) => f.to_string(),
                        None => key,
                    }
                }
                None => key,
            };
            self.value = Some((child.clone(), value));
            return seed.deserialize(KeyDe { key, path: child.clone() }).map(Some).map_err(|mut e: Error| {
                if e.path.is_empty() {
                    e.path = child;
                }
                e
            });
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let (path, value) = self.value.take().expect("value without key");
        seed.deserialize(Deserializer::new(value, path))
    }
}

/// Deserializer for map keys. Keys are always strings in the tree; numeric
/// keys (like ASNs) are parsed when the target is a number.
struct KeyDe {
    key: String,
    path: String,
}

macro_rules! key_int {
    ($name:ident, $t:ty) => {
        fn $name<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            Deserializer::new(Value::String(self.key), self.path).$name(visitor)
        }
    };
}

impl<'de> de::Deserializer<'de> for KeyDe {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_string(self.key)
    }

    key_int!(deserialize_i8, i8);
    key_int!(deserialize_i16, i16);
    key_int!(deserialize_i32, i32);
    key_int!(deserialize_i64, i64);
    key_int!(deserialize_u8, u8);
    key_int!(deserialize_u16, u16);
    key_int!(deserialize_u32, u32);
    key_int!(deserialize_u64, u64);
    key_int!(deserialize_bool, bool);

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        Deserializer::new(Value::String(self.key), self.path).deserialize_enum(name, variants, visitor)
    }

    serde::forward_to_deserialize_any! {
        f32 f64 char str string bytes byte_buf option unit unit_struct seq tuple
        tuple_struct map struct identifier ignored_any i128 u128
    }
}

struct EnumDe {
    variant: String,
    value: Value,
    path: String,
}

impl<'de> de::EnumAccess<'de> for EnumDe {
    type Error = Error;
    type Variant = VariantDe;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, VariantDe), Error> {
        let path = format!("{}.{}", self.path, self.variant);
        let v = seed.deserialize(KeyDe { key: self.variant, path: path.clone() })?;
        Ok((v, VariantDe { value: self.value, path }))
    }
}

struct VariantDe {
    value: Value,
    path: String,
}

impl<'de> de::VariantAccess<'de> for VariantDe {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(Deserializer::new(self.value, self.path))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_seq(Deserializer::new(self.value, self.path), visitor)
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_struct(Deserializer::new(self.value, self.path), "", fields, visitor)
    }
}

impl<'de> IntoDeserializer<'de, Error> for Deserializer {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

/// Convert a number into a JSON number, keeping integers as integers.
pub fn number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        Value::Number(Number::from(v as i64))
    } else {
        Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[derive(Debug, Deserialize, PartialEq, Default)]
    #[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
    struct Inner {
        name: String,
        size: u32,
    }

    #[derive(Debug, Deserialize, PartialEq, Default)]
    #[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
    struct Outer {
        max_batch_size: u64,
        enabled: bool,
        ratio: f64,
        tags: Vec<String>,
        inner: Inner,
        list: Vec<Inner>,
        asns: BTreeMap<u32, String>,
        entries: BTreeMap<String, String>,
        maybe: Option<u16>,
    }

    #[test]
    fn decode() {
        let v = json!({
            "MaxBatchSize": "0x10",
            "enabled": 1,
            "ratio": "0.5",
            "tags": "a,b",
            ".hidden": {"anything": true},
            "inner": {"NAME": "x", "size": "12"},
            "list": {"name": "single"},
            "asns": {"64501": "ACME"},
            "entries": {"segment.bytes": 1073741824},
            "maybe": 3,
        });
        let got: Outer = from_value(v).unwrap();
        assert_eq!(
            got,
            Outer {
                max_batch_size: 16,
                enabled: true,
                ratio: 0.5,
                tags: vec!["a".into(), "b".into()],
                inner: Inner { name: "x".into(), size: 12 },
                list: vec![Inner { name: "single".into(), size: 0 }],
                asns: BTreeMap::from([(64501, "ACME".into())]),
                entries: BTreeMap::from([("segment.bytes".into(), "1073741824".into())]),
                maybe: Some(3),
            }
        );
    }

    #[test]
    fn errors() {
        let err = from_value::<Outer>(json!({"inner": {"nope": 1}})).unwrap_err();
        assert_eq!(err.to_string(), "inner.nope: invalid key \"nope\"");
        let err = from_value::<Outer>(json!({"list": [{}, {"size": "big"}]})).unwrap_err();
        assert_eq!(err.to_string(), "list[1].size: cannot parse \"big\" as an integer");
        let err = from_value::<Outer>(json!({"inner": {"size": -1}})).unwrap_err();
        assert_eq!(err.to_string(), "inner.size: -1 is out of range");
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "kebab-case")]
    enum Algo {
        Random,
        ByExporter,
    }

    #[test]
    fn enums() {
        let got: Vec<Algo> = from_value(json!(["random", "by-exporter", "ByExporter"])).unwrap();
        assert_eq!(got, vec![Algo::Random, Algo::ByExporter, Algo::ByExporter]);
    }
}
