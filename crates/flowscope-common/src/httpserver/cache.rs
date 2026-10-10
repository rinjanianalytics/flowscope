// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Response cache. Port of `common/httpserver/cache.go` and
//! `common/httpserver/cachestore`. Responses are stored in memory or in
//! Redis. Only successful responses are cached.

use crate::config::Validate;
use crate::reporter::CounterVec;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

/// In-memory cache backend.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryCacheConfiguration {}

/// Redis cache backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct RedisCacheConfiguration {
    /// "tcp" or "unix".
    pub protocol: String,
    /// Server address (host:port or socket path).
    pub server: String,
    pub username: String,
    pub password: String,
    pub db: i64,
}

impl Default for RedisCacheConfiguration {
    fn default() -> Self {
        RedisCacheConfiguration {
            protocol: "tcp".into(),
            server: "127.0.0.1:6379".into(),
            username: String::new(),
            password: String::new(),
            db: 0,
        }
    }
}

impl Validate for RedisCacheConfiguration {
    fn validate(&self) -> Result<(), String> {
        if self.protocol != "tcp" && self.protocol != "unix" {
            return Err(format!("http.cache.protocol: {:?} should be tcp or unix", self.protocol));
        }
        if self.server.is_empty() {
            return Err("http.cache.server is required".into());
        }
        Ok(())
    }
}

/// A cached response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedResponse {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
}

/// A cache store.
pub trait Store: Send + Sync {
    fn get(&self, key: &str) -> BoxFuture<'_, Result<Option<Vec<u8>>, String>>;
    fn set(&self, key: &str, value: Vec<u8>, expire: Duration) -> BoxFuture<'_, Result<(), String>>;
}

/// In-memory store with a per-entry expiration.
pub struct MemoryStore {
    cache: moka::future::Cache<String, (Vec<u8>, Duration)>,
}

struct PerEntryExpiry;

impl moka::Expiry<String, (Vec<u8>, Duration)> for PerEntryExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &(Vec<u8>, Duration),
        _now: std::time::Instant,
    ) -> Option<Duration> {
        Some(value.1)
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {
            cache: moka::future::Cache::builder()
                .max_capacity(256 * 1024 * 1024)
                .weigher(|k: &String, v: &(Vec<u8>, Duration)| (k.len() + v.0.len()).try_into().unwrap_or(u32::MAX))
                .expire_after(PerEntryExpiry)
                .build(),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for MemoryStore {
    fn get(&self, key: &str) -> BoxFuture<'_, Result<Option<Vec<u8>>, String>> {
        let key = key.to_string();
        Box::pin(async move { Ok(self.cache.get(&key).await.map(|v| v.0)) })
    }

    fn set(&self, key: &str, value: Vec<u8>, expire: Duration) -> BoxFuture<'_, Result<(), String>> {
        let key = key.to_string();
        Box::pin(async move {
            self.cache.insert(key, (value, expire)).await;
            Ok(())
        })
    }
}

/// Redis store.
pub struct RedisStore {
    conn: redis::aio::MultiplexedConnection,
}

impl Store for RedisStore {
    fn get(&self, key: &str) -> BoxFuture<'_, Result<Option<Vec<u8>>, String>> {
        let key = key.to_string();
        Box::pin(async move {
            let mut conn = self.conn.clone();
            redis::cmd("GET").arg(&key).query_async::<Option<Vec<u8>>>(&mut conn).await.map_err(|e| e.to_string())
        })
    }

    fn set(&self, key: &str, value: Vec<u8>, expire: Duration) -> BoxFuture<'_, Result<(), String>> {
        let key = key.to_string();
        Box::pin(async move {
            let mut conn = self.conn.clone();
            redis::cmd("SET")
                .arg(&key)
                .arg(value)
                .arg("PX")
                .arg(expire.as_millis() as u64)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

/// Build the store for a configuration.
pub async fn new_store(config: &super::CacheConfiguration) -> Result<Arc<dyn Store>, String> {
    match config {
        super::CacheConfiguration::Memory(_) => Ok(Arc::new(MemoryStore::new())),
        super::CacheConfiguration::Redis(c) => {
            let url = redis_url(c)?;
            let client = redis::Client::open(url.as_str()).map_err(|e| e.to_string())?;
            let mut conn = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| format!("cannot ping Redis server: {e}"))?;
            redis::cmd("PING")
                .query_async::<String>(&mut conn)
                .await
                .map_err(|e| format!("cannot ping Redis server: {e}"))?;
            Ok(Arc::new(RedisStore { conn }))
        }
    }
}

/// Build a connection URL for Redis.
fn redis_url(c: &RedisCacheConfiguration) -> Result<String, String> {
    let mut url = if c.protocol == "unix" {
        let mut u = reqwest::Url::parse("redis+unix:///").map_err(|e| e.to_string())?;
        u.set_path(&c.server);
        u.query_pairs_mut().append_pair("db", &c.db.to_string());
        u
    } else {
        let mut u = reqwest::Url::parse(&format!("redis://{}", c.server))
            .map_err(|e| format!("invalid Redis server {:?}: {e}", c.server))?;
        u.set_path(&format!("/{}", c.db));
        u
    };
    if !c.username.is_empty() {
        url.set_username(&c.username).map_err(|_| "invalid Redis username".to_string())?;
    }
    if !c.password.is_empty() {
        url.set_password(Some(&c.password)).map_err(|_| "invalid Redis password".to_string())?;
    }
    Ok(url.to_string())
}

/// Tower layer caching responses.
#[derive(Clone)]
pub struct CacheLayer {
    store: Arc<dyn Store>,
    hit: CounterVec,
    miss: CounterVec,
    expire: Duration,
    by_body: bool,
}

impl CacheLayer {
    pub(super) fn new(
        store: Arc<dyn Store>,
        hit: CounterVec,
        miss: CounterVec,
        expire: Duration,
        by_body: bool,
    ) -> Self {
        CacheLayer { store, hit, miss, expire, by_body }
    }
}

impl<S> tower::Layer<S> for CacheLayer {
    type Service = CacheService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        CacheService { inner, layer: self.clone() }
    }
}

/// Service produced by `CacheLayer`.
#[derive(Clone)]
pub struct CacheService<S> {
    inner: S,
    layer: CacheLayer,
}

const BODY_LIMIT: usize = 64 * 1024 * 1024;

impl<S> tower::Service<Request> for CacheService<S>
where
    S: tower::Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Response, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let mut inner = self.inner.clone();
        std::mem::swap(&mut inner, &mut self.inner);
        let layer = self.layer.clone();
        Box::pin(async move {
            let path = req.uri().path().to_string();
            let method = req.method().to_string();
            let (req, key) = if layer.by_body {
                let (parts, body) = req.into_parts();
                let body = match axum::body::to_bytes(body, BODY_LIMIT).await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!(error = %e, "cannot read body for cache key");
                        return Ok(Response::builder().status(StatusCode::BAD_REQUEST).body(Body::empty()).unwrap());
                    }
                };
                let mut h = Sha256::new();
                h.update(format!("{method}\x00{path}\x00").as_bytes());
                h.update(&body);
                let key = format!("cache-request-{}", hex(&h.finalize()));
                (Request::from_parts(parts, Body::from(body)), key)
            } else {
                (req, format!("cache-path-{path}"))
            };

            match layer.store.get(&key).await {
                Ok(Some(raw)) => {
                    if let Ok(cached) = serde_json::from_slice::<CachedResponse>(&raw) {
                        layer.hit.with_label_values(&[&path, &method]).inc();
                        return Ok(rebuild(cached));
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!(error = %e, "cache backend error"),
            }
            layer.miss.with_label_values(&[&path, &method]).inc();

            let response = inner.call(req).await?;
            if !response.status().is_success() {
                return Ok(response);
            }
            let (parts, body) = response.into_parts();
            let body = match axum::body::to_bytes(body, BODY_LIMIT).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(error = %e, "cannot read response body");
                    return Ok(Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::empty())
                        .unwrap());
                }
            };
            let cached = CachedResponse {
                status: parts.status.as_u16(),
                headers: parts.headers.iter().map(|(k, v)| (k.to_string(), v.as_bytes().to_vec())).collect(),
                body: body.to_vec(),
            };
            if let Ok(raw) = serde_json::to_vec(&cached)
                && let Err(e) = layer.store.set(&key, raw, layer.expire).await
            {
                tracing::error!(error = %e, "cannot store cached response");
            }
            Ok(Response::from_parts(parts, Body::from(body)))
        })
    }
}

fn rebuild(cached: CachedResponse) -> Response {
    let mut headers = HeaderMap::new();
    for (k, v) in cached.headers {
        if let (Ok(k), Ok(v)) = (HeaderName::try_from(k), HeaderValue::from_bytes(&v)) {
            headers.append(k, v);
        }
    }
    let mut resp = Response::new(Body::from(cached.body));
    *resp.status_mut() = StatusCode::from_u16(cached.status).unwrap_or(StatusCode::OK);
    *resp.headers_mut() = headers;
    resp
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
