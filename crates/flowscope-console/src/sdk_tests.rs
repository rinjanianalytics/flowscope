// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Keep the SDK in step with the console: its types hold the answers of the
//! API, its OpenAPI document lists routes which exist, and its filter
//! builder writes expressions the parser accepts.

use crate::auth;
use crate::component_tests::console;
use crate::config::Configuration;
use crate::filter;
use axum::body::Body;
use axum::http::Request;
use flowscope_schema::Component;
use flowscope_sdk::filter::{Filter, Value, and, col, or};
use flowscope_sdk::models::*;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;
use tower::ServiceExt;

#[derive(Deserialize)]
struct Case {
    method: String,
    url: String,
    #[serde(default)]
    body: Option<Json>,
    status: u16,
    resp_body: String,
}

/// The documented routes: method, path template and operation.
fn operations() -> Vec<(String, String, String)> {
    let doc = serde_json::to_value(flowscope_sdk::openapi::document_value()).unwrap();
    let mut operations = vec![];
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            let id = operation["operationId"].as_str().unwrap().to_string();
            operations.push((method.to_uppercase(), path.clone(), id));
        }
    }
    operations
}

/// Tell if a path matches a template like "/widget/top/{name}".
fn matches(template: &str, path: &str) -> bool {
    let (a, b): (Vec<_>, Vec<_>) = (template.split('/').collect(), path.split('/').collect());
    a.len() == b.len() && a.iter().zip(&b).all(|(t, p)| t.starts_with('{') || t == p)
}

/// Numbers are compared by value: 10 and 10.0 are the same.
fn normalize(v: &mut Json) {
    match v {
        Json::Number(n) => *v = Json::from(n.as_f64().unwrap()),
        Json::Array(a) => a.iter_mut().for_each(normalize),
        Json::Object(o) => o.values_mut().for_each(normalize),
        _ => {}
    }
}

/// Decode a JSON value as an SDK type and encode it back.
fn through<T: DeserializeOwned + Serialize>(v: &Json) -> Result<Json, String> {
    let typed: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(typed).unwrap())
}

fn answer_through_sdk(operation: &str, v: &Json) -> Option<Result<Json, String>> {
    Some(match operation {
        "getConfiguration" => through::<Configuration_>(v),
        "getUserInfo" => through::<UserInfo>(v),
        "graphLine" => through::<LineOutput>(v),
        "graphSankey" => through::<SankeyOutput>(v),
        "graphTableInterval" => through::<TableIntervalOutput>(v),
        "graphReverse" => through::<Reverse>(v),
        "filterValidate" => through::<ValidateOutput>(v),
        "filterComplete" => through::<CompleteOutput>(v),
        "listSavedFilters" => through::<SavedFilters>(v),
        "widgetFlowRate" => through::<FlowRate>(v),
        "widgetExporters" => through::<Exporters>(v),
        "widgetTop" => through::<Top>(v),
        "widgetGraph" => through::<Traffic>(v),
        "widgetFlowLast" => through::<LastFlow>(v),
        "getDocument" => through::<Document>(v),
        _ => return None,
    })
}

fn request_through_sdk(operation: &str, v: &Json) -> Option<Result<Json, String>> {
    Some(match operation {
        "graphLine" => through::<LineInput>(v),
        "graphSankey" => through::<GraphInput>(v),
        "graphTableInterval" => through::<TableIntervalInput>(v),
        "graphReverse" => through::<Reverse>(v),
        "filterValidate" => through::<ValidateInput>(v),
        "filterComplete" => through::<CompleteInput>(v),
        "addSavedFilter" => through::<SavedFilterInput>(v),
        _ => return None,
    })
}

// The name is taken by the configuration of the console.
type Configuration_ = flowscope_sdk::models::Configuration;

/// The answers recorded from the Go console fit the types of the SDK, with
/// nothing lost. So do the requests it accepted.
#[test]
fn types_hold_the_golden_exchanges() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/console-http.json");
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let operations = operations();
    let mut failures = vec![];
    let (mut answers, mut requests, mut errors) = (0, 0, 0);
    for case in &cases {
        // The pages of the documentation are not part of the API.
        let Some(path) = case.url.split('?').next().filter(|p| p.starts_with(flowscope_sdk::openapi::PREFIX)) else {
            continue;
        };
        let ctx = format!("{} {} {:?}", case.method, case.url, case.body);
        let Some((_, _, operation)) = operations.iter().find(|(m, t, _)| *m == case.method && matches(t, path)) else {
            failures.push(format!("{ctx}: not in the OpenAPI document"));
            continue;
        };
        let Ok(mut want) = serde_json::from_str::<Json>(&case.resp_body) else { continue };
        if !(200..300).contains(&case.status) {
            // Errors all have the same shape.
            match through::<ErrorMessage>(&want) {
                Ok(got) if got == want => errors += 1,
                other => failures.push(format!("{ctx}: error answer {other:?}, want {want}")),
            }
            continue;
        }
        match answer_through_sdk(operation, &want) {
            Some(Ok(mut got)) => {
                normalize(&mut got);
                normalize(&mut want);
                if got != want {
                    failures.push(format!("{ctx}: answer changed by the SDK type\n got  {got}\n want {want}"));
                }
                answers += 1;
            }
            Some(Err(e)) => failures.push(format!("{ctx}: answer does not fit the SDK type: {e}")),
            None => failures.push(format!("{ctx}: no SDK type for the answer of {operation}")),
        }
        if let Some(body) = &case.body {
            match request_through_sdk(operation, body) {
                Some(Ok(got)) => {
                    // The SDK may write defaults, but keeps what was given.
                    for (key, value) in body.as_object().unwrap() {
                        let (mut a, mut b) = (got.get(key).cloned().unwrap_or(Json::Null), value.clone());
                        normalize(&mut a);
                        normalize(&mut b);
                        // A default left out is the same as the default.
                        let default = a.is_null() && (b == false || b == "" || b == 0.0 || b == Json::Array(vec![]));
                        if a != b && !default {
                            failures.push(format!("{ctx}: request key {key} became {a}"));
                        }
                    }
                    requests += 1;
                }
                Some(Err(e)) => failures.push(format!("{ctx}: request does not fit the SDK type: {e}")),
                None => failures.push(format!("{ctx}: no SDK type for the request of {operation}")),
            }
        }
    }
    for f in &failures {
        eprintln!("{f}\n");
    }
    assert!(failures.is_empty(), "{} failures", failures.len());
    // Guard against a test which silently checks nothing.
    assert!(answers >= 40 && requests >= 25 && errors >= 20, "{answers} answers, {requests} requests, {errors} errors");
}

/// Every documented route is served by the console.
#[tokio::test]
async fn documented_routes_exist() {
    let (c, _mock, http) = console(Configuration::default(), auth::Configuration::default()).await;
    let app = c.router(&http);
    let operations = operations();
    assert_eq!(operations.len(), 18);
    for (method, template, operation) in operations {
        let path = template.replace("{id}", "1").replace("{name}", "intro");
        let request = Request::builder()
            .method(method.as_str())
            .uri(&path)
            .header("Content-Type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        // A handler explains its errors with a message. The router does not.
        let from_handler = (200..300).contains(&status) || serde_json::from_slice::<ErrorMessage>(&body).is_ok();
        assert!(status != 405 && from_handler, "{operation}: {method} {path} is not served (status {status})");
    }
}

/// The console serves the document of the SDK.
#[tokio::test]
async fn openapi_is_served() {
    let (c, _mock, http) = console(Configuration::default(), auth::Configuration::default()).await;
    let request = Request::builder().uri("/api/v0/console/openapi.json").body(Body::empty()).unwrap();
    let response = c.router(&http).oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(String::from_utf8_lossy(&body), flowscope_sdk::openapi::json());
}

/// The filters written by the builder are accepted by the parser, and mean
/// what the builder says.
#[test]
fn built_filters_parse() {
    let meta = filter::Meta {
        schema: Component::default_schema().schema().clone(),
        database: "default".into(),
        reverse_direction: false,
    };
    let filters: Vec<Filter> = vec![
        col("InIfBoundary").eq_keyword("external"),
        col("SrcAS").eq(15169),
        col("SrcAS").ne(Value::keyword("AS2906")),
        col("SrcAS").is_in([15169, 2906]),
        col("SrcAS").eq(Value::column("DstAS")),
        col("DstPort").ge(1024),
        col("DstPort").not_in([80u16, 443]),
        col("ExporterName").eq(r#"edge "one" \ two"#),
        col("ExporterName").eq("it's (not) AND -- a /* comment"),
        col("ExporterName").eq("line\nbreak"),
        col("InIfName").like("Gi0/0/%"),
        col("InIfName").iunlike("%transit%"),
        col("InIfProvider").is_in(["cogent", "telia"]),
        col("SrcAddr").eq("2001:db8::1".parse::<std::net::IpAddr>().unwrap()),
        col("DstAddr").within("192.0.2.0/24"),
        col("EType").eq_keyword("IPv6"),
        col("Proto").eq("tcp"),
        col("Proto").eq(17),
        !col("Proto").eq("icmp"),
        and([
            col("InIfBoundary").eq_keyword("external"),
            or([col("SrcAS").eq(1), col("SrcAS").eq(2).and(col("DstPort").eq(53))]),
            !or([col("Proto").eq("udp"), col("Proto").eq("tcp")]),
        ]),
        Filter::parsed("SrcAS = 1 OR SrcAS = 2").and(col("DstAS").eq(3)),
    ];
    for built in &filters {
        let text = built.to_string();
        if let Err(e) = filter::parse(&text, &meta) {
            panic!("{text}: {}", e.human());
        }
    }
    // Text is kept as given, quotes and backslashes included.
    let sql = |text: &str| filter::parse(text, &meta).unwrap().expr.unwrap().to_string();
    assert_eq!(
        sql(&col("ExporterName").eq(r#"edge "one" \ two"#).to_string()),
        sql(r#"ExporterName = 'edge "one" \\ two'"#)
    );
    // Parentheses follow the precedence of the language.
    assert_eq!(
        sql(&or([col("SrcAS").eq(1), col("SrcAS").eq(2)]).and(col("DstAS").eq(3)).to_string()),
        sql("(SrcAS = 1 OR SrcAS = 2) AND DstAS = 3")
    );
    assert_eq!(
        sql(&col("SrcAS").eq(1).or(col("SrcAS").eq(2).and(col("DstAS").eq(3))).to_string()),
        sql("SrcAS = 1 OR SrcAS = 2 AND DstAS = 3")
    );
}
