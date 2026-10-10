// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! gNMI metadata provider: poll interface names, descriptions and speeds
//! from exporters with gNMI. Port of `outlet/metadata/provider/gnmi`.
//!
//! A collector task runs for each exporter queried. It detects the model
//! and the encoding, then polls with SubscribeOnce: other subscription modes
//! do not reliably report deletions. A query triggers a refresh, but polls
//! are at least `minimal-refresh-interval` apart.

pub mod events;

use super::gnmi_config::{GnmiConfiguration, Model};
use super::{Answer, Exporter, Provider, ProviderError, Query};
use events::{ExporterState, parse_path, subscribe_responses_to_events};
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::unmap_ip;
use flowscope_common::reporter::{Counter, CounterVec, GaugeVec, Reporter, SummaryVec};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, Endpoint, Uri};

/// Code generated from the openconfig gNMI protobuf files.
#[allow(clippy::all, missing_docs, deprecated)]
pub mod pb {
    pub mod gnmi {
        tonic::include_proto!("gnmi");
    }
    pub mod gnmi_ext {
        tonic::include_proto!("gnmi_ext");
    }
}

use pb::gnmi::g_nmi_client::GNmiClient;

struct Metrics {
    collector_count: Counter,
    ready: GaugeVec,
    models: GaugeVec,
    encodings: GaugeVec,
    errors: CounterVec,
    updates: CounterVec,
    paths: GaugeVec,
    times: SummaryVec,
}

/// State of an exporter shared between its collector and the queries.
struct ExporterSlot {
    state: RwLock<ExporterState>,
    /// Data is ready: the collector is not polling.
    ready: watch::Sender<bool>,
    /// Refresh requests, as a counter.
    refresh: watch::Sender<u64>,
}

struct Inner {
    config: GnmiConfiguration,
    exporters: Mutex<HashMap<Ipv6Addr, Arc<ExporterSlot>>>,
    metrics: Metrics,
    token: CancellationToken,
}

/// The gNMI provider.
pub struct GnmiProvider {
    inner: Arc<Inner>,
}

impl GnmiProvider {
    pub fn new(r: &Reporter, config: GnmiConfiguration, daemon: &Daemon) -> Result<Self, String> {
        for (_, a) in config.authentication_parameters.iter() {
            a.tls.make_rustls_config()?;
        }
        let m = r.metrics("outlet/metadata/provider/gnmi");
        let e = &["exporter"];
        let metrics = Metrics {
            collector_count: m.counter("collector_count", "Number of running collectors."),
            ready: m.gauge_vec("collector_ready_info", "Is the collector ready?", e),
            models: m.gauge_vec("model_info", "Model used for an exporter.", &["exporter", "model"]),
            encodings: m.gauge_vec("encoding_info", "Encoding used for an exporter.", &["exporter", "encoding"]),
            errors: m.counter_vec("errors_total", "Number of errors for a collector.", &["exporter", "error"]),
            updates: m.counter_vec("updates_total", "Number of updates received from an exporter.", e),
            paths: m.gauge_vec("paths_count", "Number of paths retrieved from an exporter.", e),
            times: m.summary_vec("collector_seconds", "Time to poll an exporter.", Duration::from_secs(600), e),
        };
        Ok(GnmiProvider {
            inner: Arc::new(Inner { config, exporters: Mutex::new(HashMap::new()), metrics, token: daemon.token() }),
        })
    }
}

impl Provider for GnmiProvider {
    fn query(&self, query: Query) -> BoxFuture<'_, Result<Answer, ProviderError>> {
        Box::pin(async move { self.inner.query(query).await })
    }
}

/// Count a query giving up before the collector is ready.
struct NotReadyGuard<'a> {
    errors: &'a CounterVec,
    exporter: String,
    armed: bool,
}

impl Drop for NotReadyGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.errors.with_label_values(&[&self.exporter, "not ready"]).inc();
        }
    }
}

impl Inner {
    async fn query(self: &Arc<Self>, query: Query) -> Result<Answer, ProviderError> {
        let exporter = {
            let mut exporters = self.exporters.lock().unwrap();
            exporters
                .entry(query.exporter_ip)
                .or_insert_with(|| {
                    let e = Arc::new(ExporterSlot {
                        state: RwLock::new(ExporterState::default()),
                        ready: watch::channel(false).0,
                        refresh: watch::channel(0).0,
                    });
                    self.metrics.collector_count.inc();
                    let this = self.clone();
                    let e2 = e.clone();
                    let ip = query.exporter_ip;
                    tokio::spawn(async move { this.run_collector(ip, e2).await });
                    e
                })
                .clone()
        };
        exporter.refresh.send_modify(|c| *c += 1);

        // Wait for the collector to be ready. The caller bounds the wait.
        let mut guard =
            NotReadyGuard { errors: &self.metrics.errors, exporter: unmap_ip(&query.exporter_ip), armed: true };
        let mut ready = exporter.ready.subscribe();
        if ready.wait_for(|r| *r).await.is_err() {
            return Err(ProviderError::Other("collector stopped".into()));
        }
        guard.armed = false;

        let state = exporter.state.read().unwrap();
        match state.interfaces.get(&query.if_index) {
            None => Ok(Answer::default()),
            Some(iface) => Ok(Answer {
                found: true,
                exporter: Exporter { name: state.name.clone(), ..Default::default() },
                interface: iface.clone(),
            }),
        }
    }

    /// Sleep, unless shutting down. Returns false when shutting down.
    async fn sleep(&self, d: Duration) -> bool {
        tokio::select! {
            _ = self.token.cancelled() => false,
            _ = tokio::time::sleep(d) => true,
        }
    }

    /// Connect to the exporter.
    async fn connect(&self, exporter: Ipv6Addr) -> Result<(GNmiClient<Channel>, Credentials), String> {
        let c = &self.config;
        let target = c.targets.lookup(&exporter).copied().unwrap_or(IpAddr::V6(exporter));
        let target = match target {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            v4 => v4,
        };
        let port = c.ports.lookup(&exporter).copied().unwrap_or(57400);
        let address = SocketAddr::new(target, port);
        let auth = c.authentication_parameters.lookup(&exporter).cloned().unwrap_or_default();
        let tls = auth.tls.make_rustls_config()?;
        // With TLS, the connector below does the handshake: tonic only sees
        // a plain HTTP/2 connection.
        let endpoint =
            Endpoint::from_shared(format!("http://{address}")).map_err(|e| e.to_string())?.connect_timeout(*c.timeout);
        let channel = match tls {
            None => tokio::time::timeout(*c.timeout, endpoint.connect())
                .await
                .map_err(|_| "connection timeout".to_string())?
                .map_err(|e| e.to_string())?,
            Some(tls) => {
                // rustls configuration from the TLS settings, so skip-verify
                // and client certificates work as with the other clients.
                let mut tls = (*tls).clone();
                tls.alpn_protocols = vec![b"h2".to_vec()];
                let connector = tokio_rustls::TlsConnector::from(Arc::new(tls));
                let host = target.to_string();
                let connect = tower::service_fn(move |uri: Uri| {
                    let connector = connector.clone();
                    let host = host.clone();
                    async move {
                        let authority = uri.authority().map(|a| a.to_string()).unwrap_or_default();
                        let tcp = tokio::net::TcpStream::connect(authority).await?;
                        let name = rustls::pki_types::ServerName::try_from(host)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                        let stream = connector.connect(name, tcp).await?;
                        Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                    }
                });
                tokio::time::timeout(*c.timeout, endpoint.connect_with_connector(connect))
                    .await
                    .map_err(|_| "connection timeout".to_string())?
                    .map_err(|e| e.to_string())?
            }
        };
        Ok((GNmiClient::new(channel), Credentials { username: auth.username, password: auth.password }))
    }

    /// Poll the exporter once for the paths of a model.
    async fn subscribe_once(
        &self,
        client: &mut GNmiClient<Channel>,
        creds: &Credentials,
        model: &Model,
        encoding: Encoding,
        target: Option<String>,
    ) -> Result<Vec<pb::gnmi::SubscribeResponse>, String> {
        use pb::gnmi::{SubscribeRequest, Subscription, SubscriptionList, subscribe_request, subscription_list};
        let paths = model
            .system_name_paths
            .iter()
            .chain(&model.if_index_paths)
            .chain(&model.if_name_paths)
            .chain(&model.if_description_paths)
            .chain(model.if_speed_paths.iter().map(|p| &p.path));
        let mut subscriptions = vec![];
        for p in paths {
            subscriptions.push(Subscription { path: Some(parse_path(p)?), ..Default::default() });
        }
        let prefix = target.map(|t| pb::gnmi::Path { target: t, ..Default::default() });
        let request = SubscribeRequest {
            request: Some(subscribe_request::Request::Subscribe(SubscriptionList {
                prefix,
                subscription: subscriptions,
                mode: subscription_list::Mode::Once as i32,
                encoding: encoding as i32,
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut req = tonic::Request::new(tokio_stream_once(request));
        for (k, v) in [("username", &creds.username), ("password", &creds.password)] {
            if !v.is_empty() {
                req.metadata_mut().insert(k, v.parse().map_err(|_| format!("invalid {k}"))?);
            }
        }
        let mut stream = client.subscribe(req).await.map_err(|e| e.to_string())?.into_inner();
        let mut responses = vec![];
        loop {
            match stream.message().await {
                Ok(Some(r)) => match r.response {
                    Some(pb::gnmi::subscribe_response::Response::SyncResponse(_)) => break,
                    Some(pb::gnmi::subscribe_response::Response::Update(_)) => responses.push(r),
                    _ => {}
                },
                Ok(None) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(responses)
    }

    /// Detect the model and the encoding: the first one giving a system name
    /// and at least an interface (some implementations do not fail on
    /// unknown paths).
    async fn detect(&self, client: &mut GNmiClient<Channel>, creds: &Credentials) -> Result<(Model, Encoding), String> {
        for model in &self.config.models {
            for encoding in [Encoding::JsonIetf, Encoding::Json] {
                let Ok(responses) = self.subscribe_once(client, creds, model, encoding, None).await else { continue };
                let mut state = ExporterState::default();
                state.update(&subscribe_responses_to_events(&responses), model);
                if !state.name.is_empty() && !state.interfaces.is_empty() {
                    return Ok((model.clone(), encoding));
                }
            }
        }
        Err("no compatible model found".into())
    }

    async fn run_collector(self: Arc<Self>, exporter_ip: Ipv6Addr, exporter: Arc<ExporterSlot>) {
        let exporter_str = unmap_ip(&exporter_ip);
        let m = &self.metrics;
        m.ready.with_label_values(&[&exporter_str]).set(0.0);
        tracing::info!(exporter = %exporter_str, "starting gNMI collector");
        let mut init_backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        let error = |e: &str| m.errors.with_label_values(&[&exporter_str, e]).inc();

        // Connect and detect the model.
        let (mut client, creds, model, encoding) = loop {
            let (mut client, creds) = match self.connect(exporter_ip).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(exporter = %exporter_str, error = %e, "unable to create client");
                    error("cannot create client");
                    if !self.sleep(init_backoff.next()).await {
                        return;
                    }
                    continue;
                }
            };
            match self.detect(&mut client, &creds).await {
                Ok((model, encoding)) => break (client, creds, model, encoding),
                Err(e) => {
                    tracing::error!(exporter = %exporter_str, error = %e, "unable to detect model");
                    error("cannot detect model");
                    if !self.sleep(init_backoff.next()).await {
                        return;
                    }
                }
            }
        };
        tracing::info!(exporter = %exporter_str, "model is {:?}, encoding is {:?}", model.name, encoding.name());
        m.models.with_label_values(&[&exporter_str, &model.name]).set(1.0);
        m.encodings.with_label_values(&[&exporter_str, encoding.name()]).set(1.0);
        let target =
            self.config.set_target.lookup(&exporter_ip).copied().unwrap_or(false).then(|| exporter_str.clone());

        let mut fetch_backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
        let mut refresh = exporter.refresh.subscribe();
        let mut has_state = false;
        loop {
            exporter.ready.send_replace(false);
            let start = Instant::now();
            let result = self.subscribe_once(&mut client, &creds, &model, encoding, target.clone()).await;
            m.times.with_label_values(&[&exporter_str]).observe(start.elapsed().as_secs_f64());
            match result {
                Ok(responses) => {
                    let events = subscribe_responses_to_events(&responses);
                    m.paths.with_label_values(&[&exporter_str]).set(events.len() as f64);
                    exporter.state.write().unwrap().update(&events, &model);
                    has_state = true;
                    m.ready.with_label_values(&[&exporter_str]).set(1.0);
                    m.updates.with_label_values(&[&exporter_str]).inc();
                    exporter.ready.send_replace(true);
                    // Ignore refresh requests for a while, then wait for one.
                    if !self.sleep(*self.config.minimal_refresh_interval).await {
                        return;
                    }
                    refresh.borrow_and_update();
                    tokio::select! {
                        _ = self.token.cancelled() => return,
                        r = refresh.changed() => if r.is_err() { return },
                    }
                    fetch_backoff.reset();
                }
                Err(e) => {
                    // Keep serving the previous state while retrying.
                    tracing::error!(exporter = %exporter_str, error = %e, "cannot poll");
                    error("cannot poll");
                    exporter.ready.send_replace(has_state);
                    if !self.sleep(fetch_backoff.next()).await {
                        return;
                    }
                }
            }
        }
    }
}

/// Username and password sent as metadata, like gnmic.
#[derive(Clone, Debug, Default)]
struct Credentials {
    username: String,
    password: String,
}

/// gNMI encodings tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum Encoding {
    Json = 0,
    JsonIetf = 4,
}

impl Encoding {
    fn name(self) -> &'static str {
        match self {
            Encoding::Json => "json",
            Encoding::JsonIetf => "json_ietf",
        }
    }
}

/// Exponential backoff with jitter.
struct Backoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    fn new(initial: Duration, max: Duration) -> Backoff {
        Backoff { initial, max, current: initial }
    }

    fn next(&mut self) -> Duration {
        let d = self.current.mul_f64(0.5 + rand::random::<f64>());
        self.current = self.current.mul_f64(1.5).min(self.max);
        d
    }

    fn reset(&mut self) {
        self.current = self.initial;
    }
}

/// A stream of one message that stays open, so the server does not see the
/// end of the requests before answering.
fn tokio_stream_once<T: Send + 'static>(item: T) -> impl futures::Stream<Item = T> + Send + 'static {
    use futures::StreamExt;
    futures::stream::once(async move { item }).chain(futures::stream::pending())
}

#[cfg(test)]
mod tests;
