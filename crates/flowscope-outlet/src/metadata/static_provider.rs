// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Static metadata provider: exporters and interfaces from the
//! configuration and from remote data sources. Port of
//! `outlet/metadata/provider/static`.

use super::{Answer, Exporter, Interface, Provider, ProviderError, Query};
use flowscope_common::config::Duration;
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::subnetmap::{SubnetMap, parse_key};
use flowscope_common::remotedatasource::{RemoteDataSource, Source};
use flowscope_common::reporter::{Counter, Reporter};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

/// Static provider configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct StaticConfiguration {
    /// Exporters, per subnet.
    pub exporters: SubnetMap<ExporterConfiguration>,
    /// Remote sources of exporters.
    pub exporter_sources: BTreeMap<String, Source>,
    /// Unused, accepted for compatibility.
    pub exporter_sources_timeout: Duration,
}

impl StaticConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        for (_, e) in self.exporters.iter() {
            if e.name.is_empty() {
                return Err("static exporters need a name".into());
            }
            if e.skip_missing_interfaces && !e.default.is_empty() {
                return Err("default interface and skip-missing-interfaces are exclusive".into());
            }
            for i in e.if_indexes.values() {
                validate_interface(i)?;
            }
        }
        for (name, s) in &self.exporter_sources {
            s.validate().map_err(|e| format!("exporter source {name}: {e}"))?;
        }
        Ok(())
    }
}

fn validate_interface(i: &Interface) -> Result<(), String> {
    if i.name.is_empty() || i.description.is_empty() || i.speed == 0 {
        return Err(format!("interface {:?} needs a name, a description and a speed", i.name));
    }
    Ok(())
}

/// An exporter in the static configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ExporterConfiguration {
    pub name: String,
    pub region: String,
    pub role: String,
    pub tenant: String,
    pub site: String,
    pub group: String,
    /// Interface used when the index is not known.
    pub default: Interface,
    pub if_indexes: BTreeMap<u32, Interface>,
    /// Let the next provider answer for unknown interfaces.
    pub skip_missing_interfaces: bool,
}

impl ExporterConfiguration {
    fn exporter(&self) -> Exporter {
        Exporter {
            name: self.name.clone(),
            region: self.region.clone(),
            role: self.role.clone(),
            tenant: self.tenant.clone(),
            site: self.site.clone(),
            group: self.group.clone(),
        }
    }
}

/// An exporter from a remote data source.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ExporterInfo {
    pub name: String,
    pub region: String,
    pub role: String,
    pub tenant: String,
    pub site: String,
    pub group: String,
    pub exporter_subnet: String,
    pub default: Interface,
    pub interfaces: Vec<ExporterInterface>,
    pub skip_missing_interfaces: bool,
}

/// An interface from a remote data source.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ExporterInterface {
    pub if_index: u32,
    pub name: String,
    pub description: String,
    pub speed: u64,
    pub provider: String,
    pub connectivity: String,
    pub boundary: flowscope_schema::InterfaceBoundary,
}

impl ExporterInfo {
    fn to_configuration(&self) -> ExporterConfiguration {
        ExporterConfiguration {
            name: self.name.clone(),
            region: self.region.clone(),
            role: self.role.clone(),
            tenant: self.tenant.clone(),
            site: self.site.clone(),
            group: self.group.clone(),
            default: self.default.clone(),
            if_indexes: self
                .interfaces
                .iter()
                .map(|i| {
                    (
                        i.if_index,
                        Interface {
                            name: i.name.clone(),
                            description: i.description.clone(),
                            speed: i.speed,
                            provider: i.provider.clone(),
                            connectivity: i.connectivity.clone(),
                            boundary: i.boundary,
                        },
                    )
                })
                .collect(),
            skip_missing_interfaces: self.skip_missing_interfaces,
        }
    }
}

struct State {
    static_exporters: SubnetMap<ExporterConfiguration>,
    from_sources: HashMap<String, Vec<ExporterInfo>>,
}

/// The static provider.
pub struct StaticProvider {
    exporters: Arc<RwLock<Arc<SubnetMap<ExporterConfiguration>>>>,
    ready: tokio::sync::watch::Receiver<bool>,
    not_ready: Counter,
}

impl StaticProvider {
    pub fn new(r: &Reporter, config: StaticConfiguration, daemon: &Daemon) -> Result<Self, String> {
        let exporters = Arc::new(RwLock::new(Arc::new(config.exporters.clone())));
        let state =
            Arc::new(Mutex::new(State { static_exporters: config.exporters.clone(), from_sources: HashMap::new() }));
        let fetcher =
            RemoteDataSource::new(r, "outlet/metadata/provider/static", "metadata", config.exporter_sources.clone());
        let exp = exporters.clone();
        let ready = fetcher.start(
            daemon,
            Arc::new(move |name: String, source: Source| -> BoxFuture<'static, Result<usize, String>> {
                let exp = exp.clone();
                let state = state.clone();
                Box::pin(async move {
                    let results: Vec<ExporterInfo> = RemoteDataSource::fetch(&name, &source).await?;
                    let count = results.len();
                    let mut st = state.lock().unwrap();
                    st.from_sources.insert(name, results);
                    // Static entries win over remote ones.
                    let mut merged = SubnetMap::new();
                    for infos in st.from_sources.values() {
                        for info in infos {
                            match parse_key(&info.exporter_subnet) {
                                Ok(prefix) => merged.insert(prefix, info.to_configuration()),
                                Err(e) => tracing::error!(error = %e, "failed to decode subnet"),
                            }
                        }
                    }
                    for (prefix, e) in st.static_exporters.iter() {
                        merged.insert(prefix, e.clone());
                    }
                    *exp.write().unwrap() = Arc::new(merged);
                    Ok(count)
                })
            }),
        );
        Ok(StaticProvider {
            exporters,
            ready,
            not_ready: r
                .metrics("outlet/metadata/provider/static")
                .counter("not_ready_total", "Number of queries failing because the remote data sources are not ready."),
        })
    }
}

impl Provider for StaticProvider {
    fn query(&self, query: Query) -> BoxFuture<'_, Result<Answer, ProviderError>> {
        Box::pin(async move {
            let mut ready = self.ready.clone();
            if !*ready.borrow() {
                // Wait for the sources (the caller has a timeout).
                if ready.wait_for(|r| *r).await.is_err() {
                    self.not_ready.inc();
                    return Err(ProviderError::Other("remote datasources are not ready".into()));
                }
            }
            let exporters = self.exporters.read().unwrap().clone();
            let Some(exporter) = exporters.lookup(&query.exporter_ip) else {
                return Err(ProviderError::Skip);
            };
            let iface = match exporter.if_indexes.get(&query.if_index) {
                Some(i) => i.clone(),
                None => {
                    if exporter.skip_missing_interfaces {
                        return Err(ProviderError::Skip);
                    }
                    if exporter.default.is_empty() {
                        return Ok(Answer::default());
                    }
                    exporter.default.clone()
                }
            };
            Ok(Answer { found: true, exporter: exporter.exporter(), interface: iface })
        })
    }
}
