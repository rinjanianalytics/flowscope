// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Attributes of networks (name, role, site, region, tenant, location, AS
//! number) from the configuration, remote data sources and GeoIP databases.
//! Port of `outlet/networks`.
//!
//! All sources are merged into one prefix tree. Attributes of a prefix are
//! inherited by its more specific prefixes, so a lookup is a single
//! longest-prefix match.

use crate::geoip::GeoIp;
use flowscope_common::config::Duration;
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::subnetmap::{SubnetMap, parse_key};
use flowscope_common::remotedatasource::{RemoteDataSource, Source};
use flowscope_common::reporter::{Counter, Gauge, Reporter};
use futures::future::BoxFuture;
use ipnet::Ipv6Net;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::Ipv6Addr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// Attributes of a network.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct NetworkAttributes {
    pub name: String,
    pub role: String,
    pub site: String,
    pub region: String,
    pub city: String,
    pub state: String,
    pub country: String,
    pub tenant: String,
    pub asn: u32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct AttributesFields {
    name: String,
    role: String,
    site: String,
    region: String,
    city: String,
    state: String,
    country: String,
    tenant: String,
    asn: u32,
}

impl<'de> Deserialize<'de> for NetworkAttributes {
    /// A string is the name of the network.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        if let serde_json::Value::String(name) = raw {
            return Ok(NetworkAttributes { name, ..Default::default() });
        }
        let f: AttributesFields = flowscope_common::config::from_value(raw).map_err(serde::de::Error::custom)?;
        Ok(NetworkAttributes {
            name: f.name,
            role: f.role,
            site: f.site,
            region: f.region,
            city: f.city,
            state: f.state,
            country: f.country,
            tenant: f.tenant,
            asn: f.asn,
        })
    }
}

impl NetworkAttributes {
    /// Merge: set fields of `new` override the existing ones.
    pub fn merge(&mut self, new: &NetworkAttributes) {
        if new.asn != 0 {
            self.asn = new.asn;
        }
        for (dst, src) in [
            (&mut self.name, &new.name),
            (&mut self.region, &new.region),
            (&mut self.site, &new.site),
            (&mut self.role, &new.role),
            (&mut self.tenant, &new.tenant),
            (&mut self.country, &new.country),
            (&mut self.state, &new.state),
            (&mut self.city, &new.city),
        ] {
            if !src.is_empty() {
                dst.clone_from(src);
            }
        }
    }
}

/// Networks configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    pub networks: SubnetMap<NetworkAttributes>,
    pub network_sources: BTreeMap<String, Source>,
    /// How long to wait for the network sources at startup.
    pub network_sources_timeout: Duration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            networks: SubnetMap::new(),
            network_sources: BTreeMap::new(),
            network_sources_timeout: Duration::from_secs(10),
        }
    }
}

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        for (name, s) in &self.network_sources {
            s.validate().map_err(|e| format!("network source {name}: {e}"))?;
        }
        Ok(())
    }
}

/// A network from a remote data source.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
#[derive(Default)]
struct ExternalNetwork {
    prefix: String,
    name: String,
    role: String,
    site: String,
    region: String,
    city: String,
    state: String,
    country: String,
    tenant: String,
    asn: u32,
}

impl ExternalNetwork {
    fn attributes(&self) -> NetworkAttributes {
        NetworkAttributes {
            name: self.name.clone(),
            role: self.role.clone(),
            site: self.site.clone(),
            region: self.region.clone(),
            city: self.city.clone(),
            state: self.state.clone(),
            country: self.country.clone(),
            tenant: self.tenant.clone(),
            asn: self.asn,
        }
    }
}

struct Metrics {
    rebuilds: Counter,
    rebuild_time: Counter,
    rebuild_last_time: Gauge,
    prefixes: Gauge,
}

/// The networks component.
pub struct Networks {
    config: Configuration,
    geoip: Option<Arc<GeoIp>>,
    sources: Mutex<HashMap<String, Vec<ExternalNetwork>>>,
    tree: RwLock<Arc<SubnetMap<Arc<NetworkAttributes>>>>,
    rebuild_lock: Mutex<()>,
    metrics: Metrics,
}

impl Networks {
    pub fn new(r: &Reporter, config: Configuration, geoip: Option<Arc<GeoIp>>) -> Arc<Networks> {
        let m = r.metrics("outlet/networks");
        Arc::new(Networks {
            config,
            geoip,
            sources: Mutex::new(HashMap::new()),
            tree: RwLock::new(Arc::new(SubnetMap::new())),
            rebuild_lock: Mutex::new(()),
            metrics: Metrics {
                rebuilds: m.counter("rebuilds_total", "Number of times the networks were rebuilt."),
                rebuild_time: m.counter("rebuild_time_seconds_total", "Cumulated time spent rebuilding the networks."),
                rebuild_last_time: m
                    .gauge("rebuild_last_time_seconds", "Time spent during the last rebuild of the networks."),
                prefixes: m.gauge("prefixes", "Number of prefixes, including the ones from the GeoIP databases."),
            },
        })
    }

    /// Look up the attributes of an address.
    pub fn lookup(&self, ip: &Ipv6Addr) -> Arc<NetworkAttributes> {
        let tree = self.tree.read().unwrap().clone();
        tree.lookup(ip).cloned().unwrap_or_default()
    }

    /// Build the tree, then rebuild on GeoIP changes and source updates.
    pub async fn start(self: &Arc<Self>, r: &Reporter, daemon: &Daemon) {
        tracing::info!("starting networks component");
        let mut geoip_updates = self.geoip.as_ref().map(|g| g.notify());
        {
            let this = self.clone();
            tokio::task::spawn_blocking(move || this.rebuild()).await.ok();
        }
        if let Some(mut rx) = geoip_updates.take() {
            let this = self.clone();
            let token = daemon.token();
            daemon.spawn_background(async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return,
                        msg = rx.recv() => {
                            if msg.is_none() { return; }
                            let this = this.clone();
                            tokio::task::spawn_blocking(move || this.rebuild()).await.ok();
                        }
                    }
                }
            });
        }
        let fetcher =
            RemoteDataSource::new(r, "outlet/networks", "network_source", self.config.network_sources.clone());
        let this = self.clone();
        let mut ready = fetcher.start(
            daemon,
            Arc::new(move |name: String, source: Source| -> BoxFuture<'static, Result<usize, String>> {
                let this = this.clone();
                Box::pin(async move {
                    let results: Vec<ExternalNetwork> = RemoteDataSource::fetch(&name, &source).await?;
                    let count = results.len();
                    let changed = {
                        let mut sources = this.sources.lock().unwrap();
                        let changed = sources.get(&name) != Some(&results);
                        sources.insert(name, results);
                        changed
                    };
                    if changed {
                        tokio::task::spawn_blocking(move || this.rebuild()).await.ok();
                    }
                    Ok(count)
                })
            }),
        );
        if !self.config.network_sources.is_empty() && !self.config.network_sources_timeout.is_zero() {
            let wait = ready.wait_for(|r| *r);
            if tokio::time::timeout(self.config.network_sources_timeout.as_std(), wait).await.is_err() {
                tracing::warn!("network sources not ready, continuing without them");
            }
        }
        tracing::info!("networks component started");
    }

    /// Rebuild the tree from all the sources.
    pub fn rebuild(&self) {
        let _guard = self.rebuild_lock.lock().unwrap();
        let start = Instant::now();
        let mut merged: SubnetMap<NetworkAttributes> = SubnetMap::new();
        let mut update = |prefix: Ipv6Net, attrs: &NetworkAttributes| {
            merged.update(prefix, |old| {
                let mut a = old.unwrap_or_default();
                a.merge(attrs);
                a
            });
        };
        if let Some(g) = &self.geoip {
            g.iter_asn(&mut |p, asn| {
                if let Ok(prefix) = parse_key(p) {
                    update(prefix, &NetworkAttributes { asn, ..Default::default() });
                }
            });
            g.iter_geo(&mut |p, info| {
                if let Ok(prefix) = parse_key(p) {
                    update(
                        prefix,
                        &NetworkAttributes {
                            state: info.state,
                            country: info.country,
                            city: info.city,
                            ..Default::default()
                        },
                    );
                }
            });
        }
        for list in self.sources.lock().unwrap().values() {
            for n in list {
                match parse_key(&n.prefix) {
                    Ok(prefix) => update(prefix, &n.attributes()),
                    Err(e) => tracing::warn!(error = %e, "invalid prefix from network source"),
                }
            }
        }
        for (prefix, attrs) in self.config.networks.iter() {
            update(prefix, attrs);
        }
        update("::ffff:0:0/96".parse().unwrap(), &NetworkAttributes::default());

        // Flatten: each prefix inherits the attributes of its ancestors.
        let mut flattened: SubnetMap<Arc<NetworkAttributes>> = SubnetMap::new();
        let mut pool: HashMap<NetworkAttributes, Arc<NetworkAttributes>> = HashMap::new();
        let mut ancestors: Vec<(Ipv6Net, NetworkAttributes)> = Vec::new();
        for (prefix, attrs) in merged.iter() {
            while ancestors.last().is_some_and(|(p, _)| !p.contains(&prefix.network())) {
                ancestors.pop();
            }
            let mut a = match ancestors.last() {
                Some((_, parent)) => parent.clone(),
                None => NetworkAttributes::default(),
            };
            a.merge(attrs);
            ancestors.push((prefix, a.clone()));
            let shared = pool.entry(a.clone()).or_insert_with(|| Arc::new(a)).clone();
            flattened.insert(prefix, shared);
        }
        let count = flattened.len();
        *self.tree.write().unwrap() = Arc::new(flattened);
        let elapsed = start.elapsed().as_secs_f64();
        self.metrics.rebuilds.inc();
        self.metrics.rebuild_time.inc_by(elapsed);
        self.metrics.rebuild_last_time.set(elapsed);
        self.metrics.prefixes.set(count as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};

    fn ip(s: &str) -> Ipv6Addr {
        flowscope_common::helpers::subnetmap::addr_to6(&s.parse().unwrap())
    }

    #[test]
    fn inheritance() {
        let config: Configuration = from_value(
            parse_str(
                "networks:
  192.0.2.0/24: customer1
  192.0.2.128/25:
    role: servers
  2001:db8::/32:
    name: corp
    tenant: acme
  2001:db8:1::/48:
    site: paris
",
            )
            .unwrap(),
        )
        .unwrap();
        let r = Reporter::new_mock();
        let n = Networks::new(&r, config, None);
        n.rebuild();
        assert_eq!(n.lookup(&ip("192.0.2.1")).name, "customer1");
        let a = n.lookup(&ip("192.0.2.200"));
        assert_eq!((a.name.as_str(), a.role.as_str()), ("customer1", "servers"));
        let a = n.lookup(&ip("2001:db8:1::1"));
        assert_eq!((a.name.as_str(), a.tenant.as_str(), a.site.as_str()), ("corp", "acme", "paris"));
        assert_eq!(*n.lookup(&ip("198.51.100.1")), NetworkAttributes::default());
    }
}
