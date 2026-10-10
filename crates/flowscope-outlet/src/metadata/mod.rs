// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Metadata about exporters and their interfaces (names, descriptions,
//! speeds). Port of `outlet/metadata`.
//!
//! Lookups are served from a cache. On a miss, providers are queried in
//! order (the caller waits, with a timeout). A circuit breaker per exporter
//! avoids hammering exporters which do not answer.

pub mod gnmi;
pub mod gnmi_config;
pub mod mock;
pub mod snmp;
pub mod snmp_config;
pub mod static_provider;

use flowscope_common::config::{Duration, migrate};
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::cache::Cache;
use flowscope_common::reporter::{Counter, CounterVec, HealthcheckResult, Reporter};
use flowscope_common::{migrated, parametrized};
use flowscope_schema::InterfaceBoundary;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime};

/// Information about an interface.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Interface {
    pub name: String,
    pub description: String,
    pub speed: u64,
    pub provider: String,
    pub connectivity: String,
    pub boundary: InterfaceBoundary,
}

impl Interface {
    pub fn is_empty(&self) -> bool {
        *self == Interface::default()
    }
}

/// Information about an exporter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Exporter {
    pub name: String,
    pub region: String,
    pub role: String,
    pub tenant: String,
    pub site: String,
    pub group: String,
}

/// A query to a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Query {
    pub exporter_ip: Ipv6Addr,
    pub if_index: u32,
}

/// An answer from a provider.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub found: bool,
    pub exporter: Exporter,
    pub interface: Interface,
}

/// Error from a provider.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderError {
    /// The provider has nothing for this exporter, try the next one.
    Skip,
    Other(String),
}

/// A metadata provider.
pub trait Provider: Send + Sync {
    fn query(&self, query: Query) -> BoxFuture<'_, Result<Answer, ProviderError>>;
}

/// Provider configuration.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // Few instances, built once.
pub enum ProviderConfiguration {
    Snmp(snmp_config::SnmpConfiguration),
    Gnmi(gnmi_config::GnmiConfiguration),
    Static(static_provider::StaticConfiguration),
}

parametrized!(ProviderConfiguration {
    "snmp" => Snmp(snmp_config::SnmpConfiguration),
    "gnmi" => Gnmi(gnmi_config::GnmiConfiguration),
    "static" => Static(static_provider::StaticConfiguration),
});

/// Metadata component configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// How long to keep entries not accessed.
    pub cache_duration: Duration,
    /// Refresh entries older than this (0 to disable).
    pub cache_refresh: Duration,
    /// How often to check the cache for expiration and refresh.
    pub cache_check_interval: Duration,
    /// Where to save the cache on stop.
    pub cache_persist_file: String,
    pub providers: Vec<ProviderConfiguration>,
    /// Timeout of a query.
    pub query_timeout: Duration,
    /// Delay during which queries wait longer, after start.
    pub initial_delay: Duration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            cache_duration: Duration::from_secs(30 * 60),
            cache_refresh: Duration::from_secs(3600),
            cache_check_interval: Duration::from_secs(120),
            cache_persist_file: String::new(),
            providers: vec![ProviderConfiguration::Snmp(Default::default())],
            query_timeout: Duration::from_secs(5),
            initial_delay: Duration::from_secs(60),
        }
    }
}

/// `provider` was renamed to `providers`.
fn migrate_metadata(map: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["Workers", "MaxBatchRequests"] {
        migrate::remove(map, k);
    }
    migrate::rename(map, "Provider", "providers")?;
    // A single provider is accepted for "providers".
    if let Some(v) = migrate::get_mut(map, "Providers")
        && v.is_object()
    {
        *v = Value::Array(vec![v.clone()]);
    }
    Ok(())
}

migrated!(Configuration, migrate_metadata);

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.cache_duration.as_secs() < 60 {
            return Err("metadata.cache-duration should be at least 1m".into());
        }
        if !self.cache_refresh.is_zero() && self.cache_refresh.as_secs() < 60 {
            return Err("metadata.cache-refresh should be 0 or at least 1m".into());
        }
        if !self.cache_refresh.is_zero() && self.cache_refresh < self.cache_duration {
            return Err("cache refresh must be greater than cache duration".into());
        }
        if self.cache_duration < self.cache_check_interval {
            return Err("cache duration must be greater than cache check interval".into());
        }
        if self.cache_check_interval.as_secs() < 1 {
            return Err("metadata.cache-check-interval should be at least 1s".into());
        }
        if self.query_timeout.as_millis() < 100 || self.query_timeout.as_secs() > 60 {
            return Err("metadata.query-timeout should be between 100ms and 1m".into());
        }
        if self.initial_delay.as_secs() < 1 || self.initial_delay.as_secs() > 3600 {
            return Err("metadata.initial-delay should be between 1s and 1h".into());
        }
        for p in &self.providers {
            match p {
                ProviderConfiguration::Snmp(c) => c.validate()?,
                ProviderConfiguration::Gnmi(c) => c.validate()?,
                ProviderConfiguration::Static(c) => c.validate()?,
            }
        }
        Ok(())
    }
}

/// Circuit breaker, like eapache/go-resiliency: it opens after
/// `error_threshold` errors (errors older than `timeout` are forgotten),
/// lets a query through after `timeout`, and closes on success.
struct Breaker {
    errors: u32,
    last_error: Option<Instant>,
    opened: Option<Instant>,
}

const BREAKER_ERRORS: u32 = 20;
const BREAKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

impl Breaker {
    fn new() -> Self {
        Breaker { errors: 0, last_error: None, opened: None }
    }

    /// Whether a query can go through.
    fn allow(&mut self) -> bool {
        !matches!(self.opened, Some(t) if t.elapsed() < BREAKER_TIMEOUT)
    }

    fn success(&mut self) {
        if self.opened.is_some() {
            // Half-open and successful: close.
            self.opened = None;
            self.errors = 0;
        }
    }

    fn failure(&mut self) {
        if self.opened.is_some() {
            // Half-open and failed: open again.
            self.opened = Some(Instant::now());
            return;
        }
        if self.errors > 0 && self.last_error.is_some_and(|t| t.elapsed() > BREAKER_TIMEOUT) {
            self.errors = 0;
        }
        self.errors += 1;
        self.last_error = Some(Instant::now());
        if self.errors >= BREAKER_ERRORS {
            self.opened = Some(Instant::now());
        }
    }
}

/// A query in flight, shared by concurrent lookups.
type InFlight = Arc<(Mutex<Option<Answer>>, Condvar)>;

struct Metrics {
    cache_hit: Counter,
    cache_miss: Counter,
    cache_expired: Counter,
    cache_refresh_runs: Counter,
    cache_refresh: Counter,
    provider_breaker_open: CounterVec,
    provider_requests: Counter,
    provider_errors: Counter,
    provider_skips: CounterVec,
}

/// The metadata component.
pub struct Metadata {
    config: Configuration,
    cache: Cache<Query, Answer>,
    providers: Vec<Box<dyn Provider>>,
    breakers: Mutex<HashMap<Ipv6Addr, Breaker>>,
    in_flight: Mutex<HashMap<Query, InFlight>>,
    initial_deadline: Mutex<Instant>,
    runtime: tokio::runtime::Handle,
    metrics: Metrics,
}

fn unmap(ip: &Ipv6Addr) -> String {
    match ip.to_ipv4_mapped() {
        Some(v4) => v4.to_string(),
        None => ip.to_string(),
    }
}

impl Metadata {
    /// Create the component. It must be created from a Tokio runtime.
    pub fn new(r: &Reporter, config: Configuration, daemon: &Daemon) -> Result<Arc<Metadata>, String> {
        let mut providers: Vec<Box<dyn Provider>> = Vec::new();
        for p in &config.providers {
            providers.push(match p {
                ProviderConfiguration::Snmp(c) => Box::new(snmp::SnmpProvider::new(r, c.clone())?),
                ProviderConfiguration::Gnmi(c) => Box::new(gnmi::GnmiProvider::new(r, c.clone(), daemon)?),
                ProviderConfiguration::Static(c) => {
                    Box::new(static_provider::StaticProvider::new(r, c.clone(), daemon)?)
                }
            });
        }
        Ok(Self::with_providers(r, config, providers))
    }

    /// Create the component with the provided providers instead of the
    /// configured ones. It must be created from a Tokio runtime.
    pub fn with_providers(r: &Reporter, config: Configuration, providers: Vec<Box<dyn Provider>>) -> Arc<Metadata> {
        let m = r.metrics("outlet/metadata");
        let metadata = Arc::new(Metadata {
            cache: Cache::new(),
            providers,
            breakers: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            initial_deadline: Mutex::new(Instant::now() + config.initial_delay.as_std()),
            runtime: tokio::runtime::Handle::current(),
            metrics: Metrics {
                cache_hit: m.counter("cache_hits_total", "Number of lookups retrieved from cache."),
                cache_miss: m.counter("cache_misses_total", "Number of lookup miss."),
                cache_expired: m.counter("cache_expired_entries_total", "Number of cache entries expired."),
                cache_refresh_runs: m
                    .counter("cache_refresh_runs_total", "Number of times the cache refresh was triggered."),
                cache_refresh: m.counter("cache_refreshes_total", "Number of entries refreshed in cache."),
                provider_breaker_open: m.counter_vec(
                    "provider_breaker_opens_total",
                    "Provider breaker was opened due to too many errors.",
                    &["exporter"],
                ),
                provider_requests: m.counter("provider_requests_total", "Number of provider requests."),
                provider_errors: m.counter("provider_errors_total", "Number of provider errors."),
                provider_skips: m.counter_vec(
                    "provider_skips_total",
                    "Number of queries no provider had an answer for.",
                    &["exporter"],
                ),
            },
            config,
        });
        let weak = Arc::downgrade(&metadata);
        m.gauge_func("cache_size_entries", "Number of entries in cache.", move || {
            weak.upgrade().map(|m| m.cache.len() as f64).unwrap_or(0.0)
        });
        metadata
    }

    /// Start the expiration and refresh loop.
    pub fn start(self: &Arc<Self>, r: &Reporter, daemon: &Daemon) {
        *self.initial_deadline.lock().unwrap() = Instant::now() + self.config.initial_delay.as_std();
        if !self.config.cache_persist_file.is_empty()
            && let Err(e) = self.cache.load(&self.config.cache_persist_file)
        {
            tracing::error!(error = %e, "cannot load cache, ignoring");
        }
        r.register_healthcheck("metadata/ticker", || async { HealthcheckResult::ok("ok") });
        let this = self.clone();
        let token = daemon.token();
        daemon.spawn_background(async move {
            let mut ticker = tokio::time::interval(this.config.cache_check_interval.as_std());
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = ticker.tick() => {
                        let this = this.clone();
                        tokio::task::spawn_blocking(move || this.expire_cache());
                    }
                }
            }
            if !this.config.cache_persist_file.is_empty()
                && let Err(e) = this.cache.save(&this.config.cache_persist_file)
            {
                tracing::error!(error = %e, "cannot save cache");
            }
            tracing::info!("metadata component stopped");
        });
    }

    /// Look up metadata for an interface. It may block while providers are
    /// queried. Must not be called from an async context.
    pub fn lookup(&self, t: SystemTime, exporter_ip: Ipv6Addr, if_index: u32) -> Answer {
        let query = Query { exporter_ip, if_index };
        if let Some(a) = self.cache.get(t, &query) {
            self.metrics.cache_hit.inc();
            return a;
        }
        self.metrics.cache_miss.inc();
        // Only one query in flight per key.
        let (slot, leader) = {
            let mut in_flight = self.in_flight.lock().unwrap();
            match in_flight.get(&query) {
                Some(s) => (s.clone(), false),
                None => {
                    let s: InFlight = Arc::new((Mutex::new(None), Condvar::new()));
                    in_flight.insert(query, s.clone());
                    (s, true)
                }
            }
        };
        if !leader {
            let (lock, cv) = &*slot;
            let mut guard = lock.lock().unwrap();
            while guard.is_none() {
                guard = cv.wait(guard).unwrap();
            }
            return guard.clone().unwrap();
        }
        let answer = self.query_providers(query).unwrap_or_default();
        let (lock, cv) = &*slot;
        *lock.lock().unwrap() = Some(answer.clone());
        cv.notify_all();
        self.in_flight.lock().unwrap().remove(&query);
        answer
    }

    fn query_providers(&self, query: Query) -> Result<Answer, String> {
        self.metrics.provider_requests.inc();
        let allowed = self.breakers.lock().unwrap().entry(query.exporter_ip).or_insert_with(Breaker::new).allow();
        if !allowed {
            self.metrics.provider_errors.inc();
            self.metrics.provider_breaker_open.with_label_values(&[&unmap(&query.exporter_ip)]).inc();
            tracing::warn!(exporter = %unmap(&query.exporter_ip), "provider breaker open");
            return Err("breaker open".into());
        }
        let deadline = {
            let initial = *self.initial_deadline.lock().unwrap();
            (Instant::now() + self.config.query_timeout.as_std()).max(initial)
        };
        let now = SystemTime::now();
        let result: Result<Answer, String> = self.runtime.block_on(async {
            for p in &self.providers {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(remaining, p.query(query)).await {
                    Err(_) => return Err("provider query timeout".to_string()),
                    Ok(Err(ProviderError::Skip)) => continue,
                    Ok(Err(ProviderError::Other(e))) => return Err(e),
                    Ok(Ok(answer)) => return Ok(answer),
                }
            }
            self.metrics.provider_skips.with_label_values(&[&unmap(&query.exporter_ip)]).inc();
            tracing::warn!(exporter = %unmap(&query.exporter_ip), "no provider has metadata for exporter");
            Ok(Answer::default())
        });
        let mut breakers = self.breakers.lock().unwrap();
        let breaker = breakers.entry(query.exporter_ip).or_insert_with(Breaker::new);
        match result {
            Ok(answer) => {
                breaker.success();
                self.cache.put(now, query, answer.clone());
                Ok(answer)
            }
            Err(e) => {
                breaker.failure();
                self.metrics.provider_errors.inc();
                Err(e)
            }
        }
    }

    fn expire_cache(&self) {
        let expired = self.cache.delete_last_accessed_before(SystemTime::now() - self.config.cache_duration.as_std());
        self.metrics.cache_expired.inc_by(expired as f64);
        if !self.config.cache_refresh.is_zero() {
            self.metrics.cache_refresh_runs.inc();
            let to_refresh =
                self.cache.items_last_updated_before(SystemTime::now() - self.config.cache_refresh.as_std());
            self.metrics.cache_refresh.inc_by(to_refresh.len() as f64);
            for (q, _) in to_refresh {
                let _ = self.query_providers(q);
            }
        }
    }
}

#[cfg(test)]
mod tests;
