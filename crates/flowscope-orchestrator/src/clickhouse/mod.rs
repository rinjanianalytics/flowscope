// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! ClickHouse management: database migrations and the CSV files the
//! dictionaries are loaded from. Port of `orchestrator/clickhouse`.

mod migrations;
pub mod version;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use flowscope_common::clickhousedb::ClickHouseDb;
use flowscope_common::config::{Duration, migrate};
use flowscope_common::daemon::Daemon;
use flowscope_common::migrated;
use flowscope_common::reporter::{Counter, Gauge, Reporter};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Embedded CSV files for the dictionaries.
pub const DATA: &[(&str, &[u8])] = &[
    ("asns.csv", include_bytes!("../../data/asns.csv")),
    ("icmp.csv", include_bytes!("../../data/icmp.csv")),
    ("protocols.csv", include_bytes!("../../data/protocols.csv")),
    ("tcp.csv", include_bytes!("../../data/tcp.csv")),
    ("udp.csv", include_bytes!("../../data/udp.csv")),
];

/// Configuration of the ClickHouse component.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Do not run the migrations.
    pub skip_migrations: bool,
    /// Resolutions of the flow tables and how long to keep them.
    pub resolutions: Vec<ResolutionConfiguration>,
    /// Number of partitions of a full consolidated table.
    pub max_partitions: u64,
    /// AS names, replacing or extending the builtin list.
    #[serde(rename = "asns")]
    pub asns: BTreeMap<u32, String>,
    /// URL to reach the orchestrator from ClickHouse.
    #[serde(rename = "orchestrator-url")]
    pub orchestrator_url: String,
    /// Basic authentication to reach the orchestrator from ClickHouse.
    pub orchestrator_basic_auth: Option<BasicAuth>,
}

/// Basic authentication credentials.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

/// A table setting value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TableSetting {
    Int(i64),
    String(String),
}

impl TableSetting {
    /// The value as a SQL expression.
    pub fn expr(&self) -> String {
        match self {
            TableSetting::Int(i) => i.to_string(),
            TableSetting::String(s) => crate::sql::quote_string(s),
        }
    }
}

/// A resolution of the flow tables.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ResolutionConfiguration {
    /// Consolidation interval. 0 is the `flows` table, without consolidation.
    pub interval: Duration,
    /// How long to keep data. 0 means forever.
    #[serde(rename = "ttl")]
    pub ttl: Duration,
    /// Additional table settings, merged with the default ones.
    pub table_settings: BTreeMap<String, TableSetting>,
}

impl Default for Configuration {
    fn default() -> Self {
        let r = |interval: u64, ttl: u64| ResolutionConfiguration {
            interval: Duration::from_secs(interval),
            ttl: Duration::from_secs(ttl),
            table_settings: BTreeMap::new(),
        };
        Configuration {
            skip_migrations: false,
            resolutions: vec![
                r(0, 15 * 24 * 3600),
                r(60, 7 * 24 * 3600),
                r(300, 3 * 30 * 24 * 3600),
                r(3600, 12 * 30 * 24 * 3600),
            ],
            max_partitions: 50,
            asns: BTreeMap::new(),
            orchestrator_url: String::new(),
            orchestrator_basic_auth: None,
        }
    }
}

fn migrate_clickhouse(map: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["SystemLogTTL", "PrometheusEndpoint", "Kafka"] {
        migrate::remove(map, k);
    }
    Ok(())
}

migrated!(Configuration, migrate_clickhouse);

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.resolutions.is_empty() {
            return Err("clickhouse.resolutions should not be empty".into());
        }
        for r in &self.resolutions {
            if !r.interval.is_zero() && r.interval.as_secs() < 5 {
                return Err("clickhouse.resolutions: interval should be 0 or at least 5s".into());
            }
            if !r.ttl.is_zero() && r.ttl.as_secs() < 3600 {
                return Err("clickhouse.resolutions: ttl should be 0 or at least 1h".into());
            }
            for k in r.table_settings.keys() {
                if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(format!("clickhouse.resolutions: invalid table setting {k:?}"));
                }
            }
        }
        if !self.orchestrator_url.is_empty() && reqwest::Url::parse(&self.orchestrator_url).is_err() {
            return Err("clickhouse.orchestrator-url should be an URL".into());
        }
        if let Some(a) = &self.orchestrator_basic_auth
            && (a.username.is_empty() || a.password.is_empty())
        {
            return Err("clickhouse.orchestrator-basic-auth needs a username and a password".into());
        }
        Ok(())
    }
}

struct Metrics {
    migrations_running: Gauge,
    migrations_applied: Counter,
    migrations_not_applied: Counter,
}

/// The ClickHouse component.
pub struct ClickHouse {
    config: Mutex<Configuration>,
    db: Option<Arc<ClickHouseDb>>,
    schema: Arc<flowscope_schema::Component>,
    metrics: Metrics,
    /// Port of the HTTP server, to guess the URL ClickHouse uses.
    http_port: Mutex<u16>,
    /// Set once the migrations are done.
    done: tokio::sync::watch::Sender<bool>,
}

impl ClickHouse {
    pub fn new(
        r: &Reporter,
        mut config: Configuration,
        db: Option<Arc<ClickHouseDb>>,
        schema: Arc<flowscope_schema::Component>,
    ) -> Result<Arc<ClickHouse>, String> {
        config.resolutions.sort_by_key(|r| r.interval);
        if config.resolutions.first().is_none_or(|r| !r.interval.is_zero()) {
            return Err("resolutions need to be configured, including interval: 0".into());
        }
        let m = r.metrics("orchestrator/clickhouse");
        Ok(Arc::new(ClickHouse {
            config: Mutex::new(config),
            db,
            schema,
            metrics: Metrics {
                migrations_running: m.gauge("running_migrations", "Database migrations in progress."),
                migrations_applied: m.counter("migrations_applied_steps_total", "Number of migration steps applied."),
                migrations_not_applied: m
                    .counter("migrations_notapplied_steps_total", "Number of migration steps not applied."),
            },
            http_port: Mutex::new(0),
            done: tokio::sync::watch::channel(false).0,
        }))
    }

    /// Run the migrations once, now (blocking). Used by tests.
    #[doc(hidden)]
    pub fn migrate_now(&self) -> Result<(), String> {
        self.migrate_database()
    }

    /// Steps applied and skipped so far.
    pub fn migration_counts(&self) -> (u64, u64) {
        (self.metrics.migrations_applied.get() as u64, self.metrics.migrations_not_applied.get() as u64)
    }

    /// Wait for the migrations to be done.
    pub async fn wait_migrations(&self) {
        let _ = self.done.subscribe().wait_for(|d| *d).await;
    }

    /// Run the migrations in the background, retrying until they succeed.
    pub fn start(self: &Arc<Self>, daemon: &Daemon, http_port: u16) {
        tracing::info!("starting ClickHouse component");
        *self.http_port.lock().unwrap() = http_port;
        if self.db.is_none() {
            return;
        }
        self.metrics.migrations_running.set(1.0);
        let this = self.clone();
        let token = daemon.token();
        daemon.spawn_background(async move {
            let mut delay = std::time::Duration::from_secs(1);
            loop {
                if !this.config.lock().unwrap().skip_migrations {
                    tracing::info!("attempting database migration");
                    let t = this.clone();
                    match tokio::task::spawn_blocking(move || t.migrate_database()).await {
                        Ok(Ok(())) => return,
                        Ok(Err(e)) => tracing::error!(error = %e, "database migration error"),
                        Err(e) => tracing::error!(error = %e, "database migration panicked"),
                    }
                }
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
                delay = (delay * 3 / 2).min(std::time::Duration::from_secs(60));
            }
        });
    }

    /// Routes serving the dictionary sources.
    pub fn router(self: &Arc<Self>) -> Router {
        let mut router = Router::new();
        // Custom dictionaries, read from their source file.
        for (name, dict) in self.schema.custom_dict_config() {
            let source = dict.source.clone();
            router = router.route(
                &format!("/api/v0/orchestrator/clickhouse/custom_dict_{name}.csv"),
                axum::routing::get(move || {
                    let source = source.clone();
                    async move {
                        match tokio::fs::read(&source).await {
                            Ok(data) => csv_response(data),
                            Err(e) => {
                                tracing::error!(error = %e, "unable to deliver custom dict csv file");
                                (StatusCode::NOT_FOUND, format!("unable to deliver custom dict csv file {source}"))
                                    .into_response()
                            }
                        }
                    }
                }),
            );
        }
        let asns = self.config.lock().unwrap().asns.clone();
        for (name, data) in DATA {
            let path = format!("/api/v0/orchestrator/clickhouse/{name}");
            if *name == "asns.csv" && !asns.is_empty() {
                let asns = asns.clone();
                router = router
                    .route(&path, axum::routing::get(move || async move { csv_response(asns_with_overrides(&asns)) }));
                continue;
            }
            router = router.route(&path, axum::routing::get(move || async move { csv_response(data.to_vec()) }));
        }
        router
    }
}

fn csv_response(data: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "text/csv; charset=utf-8")], data).into_response()
}

/// The ASN file with the configured names first, replacing builtin ones.
fn asns_with_overrides(asns: &BTreeMap<u32, String>) -> Vec<u8> {
    let mut out = csv::Writer::from_writer(vec![]);
    let _ = out.write_record(["asn", "name"]);
    for (asn, name) in asns {
        let _ = out.write_record([asn.to_string(), name.clone()]);
    }
    let builtin = DATA.iter().find(|(n, _)| *n == "asns.csv").unwrap().1;
    let mut rd = csv::ReaderBuilder::new().has_headers(true).flexible(false).from_reader(builtin);
    for record in rd.records() {
        let Ok(record) = record else { continue };
        let Some(Ok(asn)) = record.get(0).map(|a| a.parse::<u32>()) else { continue };
        if !asns.contains_key(&asn) {
            let _ = out.write_record(&record);
        }
    }
    out.into_inner().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asns_override() {
        let asns = BTreeMap::from([(1, "Custom".to_string()), (4200000001, "Private".to_string())]);
        let out = String::from_utf8(asns_with_overrides(&asns)).unwrap();
        let mut lines = out.lines();
        assert_eq!(lines.next(), Some("asn,name"));
        assert_eq!(lines.next(), Some("1,Custom"));
        assert_eq!(lines.next(), Some("4200000001,Private"));
        assert!(!out.lines().any(|l| l.starts_with("1,") && l != "1,Custom"));
        assert!(out.contains("2,University of Delaware"));
    }

    #[test]
    fn configuration() {
        use flowscope_common::config::{from_value, parse_str};
        let got: Configuration = from_value(
            parse_str(
                "resolutions:\n  - {interval: 0, ttl: 360h}\n  - {interval: 1m, ttl: 168h, table-settings: {storage_policy: hot, max_parts: 3}}\nsystem-log-ttl: 1h\nasns: {65000: Custom}\n",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(got.resolutions.len(), 2);
        assert_eq!(got.resolutions[1].table_settings["storage_policy"], TableSetting::String("hot".into()));
        assert_eq!(got.resolutions[1].table_settings["max_parts"], TableSetting::Int(3));
        assert_eq!(got.asns[&65000], "Custom");
        got.validate().unwrap();
    }
}
