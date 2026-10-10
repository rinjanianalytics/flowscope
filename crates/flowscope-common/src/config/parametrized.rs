// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Configurations selected by a `type` key, like Go's
//! `ParametrizedConfigurationUnmarshallerHook`:
//!
//! ```yaml
//! cache:
//!   type: redis
//!   server: 127.0.0.1:6379
//! ```
//!
//! serde's internally tagged enums buffer their input and bypass the
//! normalization of keys done by our deserializer, so these enums are decoded
//! in two steps: first as a raw tree to extract the type, then the remaining
//! keys with our deserializer.

use serde_json::{Map, Value};

/// Extract the `type` key (case-insensitive) from a raw tree. The type is
/// lowercased, like in Go.
pub fn take_type(value: &mut Value) -> Result<(String, Value), String> {
    let map = match value {
        Value::Object(m) => m,
        Value::Null => return Err("configuration has no type".into()),
        _ => return Err("configuration should be a map".into()),
    };
    if map.keys().any(|k| k.eq_ignore_ascii_case("config")) {
        return Err("configuration should not have a `config' key".into());
    }
    let key = map
        .keys()
        .find(|k| k.eq_ignore_ascii_case("type"))
        .cloned()
        .ok_or_else(|| "configuration has no type".to_string())?;
    let t = match map.remove(&key) {
        Some(Value::String(s)) => s.to_lowercase(),
        Some(other) => return Err(format!("type should be a string not {other}")),
        None => unreachable!(),
    };
    Ok((t, Value::Object(std::mem::take(map))))
}

/// Serialize a variant: the type first, then the fields of the inner
/// configuration.
pub fn with_type<T: serde::Serialize>(name: &str, inner: &T) -> Result<Value, String> {
    let mut out = Map::new();
    out.insert("type".into(), Value::String(name.into()));
    match serde_json::to_value(inner).map_err(|e| e.to_string())? {
        Value::Object(m) => out.extend(m),
        Value::Null => {}
        other => return Err(format!("unexpected configuration {other}")),
    }
    Ok(Value::Object(out))
}

/// Define serialization for an enum of configurations selected by a `type`
/// key. Each variant holds one configuration struct.
///
/// ```ignore
/// parametrized!(CacheBackend {
///     "memory" => Memory(MemoryConfiguration),
///     "redis" => Redis(RedisConfiguration),
/// });
/// ```
#[macro_export]
macro_rules! parametrized {
    ($enum:ident { $($name:literal => $variant:ident($inner:ty)),+ $(,)? }) => {
        impl $enum {
            /// Name of the type, as used in the configuration.
            pub fn type_name(&self) -> &'static str {
                match self {
                    $($enum::$variant(_) => $name,)+
                }
            }

            /// Decode from a type and a tree without the type key.
            pub fn from_type(t: &str, value: serde_json::Value) -> Result<Self, $crate::config::DecodeError> {
                match t {
                    $($name => $crate::config::from_value::<$inner>(value).map($enum::$variant),)+
                    other => Err(<$crate::config::DecodeError as serde::de::Error>::custom(
                        format!("{other:?} is not a known type"))),
                }
            }
        }

        impl serde::Serialize for $enum {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let v = match self {
                    $($enum::$variant(inner) => $crate::config::parametrized::with_type($name, inner),)+
                };
                v.map_err(serde::ser::Error::custom)?.serialize(s)
            }
        }

        impl<'de> serde::Deserialize<'de> for $enum {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let mut raw = serde_json::Value::deserialize(d)?;
                let (t, rest) = $crate::config::parametrized::take_type(&mut raw)
                    .map_err(serde::de::Error::custom)?;
                $enum::from_type(&t, rest).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// Split a raw map between the fields of an outer configuration (matched
/// ignoring case and dashes) and the rest, for configurations mixing common
/// fields with fields of the selected type (`decoder` and `listen` for an
/// inlet input).
pub fn split_outer(map: Map<String, Value>, outer_fields: &[&str]) -> (Map<String, Value>, Map<String, Value>) {
    let outer_norm: Vec<String> = outer_fields.iter().map(|f| super::de::normalize_key(f)).collect();
    let mut outer = Map::new();
    let mut inner = Map::new();
    for (k, v) in map {
        if outer_norm.contains(&super::de::normalize_key(&k)) {
            outer.insert(k, v);
        } else {
            inner.insert(k, v);
        }
    }
    (outer, inner)
}
