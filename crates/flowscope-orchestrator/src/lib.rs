// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Orchestrator service: it serves the configuration of the other services,
//! manages the ClickHouse database and the Kafka topics. Port of
//! `orchestrator/`.

pub mod clickhouse;
pub mod kafka;
pub mod sql;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Services the orchestrator has a configuration for.
pub const SERVICES: &[&str] = &["inlet", "outlet", "orchestrator", "console", "demo-exporter"];

/// Configurations of the other services, served over HTTP.
#[derive(Default)]
pub struct Orchestrator {
    configurations: Mutex<BTreeMap<String, Vec<serde_json::Value>>>,
}

impl Orchestrator {
    pub fn new() -> Arc<Orchestrator> {
        Arc::new(Orchestrator::default())
    }

    /// Register one configuration for a service.
    pub fn register_configuration(&self, service: &str, configuration: serde_json::Value) {
        self.configurations.lock().unwrap().entry(service.into()).or_default().push(configuration);
    }

    /// Routes serving the configurations.
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/api/v0/orchestrator/configuration/{service}", axum::routing::get(configuration))
            .route("/api/v0/orchestrator/configuration/{service}/{index}", axum::routing::get(configuration_index))
            .with_state(self.clone())
    }

    /// The configuration of a service. An index out of range gives the first
    /// one, so a service with several instances can use its own index.
    fn lookup(&self, service: &str, index: usize) -> Option<serde_json::Value> {
        let configurations = self.configurations.lock().unwrap();
        let list = configurations.get(service)?;
        list.get(index).or(list.first()).cloned()
    }
}

fn not_found(message: &str) -> Response {
    (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"message": message}))).into_response()
}

fn yaml(value: &serde_json::Value) -> Response {
    match serde_norway::to_string(value) {
        Ok(body) => ([(header::CONTENT_TYPE, "application/yaml; charset=utf-8")], body).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn configuration(State(o): State<Arc<Orchestrator>>, Path(service): Path<String>) -> Response {
    match o.lookup(&service, 0) {
        Some(c) => yaml(&c),
        None => not_found("Configuration not found."),
    }
}

async fn configuration_index(
    State(o): State<Arc<Orchestrator>>,
    Path((service, index)): Path<(String, String)>,
) -> Response {
    let Ok(index) = index.parse::<usize>() else { return not_found("Invalid configuration index.") };
    match o.lookup(&service, index) {
        Some(c) => yaml(&c),
        None => not_found("Configuration not found."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    async fn get(router: &Router, path: &str) -> (StatusCode, String) {
        let resp = router
            .clone()
            .oneshot(axum::http::Request::get(path).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn configurations() {
        let o = Orchestrator::new();
        o.register_configuration("inlet", serde_json::json!({"name": "first"}));
        o.register_configuration("inlet", serde_json::json!({"name": "second"}));
        let router = o.router();
        assert_eq!(
            get(&router, "/api/v0/orchestrator/configuration/inlet").await,
            (StatusCode::OK, "name: first\n".into())
        );
        assert_eq!(get(&router, "/api/v0/orchestrator/configuration/inlet/1").await.1, "name: second\n");
        assert_eq!(get(&router, "/api/v0/orchestrator/configuration/inlet/5").await.1, "name: first\n");
        let (status, body) = get(&router, "/api/v0/orchestrator/configuration/inlet/x").await;
        assert_eq!((status, body.as_str()), (StatusCode::NOT_FOUND, r#"{"message":"Invalid configuration index."}"#));
        let (status, body) = get(&router, "/api/v0/orchestrator/configuration/console").await;
        assert_eq!((status, body.as_str()), (StatusCode::NOT_FOUND, r#"{"message":"Configuration not found."}"#));
    }
}
