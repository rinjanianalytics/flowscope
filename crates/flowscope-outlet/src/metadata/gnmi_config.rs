// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! gNMI provider configuration. Port of
//! `outlet/metadata/provider/gnmi/config.go`.

use flowscope_common::config::{Duration, migrate};
use flowscope_common::helpers::{SubnetMap, TlsConfiguration};
use flowscope_common::migrated;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::net::IpAddr;

/// gNMI provider configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct GnmiConfiguration {
    pub timeout: Duration,
    pub minimal_refresh_interval: Duration,
    /// Target address, per exporter subnet, when different.
    pub targets: SubnetMap<IpAddr>,
    /// Whether to set the target in requests, per exporter subnet.
    pub set_target: SubnetMap<bool>,
    pub ports: SubnetMap<u16>,
    pub authentication_parameters: SubnetMap<AuthenticationParameter>,
    pub models: Vec<Model>,
}

impl Default for GnmiConfiguration {
    fn default() -> Self {
        let mut ports = SubnetMap::new();
        ports.insert("::/0".parse().unwrap(), 9339);
        GnmiConfiguration {
            timeout: Duration::from_secs(1),
            minimal_refresh_interval: Duration::from_secs(60),
            targets: SubnetMap::new(),
            set_target: SubnetMap::new(),
            ports,
            authentication_parameters: SubnetMap::new(),
            models: default_models(),
        }
    }
}

/// "defaults" in the list of models is replaced by the default models.
fn migrate_gnmi(map: &mut Map<String, Value>) -> Result<(), String> {
    if let Some(Value::Array(models)) = migrate::get_mut(map, "Models")
        && let Some(pos) = models.iter().position(|m| m == "defaults")
    {
        let defaults: Vec<Value> = default_models().iter().map(|m| serde_json::to_value(m).unwrap()).collect();
        models.splice(pos..=pos, defaults);
    }
    Ok(())
}

migrated!(GnmiConfiguration, migrate_gnmi);

/// Authentication for a target.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct AuthenticationParameter {
    pub username: String,
    pub password: String,
    pub tls: TlsConfiguration,
}

/// Old TLS settings (`insecure`, `skip-verify`, `tls-ca`, `tls-cert`,
/// `tls-key`) moved to `tls`. TLS is enabled unless said otherwise.
fn migrate_auth(map: &mut Map<String, Value>) -> Result<(), String> {
    let old = ["Insecure", "SkipVerify", "TLSCA", "TLSCert", "TLSKey"];
    let has_old = old.iter().any(|k| migrate::find_key(map, k).is_some());
    if let Some(tls) = migrate::get_mut(map, "TLS") {
        if has_old {
            return Err("cannot mix old TLS configuration (Insecure, SkipVerify, TLSCA, TLSCert, TLSKey) with new TLS configuration".into());
        }
        if let Value::Object(t) = tls
            && migrate::find_key(t, "Enable").is_none()
        {
            t.insert("enable".into(), Value::Bool(true));
        }
        return Ok(());
    }
    let mut tls = Map::new();
    match migrate::remove(map, "Insecure") {
        Some(Value::Bool(b)) => {
            tls.insert("enable".into(), Value::Bool(!b));
        }
        Some(_) => {}
        None => {
            tls.insert("enable".into(), Value::Bool(true));
        }
    }
    if let Some(Value::Bool(b)) = migrate::remove(map, "SkipVerify") {
        tls.insert("skip-verify".into(), Value::Bool(b));
    }
    for (old, new) in [("TLSCA", "ca-file"), ("TLSCert", "cert-file"), ("TLSKey", "key-file")] {
        if let Some(Value::String(s)) = migrate::remove(map, old) {
            tls.insert(new.into(), Value::String(s));
        }
    }
    map.insert("tls".into(), Value::Object(tls));
    Ok(())
}

migrated!(AuthenticationParameter, migrate_auth);

/// A model: where to find the information in the gNMI tree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Model {
    pub name: String,
    pub system_name_paths: Vec<String>,
    pub if_index_paths: Vec<String>,
    pub if_name_keys: Vec<String>,
    pub if_name_paths: Vec<String>,
    pub if_description_paths: Vec<String>,
    pub if_speed_paths: Vec<IfSpeedPath>,
}

/// A path to the speed of an interface, with its unit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct IfSpeedPath {
    pub path: String,
    pub unit: IfSpeedPathUnit,
}

/// Unit of an interface speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IfSpeedPathUnit {
    #[default]
    Bps,
    Mbps,
    Ethernet,
    Human,
}

fn speed(path: &str, unit: IfSpeedPathUnit) -> IfSpeedPath {
    IfSpeedPath { path: path.into(), unit }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Models known by default.
pub fn default_models() -> Vec<Model> {
    use IfSpeedPathUnit::*;
    vec![
        Model {
            name: "Nokia SR OS".into(),
            system_name_paths: strings(&["/configure/system/name"]),
            if_index_paths: strings(&["/state/port/if-index", "/state/lag/if-index"]),
            if_name_keys: strings(&["port-id", "lag-name"]),
            if_name_paths: vec![],
            if_description_paths: strings(&["/configure/port/description", "/configure/lag/description"]),
            if_speed_paths: vec![speed("/state/port/ethernet/oper-speed", Mbps), speed("/state/lag/bandwidth", Bps)],
        },
        Model {
            name: "Nokia SR Linux".into(),
            system_name_paths: strings(&["/system/name/host-name"]),
            if_index_paths: strings(&["/interface/ifindex", "/interface/subinterface/ifindex"]),
            if_name_keys: strings(&["name"]),
            if_name_paths: strings(&["/interface/subinterface/name"]),
            if_description_paths: strings(&["/interface/description", "/interface/subinterface/description"]),
            if_speed_paths: vec![
                speed("/interface/ethernet/port-speed", Human),
                speed("/interface/lag/lag-speed", Bps),
            ],
        },
        Model {
            name: "OpenConfig".into(),
            system_name_paths: strings(&["/system/config/hostname"]),
            if_index_paths: strings(&[
                "/interfaces/interface/state/ifindex",
                "/interfaces/interface/subinterfaces/subinterface/state/ifindex",
            ]),
            if_name_keys: strings(&["name"]),
            if_name_paths: strings(&["/interfaces/interface/subinterfaces/subinterface/state/name"]),
            if_description_paths: strings(&[
                "/interfaces/interface/state/description",
                "/interfaces/interface/subinterfaces/subinterface/state/description",
            ]),
            if_speed_paths: vec![
                speed("/interfaces/interface/aggregation/state/lag-speed", Mbps),
                speed("/interfaces/interface/ethernet/state/negotiated-port-speed", Ethernet),
                speed("/interfaces/interface/ethernet/state/port-speed", Ethernet),
            ],
        },
        Model {
            name: "IETF".into(),
            system_name_paths: strings(&["/system/hostname"]),
            if_index_paths: strings(&["/interfaces/interface/if-index"]),
            if_name_keys: vec![],
            if_name_paths: strings(&["/interfaces/interface/name"]),
            if_description_paths: strings(&["/interfaces/interface/description"]),
            if_speed_paths: vec![speed("/interfaces/interface/speed", Bps)],
        },
    ]
}

impl GnmiConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if self.timeout.as_millis() < 100 {
            return Err("gNMI timeout should be at least 100ms".into());
        }
        if self.minimal_refresh_interval.as_secs() < 1 {
            return Err("gNMI minimal refresh interval should be at least 1s".into());
        }
        if self.models.is_empty() {
            return Err("gNMI needs at least one model".into());
        }
        for m in &self.models {
            if m.name.is_empty() || m.system_name_paths.is_empty() || m.if_index_paths.is_empty() {
                return Err(format!("gNMI model {:?} is incomplete", m.name));
            }
            if m.if_name_keys.is_empty() && m.if_name_paths.is_empty() {
                return Err(format!("gNMI model {:?} needs interface name keys or paths", m.name));
            }
            if m.if_description_paths.is_empty() || m.if_speed_paths.is_empty() {
                return Err(format!("gNMI model {:?} is incomplete", m.name));
            }
        }
        for (_, a) in self.authentication_parameters.iter() {
            if !a.username.is_empty() && a.password.is_empty() {
                return Err("gNMI password is required with a username".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};

    #[test]
    fn models() {
        let c: GnmiConfiguration = from_value(
            parse_str("models:\n  - name: custom\n    system-name-paths: [/a]\n    if-index-paths: [/b]\n    if-name-paths: [/c]\n    if-description-paths: [/d]\n    if-speed-paths: [{path: /e, unit: human}]\n  - defaults\n").unwrap(),
        )
        .unwrap();
        assert_eq!(c.models.len(), 5);
        assert_eq!(c.models[0].name, "custom");
        assert_eq!(c.models[1].name, "Nokia SR OS");
        c.validate().unwrap();
    }

    #[test]
    fn auth_migration() {
        let c: GnmiConfiguration = from_value(
            parse_str("authentication-parameters:\n  ::/0:\n    username: a\n    password: b\n    insecure: true\n")
                .unwrap(),
        )
        .unwrap();
        let (_, a) = c.authentication_parameters.iter().next().unwrap();
        assert!(!a.tls.enable);
        let c: GnmiConfiguration =
            from_value(parse_str("authentication-parameters:\n  ::/0:\n    tls:\n      ca-file: /x\n").unwrap())
                .unwrap();
        let (_, a) = c.authentication_parameters.iter().next().unwrap();
        assert!(a.tls.enable);
        assert_eq!(a.tls.ca_file, "/x");
    }
}
