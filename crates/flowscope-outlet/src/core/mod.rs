// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Core of the outlet: plumb the other components together. Each Kafka
//! worker decodes raw flows, enriches them and sends them to ClickHouse (and
//! optionally to Kafka). Port of `outlet/core`.

pub mod classifier;
pub mod config;
mod enricher;
pub mod rate;
mod worker;

pub use config::Configuration;
pub use enricher::is_private_as;

use crate::clickhouse;
use crate::kafkainput::{self, WorkerBuilder};
use crate::kafkaoutput::KafkaOutput;
use crate::metadata::Metadata;
use crate::networks::Networks;
use crate::routing::Routing;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use classifier::{ExporterClassification, ExporterInfo, InterfaceClassification, InterfaceInfo};
use flowscope_common::helpers::cache::Cache;
use flowscope_common::reporter::{Counter, CounterVec, Reporter};
use flowscope_decoder::Flow;
use rate::RateLimiter;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

/// Dependencies of the core component.
pub struct Dependencies {
    pub flow: Arc<Flow>,
    pub metadata: Arc<Metadata>,
    pub routing: Arc<Routing>,
    pub networks: Option<Arc<Networks>>,
    pub kafka_input: Arc<dyn kafkainput::Component>,
    pub kafka_output: Option<Arc<KafkaOutput>>,
    pub clickhouse: Arc<dyn clickhouse::Component>,
    pub schema: Arc<flowscope_schema::Component>,
    /// Runtime for async lookups from the worker threads.
    pub runtime: tokio::runtime::Handle,
}

pub(crate) struct Metrics {
    raw_flows_received: Counter,
    raw_flows_errors: CounterVec,
    flows_received: CounterVec,
    flows_forwarded: CounterVec,
    flows_errors: CounterVec,
    flows_rate_limited: CounterVec,
    classifier_errors: CounterVec,
}

/// Log at most `burst` messages per period.
pub(crate) struct LogSampler {
    state: Mutex<(Instant, u32)>,
    period: Duration,
    burst: u32,
}

impl LogSampler {
    fn new(period: Duration, burst: u32) -> Self {
        LogSampler { state: Mutex::new((Instant::now(), 0)), period, burst }
    }

    pub(crate) fn allow(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.0.elapsed() > self.period {
            *s = (Instant::now(), 0);
        }
        s.1 += 1;
        s.1 <= self.burst
    }
}

pub(crate) struct Inner {
    config: Configuration,
    deps: Dependencies,
    metrics: Metrics,
    http_flow_clients: AtomicU32,
    http_flow_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    http_flow_rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    exporter_cache: Cache<ExporterInfo, ExporterClassification>,
    interface_cache: Cache<(ExporterInfo, InterfaceInfo), InterfaceClassification>,
    classifier_errors_log: LogSampler,
    rate_limiter: RateLimiter,
    token: CancellationToken,
}

/// The core component.
pub struct Core {
    inner: Arc<Inner>,
}

impl Core {
    pub fn new(r: &Reporter, config: Configuration, deps: Dependencies) -> Result<Core, String> {
        config.validate()?;
        let m = r.metrics("outlet/core");
        let metrics = Metrics {
            raw_flows_received: m.counter("received_raw_flows_total", "Number of incoming raw flows (proto)."),
            raw_flows_errors: m.counter_vec("raw_flows_errors_total", "Number of raw flows with errors.", &["error"]),
            flows_received: m.counter_vec("received_flows_total", "Number of incoming flows.", &["exporter"]),
            flows_forwarded: m.counter_vec(
                "forwarded_flows_total",
                "Number of flows forwarded to Kafka.",
                &["exporter"],
            ),
            flows_errors: m.counter_vec("flows_errors_total", "Number of flows with errors.", &["exporter", "error"]),
            flows_rate_limited: m.counter_vec(
                "flows_rate_limited_total",
                "Number of flows dropped by rate limiter.",
                &["exporter"],
            ),
            classifier_errors: m.counter_vec(
                "classifier_errors_total",
                "Number of errors when evaluating a classifer.",
                &["type", "index"],
            ),
        };
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let inner = Arc::new(Inner {
            config,
            deps,
            metrics,
            http_flow_clients: AtomicU32::new(0),
            http_flow_tx: tx,
            http_flow_rx: tokio::sync::Mutex::new(rx),
            exporter_cache: Cache::new(),
            interface_cache: Cache::new(),
            classifier_errors_log: LogSampler::new(Duration::from_secs(10), 3),
            rate_limiter: RateLimiter::new(),
            token: CancellationToken::new(),
        });
        let weak = Arc::downgrade(&inner);
        m.gauge_func("flows_http_clients", "Number of HTTP clients requesting flows.", move || {
            weak.upgrade().map(|i| i.http_flow_clients.load(Ordering::Relaxed) as f64).unwrap_or(0.0)
        });
        let weak = Arc::downgrade(&inner);
        m.counter_func(
            "classifier_exporter_cache_items_total",
            "Number of items in the exporter classifier cache.",
            move || weak.upgrade().map(|i| i.exporter_cache.len() as f64).unwrap_or(0.0),
        );
        let weak = Arc::downgrade(&inner);
        m.counter_func(
            "classifier_interface_cache_items_total",
            "Number of items in the interface classifier cache.",
            move || weak.upgrade().map(|i| i.interface_cache.len() as f64).unwrap_or(0.0),
        );
        Ok(Core { inner })
    }

    /// Routes of the component.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/api/v0/outlet/flows", axum::routing::get(flows_http_handler))
            .with_state(self.inner.clone())
    }

    /// Start the cache expiration and the workers. Must be called from a
    /// Tokio runtime.
    pub fn start(&self) -> Result<(), String> {
        tracing::info!("starting core component");
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let every = *inner.config.classifier_cache_duration;
            loop {
                tokio::select! {
                    _ = inner.token.cancelled() => return,
                    _ = tokio::time::sleep(every) => {
                        let before = SystemTime::now() - every;
                        inner.exporter_cache.delete_last_accessed_before(before);
                        inner.interface_cache.delete_last_accessed_before(before);
                    }
                }
            }
        });
        let builder = self.worker_builder();
        let delay = *self.inner.config.startup_delay;
        if !delay.is_zero() {
            // Let the other components collect their data first.
            tracing::info!("wait {:?} before processing flows", delay);
            let inner = self.inner.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = inner.token.cancelled() => {}
                    _ = tokio::time::sleep(delay) => {
                        tracing::info!("start processing flows");
                        let input = inner.deps.kafka_input.clone();
                        let r = tokio::task::spawn_blocking(move || input.start_workers(builder)).await;
                        if let Ok(Err(e)) = r {
                            tracing::error!(error = %e, "cannot start workers");
                        }
                    }
                }
            });
            return Ok(());
        }
        self.inner.deps.kafka_input.start_workers(builder)
    }

    /// Stop the workers. They flush their batches first.
    pub fn stop(&self) {
        tracing::info!("stopping core component");
        self.inner.token.cancel();
        self.inner.deps.kafka_input.stop_workers();
        tracing::info!("core component stopped");
    }

    pub(crate) fn worker_builder(&self) -> WorkerBuilder {
        let inner = self.inner.clone();
        Arc::new(move |i, scale| Box::new(worker::Worker::new(inner.clone(), i, scale)))
    }
}

/// Stream a JSON copy of the flows sent to ClickHouse. Under load, some
/// flows are skipped. This is meant for debugging.
async fn flows_http_handler(
    State(inner): State<Arc<Inner>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let limit = match params.get("limit").filter(|s| !s.is_empty()) {
        None => 0,
        Some(raw) => match raw.parse::<u64>() {
            Ok(l) => l,
            Err(_) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"message": "Invalid limit"})),
                )
                    .into_response();
            }
        },
    };

    /// Count the client while the response is alive.
    struct ClientGuard(Arc<Inner>);
    impl Drop for ClientGuard {
        fn drop(&mut self) {
            self.0.http_flow_clients.fetch_sub(1, Ordering::Relaxed);
        }
    }
    inner.http_flow_clients.fetch_add(1, Ordering::Relaxed);
    let guard = ClientGuard(inner);
    let stream = futures::stream::unfold((guard, 0u64), move |(guard, count)| async move {
        if limit > 0 && count >= limit {
            return None;
        }
        let msg = {
            let mut rx = tokio::select! {
                _ = guard.0.token.cancelled() => return None,
                rx = guard.0.http_flow_rx.lock() => rx,
            };
            tokio::select! {
                _ = guard.0.token.cancelled() => return None,
                msg = rx.recv() => msg?,
            }
        };
        let mut line = msg;
        line.push(b'\n');
        Some((Ok::<_, std::convert::Infallible>(line), (guard, count + 1)))
    });
    ([(axum::http::header::CONTENT_TYPE, "application/json")], axum::body::Body::from_stream(stream)).into_response()
}

#[cfg(test)]
mod tests;
