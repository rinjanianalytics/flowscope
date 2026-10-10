// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Core configuration. Port of `outlet/core/config.go`.

use super::classifier::{ExporterClassifierRule, InterfaceClassifierRule};
use flowscope_common::config::{Duration, migrate};
use flowscope_common::helpers::SubnetMap;
use flowscope_common::migrated;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Where to get the AS number from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AsnProvider {
    Flow,
    FlowExceptPrivate,
    FlowExceptDefaultRoute,
    Routing,
    RoutingExceptPrivate,
    Networks,
}

impl<'de> Deserialize<'de> for AsnProvider {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?.to_lowercase();
        Ok(match s.as_str() {
            "flow" => AsnProvider::Flow,
            "flow-except-private" => AsnProvider::FlowExceptPrivate,
            "flow-except-default-route" => AsnProvider::FlowExceptDefaultRoute,
            "routing" | "bmp" => AsnProvider::Routing,
            "routing-except-private" | "bmp-except-private" => AsnProvider::RoutingExceptPrivate,
            "networks" | "geo-ip" => AsnProvider::Networks,
            other => return Err(serde::de::Error::custom(format!("unknown ASN provider {other:?}"))),
        })
    }
}

/// Where to get the network mask and next hop from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetProvider {
    Flow,
    Routing,
}

impl<'de> Deserialize<'de> for NetProvider {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?.to_lowercase();
        Ok(match s.as_str() {
            "flow" => NetProvider::Flow,
            "routing" | "bmp" => NetProvider::Routing,
            other => return Err(serde::de::Error::custom(format!("unknown net provider {other:?}"))),
        })
    }
}

/// Core configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    pub exporter_classifiers: Vec<ExporterClassifierRule>,
    pub interface_classifiers: Vec<InterfaceClassifierRule>,
    pub classifier_cache_duration: Duration,
    pub default_sampling_rate: SubnetMap<u64>,
    pub override_sampling_rate: SubnetMap<u64>,
    #[serde(rename = "asn-providers")]
    pub asn_providers: Vec<AsnProvider>,
    pub net_providers: Vec<NetProvider>,
    /// Wait before processing flows.
    pub startup_delay: Duration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            exporter_classifiers: vec![],
            interface_classifiers: vec![],
            classifier_cache_duration: Duration::from_secs(300),
            default_sampling_rate: SubnetMap::new(),
            override_sampling_rate: SubnetMap::new(),
            asn_providers: vec![AsnProvider::Flow, AsnProvider::Routing, AsnProvider::Networks],
            net_providers: vec![NetProvider::Flow, NetProvider::Routing],
            startup_delay: Duration::ZERO,
        }
    }
}

/// `ignore-asn-from-flow` was replaced by `asn-providers`.
fn migrate_core(map: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["Workers", "ClassifierCacheSize"] {
        migrate::remove(map, k);
    }
    let old = migrate::find_key(map, "IgnoreASNFromFlow");
    let new = migrate::find_key(map, "ASNProviders");
    if let (Some(o), Some(n)) = (&old, &new) {
        return Err(format!("cannot have both {o:?} and {n:?}"));
    }
    if let Some(o) = old
        && map.remove(&o) == Some(Value::Bool(true))
    {
        map.insert("asn-providers".into(), json!(["routing", "networks"]));
    }
    Ok(())
}

migrated!(Configuration, migrate_core);

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.classifier_cache_duration.as_secs() < 1 {
            return Err("core.classifier-cache-duration should be at least 1s".into());
        }
        if !self.startup_delay.is_zero() && self.startup_delay.as_secs() < 1 {
            return Err("core.startup-delay should be 0 or at least 1s".into());
        }
        Ok(())
    }
}
