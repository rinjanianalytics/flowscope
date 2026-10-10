// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Logging, metrics and healthchecks. Port of `common/reporter`.

pub mod metrics;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use metrics::{
    Collector, Counter, CounterVec, Family, Gauge, GaugeVec, Histogram, HistogramVec, Registry, Sample, Summary,
    SummaryVec,
};

/// Reporter configuration. There is nothing to configure: the Go version has
/// empty logging and metrics sections, kept for compatibility.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    pub logging: Empty,
    pub metrics: Empty,
}

/// An empty configuration section.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// Status of a healthcheck. Ordered by severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum HealthcheckStatus {
    #[default]
    Ok,
    Warning,
    Error,
}

impl HealthcheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl Serialize for HealthcheckStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HealthcheckStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "ok" => Ok(Self::Ok),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            other => Err(serde::de::Error::custom(format!("unknown status {other:?}"))),
        }
    }
}

/// Result of a healthcheck.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthcheckResult {
    pub status: HealthcheckStatus,
    pub reason: String,
}

impl HealthcheckResult {
    pub fn ok(reason: impl Into<String>) -> Self {
        HealthcheckResult { status: HealthcheckStatus::Ok, reason: reason.into() }
    }
    pub fn warning(reason: impl Into<String>) -> Self {
        HealthcheckResult { status: HealthcheckStatus::Warning, reason: reason.into() }
    }
    pub fn error(reason: impl Into<String>) -> Self {
        HealthcheckResult { status: HealthcheckStatus::Error, reason: reason.into() }
    }
}

/// Results of all healthchecks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MultipleHealthcheckResults {
    pub status: HealthcheckStatus,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, HealthcheckResult>,
}

/// A healthcheck function.
pub type HealthcheckFn = Arc<dyn Fn() -> BoxFuture<'static, HealthcheckResult> + Send + Sync>;

/// Logging, metrics and healthchecks for a component.
#[derive(Clone, Default)]
pub struct Reporter {
    registry: Registry,
    healthchecks: Arc<Mutex<BTreeMap<String, HealthcheckFn>>>,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reporter").finish_non_exhaustive()
    }
}

impl Reporter {
    pub fn new(_config: &Configuration) -> Reporter {
        let r = Reporter::default();
        r.registry.register_collector("", Arc::new(metrics::ProcessCollector));
        r
    }

    /// A reporter for tests, without process metrics.
    pub fn new_mock() -> Reporter {
        Reporter::default()
    }

    /// The metrics registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Metrics factory for a module, named like the Go package it was ported
    /// from ("outlet/core"): its metrics are prefixed with
    /// "flowscope_outlet_core_". Akvorado uses "akvorado_outlet_core_".
    pub fn metrics(&self, module: &str) -> Metrics {
        let prefix = if module.is_empty() {
            "flowscope_cmd_".to_string()
        } else {
            format!("flowscope_{}_", module.replace(['/', '.'], "_"))
        };
        Metrics { prefix, registry: self.registry.clone() }
    }

    /// Render metrics in the Prometheus text format.
    pub fn render_metrics(&self) -> String {
        self.registry.render()
    }

    /// Register a healthcheck.
    pub fn register_healthcheck<F, Fut>(&self, name: &str, f: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = HealthcheckResult> + Send + 'static,
    {
        let f: HealthcheckFn = Arc::new(move || Box::pin(f()));
        self.healthchecks.lock().unwrap().insert(name.to_string(), f);
    }

    /// Run all healthchecks concurrently, with a timeout for each of them.
    pub async fn run_healthchecks(&self, timeout: Duration) -> MultipleHealthcheckResults {
        let checks: Vec<(String, HealthcheckFn)> =
            self.healthchecks.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let futures = checks.into_iter().map(|(name, f)| async move {
            let result = match tokio::time::timeout(timeout, f()).await {
                Ok(r) => r,
                Err(_) => HealthcheckResult::error("timeout during check"),
            };
            (name, result)
        });
        let details: BTreeMap<String, HealthcheckResult> =
            futures::future::join_all(futures).await.into_iter().collect();
        let status = details.values().map(|r| r.status).max().unwrap_or_default();
        MultipleHealthcheckResults { status, details }
    }
}

/// Request sent to a worker loop to check it is alive.
pub type HealthcheckRequest = tokio::sync::oneshot::Sender<HealthcheckResult>;

/// Build a healthcheck asking a worker loop for its status through a channel,
/// like Go's ChannelHealthcheck. The worker answers on the provided oneshot.
pub fn channel_healthcheck(
    contact: tokio::sync::mpsc::Sender<HealthcheckRequest>,
) -> impl Fn() -> BoxFuture<'static, HealthcheckResult> + Send + Sync + 'static {
    move || {
        let contact = contact.clone();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if contact.send(tx).await.is_err() {
                return HealthcheckResult::error("dead");
            }
            rx.await.unwrap_or_else(|_| HealthcheckResult::error("dead"))
        })
    }
}

/// Metrics factory with a name prefix.
#[derive(Clone, Debug)]
pub struct Metrics {
    prefix: String,
    registry: Registry,
}

impl Metrics {
    fn name(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn counter(&self, name: &str, help: &str) -> Counter {
        self.registry.counter(&self.name(name), help)
    }

    pub fn counter_vec(&self, name: &str, help: &str, labels: &[&str]) -> CounterVec {
        self.registry.counter_vec(&self.name(name), help, labels)
    }

    pub fn counter_func(&self, name: &str, help: &str, f: impl Fn() -> f64 + Send + Sync + 'static) {
        self.registry.counter_func(&self.name(name), help, f)
    }

    pub fn gauge(&self, name: &str, help: &str) -> Gauge {
        self.registry.gauge(&self.name(name), help)
    }

    pub fn gauge_vec(&self, name: &str, help: &str, labels: &[&str]) -> GaugeVec {
        self.registry.gauge_vec(&self.name(name), help, labels)
    }

    pub fn gauge_func(&self, name: &str, help: &str, f: impl Fn() -> f64 + Send + Sync + 'static) {
        self.registry.gauge_func(&self.name(name), help, f)
    }

    pub fn histogram(&self, name: &str, help: &str, buckets: &[f64]) -> Histogram {
        self.registry.histogram(&self.name(name), help, buckets)
    }

    pub fn histogram_vec(&self, name: &str, help: &str, buckets: &[f64], labels: &[&str]) -> HistogramVec {
        self.registry.histogram_vec(&self.name(name), help, buckets, labels)
    }

    /// A summary with the quantiles FlowScope uses (0.5, 0.9, 0.99).
    pub fn summary(&self, name: &str, help: &str, max_age: Duration) -> Summary {
        self.registry.summary(&self.name(name), help, &[0.5, 0.9, 0.99], max_age)
    }

    pub fn summary_vec(&self, name: &str, help: &str, max_age: Duration, labels: &[&str]) -> SummaryVec {
        self.registry.summary_vec(&self.name(name), help, &[0.5, 0.9, 0.99], max_age, labels)
    }

    pub fn register_collector(&self, c: Arc<dyn Collector>) {
        self.registry.register_collector(&self.prefix, c)
    }
}

/// Set up logging. Logs are JSON on stderr unless it is a terminal.
pub fn setup_logging(debug: bool) {
    use std::io::IsTerminal;
    use tracing_subscriber::EnvFilter;
    let default = if debug { "debug" } else { "info" };
    let filter = EnvFilter::try_from_env("FLOWSCOPE_LOG").unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_target(true);
    let result = if std::io::stderr().is_terminal() {
        builder.try_init()
    } else {
        builder.json().flatten_event(true).with_current_span(false).try_init()
    };
    let _ = result;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        let r = Reporter::new_mock();
        let m = r.metrics("outlet/core");
        assert_eq!(m.prefix(), "flowscope_outlet_core_");
        m.counter("flows_received_total", "Flows.").inc();
        assert!(r.render_metrics().contains("flowscope_outlet_core_flows_received_total 1\n"));
    }

    #[tokio::test]
    async fn healthchecks() {
        let r = Reporter::new_mock();
        let empty = r.run_healthchecks(Duration::from_millis(100)).await;
        assert_eq!(serde_json::to_string(&empty).unwrap(), r#"{"status":"ok"}"#);

        r.register_healthcheck("hc1", || async { HealthcheckResult::ok("all well") });
        r.register_healthcheck("hc2", || async { HealthcheckResult::warning("not so well") });
        let got = r.run_healthchecks(Duration::from_millis(100)).await;
        assert_eq!(got.status, HealthcheckStatus::Warning);

        r.register_healthcheck("hc3", || async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            HealthcheckResult::ok("too late")
        });
        let got = r.run_healthchecks(Duration::from_millis(50)).await;
        assert_eq!(got.status, HealthcheckStatus::Error);
        assert_eq!(got.details["hc3"], HealthcheckResult::error("timeout during check"));
    }

    #[tokio::test]
    async fn channel() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<HealthcheckRequest>(1);
        let r = Reporter::new_mock();
        r.register_healthcheck("worker", channel_healthcheck(tx));
        tokio::spawn(async move {
            while let Some(answer) = rx.recv().await {
                let _ = answer.send(HealthcheckResult::ok("alive"));
            }
        });
        let got = r.run_healthchecks(Duration::from_secs(1)).await;
        assert_eq!(got.details["worker"], HealthcheckResult::ok("alive"));
    }
}
