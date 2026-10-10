// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Schema configuration. Port of `common/schema/config.go`.

use crate::key::ColumnKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A ClickHouse data-skipping index: "minmax", "set(N)" with N >= 0, or
/// "bloom(P)" with 0 < P < 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SkipIndexType(String);

impl SkipIndexType {
    pub fn new(s: &str) -> Result<Self, String> {
        let v = SkipIndexType(s.to_string());
        v.clickhouse_type()?;
        Ok(v)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The TYPE clause used in `ALTER TABLE ... ADD INDEX`.
    pub fn clickhouse_type(&self) -> Result<String, String> {
        let s = self.0.as_str();
        if s == "minmax" {
            return Ok("minmax".into());
        }
        if let Some(inner) = s.strip_prefix("set(").and_then(|r| r.strip_suffix(')')) {
            return match inner.parse::<i64>() {
                Ok(n) if n >= 0 => Ok(s.into()),
                _ => Err(format!("invalid set index {s:?}: argument must be a non-negative integer")),
            };
        }
        if let Some(inner) = s.strip_prefix("bloom(").and_then(|r| r.strip_suffix(')')) {
            return match inner.parse::<f64>() {
                Ok(p) if p > 0.0 && p < 1.0 => Ok(format!("bloom_filter({})", format_g(p))),
                _ => Err(format!("invalid bloom index {s:?}: FPP must be in (0, 1)")),
            };
        }
        Err(format!("unknown skip index type {s:?}: use minmax, set(N), or bloom(P)"))
    }
}

/// Format a float like Go's `%g` for the values we use (shortest
/// representation, exponent for small values).
pub fn format_g(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let exp = v.abs().log10().floor() as i32;
    if !(-4..21).contains(&exp) {
        // Go: 1e-05, 1.5e-07
        let s = format!("{v:e}");
        let (mantissa, e) = s.split_once('e').unwrap();
        let e: i32 = e.parse().unwrap();
        let sign = if e < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", e.abs())
    } else {
        format!("{v}")
    }
}

impl<'de> Deserialize<'de> for SkipIndexType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        SkipIndexType::new(&s).map_err(serde::de::Error::custom)
    }
}

/// Configuration for the schema component.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Columns disabled in addition to the ones disabled by default.
    pub disabled: Vec<ColumnKey>,
    /// Columns enabled in addition to the ones enabled by default.
    pub enabled: Vec<ColumnKey>,
    /// Columns moved to the main table only.
    pub main_table_only: Vec<ColumnKey>,
    /// Columns moved out of the main table only.
    pub not_main_table_only: Vec<ColumnKey>,
    /// Columns materialized at ingest instead of computed at query time.
    pub materialize: Vec<ColumnKey>,
    /// Data-skipping indexes, merged with the defaults.
    pub indexes: BTreeMap<ColumnKey, SkipIndexType>,
    /// Columns whose default skip index is removed.
    pub no_indexes: Vec<ColumnKey>,
    /// Custom dictionaries to enrich flows.
    pub custom_dictionaries: BTreeMap<String, CustomDict>,
}

impl Configuration {
    /// Validate the configuration, like the Go validator tags.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(k) = self.enabled.iter().find(|k| self.disabled.contains(k)) {
            return Err(format!("{k} cannot be both enabled and disabled"));
        }
        if let Some(k) = self.not_main_table_only.iter().find(|k| self.main_table_only.contains(k)) {
            return Err(format!("{k} cannot be both main table only and not main table only"));
        }
        for (name, dict) in &self.custom_dictionaries {
            dict.validate().map_err(|e| format!("custom dictionary {name}: {e}"))?;
        }
        Ok(())
    }
}

/// Layout of a custom dictionary. Same names as ClickHouse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CustomDictLayout(String);

impl CustomDictLayout {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CustomDictLayout {
    fn default() -> Self {
        CustomDictLayout("hashed".into())
    }
}

impl std::str::FromStr for CustomDictLayout {
    type Err = String;
    /// "iptrie" is accepted as an alias for "ip_trie".
    fn from_str(s: &str) -> Result<Self, String> {
        let layout = if s == "iptrie" { "ip_trie" } else { s };
        match layout {
            "hashed" | "complex_key_hashed" | "ip_trie" => Ok(CustomDictLayout(layout.into())),
            _ => Err(format!("unknown dictionary layout {s:?}: use hashed, complex_key_hashed, or ip_trie")),
        }
    }
}

impl<'de> Deserialize<'de> for CustomDictLayout {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// A custom dictionary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct CustomDict {
    pub keys: Vec<CustomDictKey>,
    pub attributes: Vec<CustomDictAttribute>,
    pub source: String,
    pub layout: CustomDictLayout,
    pub dimensions: Vec<String>,
}

/// A key (matching) column of a custom dictionary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct CustomDictKey {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub match_dimension: String,
    pub match_dimension_suffix: String,
}

impl Default for CustomDictKey {
    fn default() -> Self {
        CustomDictKey {
            name: String::new(),
            r#type: "String".into(),
            match_dimension: String::new(),
            match_dimension_suffix: String::new(),
        }
    }
}

/// A value column of a custom dictionary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct CustomDictAttribute {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    /// Empty means the capitalized name is used.
    pub label: String,
    pub default: String,
}

impl Default for CustomDictAttribute {
    fn default() -> Self {
        CustomDictAttribute {
            name: String::new(),
            r#type: "String".into(),
            label: String::new(),
            default: String::new(),
        }
    }
}

const DICT_TYPES: [&str; 6] = ["String", "UInt8", "UInt16", "UInt32", "UInt64", "IPv6"];

fn alphanum(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric())
}

impl CustomDict {
    fn validate(&self) -> Result<(), String> {
        if self.keys.is_empty() {
            return Err("keys is required".into());
        }
        if self.attributes.is_empty() {
            return Err("attributes is required".into());
        }
        if self.source.is_empty() {
            return Err("source is required".into());
        }
        if self.dimensions.is_empty() {
            return Err("dimensions is required".into());
        }
        for k in &self.keys {
            if !alphanum(&k.name) {
                return Err(format!("invalid key name {:?}", k.name));
            }
            if !DICT_TYPES.contains(&k.r#type.as_str()) {
                return Err(format!("invalid key type {:?}", k.r#type));
            }
            for v in [&k.match_dimension, &k.match_dimension_suffix] {
                if !v.is_empty() && !alphanum(v) {
                    return Err(format!("invalid match dimension {v:?}"));
                }
            }
        }
        for a in &self.attributes {
            if !alphanum(&a.name) {
                return Err(format!("invalid attribute name {:?}", a.name));
            }
            if !DICT_TYPES.contains(&a.r#type.as_str()) {
                return Err(format!("invalid attribute type {:?}", a.r#type));
            }
            for v in [&a.label, &a.default] {
                if !v.is_empty() && !alphanum(v) {
                    return Err(format!("invalid value {v:?}"));
                }
            }
        }
        Ok(())
    }
}

/// Skip indexes applied when none are explicitly configured.
pub fn default_indexes() -> BTreeMap<ColumnKey, SkipIndexType> {
    use ColumnKey as K;
    let bloom = || SkipIndexType("bloom(0.001)".into());
    let set0 = || SkipIndexType("set(0)".into());
    BTreeMap::from([
        (K::SrcAddr, bloom()),
        (K::DstAddr, bloom()),
        (K::SrcAS, bloom()),
        (K::DstAS, bloom()),
        (K::SrcPort, bloom()),
        (K::DstPort, bloom()),
        (K::SrcCountry, bloom()),
        (K::DstCountry, bloom()),
        (K::ExporterName, SkipIndexType("minmax".into())),
        (K::InIfProvider, set0()),
        (K::OutIfProvider, set0()),
        (K::InIfConnectivity, set0()),
        (K::OutIfConnectivity, set0()),
        (K::InIfBoundary, set0()),
        (K::OutIfBoundary, set0()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_index() {
        let cases = [
            ("minmax", Ok("minmax")),
            ("set(0)", Ok("set(0)")),
            ("set(100)", Ok("set(100)")),
            ("bloom(0.001)", Ok("bloom_filter(0.001)")),
            ("bloom(0.00001)", Ok("bloom_filter(1e-05)")),
            ("bloom(0.5)", Ok("bloom_filter(0.5)")),
            ("set(-1)", Err(())),
            ("set(a)", Err(())),
            ("bloom(0)", Err(())),
            ("bloom(1)", Err(())),
            ("unknown", Err(())),
        ];
        for (input, expected) in cases {
            let got = SkipIndexType(input.into()).clickhouse_type();
            match expected {
                Ok(e) => assert_eq!(got.as_deref(), Ok(e), "{input}"),
                Err(()) => assert!(got.is_err(), "{input}"),
            }
        }
    }

    #[test]
    fn layout() {
        assert_eq!("iptrie".parse::<CustomDictLayout>().unwrap().as_str(), "ip_trie");
        assert!("nope".parse::<CustomDictLayout>().is_err());
    }
}
