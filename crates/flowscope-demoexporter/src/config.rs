// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Configuration of the demo exporter.

use flowscope_common::config::Validate;
use flowscope_common::config::duration::Duration;
use flowscope_common::helpers::validate_listen;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;

/// "host:port", like Go's `hostname_port` validation.
fn validate_target(target: &str) -> Result<(), String> {
    match target.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0) => Ok(()),
        _ => Err(format!("{target:?} is not a host and a port")),
    }
}

/// Configuration of the SNMP agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct SnmpConfiguration {
    /// System name of the exporter.
    pub name: String,
    /// Interfaces of the exporter: their index and their description.
    pub interfaces: BTreeMap<u32, String>,
    /// Address the agent listens to.
    pub listen: String,
}

impl Default for SnmpConfiguration {
    fn default() -> Self {
        SnmpConfiguration { name: String::new(), interfaces: BTreeMap::new(), listen: ":161".into() }
    }
}

impl Validate for SnmpConfiguration {
    fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("SNMP.Name is required".into());
        }
        if self.interfaces.is_empty() {
            return Err("SNMP.Interfaces: at least one interface is required".into());
        }
        for (index, description) in &self.interfaces {
            if *index == 0 {
                return Err("SNMP.Interfaces: an index should be at least 1".into());
            }
            if description.len() < 3 {
                return Err(format!("SNMP.Interfaces[{index}]: the description should have at least 3 characters"));
            }
        }
        validate_listen(&self.listen).map_err(|e| format!("SNMP.Listen: {e}"))
    }
}

/// A standard community, written "ASN:value".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Community(pub u32);

impl std::str::FromStr for Community {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let error = || "community should be ASN:XX".to_string();
        let (asn, local) = text.split_once(':').ok_or_else(error)?;
        let asn: u16 = asn.parse().map_err(|_| error())?;
        let local: u16 = local.parse().map_err(|_| error())?;
        Ok(Community(((asn as u32) << 16) + local as u32))
    }
}

impl std::fmt::Display for Community {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.0 >> 16, self.0 & 0xffff)
    }
}

/// A large community, written "ASN:value:value".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LargeCommunity(pub u32, pub u32, pub u32);

impl std::str::FromStr for LargeCommunity {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let error = || "community should be ASN:XX:YY".to_string();
        let parts: Vec<u32> = text.split(':').map(|p| p.parse().map_err(|_| error())).collect::<Result<_, _>>()?;
        match parts[..] {
            [asn, a, b] => Ok(LargeCommunity(asn, a, b)),
            _ => Err(error()),
        }
    }
}

impl std::fmt::Display for LargeCommunity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.0, self.1, self.2)
    }
}

macro_rules! text_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}
text_serde!(Community);
text_serde!(LargeCommunity);

/// Routes to announce over BMP.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct RouteConfiguration {
    pub prefixes: Vec<IpNet>,
    #[serde(rename = "aspath")]
    pub as_path: Vec<u32>,
    pub communities: Vec<Community>,
    pub large_communities: Vec<LargeCommunity>,
}

/// Configuration of the BMP client. One peer is simulated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct BmpConfiguration {
    /// Collector to send the routes to. Empty to disable BMP.
    pub target: String,
    pub routes: Vec<RouteConfiguration>,
    #[serde(rename = "local-asn")]
    pub local_asn: u16,
    #[serde(rename = "peer-asn")]
    pub peer_asn: u16,
    #[serde(rename = "local-ip")]
    pub local_ip: IpAddr,
    #[serde(rename = "peer-ip")]
    pub peer_ip: IpAddr,
    /// Time to wait before connecting again.
    pub retry_after: Duration,
    /// Time between two statistics messages, sent to notice a closed
    /// connection.
    pub stats_delay: Duration,
}

impl Default for BmpConfiguration {
    fn default() -> Self {
        BmpConfiguration {
            target: String::new(),
            routes: vec![],
            local_asn: 64496,
            peer_asn: 64497,
            local_ip: "2001:db8::1".parse().unwrap(),
            peer_ip: "2001:db8::2".parse().unwrap(),
            retry_after: Duration::from_secs(5),
            stats_delay: Duration::from_secs(60),
        }
    }
}

impl Validate for BmpConfiguration {
    fn validate(&self) -> Result<(), String> {
        if !self.target.is_empty() {
            validate_target(&self.target).map_err(|e| format!("BMP.Target: {e}"))?;
        }
        if self.local_asn == 0 || self.peer_asn == 0 {
            return Err("BMP: LocalASN and PeerASN are required".into());
        }
        for (index, route) in self.routes.iter().enumerate() {
            if route.prefixes.is_empty() {
                return Err(format!("BMP.Routes[{index}].Prefixes: at least one prefix is required"));
            }
            if route.as_path.is_empty() {
                return Err(format!("BMP.Routes[{index}].ASPath: at least one AS is required"));
            }
        }
        Ok(())
    }
}

/// A kind of flows to generate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FlowConfiguration {
    /// Flows to create each second.
    pub per_second: f64,
    /// Input interfaces to pick from.
    pub in_if_index: Vec<u32>,
    /// Output interfaces to pick from.
    pub out_if_index: Vec<u32>,
    /// Time of the day with the most traffic.
    pub peak_hour: Duration,
    /// How many times more flows there are at the peak hour.
    pub multiplier: f64,
    pub src_net: Option<IpNet>,
    pub dst_net: Option<IpNet>,
    #[serde(rename = "src-as")]
    pub src_as: Vec<u32>,
    #[serde(rename = "dst-as")]
    pub dst_as: Vec<u32>,
    /// Source ports to pick from. 0 or nothing for a high random port.
    pub src_port: Vec<u16>,
    /// Destination ports to pick from. 0 or nothing for a high random port.
    pub dst_port: Vec<u16>,
    /// Protocols to pick from: tcp, udp or icmp.
    pub protocol: Vec<String>,
    /// Size of the packets. 0 for a random size.
    pub size: u32,
    /// When set, each flow gets a flow in the opposite direction, with its
    /// size multiplied by this ratio.
    pub reverse_direction_ratio: f32,
}

/// Configuration of the flow generator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FlowsConfiguration {
    /// Sampling rate announced by the exporter.
    pub sampling_rate: u32,
    pub flows: Vec<FlowConfiguration>,
    /// Where to send the flows ("host:port").
    pub target: String,
    /// Without a seed of their own, two exporters with the same flows send
    /// the same data.
    pub seed: i64,
}

impl Default for FlowsConfiguration {
    fn default() -> Self {
        FlowsConfiguration { sampling_rate: 1000, flows: vec![], target: String::new(), seed: 0 }
    }
}

impl Validate for FlowsConfiguration {
    fn validate(&self) -> Result<(), String> {
        if self.sampling_rate < 1 {
            return Err("Flows.SamplingRate should be at least 1".into());
        }
        if self.flows.is_empty() {
            return Err("Flows.Flows: at least one flow is required".into());
        }
        validate_target(&self.target).map_err(|e| format!("Flows.Target: {e}"))?;
        for (index, flow) in self.flows.iter().enumerate() {
            let error = |message: &str| Err(format!("Flows.Flows[{index}].{message}"));
            if flow.per_second.is_nan() || flow.per_second <= 0.0 {
                return error("PerSecond should be greater than 0");
            }
            if flow.multiplier.is_nan() || flow.multiplier <= 0.0 {
                return error("Multiplier should be greater than 0");
            }
            if flow.in_if_index.is_empty() || flow.in_if_index.contains(&0) {
                return error("InIfIndex: at least one index, starting from 1, is required");
            }
            if flow.out_if_index.is_empty() || flow.out_if_index.contains(&0) {
                return error("OutIfIndex: at least one index, starting from 1, is required");
            }
            // Go requires a peak hour, so midnight is written "24h".
            if flow.peak_hour.is_zero() || flow.peak_hour.as_std().as_secs() > 24 * 3600 {
                return error("PeakHour should be between 1ns and 24h");
            }
            let (Some(src), Some(dst)) = (flow.src_net, flow.dst_net) else {
                return error("SrcNet and DstNet are required");
            };
            if src.addr().is_ipv4() != dst.addr().is_ipv4() {
                return error("SrcNet and DstNet should be of the same family");
            }
            if flow.src_as.is_empty() || flow.dst_as.is_empty() {
                return error("SrcAS and DstAS are required");
            }
            if flow.protocol.is_empty() || flow.protocol.iter().any(|p| !["tcp", "udp", "icmp"].contains(&p.as_str())) {
                return error("Protocol should be tcp, udp or icmp");
            }
            if flow.size != 0 && !(64..=9000).contains(&flow.size) {
                return error("Size should be between 64 and 9000");
            }
            if flow.reverse_direction_ratio.is_nan() || flow.reverse_direction_ratio < 0.0 {
                return error("ReverseDirectionRatio should not be negative");
            }
        }
        Ok(())
    }
}
