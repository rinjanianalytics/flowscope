// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SNMP provider configuration. Port of
//! `outlet/metadata/provider/snmp/config.go`.

use flowscope_common::config::{Duration, migrate};
use flowscope_common::helpers::SubnetMap;
use flowscope_common::migrated;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::net::IpAddr;

/// SNMP provider configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SnmpConfiguration {
    /// Number of retries.
    pub poller_retries: u32,
    /// Timeout for each request.
    pub poller_timeout: Duration,
    /// Credentials, per exporter subnet.
    pub credentials: SubnetMap<Credentials>,
    /// Agent to query for an exporter, when different.
    pub agents: BTreeMap<IpAddr, IpAddr>,
    /// Port, per exporter subnet.
    pub ports: SubnetMap<u16>,
}

impl Default for SnmpConfiguration {
    fn default() -> Self {
        let mut credentials = SubnetMap::new();
        credentials
            .insert("::/0".parse().unwrap(), Credentials { communities: vec!["public".into()], ..Default::default() });
        let mut ports = SubnetMap::new();
        ports.insert("::/0".parse().unwrap(), 161);
        SnmpConfiguration {
            poller_retries: 1,
            poller_timeout: Duration::from_secs(1),
            credentials,
            agents: BTreeMap::new(),
            ports,
        }
    }
}

/// SNMP credentials: communities (v2c) or a user (v3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Credentials {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub communities: Vec<String>,
    #[serde(rename = "user-name", skip_serializing_if = "String::is_empty")]
    pub user_name: String,
    #[serde(skip_serializing_if = "is_default")]
    pub authentication_protocol: AuthProtocol,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub authentication_passphrase: String,
    #[serde(skip_serializing_if = "is_default")]
    pub privacy_protocol: PrivProtocol,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub privacy_passphrase: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub context_name: String,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl Credentials {
    pub fn validate(&self) -> Result<(), String> {
        let v3 = !self.user_name.is_empty();
        match (self.communities.is_empty(), v3) {
            (true, false) => return Err("SNMP credentials need communities or a user name".into()),
            (false, true) => return Err("SNMP credentials cannot have both communities and a user name".into()),
            _ => {}
        }
        if self.communities.iter().any(|c| c.is_empty()) {
            return Err("SNMP communities cannot be empty".into());
        }
        if self.privacy_protocol != PrivProtocol::None && self.authentication_protocol == AuthProtocol::None {
            return Err("SNMP privacy protocol needs an authentication protocol".into());
        }
        if self.authentication_protocol != AuthProtocol::None && self.authentication_passphrase.is_empty() {
            return Err("SNMP authentication protocol needs a passphrase".into());
        }
        if self.privacy_protocol != PrivProtocol::None && self.privacy_passphrase.is_empty() {
            return Err("SNMP privacy protocol needs a passphrase".into());
        }
        Ok(())
    }
}

/// SNMPv3 authentication protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthProtocol {
    #[default]
    None,
    Md5,
    Sha,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

/// SNMPv3 privacy protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivProtocol {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "des")]
    Des,
    #[serde(rename = "aes")]
    Aes,
    #[serde(rename = "aes192")]
    Aes192,
    #[serde(rename = "aes256")]
    Aes256,
    #[serde(rename = "aes192-c")]
    Aes192C,
    #[serde(rename = "aes256-c")]
    Aes256C,
}

fn looks_like_subnet_map(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.keys().all(|k| {
            let addr = k.split('/').next().unwrap_or("");
            !addr.is_empty()
                && addr.contains([':', '.'])
                && addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        }),
        _ => false,
    }
}

/// Old settings (`default-community`, `communities`, `security-parameters`)
/// are merged into `credentials`.
fn migrate_snmp(map: &mut Map<String, Value>) -> Result<(), String> {
    let default_community = migrate::remove(map, "DefaultCommunity");
    let communities = migrate::remove(map, "Communities");
    let security = migrate::remove(map, "SecurityParameters");
    let mut credentials =
        migrate::remove(map, "Credentials").unwrap_or_else(|| json!({"::/0": {"communities": "public"}}));
    if !looks_like_subnet_map(&credentials) {
        credentials = json!({"::/0": credentials});
    }
    let creds = credentials.as_object_mut().unwrap();
    let non_zero = |v: &Value| !matches!(v, Value::Null) && v != &json!("") && v != &json!([]) && v != &json!({});
    if let Some(dc) = default_community.filter(non_zero) {
        creds.insert("::/0".into(), dc);
    }
    for extra in [communities, security].into_iter().flatten() {
        if !non_zero(&extra) {
            continue;
        }
        if looks_like_subnet_map(&extra) {
            for (k, v) in extra.as_object().unwrap() {
                creds.insert(k.clone(), v.clone());
            }
        } else {
            creds.insert("::/0".into(), extra);
        }
    }
    for v in creds.values_mut() {
        if v.is_string() || v.is_array() {
            *v = json!({"communities": v.clone()});
        }
    }
    map.insert("credentials".into(), credentials);
    Ok(())
}

migrated!(SnmpConfiguration, migrate_snmp);

impl SnmpConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if self.poller_timeout.as_millis() < 100 {
            return Err("SNMP poller timeout should be at least 100ms".into());
        }
        for (_, c) in self.credentials.iter() {
            c.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};

    fn creds_for(c: &SnmpConfiguration, ip: &str) -> Credentials {
        let ip = flowscope_common::helpers::subnetmap::addr_to6(&ip.parse().unwrap());
        c.credentials.lookup(&ip).cloned().unwrap()
    }

    #[test]
    fn migrations() {
        let c: SnmpConfiguration = from_value(parse_str("default-community: private\n").unwrap()).unwrap();
        assert_eq!(creds_for(&c, "192.0.2.1").communities, vec!["private"]);

        let c: SnmpConfiguration =
            from_value(parse_str("communities:\n  203.0.113.0/24: other\n  ::/0: public2\n").unwrap()).unwrap();
        assert_eq!(creds_for(&c, "203.0.113.1").communities, vec!["other"]);
        assert_eq!(creds_for(&c, "192.0.2.1").communities, vec!["public2"]);

        let c: SnmpConfiguration = from_value(
            parse_str("security-parameters:\n  user-name: alfred\n  authentication-protocol: sha\n  authentication-passphrase: hello\n")
                .unwrap(),
        )
        .unwrap();
        let cr = creds_for(&c, "192.0.2.1");
        assert_eq!(cr.user_name, "alfred");
        assert_eq!(cr.authentication_protocol, AuthProtocol::Sha);
        c.validate().unwrap();

        let c: SnmpConfiguration = from_value(parse_str("credentials:\n  communities: [a, b]\n").unwrap()).unwrap();
        assert_eq!(creds_for(&c, "::1").communities, vec!["a", "b"]);

        let c: SnmpConfiguration = from_value(parse_str("{}").unwrap()).unwrap();
        assert_eq!(c, SnmpConfiguration::default());
    }
}
