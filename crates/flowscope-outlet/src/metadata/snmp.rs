// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SNMP metadata provider. Port of `outlet/metadata/provider/snmp`.

use super::snmp_config::{
    AuthProtocol as CfgAuth, Credentials as CfgCredentials, PrivProtocol as CfgPriv, SnmpConfiguration,
};
use super::{Answer, Exporter, Interface, Provider, ProviderError, Query};
use flowscope_common::helpers::subnetmap::addr_to6;
use flowscope_common::reporter::{CounterVec, Reporter, SummaryVec};
use flowscope_snmp::ber::parse_oid;
use flowscope_snmp::{AuthProtocol, Credentials, EngineState, Options, PrivProtocol, Value};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Metrics {
    successes: CounterVec,
    errors: CounterVec,
    retries: CounterVec,
    times: SummaryVec,
    v3_cache_hits: CounterVec,
    v3_cache_misses: CounterVec,
}

/// The SNMP provider.
pub struct SnmpProvider {
    config: SnmpConfiguration,
    agents: HashMap<Ipv6Addr, Ipv6Addr>,
    v3_cache: Mutex<HashMap<Ipv6Addr, EngineState>>,
    metrics: Metrics,
    last_error_log: Mutex<Option<Instant>>,
}

fn unmap(ip: &Ipv6Addr) -> IpAddr {
    match ip.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(*ip),
    }
}

fn convert_credentials(c: &CfgCredentials) -> Vec<Credentials> {
    if c.user_name.is_empty() {
        return c.communities.iter().map(|c| Credentials::Community(c.clone())).collect();
    }
    let auth = match c.authentication_protocol {
        CfgAuth::None => AuthProtocol::None,
        CfgAuth::Md5 => AuthProtocol::Md5,
        CfgAuth::Sha => AuthProtocol::Sha,
        CfgAuth::Sha224 => AuthProtocol::Sha224,
        CfgAuth::Sha256 => AuthProtocol::Sha256,
        CfgAuth::Sha384 => AuthProtocol::Sha384,
        CfgAuth::Sha512 => AuthProtocol::Sha512,
    };
    let privacy = match c.privacy_protocol {
        CfgPriv::None => PrivProtocol::None,
        CfgPriv::Des => PrivProtocol::Des,
        CfgPriv::Aes => PrivProtocol::Aes,
        CfgPriv::Aes192 => PrivProtocol::Aes192,
        CfgPriv::Aes256 => PrivProtocol::Aes256,
        CfgPriv::Aes192C => PrivProtocol::Aes192C,
        CfgPriv::Aes256C => PrivProtocol::Aes256C,
    };
    vec![Credentials::User {
        user_name: c.user_name.clone(),
        auth,
        auth_passphrase: c.authentication_passphrase.clone(),
        privacy,
        priv_passphrase: c.privacy_passphrase.clone(),
        context_name: c.context_name.clone(),
    }]
}

impl SnmpProvider {
    pub fn new(r: &Reporter, config: SnmpConfiguration) -> Result<Self, String> {
        let m = r.metrics("outlet/metadata/provider/snmp");
        let agents = config.agents.iter().map(|(e, a)| (addr_to6(e), addr_to6(a))).collect();
        Ok(SnmpProvider {
            agents,
            v3_cache: Mutex::new(HashMap::new()),
            metrics: Metrics {
                successes: m.counter_vec(
                    "poller_success_requests_total",
                    "Number of successful requests.",
                    &["exporter"],
                ),
                errors: m.counter_vec(
                    "poller_error_requests_total",
                    "Number of failed requests.",
                    &["exporter", "error"],
                ),
                retries: m.counter_vec("poller_retry_requests_total", "Number of retried requests.", &["exporter"]),
                times: m.summary_vec(
                    "poller_seconds",
                    "Time to successfully poll for values.",
                    Duration::from_secs(3600),
                    &["exporter"],
                ),
                v3_cache_hits: m.counter_vec(
                    "poller_v3_cache_hits_total",
                    "Number of SNMPv3 requests using cached engine state.",
                    &["exporter"],
                ),
                v3_cache_misses: m.counter_vec(
                    "poller_v3_cache_misses_total",
                    "Number of SNMPv3 requests needing discovery.",
                    &["exporter"],
                ),
            },
            config,
            last_error_log: Mutex::new(None),
        })
    }

    fn log_error(&self, exporter: &str, msg: &str) {
        let mut last = self.last_error_log.lock().unwrap();
        if last.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            *last = Some(Instant::now());
            tracing::error!(exporter, "{msg}");
        }
    }

    async fn poll(
        &self,
        exporter: Ipv6Addr,
        agent: Ipv6Addr,
        port: u16,
        if_index: u32,
    ) -> Result<Answer, ProviderError> {
        let exporter_str = unmap(&exporter).to_string();
        let target = SocketAddr::new(unmap(&agent), port);
        let options = Options { timeout: self.config.poller_timeout.as_std(), retries: self.config.poller_retries };
        let oids: Vec<Vec<u32>> = [
            "1.3.6.1.2.1.1.5.0".to_string(),
            format!("1.3.6.1.2.1.2.2.1.2.{if_index}"),
            format!("1.3.6.1.2.1.31.1.1.1.1.{if_index}"),
            format!("1.3.6.1.2.1.31.1.1.1.18.{if_index}"),
            format!("1.3.6.1.2.1.31.1.1.1.15.{if_index}"),
        ]
        .iter()
        .map(|s| parse_oid(s).unwrap())
        .collect();
        let credentials = self
            .config
            .credentials
            .lookup(&exporter)
            .map(convert_credentials)
            .unwrap_or_else(|| vec![Credentials::Community("public".into())]);
        let start = Instant::now();
        let retries = self.metrics.retries.with_label_values(&[&exporter_str]);
        let mut on_retry = || retries.inc();
        let mut results: Option<Vec<Value>> = None;
        let n = credentials.len();
        let mut success = false;
        for (idx, creds) in credentials.iter().enumerate() {
            let can_error = idx == n - 1 && !success;
            let response = match creds {
                Credentials::Community(c) => flowscope_snmp::get_v2c(target, c, &oids, &options, &mut on_retry).await,
                Credentials::User { .. } => {
                    let mut state = self.v3_cache.lock().unwrap().get(&exporter).cloned();
                    if state.is_some() {
                        self.metrics.v3_cache_hits.with_label_values(&[&exporter_str]).inc();
                    } else {
                        self.metrics.v3_cache_misses.with_label_values(&[&exporter_str]).inc();
                    }
                    let r = flowscope_snmp::get_v3(target, creds, &oids, &options, &mut state, &mut on_retry).await;
                    if r.is_ok() {
                        if let Some(s) = state {
                            self.v3_cache.lock().unwrap().insert(exporter, s);
                        }
                    } else {
                        self.v3_cache.lock().unwrap().remove(&exporter);
                    }
                    r
                }
            };
            let error = match response {
                Err(e) => Some(e.to_string()),
                Ok(r) if r.error_status != 0 && r.error_index == 0 => Some(format!("SNMP error {}", r.error_status)),
                Ok(r) if r.varbinds.len() != oids.len() => Some("SNMP mismatch on variable lengths".into()),
                Ok(r) => {
                    let values: Vec<Value> = r.varbinds.into_iter().map(|(_, v)| v).collect();
                    match &mut results {
                        None => results = Some(values),
                        Some(prev) => {
                            for (p, v) in prev.iter_mut().zip(values) {
                                if matches!(p, Value::NoSuchInstance | Value::NoSuchObject | Value::Null) {
                                    *p = v;
                                }
                            }
                        }
                    }
                    success = true;
                    None
                }
            };
            if let Some(e) = error
                && can_error
            {
                self.metrics.errors.with_label_values(&[&exporter_str, "get"]).inc();
                self.log_error(&exporter_str, &format!("unable to GET OIDs for index {if_index}: {e}"));
                return Err(ProviderError::Other(e));
            }
        }
        let Some(results) = results else {
            return Err(ProviderError::Other("no SNMP answer".into()));
        };
        self.metrics.times.with_label_values(&[&exporter_str]).observe(start.elapsed().as_secs_f64());

        let errors = &self.metrics.errors;
        let str_at = |idx: usize, what: &str| -> Option<String> {
            match &results[idx] {
                Value::OctetString(b) => Some(String::from_utf8_lossy(b).into_owned()),
                Value::NoSuchInstance | Value::NoSuchObject | Value::Null => {
                    errors.with_label_values(&[&exporter_str, &format!("{what} missing")]).inc();
                    None
                }
                _ => {
                    errors.with_label_values(&[&exporter_str, &format!("{what} unknown type")]).inc();
                    None
                }
            }
        };
        let Some(sys_name) = str_at(0, "sysname") else {
            return Err(ProviderError::Other("unable to get sysName".into()));
        };
        if sys_name.is_empty() {
            errors.with_label_values(&[&exporter_str, "system name empty"]).inc();
            return Err(ProviderError::Other("sysName empty".into()));
        }
        let descr = str_at(1, "ifdescr");
        let name = str_at(2, "ifname");
        let alias = str_at(3, "ifalias");
        let speed = match &results[4] {
            Value::Gauge32(v) => Some(*v as u64),
            Value::NoSuchInstance | Value::NoSuchObject | Value::Null => {
                errors.with_label_values(&[&exporter_str, "ifspeed missing"]).inc();
                None
            }
            _ => {
                errors.with_label_values(&[&exporter_str, "ifspeed unknown type"]).inc();
                None
            }
        };
        let Some(name) = name else { return Ok(Answer::default()) };
        let description = match (&alias, &descr) {
            (Some(a), _) if *a != name => a.clone(),
            (_, Some(d)) if *d != name => d.clone(),
            _ => String::new(),
        };
        let Some(speed) = speed else { return Ok(Answer::default()) };
        self.metrics.successes.with_label_values(&[&exporter_str]).inc();
        Ok(Answer {
            found: true,
            exporter: Exporter { name: sys_name, ..Default::default() },
            interface: Interface { name, description, speed, ..Default::default() },
        })
    }
}

impl Provider for SnmpProvider {
    fn query(&self, query: Query) -> BoxFuture<'_, Result<Answer, ProviderError>> {
        Box::pin(async move {
            let agent = self.agents.get(&query.exporter_ip).copied().unwrap_or(query.exporter_ip);
            let port = self.config.ports.lookup(&query.exporter_ip).copied().unwrap_or(161);
            self.poll(query.exporter_ip, agent, port, query.if_index).await
        })
    }
}
