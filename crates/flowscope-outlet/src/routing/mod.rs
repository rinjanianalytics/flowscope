// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Routing information (AS numbers, AS paths, communities, prefix lengths,
//! next hops) from BMP or BioRIS. Port of `outlet/routing`.

pub mod bioris;
pub mod bmp;
pub mod mock;
pub mod rd;

use flowscope_common::daemon::Daemon;
use flowscope_common::parametrized;
use flowscope_common::reporter::{Counter, Reporter};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::net::Ipv6Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A large community.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LargeCommunity {
    pub asn: u32,
    pub local_data1: u32,
    pub local_data2: u32,
}

impl LargeCommunity {
    /// UInt128 form used in ClickHouse.
    pub fn to_u128(self) -> u128 {
        ((self.asn as u128) << 64) | ((self.local_data1 as u128) << 32) | self.local_data2 as u128
    }
}

/// Result of a lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LookupResult {
    pub asn: u32,
    pub as_path: Vec<u32>,
    pub communities: Vec<u32>,
    pub large_communities: Vec<LargeCommunity>,
    pub net_mask: u8,
    pub next_hop: Option<Ipv6Addr>,
}

/// A routing provider.
pub trait Provider: Send + Sync {
    /// Start background tasks. Called from a Tokio runtime.
    fn start(&self) -> Result<(), String> {
        Ok(())
    }

    /// Look up an address, with the next hop from the flow and the exporter.
    fn lookup(
        &self,
        ip: Ipv6Addr,
        nh: Option<Ipv6Addr>,
        agent: Ipv6Addr,
    ) -> BoxFuture<'_, Result<LookupResult, String>>;
}

/// Provider configuration.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderConfiguration {
    Bmp(bmp::BmpConfiguration),
    Bioris(bioris::BiorisConfiguration),
}

parametrized!(ProviderConfiguration {
    "bmp" => Bmp(bmp::BmpConfiguration),
    "bioris" => Bioris(bioris::BiorisConfiguration),
});

/// Routing component configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    pub provider: ProviderConfiguration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration { provider: ProviderConfiguration::Bmp(Default::default()) }
    }
}

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        match &self.provider {
            ProviderConfiguration::Bmp(c) => c.validate(),
            ProviderConfiguration::Bioris(c) => c.validate(),
        }
    }
}

/// The routing component.
pub struct Routing {
    provider: Box<dyn Provider>,
    lookups: Counter,
    failed: Counter,
    last_error_log: Mutex<Option<Instant>>,
}

impl Routing {
    pub fn new(r: &Reporter, config: &Configuration, daemon: &Daemon) -> Result<Routing, String> {
        let provider: Box<dyn Provider> = match &config.provider {
            ProviderConfiguration::Bmp(c) => Box::new(bmp::BmpProvider::new(r, c.clone(), daemon)?),
            ProviderConfiguration::Bioris(c) => Box::new(bioris::BiorisProvider::new(r, c.clone(), daemon)?),
        };
        Ok(Self::with_provider(r, provider))
    }

    /// Create the component with the provided provider.
    pub fn with_provider(r: &Reporter, provider: Box<dyn Provider>) -> Routing {
        let m = r.metrics("outlet/routing");
        Routing {
            provider,
            lookups: m.counter("routing_lookups_total", "Number of routing lookups."),
            failed: m.counter("routing_failed_lookups_total", "Number of failed routing lookups."),
            last_error_log: Mutex::new(None),
        }
    }

    /// Start the provider.
    pub fn start(&self) -> Result<(), String> {
        self.provider.start()
    }

    /// Look up routing information. Errors give an empty result.
    pub async fn lookup(&self, ip: Option<Ipv6Addr>, nh: Option<Ipv6Addr>, agent: Option<Ipv6Addr>) -> LookupResult {
        self.lookups.inc();
        let (Some(ip), Some(agent)) = (ip, agent) else {
            return LookupResult::default();
        };
        match self.provider.lookup(ip, nh, agent).await {
            Ok(r) => r,
            Err(e) => {
                self.failed.inc();
                let mut last = self.last_error_log.lock().unwrap();
                if last.is_none_or(|t| t.elapsed() > Duration::from_secs(20)) {
                    *last = Some(Instant::now());
                    tracing::error!(error = %e, "routing: error while looking up {ip} at {agent}");
                }
                LookupResult::default()
            }
        }
    }
}
