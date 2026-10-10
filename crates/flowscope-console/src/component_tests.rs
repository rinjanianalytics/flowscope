// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Replay the requests of `console/golden_dump_test.go` (TestGoldenDumpHTTP)
//! with the same ClickHouse rows, and compare with the answers of Go.

use crate::auth::{self, Authentication};
use crate::component::{ClickHouseQuery, Console, Dependencies};
use crate::config::Configuration;
use crate::database::{self, Database};
use axum::body::Body;
use axum::http::Request;
use flowscope_clickhouse::{Block, Column, DataType, QueryResult, Values};
use flowscope_common::httpserver::HttpServer;
use flowscope_common::reporter::Reporter;
use flowscope_schema::Component;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// A mocked ClickHouse answering the next prepared result.
#[derive(Default)]
pub(crate) struct Mock {
    pub next: Mutex<Option<QueryResult>>,
    pub queries: Mutex<Vec<String>>,
}

impl ClickHouseQuery for Arc<Mock> {
    fn query(&self, sql: String) -> Pin<Box<dyn Future<Output = Result<QueryResult, String>> + Send>> {
        self.queries.lock().unwrap().push(sql);
        let next = self.next.lock().unwrap().take();
        Box::pin(async move { next.ok_or_else(|| "unexpected query".to_string()) })
    }
}

/// A result with the provided columns.
pub(crate) fn result(columns: Vec<(&str, DataType, Values)>) -> QueryResult {
    let rows = columns.first().map(|(_, _, v)| v.len()).unwrap_or(0);
    let columns =
        columns.into_iter().map(|(name, data_type, values)| Column { name: name.into(), data_type, values }).collect();
    QueryResult { blocks: vec![Block { columns, rows }], ..Default::default() }
}

fn strings(rows: &[Json], key: &str) -> Values {
    Values::String(rows.iter().map(|r| r[key].as_str().unwrap_or_default().as_bytes().to_vec()).collect())
}

fn floats(rows: &[Json], key: &str) -> Values {
    Values::Float64(rows.iter().map(|r| r[key].as_f64().unwrap()).collect())
}

fn times(rows: &[Json], key: &str) -> Values {
    Values::UInt32(
        rows.iter()
            .map(|r| chrono::DateTime::parse_from_rfc3339(r[key].as_str().unwrap()).unwrap().timestamp() as u32)
            .collect(),
    )
}

fn arrays(rows: &[Json], key: &str) -> Values {
    let mut offsets = vec![];
    let mut values = vec![];
    for r in rows {
        for v in r[key].as_array().cloned().unwrap_or_default() {
            values.push(v.as_str().unwrap().as_bytes().to_vec());
        }
        offsets.push(values.len() as u64);
    }
    Values::Array(offsets, Box::new(Values::String(values)))
}

fn axes(rows: &[Json]) -> Values {
    Values::UInt8(rows.iter().map(|r| r["axis"].as_u64().unwrap() as u8).collect())
}

/// The result for a kind of rows of the Go dumper.
fn rows_result(kind: &str, rows: &[Json]) -> QueryResult {
    let string = || DataType::String;
    let array = || DataType::Array(Box::new(DataType::String));
    match kind {
        "label" => result(vec![("label", string(), strings(rows, "label"))]),
        "label-detail" => {
            result(vec![("label", string(), strings(rows, "label")), ("detail", string(), strings(rows, "detail"))])
        }
        "line" => result(vec![
            ("axis", DataType::UInt8, axes(rows)),
            ("time", DataType::DateTime(None), times(rows, "time")),
            ("xps", DataType::Float64, floats(rows, "xps")),
            ("dimensions", array(), arrays(rows, "dimensions")),
        ]),
        "sankey" => result(vec![
            ("axis", DataType::UInt8, axes(rows)),
            ("xps", DataType::Float64, floats(rows, "xps")),
            ("dimensions", array(), arrays(rows, "dimensions")),
        ]),
        "top" => result(vec![
            ("Name", string(), strings(rows, "Name")),
            ("Percent", DataType::Float64, floats(rows, "Percent")),
        ]),
        "exporters" => result(vec![("ExporterName", string(), strings(rows, "ExporterName"))]),
        "graph" => result(vec![
            ("Time", DataType::DateTime(None), times(rows, "Time")),
            ("Gbps", DataType::Float64, floats(rows, "Gbps")),
        ]),
        k => panic!("unknown rows kind {k}"),
    }
}

/// A console with a mocked ClickHouse, an in-memory database and a fixed
/// clock.
pub(crate) async fn console(config: Configuration, auth: auth::Configuration) -> (Arc<Console>, Arc<Mock>, HttpServer) {
    let mock = Arc::new(Mock::default());
    let r = Reporter::new(&Default::default());
    let deps = Dependencies {
        clickhouse: Arc::new(mock.clone()),
        database_name: "default".into(),
        schema: Component::default_schema().schema().clone(),
        auth: Authentication::new(auth),
        database: Database::new(database::Configuration::default()).await.unwrap(),
        clock: Arc::new(|| chrono::DateTime::parse_from_rfc3339("2022-04-11T15:45:10Z").unwrap()),
    };
    let c = Console::new(&r, config, deps).unwrap();
    let http = HttpServer::new(&r, "console", Default::default()).await.unwrap();
    (c, mock, http)
}

#[derive(Deserialize)]
struct Case {
    method: String,
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<Json>,
    #[serde(default)]
    rows_kind: String,
    #[serde(default)]
    rows: Vec<Json>,
    status: u16,
    resp_headers: BTreeMap<String, String>,
    resp_body: String,
}

#[tokio::test]
async fn same_as_go() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/console-http.json");
    let mut cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    // The avatars are not the ones of Akvorado: their ETag has a version.
    for case in cases.iter_mut().filter(|c| c.url.ends_with("/user/avatar")) {
        for value in case.headers.get_mut("If-None-Match").into_iter().chain(case.resp_headers.get_mut("Etag")) {
            *value = value.replacen('"', "\"fs1-", 1);
        }
    }
    let (c, mock, http) = console(Configuration::default(), auth::Configuration::default()).await;
    let app = c.router(&http);
    let mut failures = vec![];
    for case in &cases {
        *mock.next.lock().unwrap() = (!case.rows_kind.is_empty()).then(|| rows_result(&case.rows_kind, &case.rows));
        let mut req = Request::builder().method(case.method.as_str()).uri(&case.url);
        req = req.header("Content-Type", "application/json");
        for (k, v) in &case.headers {
            req = req.header(k, v);
        }
        let body = case.body.as_ref().map(|b| Body::from(serde_json::to_vec(b).unwrap())).unwrap_or_default();
        let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let headers: BTreeMap<String, String> = ["Content-Type", "Etag", "Vary", "Cache-Control"]
            .iter()
            .filter_map(|k| resp.headers().get(*k).map(|v| (k.to_string(), v.to_str().unwrap().to_string())))
            .collect();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let ctx = format!("{} {} {:?}", case.method, case.url, case.body);
        if status != case.status {
            failures.push(format!("{ctx}: status {status}, want {}: {}", case.status, String::from_utf8_lossy(&bytes)));
            continue;
        }
        if headers != case.resp_headers {
            failures.push(format!("{ctx}: headers {headers:?}, want {:?}", case.resp_headers));
        }
        let text = String::from_utf8_lossy(&bytes);
        if case.resp_body.starts_with('<') && case.resp_body.ends_with(" bytes>") {
            continue;
        }
        // The documentation changes after the answers were recorded. Its
        // rendering is compared in docs.rs, on a copy taken with them.
        if case.url.contains("/docs/") && status == 200 {
            continue;
        }
        let same = match (serde_json::from_str::<Json>(&text), serde_json::from_str::<Json>(&case.resp_body)) {
            (Ok(mut a), Ok(mut b)) => {
                // Go tests run with the "dev" version.
                for v in [&mut a, &mut b] {
                    if let Some(o) = v.as_object_mut() {
                        o.remove("version");
                    }
                }
                a == b
            }
            _ => text == case.resp_body,
        };
        if !same {
            failures.push(format!(
                "{ctx}:\n got  {}\n want {}",
                &text[..text.len().min(600)],
                &case.resp_body[..case.resp_body.len().min(600)]
            ));
        }
    }
    for f in &failures {
        eprintln!("{f}\n");
    }
    assert!(failures.is_empty(), "{} failures out of {}", failures.len(), cases.len());
}
