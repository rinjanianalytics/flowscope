// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A map from IP subnets to values, with longest-prefix lookups. Port of
//! `common/helpers/subnetmap.go`. All subnets are stored as IPv6 (IPv4 ones
//! are mapped into ::ffff:0:0/96).
//!
//! In configurations, a subnet map is either a map from subnets (or single
//! addresses) to values, or a single value applying to every address.

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr};

/// A map from subnets to values.
#[derive(Clone)]
pub struct SubnetMap<V> {
    table: PrefixMap<Ipv6Net, V>,
}

impl<V> Default for SubnetMap<V> {
    fn default() -> Self {
        SubnetMap { table: PrefixMap::new() }
    }
}

impl<V: std::fmt::Debug> std::fmt::Debug for SubnetMap<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter().map(|(p, v)| (unmap_prefix(&p), v))).finish()
    }
}

impl<V: PartialEq> PartialEq for SubnetMap<V> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

/// Convert an IPv4 prefix to its IPv6-mapped form.
pub fn prefix_to6(p: &IpNet) -> Ipv6Net {
    match p {
        IpNet::V4(v4) => Ipv6Net::new(v4.network().to_ipv6_mapped(), v4.prefix_len() + 96).unwrap(),
        IpNet::V6(v6) => v6.trunc(),
    }
}

/// Address in IPv6 form.
pub fn addr_to6(ip: &IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => *v6,
    }
}

/// Text form of a prefix, IPv4-mapped prefixes shown as IPv4.
pub fn unmap_prefix(p: &Ipv6Net) -> String {
    if p.prefix_len() >= 96
        && let Some(v4) = p.network().to_ipv4_mapped()
        && let Ok(n) = Ipv4Net::new(v4, p.prefix_len() - 96)
    {
        return n.to_string();
    }
    p.to_string()
}

/// Parse a subnet map key: a subnet or an address.
pub fn parse_key(k: &str) -> Result<Ipv6Net, String> {
    if k.contains('/') {
        let p: IpNet = k.parse().map_err(|e| format!("failed to parse key {k}: {e}"))?;
        Ok(prefix_to6(&p))
    } else {
        let ip: IpAddr = k.parse().map_err(|e| format!("failed to parse key {k}: {e}"))?;
        Ok(match ip {
            IpAddr::V4(v4) => Ipv6Net::new(v4.to_ipv6_mapped(), 128).unwrap(),
            IpAddr::V6(v6) => Ipv6Net::new(v6, 128).unwrap(),
        })
    }
}

/// Does this key look like a subnet or an address?
fn looks_like_subnet(k: &str) -> bool {
    let (addr, len) = match k.split_once('/') {
        Some((a, l)) => (a, Some(l)),
        None => (k, None),
    };
    !addr.is_empty()
        && addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        && addr.contains([':', '.'])
        && len.is_none_or(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()))
}

impl<V> SubnetMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value for a subnet.
    pub fn insert(&mut self, prefix: Ipv6Net, v: V) {
        self.table.insert(prefix.trunc(), v);
    }

    /// Longest prefix match.
    pub fn lookup(&self, ip: &Ipv6Addr) -> Option<&V> {
        self.table.get_lpm(&Ipv6Net::new(*ip, 128).unwrap()).map(|(_, v)| v)
    }

    /// Longest prefix match, with the matching prefix.
    pub fn lookup_prefix(&self, ip: &Ipv6Addr) -> Option<(Ipv6Net, &V)> {
        self.table.get_lpm(&Ipv6Net::new(*ip, 128).unwrap())
    }

    /// Exact match.
    pub fn get(&self, prefix: &Ipv6Net) -> Option<&V> {
        self.table.get(&prefix.trunc())
    }

    pub fn get_mut(&mut self, prefix: &Ipv6Net) -> Option<&mut V> {
        self.table.get_mut(&prefix.trunc())
    }

    /// Update or insert the value for a subnet.
    pub fn update(&mut self, prefix: Ipv6Net, f: impl FnOnce(Option<V>) -> V) {
        let p = prefix.trunc();
        let old = self.table.remove(&p);
        self.table.insert(p, f(old));
    }

    /// All entries, sorted by prefix.
    pub fn iter(&self) -> impl Iterator<Item = (Ipv6Net, &V)> {
        self.table.iter()
    }

    /// All supernets of a prefix (including itself), shortest first.
    pub fn supernets(&self, prefix: &Ipv6Net) -> Vec<(Ipv6Net, &V)> {
        let mut out: Vec<(Ipv6Net, &V)> = self.table.cover(&prefix.trunc()).collect();
        out.sort_by_key(|(p, _)| p.prefix_len());
        out
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.len() == 0
    }

    /// Map form, with IPv4 subnets unmapped.
    pub fn to_map(&self) -> BTreeMap<String, &V> {
        self.iter().map(|(p, v)| (unmap_prefix(&p), v)).collect()
    }

    /// Build from a map of subnets.
    pub fn from_map(m: impl IntoIterator<Item = (String, V)>) -> Result<Self, String> {
        let mut sm = SubnetMap::new();
        for (k, v) in m {
            sm.insert(parse_key(&k)?, v);
        }
        Ok(sm)
    }
}

impl<V: Serialize> Serialize for SubnetMap<V> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_map().serialize(s)
    }
}

impl<'de, V: DeserializeOwned> Deserialize<'de> for SubnetMap<V> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        let entries: Vec<(String, Value)> = match raw {
            Value::Null => vec![],
            Value::Object(m) if m.keys().all(|k| looks_like_subnet(k)) => m.into_iter().collect(),
            other => vec![("::/0".to_string(), other)],
        };
        let mut sm = SubnetMap::new();
        for (k, v) in entries {
            let prefix = parse_key(&k).map_err(serde::de::Error::custom)?;
            let value: V = crate::config::from_value(v).map_err(serde::de::Error::custom)?;
            sm.insert(prefix, value);
        }
        Ok(sm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{from_value, parse_str};

    fn ip(s: &str) -> Ipv6Addr {
        addr_to6(&s.parse().unwrap())
    }

    #[test]
    fn keys() {
        assert_eq!(parse_key("10.0.0.1").unwrap().to_string(), "::ffff:10.0.0.1/128");
        assert_eq!(parse_key("10.0.0.0/28").unwrap().to_string(), "::ffff:10.0.0.0/124");
        assert_eq!(parse_key("2001:db8:2::/48").unwrap().to_string(), "2001:db8:2::/48");
        assert!(parse_key("10.0.0").is_err());
        assert!(parse_key("10.0.0.0/33").is_err());
    }

    #[test]
    fn decode() {
        let sm: SubnetMap<String> = from_value(
            parse_str("203.0.113.0/24: customer1\n2001:db8:1::/64: customer2\n203.0.113.128/25: customer3\n").unwrap(),
        )
        .unwrap();
        assert_eq!(sm.lookup(&ip("203.0.113.18")).map(|s| s.as_str()), Some("customer1"));
        assert_eq!(sm.lookup(&ip("203.0.113.200")).map(|s| s.as_str()), Some("customer3"));
        assert_eq!(sm.lookup(&ip("2001:db8:1::5")).map(|s| s.as_str()), Some("customer2"));
        assert_eq!(sm.lookup(&ip("192.0.2.1")), None);

        // A single value applies everywhere.
        let sm: SubnetMap<u32> = from_value(parse_str("1000").unwrap()).unwrap();
        assert_eq!(sm.lookup(&ip("192.0.2.1")), Some(&1000));

        // A map not looking like subnets is a single value.
        #[derive(Debug, PartialEq, Deserialize, Default)]
        #[serde(default, rename_all = "kebab-case")]
        struct S {
            blip: String,
            blop: String,
        }
        let sm: SubnetMap<S> = from_value(parse_str("blip: some\nblop: thing\n").unwrap()).unwrap();
        assert_eq!(sm.lookup(&ip("::1")), Some(&S { blip: "some".into(), blop: "thing".into() }));

        assert!(from_value::<SubnetMap<u32>>(parse_str("10.0.0.0/40: 1").unwrap()).is_err());
    }

    #[test]
    fn to_map() {
        let sm =
            SubnetMap::from_map([("2001:db8::/64".to_string(), "hello"), ("::ffff:192.0.2.0/120".to_string(), "bye")])
                .unwrap();
        let m: BTreeMap<String, &str> = sm.to_map().into_iter().map(|(k, v)| (k, *v)).collect();
        assert_eq!(m, BTreeMap::from([("2001:db8::/64".into(), "hello"), ("192.0.2.0/24".into(), "bye")]));
    }

    #[test]
    fn supernets() {
        let sm = SubnetMap::from_map([
            ("0.0.0.0/0".to_string(), 1),
            ("10.0.0.0/8".to_string(), 2),
            ("10.1.0.0/16".to_string(), 3),
            ("192.0.2.0/24".to_string(), 4),
        ])
        .unwrap();
        let got: Vec<i32> = sm.supernets(&parse_key("10.1.2.0/24").unwrap()).into_iter().map(|(_, v)| *v).collect();
        assert_eq!(got, vec![1, 2, 3]);
    }
}
