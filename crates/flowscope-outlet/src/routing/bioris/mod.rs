// SPDX-FileCopyrightText: 2024 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! BioRIS routing provider: longest prefix match lookups against bio-rd
//! RIS instances over gRPC. Port of `outlet/routing/provider/bioris`.

use super::{LargeCommunity, LookupResult, Provider};
use flowscope_common::config::Duration;
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::{unmap_ip, validate_listen};
use flowscope_common::reporter::{CounterVec, GaugeVec, Reporter};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, RwLock};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

/// Code generated from the bio-rd protobuf files.
#[allow(clippy::all, missing_docs)]
pub mod pb {
    pub mod bio {
        pub mod net {
            tonic::include_proto!("bio.net");
        }
        pub mod route {
            tonic::include_proto!("bio.route");
        }
        pub mod ris {
            tonic::include_proto!("bio.ris");
        }
    }
    pub use bio::net::{Ip, Prefix, ip::Version};
    pub use bio::ris::routing_information_service_client::RoutingInformationServiceClient;
    pub use bio::ris::*;
    pub use bio::route::{AsPathSegment, BgpPath, LargeCommunity, Path, Route};
}

/// BioRIS provider configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct BiorisConfiguration {
    /// RIS instances to query.
    #[serde(rename = "ris-instances")]
    pub ris_instances: Vec<RisInstance>,
    /// Timeout of a lookup.
    pub timeout: Duration,
    /// Interval between two refreshes of the list of routers.
    pub refresh: Duration,
    /// Timeout to get the list of routers from an instance.
    pub refresh_timeout: Duration,
}

impl Default for BiorisConfiguration {
    fn default() -> Self {
        BiorisConfiguration {
            ris_instances: vec![],
            timeout: Duration::from_millis(200),
            refresh: Duration::from_secs(30 * 60),
            refresh_timeout: Duration::from_secs(10),
        }
    }
}

/// A RIS instance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct RisInstance {
    #[serde(rename = "grpc-addr")]
    pub grpc_addr: String,
    #[serde(rename = "grpc-secure")]
    pub grpc_secure: bool,
    #[serde(rename = "vrf-id")]
    pub vrf_id: u64,
    pub vrf: String,
}

impl BiorisConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        for i in &self.ris_instances {
            validate_listen(&i.grpc_addr).map_err(|e| format!("routing.provider.ris-instances: {e}"))?;
            if i.vrf_id != 0 && !i.vrf.is_empty() {
                return Err("vrf and vrf-id are exclusive".into());
            }
        }
        if self.timeout.as_millis() < 1 {
            return Err("routing.provider.timeout should be at least 1ms".into());
        }
        if self.refresh.as_secs() < 1 || self.refresh_timeout.as_secs() < 1 {
            return Err("routing.provider.refresh and refresh-timeout should be at least 1s".into());
        }
        Ok(())
    }
}

/// Errors, with the same messages as the Go version.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("no router")]
    NoRouter,
    #[error("no route found")]
    NoRouteFound,
    #[error("no path found")]
    NoPathFound,
    #[error("invalid next hop")]
    InvalidNextHop,
    #[error("lpm lookup timeout")]
    Timeout,
    #[error("lpm lookup failed: {0}")]
    Failed(String),
}

struct Metrics {
    connection_up: GaugeVec,
    known_routers: GaugeVec,
    lpm_requests: CounterVec,
    lpm_request_errors: CounterVec,
    lpm_request_timeouts: CounterVec,
    lpm_request_success: CounterVec,
    router_chosen_agent_id_match: CounterVec,
    router_chosen_fallback: CounterVec,
    grpc_started: CounterVec,
    grpc_handled: CounterVec,
    grpc_msg_sent: CounterVec,
    grpc_msg_received: CounterVec,
}

const GRPC_SERVICE: &str = "bio.ris.RoutingInformationService";

/// A connection to a RIS instance.
#[derive(Debug)]
pub struct RisInstanceRuntime {
    pub config: RisInstance,
    client: Option<pb::RoutingInformationServiceClient<Channel>>,
}

struct Inner {
    config: BiorisConfiguration,
    instances: Vec<Arc<RisInstanceRuntime>>,
    /// RIS instances knowing each router.
    routers: RwLock<HashMap<Ipv6Addr, Vec<Arc<RisInstanceRuntime>>>>,
    metrics: Metrics,
}

/// The BioRIS provider.
pub struct BiorisProvider {
    inner: Arc<Inner>,
    daemon: Daemon,
}

fn to6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

impl BiorisProvider {
    pub fn new(r: &Reporter, config: BiorisConfiguration, daemon: &Daemon) -> Result<Self, String> {
        let m = r.metrics("outlet/routing/provider/bioris");
        let rr = &["ris", "router"];
        let grpc = &["grpc_type", "grpc_service", "grpc_method"];
        let metrics = Metrics {
            connection_up: m.gauge_vec("connection_up", "Connection to BioRIS instance up.", &["ris"]),
            known_routers: m.gauge_vec("known_routers_total", "Number of known routers per RIS.", &["ris"]),
            lpm_requests: m.counter_vec("lpm_requests_total", "Number of LPM requests per RIS and router.", rr),
            lpm_request_errors: m.counter_vec(
                "lpm_request_errors_total",
                "Number of failed LPM requests per RIS and router.",
                rr,
            ),
            lpm_request_timeouts: m.counter_vec(
                "lpm_request_timeouts_total",
                "Number of LPM requests that timed out per RIS and router.",
                rr,
            ),
            lpm_request_success: m.counter_vec(
                "lpm_success_requests_total",
                "Number of successful LPM requests per RIS and router.",
                rr,
            ),
            router_chosen_agent_id_match: m.counter_vec(
                "router_agentid_requests_total",
                "Number of times the router/ris combination was returned with an exact match of the agent ID.",
                rr,
            ),
            router_chosen_fallback: m.counter_vec(
                "router_fallback_requests_total",
                "Number of times the router/ris combination was returned without an exact match of the agent ID.",
                rr,
            ),
            grpc_started: m.counter_vec(
                "grpc_client_started_total",
                "Total number of RPCs started on the client.",
                grpc,
            ),
            grpc_handled: m.counter_vec(
                "grpc_client_handled_total",
                "Total number of RPCs completed by the client, regardless of success or failure.",
                &["grpc_type", "grpc_service", "grpc_method", "grpc_code"],
            ),
            grpc_msg_sent: m.counter_vec(
                "grpc_client_msg_sent_total",
                "Total number of gRPC stream messages sent by the client.",
                grpc,
            ),
            grpc_msg_received: m.counter_vec(
                "grpc_client_msg_received_total",
                "Total number of RPC stream messages received by the client.",
                grpc,
            ),
        };
        let mut instances = vec![];
        for c in &config.ris_instances {
            metrics.connection_up.with_label_values(&[&c.grpc_addr]).set(0.0);
            let scheme = if c.grpc_secure { "https" } else { "http" };
            let mut endpoint = Endpoint::from_shared(format!("{scheme}://{}", c.grpc_addr))
                .map_err(|e| format!("error while dialing RIS {}: {e}", c.grpc_addr))?
                .connect_timeout(*config.refresh_timeout);
            if c.grpc_secure {
                endpoint = endpoint
                    .tls_config(ClientTlsConfig::new().with_native_roots())
                    .map_err(|e| format!("error while dialing RIS {}: {e}", c.grpc_addr))?;
            }
            // Like grpc.NewClient, the connection happens on first use.
            let client = pb::RoutingInformationServiceClient::new(endpoint.connect_lazy());
            instances.push(Arc::new(RisInstanceRuntime { config: c.clone(), client: Some(client) }));
        }
        Ok(BiorisProvider {
            inner: Arc::new(Inner { config, instances, routers: RwLock::new(HashMap::new()), metrics }),
            daemon: daemon.clone(),
        })
    }

    #[cfg(test)]
    fn with_routers(&self, routers: HashMap<Ipv6Addr, Vec<Arc<RisInstanceRuntime>>>) {
        *self.inner.routers.write().unwrap() = routers;
    }
}

impl Provider for BiorisProvider {
    fn start(&self) -> Result<(), String> {
        tracing::info!("starting BioRIS provider");
        let inner = self.inner.clone();
        let token = self.daemon.token();
        self.daemon.spawn_background(async move {
            loop {
                inner.refresh().await;
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(*inner.config.refresh) => {}
                }
            }
        });
        Ok(())
    }

    fn lookup(
        &self,
        ip: Ipv6Addr,
        _nh: Option<Ipv6Addr>,
        agent: Ipv6Addr,
    ) -> BoxFuture<'_, Result<LookupResult, String>> {
        Box::pin(async move { self.inner.lookup(ip, agent).await.map_err(|e| e.to_string()) })
    }
}

impl Inner {
    /// Count a unary RPC like go-grpc-prometheus, and keep track of the
    /// connection state. tonic does not expose it, so a transport error
    /// means down and a reply means up.
    fn record_rpc(&self, ris: &str, method: &str, status: Result<(), &tonic::Status>) {
        let m = &self.metrics;
        let labels = ["unary", GRPC_SERVICE, method];
        m.grpc_started.with_label_values(&labels).inc();
        m.grpc_msg_sent.with_label_values(&labels).inc();
        m.grpc_msg_received.with_label_values(&labels).inc();
        let code = match status {
            Ok(()) => tonic::Code::Ok,
            Err(s) => s.code(),
        };
        m.grpc_handled.with_label_values(&["unary", GRPC_SERVICE, method, &format!("{code:?}")]).inc();
        let up = !matches!(code, tonic::Code::Unavailable | tonic::Code::DeadlineExceeded);
        m.connection_up.with_label_values(&[ris]).set(if up { 1.0 } else { 0.0 });
    }

    /// Get the list of routers from each instance.
    async fn refresh(&self) {
        let mut routers: HashMap<Ipv6Addr, Vec<Arc<RisInstanceRuntime>>> = HashMap::new();
        for instance in &self.instances {
            let addr = &instance.config.grpc_addr;
            let Some(mut client) = instance.client.clone() else { continue };
            let request = tonic::Request::new(pb::GetRoutersRequest {});
            let reply = tokio::time::timeout(*self.config.refresh_timeout, client.get_routers(request)).await;
            let reply = match reply {
                Err(_) => {
                    let status = tonic::Status::deadline_exceeded("timeout");
                    self.record_rpc(addr, "GetRouters", Err(&status));
                    tracing::error!(error = %status, "error while getting routers from {addr}");
                    continue;
                }
                Ok(Err(status)) => {
                    self.record_rpc(addr, "GetRouters", Err(&status));
                    tracing::error!(error = %status, "error while getting routers from {addr}");
                    continue;
                }
                Ok(Ok(r)) => {
                    self.record_rpc(addr, "GetRouters", Ok(()));
                    r.into_inner()
                }
            };
            let m = &self.metrics;
            m.known_routers.with_label_values(&[addr]).set(0.0);
            for router in reply.routers {
                let Ok(ip) = router.address.parse::<IpAddr>() else {
                    tracing::error!("error while parsing router address {}", router.address);
                    continue;
                };
                routers.entry(to6(ip)).or_default().push(instance.clone());
                m.known_routers.with_label_values(&[addr]).inc();
                let labels = [addr.as_str(), router.address.as_str()];
                for c in [
                    &m.lpm_request_timeouts,
                    &m.lpm_request_errors,
                    &m.lpm_request_success,
                    &m.lpm_requests,
                    &m.router_chosen_agent_id_match,
                    &m.router_chosen_fallback,
                ] {
                    c.with_label_values(&labels);
                }
            }
        }
        *self.routers.write().unwrap() = routers;
    }

    /// Choose the router for an agent: the agent itself when a RIS knows
    /// it, otherwise the router with the lowest address (Go picked one in
    /// map order, that is at random). Then a random RIS knowing it.
    fn choose_router(&self, agent: Ipv6Addr) -> Result<(Ipv6Addr, Arc<RisInstanceRuntime>), Error> {
        let routers = self.routers.read().unwrap();
        let (router, exact) = match routers.get_key_value(&agent) {
            Some((r, _)) => (*r, true),
            None => (*routers.keys().min().ok_or(Error::NoRouter)?, false),
        };
        let candidates = &routers[&router];
        let ris = candidates[rand::random_range(0..candidates.len())].clone();
        let labels = [ris.config.grpc_addr.as_str(), &unmap_ip(&router)];
        if exact {
            self.metrics.router_chosen_agent_id_match.with_label_values(&labels).inc();
        } else {
            self.metrics.router_chosen_fallback.with_label_values(&labels).inc();
        }
        Ok((router, ris))
    }

    async fn lookup(&self, ip: Ipv6Addr, agent: Ipv6Addr) -> Result<LookupResult, Error> {
        let (router, ris) = self.choose_router(agent)?;
        let router_str = unmap_ip(&router);
        let addr = ris.config.grpc_addr.as_str();
        let labels = [addr, router_str.as_str()];
        let m = &self.metrics;
        m.lpm_requests.with_label_values(&labels).inc();
        let Some(mut client) = ris.client.clone() else { return Err(Error::NoRouter) };
        let request = pb::LpmRequest {
            router: router_str.clone(),
            vrf_id: ris.config.vrf_id,
            vrf: ris.config.vrf.clone(),
            pfx: Some(host_prefix(ip)),
        };
        let reply = match tokio::time::timeout(*self.config.timeout, client.lpm(request)).await {
            Err(_) => {
                self.record_rpc(addr, "LPM", Err(&tonic::Status::deadline_exceeded("timeout")));
                m.lpm_request_timeouts.with_label_values(&labels).inc();
                return Err(Error::Timeout);
            }
            Ok(Err(status)) => {
                self.record_rpc(addr, "LPM", Err(&status));
                m.lpm_request_errors.with_label_values(&labels).inc();
                return Err(Error::Failed(status.to_string()));
            }
            Ok(Ok(r)) => {
                self.record_rpc(addr, "LPM", Ok(()));
                r.into_inner()
            }
        };
        m.lpm_request_success.with_label_values(&labels).inc();
        lpm_response_to_lookup_result(&reply)
    }
}

/// The /32 or /128 prefix for an address.
fn host_prefix(ip: Ipv6Addr) -> pb::Prefix {
    let (address, length) = match ip.to_ipv4_mapped() {
        Some(v4) => (pb::Ip { higher: 0, lower: u32::from(v4) as u64, version: pb::Version::IPv4 as i32 }, 32),
        None => {
            let v = u128::from(ip);
            (pb::Ip { higher: (v >> 64) as u64, lower: v as u64, version: pb::Version::IPv6 as i32 }, 128)
        }
    };
    pb::Prefix { address: Some(address), length }
}

/// Convert an address from bio-rd (IPv4 is mapped to IPv6).
fn ip_from_proto(ip: &pb::Ip) -> Option<Ipv6Addr> {
    if ip.version == pb::Version::IPv4 as i32 {
        let v = u32::try_from(ip.lower).ok()?;
        Some(Ipv4Addr::from(v).to_ipv6_mapped())
    } else {
        Some(Ipv6Addr::from(((ip.higher as u128) << 64) | ip.lower as u128))
    }
}

/// Turn a LPM response into a lookup result: the most specific route, its
/// first path.
pub fn lpm_response_to_lookup_result(lpm: &pb::LpmResponse) -> Result<LookupResult, Error> {
    let mut best: Option<&pb::Route> = None;
    for r in &lpm.routes {
        let len = r.pfx.map(|p| p.length as i64).unwrap_or(0);
        if best.is_none_or(|b| len > b.pfx.map(|p| p.length as i64).unwrap_or(0)) {
            best = Some(r);
        }
    }
    let route = best.ok_or(Error::NoRouteFound)?;
    let bgp = route.paths.first().and_then(|p| p.bgp_path.as_ref()).ok_or(Error::NoPathFound)?;
    let mut res = LookupResult {
        communities: bgp.communities.clone(),
        large_communities: bgp
            .large_communities
            .iter()
            .map(|c| LargeCommunity {
                asn: c.global_administrator,
                local_data1: c.data_part1,
                local_data2: c.data_part2,
            })
            .collect(),
        as_path: bgp.as_path.iter().flat_map(|s| s.asns.iter().copied()).collect(),
        net_mask: route.pfx.map(|p| p.length as u8).unwrap_or(0),
        ..Default::default()
    };
    res.asn = res.as_path.last().copied().unwrap_or(0);
    if let Some(nh) = &bgp.next_hop {
        res.next_hop = Some(ip_from_proto(nh).ok_or(Error::InvalidNextHop)?);
    }
    Ok(res)
}

#[cfg(test)]
mod tests;
