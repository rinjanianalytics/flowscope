// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Configuration migrations: renamed keys, deprecated keys, keys moved to
//! another section. Port of the mapstructure hooks of the Go version.
//!
//! A type with migrations derives `Serialize` and `Deserialize` with
//! `#[serde(remote = "Self")]` and uses `migrated!(Type, migrate_fn)`: the raw
//! tree is transformed by `migrate_fn` before being decoded.

use super::de::normalize_key;
use serde_json::{Map, Value};

/// Find the actual key matching `name` (ignoring case and dashes).
pub fn find_key(map: &Map<String, Value>, name: &str) -> Option<String> {
    let n = normalize_key(name);
    map.keys().find(|k| normalize_key(k) == n).cloned()
}

/// Get a value by name.
pub fn get<'a>(map: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    find_key(map, name).and_then(|k| map.get(&k))
}

/// Get a mutable value by name.
pub fn get_mut<'a>(map: &'a mut Map<String, Value>, name: &str) -> Option<&'a mut Value> {
    let k = find_key(map, name)?;
    map.get_mut(&k)
}

/// Remove a value by name.
pub fn remove(map: &mut Map<String, Value>, name: &str) -> Option<Value> {
    let k = find_key(map, name)?;
    map.remove(&k)
}

/// Rename a key. Having both the old and the new key is an error.
pub fn rename(map: &mut Map<String, Value>, from: &str, to: &str) -> Result<(), String> {
    let old = find_key(map, from);
    let new = find_key(map, to);
    match (old, new) {
        (Some(o), Some(n)) => Err(format!("cannot have both {o:?} and {n:?}")),
        (Some(o), None) => {
            let v = map.remove(&o).unwrap();
            map.insert(to.to_string(), v);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Get or create a sub-map.
pub fn submap<'a>(map: &'a mut Map<String, Value>, name: &str) -> &'a mut Map<String, Value> {
    let key = find_key(map, name).unwrap_or_else(|| name.to_string());
    let entry = map.entry(key).or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().unwrap()
}

/// Implement `Serialize` and `Deserialize` for a type deriving them with
/// `#[serde(remote = "Self")]`, applying a migration on the raw tree first.
#[macro_export]
macro_rules! migrated {
    ($t:ty, $migrate:path) => {
        impl serde::Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                <$t>::serialize(self, s)
            }
        }

        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let mut raw = serde_json::Value::deserialize(d)?;
                if let serde_json::Value::Object(map) = &mut raw {
                    let migrate: fn(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), String> = $migrate;
                    migrate(map).map_err(serde::de::Error::custom)?;
                }
                <$t>::deserialize($crate::config::de::Deserializer::new(raw, String::new()))
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}
