// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP server shared by all components. Port of `common/httpserver`.
//!
//! The server listens on the configured TCP address and, on Linux, on the
//! abstract Unix sockets `@flowscope` and `@flowscope/<service>` used by the
//! `healthcheck` command. Handlers are instrumented with the same metrics as
//! the Go version.

pub mod cache;

use crate::config::Validate;
use crate::daemon::Daemon;
use crate::parametrized;
use crate::reporter::{CounterVec, Gauge, HistogramVec, Reporter};
use axum::Router;
use axum::body::{Body, HttpBody};
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use cache::{MemoryCacheConfiguration, RedisCacheConfiguration, Store};

/// HTTP server configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Address to listen to.
    pub listen: String,
    /// Enable the profiler. Accepted for compatibility, there is no Go
    /// profiler in the Rust version.
    pub profiler: bool,
    /// Cache backend for responses.
    pub cache: CacheConfiguration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            listen: ":8080".into(),
            profiler: true,
            cache: CacheConfiguration::Memory(MemoryCacheConfiguration {}),
        }
    }
}

impl Validate for Configuration {
    fn validate(&self) -> Result<(), String> {
        if self.listen.is_empty() {
            return Err("http.listen is required".into());
        }
        crate::helpers::validate_listen(&self.listen).map_err(|e| format!("http.listen: {e}"))?;
        if let CacheConfiguration::Redis(r) = &self.cache {
            r.validate()?;
        }
        Ok(())
    }
}

/// Cache backend.
#[derive(Clone, Debug, PartialEq)]
pub enum CacheConfiguration {
    Memory(MemoryCacheConfiguration),
    Redis(RedisCacheConfiguration),
}

parametrized!(CacheConfiguration {
    "memory" => Memory(MemoryCacheConfiguration),
    "redis" => Redis(RedisCacheConfiguration),
});

#[derive(Clone)]
struct HttpMetrics {
    inflights: Gauge,
    requests: CounterVec,
    durations: HistogramVec,
    sizes: HistogramVec,
    cache_hit: CounterVec,
    cache_miss: CounterVec,
}

impl HttpMetrics {
    fn new(r: &Reporter) -> Self {
        let m = r.metrics("common/httpserver");
        HttpMetrics {
            inflights: m.gauge("inflight_requests", "Number of requests currently being served by the HTTP server."),
            requests: m.counter_vec(
                "requests_total",
                "Number of requests handled by an handler.",
                &["handler", "code", "method"],
            ),
            durations: m.histogram_vec(
                "request_duration_seconds",
                "Latencies for served requests.",
                &[0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
                &["handler", "method"],
            ),
            sizes: m.histogram_vec(
                "response_size_bytes",
                "Response sizes for requests.",
                &[200.0, 500.0, 1000.0, 1500.0, 5000.0],
                &["handler", "method"],
            ),
            cache_hit: m.counter_vec("cache_hit_total", "Number of requests served from cache.", &["path", "method"]),
            cache_miss: m.counter_vec(
                "cache_miss_total",
                "Number of requests not served from cache.",
                &["path", "method"],
            ),
        }
    }
}

/// The HTTP server component.
#[derive(Clone)]
pub struct HttpServer {
    service: String,
    config: Configuration,
    metrics: HttpMetrics,
    handlers: Arc<Mutex<Vec<(String, Router)>>>,
    api: Arc<Mutex<Router>>,
    cache: Arc<dyn Store>,
    local_addr: Arc<Mutex<Option<SocketAddr>>>,
}

impl std::fmt::Debug for HttpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer").field("service", &self.service).finish_non_exhaustive()
    }
}

impl HttpServer {
    /// Create the HTTP server. The cache backend is connected immediately.
    pub async fn new(r: &Reporter, service: &str, config: Configuration) -> Result<Self, String> {
        let cache = cache::new_store(&config.cache).await?;
        Ok(HttpServer {
            service: service.to_string(),
            config,
            metrics: HttpMetrics::new(r),
            handlers: Arc::new(Mutex::new(Vec::new())),
            api: Arc::new(Mutex::new(Router::new())),
            cache,
            local_addr: Arc::new(Mutex::new(None)),
        })
    }

    /// Register handlers under a location. The location is used as the
    /// "handler" label of the metrics. "/" is the fallback.
    pub fn add_handler(&self, location: &str, router: Router) {
        self.handlers.lock().unwrap().push((location.to_string(), router));
    }

    /// Register API routes. Routes are given with their full path
    /// ("/api/v0/console/..."). They are all accounted under "/api/".
    pub fn add_api(&self, router: Router) {
        let mut api = self.api.lock().unwrap();
        let current = std::mem::take(&mut *api);
        *api = current.merge(router);
    }

    /// Cache store, for components needing it.
    pub fn cache_store(&self) -> Arc<dyn Store> {
        self.cache.clone()
    }

    /// Middleware caching successful responses by request path.
    pub fn cache_by_path(&self, expire: Duration) -> cache::CacheLayer {
        cache::CacheLayer::new(
            self.cache.clone(),
            self.metrics.cache_hit.clone(),
            self.metrics.cache_miss.clone(),
            expire,
            false,
        )
    }

    /// Middleware caching successful responses by method, path and body.
    pub fn cache_by_body(&self, expire: Duration) -> cache::CacheLayer {
        cache::CacheLayer::new(
            self.cache.clone(),
            self.metrics.cache_hit.clone(),
            self.metrics.cache_miss.clone(),
            expire,
            true,
        )
    }

    /// Local TCP address, once started.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        *self.local_addr.lock().unwrap()
    }

    fn instrument(&self, location: &str, router: Router) -> Router {
        let metrics = self.metrics.clone();
        let location = location.to_string();
        router.layer(axum::middleware::from_fn(move |req: Request, next: Next| {
            let metrics = metrics.clone();
            let location = location.clone();
            async move { instrumented(metrics, location, req, next).await }
        }))
    }

    /// Build the application router.
    pub fn router(&self) -> Router {
        let mut app = Router::new();
        let api = std::mem::take(&mut *self.api.lock().unwrap());
        app = app.merge(self.instrument("/api/", api));
        let handlers = std::mem::take(&mut *self.handlers.lock().unwrap());
        for (location, router) in handlers {
            let router = self.instrument(&location, router);
            if location == "/" {
                app = app.fallback_service(router);
            } else {
                app = app.merge(router);
            }
        }
        app
    }

    /// Start listening. Tasks are tracked by the daemon and stop when it
    /// terminates.
    pub async fn start(&self, daemon: &Daemon) -> Result<(), String> {
        let app = self.router();
        let listen = normalize_listen(&self.config.listen);
        tracing::info!(listen = %self.config.listen, "starting HTTP server");
        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .map_err(|e| format!("unable to listen to {}: {e}", self.config.listen))?;
        *self.local_addr.lock().unwrap() = listener.local_addr().ok();
        let token = daemon.token();
        let tcp_app = app.clone().into_make_service_with_connect_info::<SocketAddr>();
        daemon.spawn("common/http", async move {
            axum::serve(listener, tcp_app)
                .with_graceful_shutdown(async move { token.cancelled().await })
                .await
                .map_err(|e| format!("unable to start HTTP server: {e}"))
        });

        #[cfg(target_os = "linux")]
        for name in ["flowscope".to_string(), format!("flowscope/{}", self.service)] {
            match bind_abstract(&name) {
                Ok(listener) => {
                    let token = daemon.token();
                    let app = app.clone();
                    daemon.spawn_background(async move {
                        let _ = axum::serve(listener, app)
                            .with_graceful_shutdown(async move { token.cancelled().await })
                            .await;
                    });
                }
                Err(e) => tracing::info!(error = %e, listen = %format!("@{name}"), "cannot start HTTP server"),
            }
        }
        Ok(())
    }
}

/// Go accepts ":8080" to listen on all addresses.
fn normalize_listen(listen: &str) -> String {
    if listen.starts_with(':') { format!("[::]{listen}") } else { listen.to_string() }
}

#[cfg(target_os = "linux")]
fn bind_abstract(name: &str) -> std::io::Result<tokio::net::UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr)?;
    listener.set_nonblocking(true)?;
    tokio::net::UnixListener::from_std(listener)
}

async fn instrumented(metrics: HttpMetrics, location: String, req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    let url = req.uri().to_string();
    let path = req.uri().path().to_string();
    let ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.to_string())
        .unwrap_or_else(|| "@".into());
    let user_agent = req.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    metrics.inflights.inc();
    let response = next.run(req).await;
    metrics.inflights.dec();
    let status = response.status().as_u16();
    let size = response.body().size_hint().exact().unwrap_or(0);
    let duration = start.elapsed();
    let method_label = method.to_lowercase();
    metrics.requests.with_label_values(&[&location, &status.to_string(), &method_label]).inc();
    metrics.durations.with_label_values(&[&location, &method_label]).observe(duration.as_secs_f64());
    metrics.sizes.with_label_values(&[&location, &method_label]).observe(size as f64);
    if path == "/api/v0/metrics" || path == "/api/v0/healthcheck" {
        tracing::debug!(handler = %location, method, url, ip, user_agent, status, size, duration = ?duration, "HTTP request");
    } else {
        tracing::info!(handler = %location, method, url, ip, user_agent, status, size, duration = ?duration, "HTTP request");
    }
    response
}

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// A JSON response with Go's content type and a trailing newline, like
/// `json.Encoder`.
pub fn json_response<T: Serialize>(status: StatusCode, data: &T) -> Response {
    match serde_json::to_vec(data) {
        Ok(mut body) => {
            body.push(b'\n');
            let mut resp = (status, body).into_response();
            resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE));
            resp
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// A YAML response.
pub fn yaml_response<T: Serialize>(status: StatusCode, data: &T) -> Response {
    match serde_norway::to_string(data) {
        Ok(body) => {
            let mut resp = (status, body).into_response();
            resp.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/yaml; charset=utf-8"));
            resp
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// An error message as JSON: `{"message": "..."}`.
pub fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    json_response(status, &serde_json::json!({ "message": message.into() }))
}

/// Read a body into bytes, with a size limit.
pub async fn read_body(body: Body, limit: usize) -> Result<bytes::Bytes, String> {
    axum::body::to_bytes(body, limit).await.map_err(|e| format!("cannot read body: {e}"))
}

/// Routes common to all components: metrics, healthcheck and version.
pub fn common_routes(r: &Reporter, service: &str, version: &'static str) -> (Router, Router) {
    use axum::routing::get;
    let metrics_reporter = r.clone();
    let metrics = move || {
        let r = metrics_reporter.clone();
        async move {
            let mut resp = r.render_metrics().into_response();
            resp.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"));
            resp
        }
    };
    let metrics_router = Router::new()
        .route(&format!("/api/v0/{service}/metrics"), get(metrics.clone()))
        .route("/api/v0/metrics", get(metrics));
    let hc_reporter = r.clone();
    let healthcheck = move || {
        let r = hc_reporter.clone();
        async move {
            let results = r.run_healthchecks(Duration::from_secs(5)).await;
            let status = if results.status == crate::reporter::HealthcheckStatus::Error {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            };
            json_response(status, &results)
        }
    };
    let version_handler = move || async move {
        json_response(
            StatusCode::OK,
            &serde_json::json!({ "version": version, "compiler": crate::helpers::compiler_version() }),
        )
    };
    let api = Router::new()
        .route(&format!("/api/v0/{service}/healthcheck"), get(healthcheck.clone()))
        .route("/api/v0/healthcheck", get(healthcheck))
        .route(&format!("/api/v0/{service}/version"), get(version_handler))
        .route("/api/v0/version", get(version_handler));
    (metrics_router, api)
}

#[cfg(test)]
mod tests;
