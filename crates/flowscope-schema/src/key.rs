// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Name registry for column keys. Like the Go version, it is global: custom
//! dictionary columns are registered at runtime when the schema is built, and
//! configuration decoding needs to resolve their names afterwards.

use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, RwLock};

pub use crate::keys_gen::ColumnKey;
use crate::keys_gen::STATIC_NAMES;

struct Registry {
    names: Vec<String>,
    keys: HashMap<String, ColumnKey>,
}

static REGISTRY: LazyLock<RwLock<Registry>> = LazyLock::new(|| {
    let names: Vec<String> = STATIC_NAMES.iter().map(|s| s.to_string()).collect();
    let keys = names.iter().enumerate().skip(1).map(|(i, n)| (n.clone(), ColumnKey(i as u32))).collect();
    RwLock::new(Registry { names, keys })
});

/// Register a name for a dynamic column key.
pub(crate) fn register(key: ColumnKey, name: &str) {
    let mut reg = REGISTRY.write().unwrap();
    let idx = key.0 as usize;
    if reg.names.len() <= idx {
        reg.names.resize(idx + 1, String::new());
    }
    if let Some(old) = reg.keys.iter().find(|(_, k)| **k == key).map(|(n, _)| n.clone()) {
        reg.keys.remove(&old);
    }
    reg.names[idx] = name.to_string();
    reg.keys.insert(name.to_string(), key);
}

impl ColumnKey {
    /// Name of the column, empty when unknown.
    pub fn name(self) -> String {
        let reg = REGISTRY.read().unwrap();
        reg.names.get(self.0 as usize).cloned().unwrap_or_default()
    }

    /// Lookup a column key from its name.
    pub fn from_name(name: &str) -> Option<ColumnKey> {
        REGISTRY.read().unwrap().keys.get(name).copied()
    }

    /// Key for the same column in the opposite direction (Src/Dst, In/Out).
    /// It mirrors the generated Go `columnReverseTable`: AS path columns only
    /// exist for the destination and map to the invalid key 0.
    pub fn reverse_direction(self) -> ColumnKey {
        if self.0 == 0 || self.0 >= ColumnKey::Last.0 {
            return self;
        }
        let name = STATIC_NAMES[self.0 as usize];
        if name == "DstASPath" || (name.starts_with("Dst") && name.ends_with("AS") && name.len() == 8) {
            // Dst1stAS, Dst2ndAS, Dst3rdAS
            return ColumnKey(0);
        }
        let swapped = swap_direction_prefix(name);
        match swapped {
            Some(other) => ColumnKey::from_name(&other).unwrap_or(self),
            None => self,
        }
    }
}

/// Swap the direction prefix using the same rules as the Go generator: In and
/// Out first, then Src and Dst. The character after the prefix must be an
/// uppercase letter or a digit.
fn swap_direction_prefix(name: &str) -> Option<String> {
    let follows = |rest: &str| rest.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    for (from, to) in [("In", "Out"), ("Out", "In"), ("Src", "Dst"), ("Dst", "Src")] {
        if let Some(rest) = name.strip_prefix(from)
            && follows(rest)
        {
            return Some(format!("{to}{rest}"));
        }
    }
    None
}

impl fmt::Display for ColumnKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl fmt::Debug for ColumnKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.name();
        if name.is_empty() { write!(f, "ColumnKey({})", self.0) } else { f.write_str(&name) }
    }
}

impl serde::Serialize for ColumnKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let name = self.name();
        if name.is_empty() {
            return Err(serde::ser::Error::custom("unknown column name"));
        }
        s.serialize_str(&name)
    }
}

impl<'de> serde::Deserialize<'de> for ColumnKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ColumnKey::from_name(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown column {s:?}")))
    }
}

impl std::str::FromStr for ColumnKey {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ColumnKey::from_name(s).ok_or_else(|| format!("unknown column {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(ColumnKey::TimeReceived.0, 1);
        assert_eq!(ColumnKey::TimeReceived.name(), "TimeReceived");
        assert_eq!(ColumnKey::from_name("EgressVRFID"), Some(ColumnKey::EgressVRFID));
        assert_eq!(ColumnKey::from_name("Nope"), None);
    }

    #[test]
    fn reverse() {
        let cases = [
            (ColumnKey::SrcAddr, ColumnKey::DstAddr),
            (ColumnKey::DstAddr, ColumnKey::SrcAddr),
            (ColumnKey::InIfName, ColumnKey::OutIfName),
            (ColumnKey::OutIfBoundary, ColumnKey::InIfBoundary),
            (ColumnKey::DstASPath, ColumnKey(0)),
            (ColumnKey::Dst1stAS, ColumnKey(0)),
            (ColumnKey::Dst3rdAS, ColumnKey(0)),
            (ColumnKey::IngressVRFID, ColumnKey::IngressVRFID),
            (ColumnKey::Bytes, ColumnKey::Bytes),
            (ColumnKey::SrcMAC, ColumnKey::DstMAC),
        ];
        for (input, expected) in cases {
            assert_eq!(input.reverse_direction(), expected, "{input:?}");
        }
    }
}
