// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Periodic fetch of remote data sources (JSON, CSV or text over HTTP),
//! transformed with a jq filter. Port of `common/remotedatasource`.

use crate::config::{Duration, from_value};
use crate::daemon::Daemon;
use crate::helpers::TlsConfiguration;
use crate::reporter::{CounterVec, GaugeVec, Reporter};
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How to parse the response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParserType {
    #[default]
    Json,
    CsvComma,
    CsvSemicolon,
    CsvColon,
    Plain,
}

/// How to find the next page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaginationType {
    #[default]
    Auto,
    None,
    LinkNext,
    RelNext,
}

/// A jq filter, kept as source and checked when decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformQuery(pub String);

impl Default for TransformQuery {
    fn default() -> Self {
        TransformQuery(".".into())
    }
}

impl Serialize for TransformQuery {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TransformQuery {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let s = if s.trim().is_empty() { ".".to_string() } else { s };
        check_jq(&s).map_err(|e| serde::de::Error::custom(format!("invalid jq filter: {e}")))?;
        Ok(TransformQuery(s))
    }
}

/// A remote data source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Source {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub proxy: bool,
    pub timeout: Duration,
    pub parser: ParserType,
    pub transform: TransformQuery,
    pub interval: Duration,
    pub pagination: PaginationType,
    pub tls: TlsConfiguration,
}

impl Default for Source {
    fn default() -> Self {
        Source {
            url: String::new(),
            method: "GET".into(),
            headers: BTreeMap::new(),
            proxy: false,
            timeout: Duration::from_secs(60),
            parser: ParserType::Json,
            transform: TransformQuery::default(),
            interval: Duration::ZERO,
            pagination: PaginationType::Auto,
            tls: TlsConfiguration::default(),
        }
    }
}

impl Source {
    pub fn validate(&self) -> Result<(), String> {
        reqwest::Url::parse(&self.url).map_err(|e| format!("invalid URL {:?}: {e}", self.url))?;
        if self.method != "GET" && self.method != "POST" {
            return Err(format!("method should be GET or POST, not {:?}", self.method));
        }
        if self.timeout.as_secs() < 1 {
            return Err("timeout should be at least 1s".into());
        }
        if self.interval.as_secs() < 60 {
            return Err("interval should be at least 1m".into());
        }
        self.tls.validate()
    }
}

/// Check a jq filter compiles.
pub fn check_jq(code: &str) -> Result<(), String> {
    use jaq_core::Compiler;
    use jaq_core::load::{Arena, File, Loader};
    let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
    let funs =
        jaq_core::funs::<jaq_core::data::JustLut<jaq_json::Val>>().chain(jaq_std::funs()).chain(jaq_json::funs());
    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader
        .load(&arena, File { code, path: () })
        .map_err(|errs| format!("cannot parse jq filter: {} error(s)", errs.len()))?;
    Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errs| format!("cannot compile jq filter: {} error(s)", errs.len()))?;
    Ok(())
}

/// Run a jq filter on a value.
pub fn run_jq(code: &str, input: Value) -> Result<Vec<Value>, String> {
    use jaq_core::load::{Arena, File, Loader};
    use jaq_core::{Compiler, Ctx, Vars, data, unwrap_valr};
    use jaq_json::Val;

    let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
    let funs = jaq_core::funs().chain(jaq_std::funs()).chain(jaq_json::funs());
    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader
        .load(&arena, File { code, path: () })
        .map_err(|errs| format!("cannot parse jq filter: {} error(s)", errs.len()))?;
    let filter = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errs| format!("cannot compile jq filter: {} error(s)", errs.len()))?;
    let raw = serde_json::to_vec(&input).map_err(|e| e.to_string())?;
    let input = jaq_json::read::parse_single(&raw).map_err(|e| format!("{e:?}"))?;
    let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let mut out = Vec::new();
    for v in filter.id.run((ctx, input)).map(unwrap_valr) {
        let v = v.map_err(|e| format!("cannot execute jq filter: {e}"))?;
        out.push(serde_json::from_str(&v.to_string()).map_err(|e| format!("cannot convert jq result: {e}"))?);
    }
    Ok(out)
}

/// Parse a response body.
pub fn parse_response(parser: ParserType, body: &[u8]) -> Result<Value, String> {
    match parser {
        ParserType::Json => serde_json::from_slice(body).map_err(|_| "cannot decode JSON".into()),
        ParserType::CsvComma | ParserType::CsvSemicolon | ParserType::CsvColon => {
            let delimiter = match parser {
                ParserType::CsvSemicolon => b';',
                ParserType::CsvColon => b':',
                _ => b',',
            };
            let mut reader =
                csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(delimiter).from_reader(body);
            let mut out = Vec::new();
            for record in reader.records() {
                let record = record.map_err(|_| "cannot decode CSV".to_string())?;
                let obj: serde_json::Map<String, Value> = record
                    .iter()
                    .enumerate()
                    .map(|(j, f)| (format!("f{}", j + 1), Value::String(f.to_string())))
                    .collect();
                out.push(Value::Object(obj));
            }
            Ok(Value::Array(out))
        }
        ParserType::Plain => {
            let text = String::from_utf8_lossy(body);
            Ok(Value::Array(text.lines().map(|l| Value::String(l.trim_end_matches('\r').to_string())).collect()))
        }
    }
}

/// Find the URL with rel=next in a Link header (RFC 8288).
pub fn parse_link_header(header: &str, rel: &str) -> Option<String> {
    let split = |s: &str, sep: char, angle: bool| -> Vec<String> {
        let mut out = Vec::new();
        let (mut in_quotes, mut in_angle) = (false, false);
        let mut cur = String::new();
        for c in s.chars() {
            match c {
                '"' if !in_angle => in_quotes = !in_quotes,
                '<' if angle && !in_quotes => in_angle = true,
                '>' if angle && !in_quotes => in_angle = false,
                c if c == sep && !in_quotes && !in_angle => {
                    out.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        out.push(cur);
        out
    };
    for value in split(header, ',', true) {
        let value = value.trim();
        if !value.starts_with('<') {
            continue;
        }
        let Some(closing) = value.find('>') else { continue };
        let uri = &value[1..closing];
        for param in split(&value[closing + 1..], ';', false) {
            let param = param.trim();
            let (key, v) = param.split_once('=').unwrap_or((param, ""));
            if !key.trim().eq_ignore_ascii_case("rel") {
                continue;
            }
            let v = v.trim().trim_matches('"');
            if v.split_whitespace().any(|r| r.eq_ignore_ascii_case(rel)) {
                return Some(uri.to_string());
            }
        }
    }
    None
}

fn link_next(body: &Value) -> Option<String> {
    body.get("next").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from)
}

#[derive(Clone)]
struct Metrics {
    updates: CounterVec,
    errors: CounterVec,
    count: GaugeVec,
}

/// Function storing the fetched data. Returns the number of objects.
pub type ProviderFn = Arc<dyn Fn(String, Source) -> BoxFuture<'static, Result<usize, String>> + Send + Sync>;

/// Fetcher for a set of data sources.
pub struct RemoteDataSource {
    data_type: String,
    sources: BTreeMap<String, Source>,
    metrics: Metrics,
}

impl RemoteDataSource {
    /// Create a fetcher. `module` is used to name the metrics.
    pub fn new(r: &Reporter, module: &str, data_type: &str, sources: BTreeMap<String, Source>) -> Self {
        let m = r.metrics(module);
        RemoteDataSource {
            data_type: data_type.to_string(),
            sources,
            metrics: Metrics {
                updates: m.counter_vec(
                    "updates_total",
                    "Number of successful updates for a remote data source.",
                    &["type", "source"],
                ),
                errors: m.counter_vec(
                    "errors_total",
                    "Number of failed updates for a remote data source.",
                    &["type", "source", "error"],
                ),
                count: m.gauge_vec(
                    "data_total",
                    "Number of objects imported from a given source.",
                    &["type", "source"],
                ),
            },
        }
    }

    /// Fetch a source and decode each result of the jq filter as `T`.
    pub async fn fetch<T: DeserializeOwned>(name: &str, source: &Source) -> Result<Vec<T>, String> {
        tracing::info!(name, url = %source.url, "update data source");
        let mut builder = reqwest::Client::builder().timeout(source.timeout.as_std());
        if !source.proxy {
            builder = builder.no_proxy();
        }
        if source.tls.enable && source.tls.skip_verify {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if source.tls.enable && !source.tls.ca_file.is_empty() {
            let pem = std::fs::read(&source.tls.ca_file).map_err(|e| format!("cannot read CA: {e}"))?;
            let cert = reqwest::Certificate::from_pem(&pem).map_err(|e| format!("cannot parse CA: {e}"))?;
            builder = builder.add_root_certificate(cert);
        }
        let client = builder.build().map_err(|_| "cannot build HTTP request".to_string())?;
        let mut results = Vec::new();
        let mut current = Some(source.url.clone());
        let mut pagination = source.pagination;
        while let Some(url) = current.take() {
            let method = reqwest::Method::from_bytes(source.method.as_bytes()).unwrap_or(reqwest::Method::GET);
            let mut req = client.request(method, &url);
            for (k, v) in &source.headers {
                req = req.header(k, v);
            }
            req = req.header(
                "accept",
                match source.parser {
                    ParserType::Json => "application/json",
                    ParserType::Plain => "text/plain",
                    _ => "text/csv",
                },
            );
            let resp = req.send().await.map_err(|e| {
                tracing::error!(name, error = %e, "unable to fetch data source");
                "cannot fetch data source".to_string()
            })?;
            if resp.status() != 200 {
                tracing::error!(name, status = resp.status().as_u16(), "unexpected status code");
                return Err("unexpected HTTP status code".into());
            }
            let links: Vec<String> =
                resp.headers().get_all("link").iter().filter_map(|v| v.to_str().ok().map(String::from)).collect();
            let body = resp.bytes().await.map_err(|_| "cannot fetch data source".to_string())?;
            let got = parse_response(source.parser, &body)?;
            let rel_next = || links.iter().find_map(|h| parse_link_header(h, "next"));
            let next = match pagination {
                PaginationType::None => None,
                PaginationType::RelNext => rel_next(),
                PaginationType::LinkNext => link_next(&got),
                PaginationType::Auto => {
                    if let Some(u) = rel_next() {
                        pagination = PaginationType::RelNext;
                        Some(u)
                    } else if let Some(u) = link_next(&got) {
                        pagination = PaginationType::LinkNext;
                        Some(u)
                    } else {
                        None
                    }
                }
            };
            current = next.and_then(|n| {
                reqwest::Url::parse(&url).ok().and_then(|base| base.join(&n).ok()).map(|u| u.to_string())
            });
            for v in run_jq(&source.transform.0, got)? {
                let item: T = from_value(v).map_err(|e| {
                    tracing::error!(name, error = %e, "cannot map returned value");
                    "cannot map result".to_string()
                })?;
                results.push(item);
            }
        }
        if results.is_empty() {
            tracing::error!(name, "empty result");
            return Err("empty result".into());
        }
        Ok(results)
    }

    /// Start fetching. `ready` is notified once every source got data once.
    pub fn start(self, daemon: &Daemon, provider: ProviderFn) -> tokio::sync::watch::Receiver<bool> {
        let (tx, rx) = tokio::sync::watch::channel(self.sources.is_empty());
        let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(self.sources.len()));
        let tx = Arc::new(tx);
        for (name, source) in self.sources {
            let token = daemon.token();
            let provider = provider.clone();
            let metrics = self.metrics.clone();
            let data_type = self.data_type.clone();
            let remaining = remaining.clone();
            let tx = tx.clone();
            daemon.spawn_background(async move {
                metrics.count.with_label_values(&[&data_type, &name]).set(0.0);
                let interval = source.interval.as_std().max(std::time::Duration::from_secs(1));
                let mut retry = interval / 10;
                let mut ready = false;
                loop {
                    let result = tokio::time::timeout(source.timeout.as_std(), provider(name.clone(), source.clone()))
                        .await
                        .unwrap_or_else(|_| Err("timeout".into()));
                    let wait = match result {
                        Ok(count) => {
                            metrics.updates.with_label_values(&[&data_type, &name]).inc();
                            metrics.count.with_label_values(&[&data_type, &name]).set(count as f64);
                            if !ready {
                                ready = true;
                                if remaining.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                                    let _ = tx.send(true);
                                }
                            }
                            retry = interval / 10;
                            interval
                        }
                        Err(e) => {
                            metrics.errors.with_label_values(&[&data_type, &name, &e]).inc();
                            let w = retry;
                            retry = (retry * 3 / 2).min(interval);
                            w
                        }
                    };
                    tokio::select! {
                        _ = token.cancelled() => return,
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
            });
        }
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jq() {
        let got = run_jq(".[] | {name: .n, value: (.v * 2)}", json!([{"n": "a", "v": 1}, {"n": "b", "v": 2}])).unwrap();
        assert_eq!(got, vec![json!({"name": "a", "value": 2}), json!({"name": "b", "value": 4})]);
        assert!(check_jq(".[").is_err());
        assert!(check_jq(".[] | nope").is_err());
        check_jq(".[]").unwrap();
    }

    #[test]
    fn parsers() {
        assert_eq!(
            parse_response(ParserType::CsvSemicolon, b"a;b\nc;d\n").unwrap(),
            json!([{"f1": "a", "f2": "b"}, {"f1": "c", "f2": "d"}])
        );
        assert_eq!(parse_response(ParserType::Plain, b"one\r\ntwo\n").unwrap(), json!(["one", "two"]));
        assert!(parse_response(ParserType::Json, b"{").is_err());
    }

    #[test]
    fn link_header() {
        assert_eq!(
            parse_link_header(r#"<https://x/?page=1>; rel="prev", <https://x/?page=3>; rel="next last""#, "next"),
            Some("https://x/?page=3".into())
        );
        assert_eq!(parse_link_header("<https://x/a,b>; rel=next", "next"), Some("https://x/a,b".into()));
        assert_eq!(parse_link_header("<https://x/>; rel=prev", "next"), None);
    }

    #[test]
    fn config() {
        let s: Source =
            from_value(json!({"url": "https://example.com/data.json", "interval": "10m", "transform": ".[]"})).unwrap();
        assert_eq!(s.method, "GET");
        s.validate().unwrap();
        assert!(from_value::<Source>(json!({"transform": ".["})).is_err());
    }
}
