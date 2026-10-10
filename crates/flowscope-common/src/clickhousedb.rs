// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Connection to a ClickHouse database. Port of `common/clickhousedb`.
//!
//! Connections are synchronous. Async callers should go through
//! `tokio::task::spawn_blocking`.

use crate::config::Duration;
use crate::helpers::{FLOWSCOPE_VERSION, TlsConfiguration, validate_listen};
use crate::reporter::{HealthcheckResult, Reporter};
use flowscope_clickhouse::{Connection, Error, Options};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// How to connect to a ClickHouse database.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// ClickHouse servers to connect to (with ports).
    pub servers: Vec<String>,
    /// Cluster to operate on. It changes some modes of operation.
    pub cluster: String,
    /// Database to use.
    pub database: String,
    /// Username for authentication.
    pub username: String,
    /// Password for authentication.
    pub password: String,
    /// Number of parallel connections.
    pub max_open_conns: usize,
    /// How long to wait when connecting.
    pub dial_timeout: Duration,
    /// TLS parameters. When disabled, plain TCP is used.
    pub tls: TlsConfiguration,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            servers: vec!["127.0.0.1:9000".into()],
            cluster: String::new(),
            database: "default".into(),
            username: "default".into(),
            password: String::new(),
            max_open_conns: 10,
            dial_timeout: Duration::from_secs(5),
            tls: TlsConfiguration::default(),
        }
    }
}

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.servers.is_empty() {
            return Err("clickhousedb.servers should not be empty".into());
        }
        for s in &self.servers {
            validate_listen(s).map_err(|e| format!("clickhousedb.servers: {e}"))?;
        }
        if self.database.is_empty() {
            return Err("clickhousedb.database is required".into());
        }
        if self.username.is_empty() {
            return Err("clickhousedb.username is required".into());
        }
        if self.max_open_conns < 1 {
            return Err("clickhousedb.max-open-conns should be at least 1".into());
        }
        if self.dial_timeout.as_millis() < 100 {
            return Err("clickhousedb.dial-timeout should be at least 100ms".into());
        }
        self.tls.validate()
    }
}

/// The ClickHouse component: options for workers and a small connection
/// pool for other users.
pub struct ClickHouseDb {
    config: Configuration,
    options: Options,
    idle: Mutex<Vec<Connection>>,
    next: AtomicUsize,
}

impl ClickHouseDb {
    pub fn new(config: Configuration) -> Result<Arc<ClickHouseDb>, String> {
        let tls = config.tls.make_rustls_config()?;
        let options = Options {
            address: config.servers[0].clone(),
            database: config.database.clone(),
            username: config.username.clone(),
            password: config.password.clone(),
            compression: true,
            dial_timeout: *config.dial_timeout,
            tls,
            client_name: format!("flowscope/{FLOWSCOPE_VERSION}"),
            ..Default::default()
        };
        Ok(Arc::new(ClickHouseDb { config, options, idle: Mutex::new(vec![]), next: AtomicUsize::new(0) }))
    }

    /// Register the healthcheck.
    pub fn start(self: &Arc<Self>, r: &Reporter) {
        let this = self.clone();
        r.register_healthcheck("clickhousedb", move || {
            let this = this.clone();
            async move {
                let ok = tokio::task::spawn_blocking(move || this.with_conn(|c| c.query("SELECT 1", &[])).is_ok())
                    .await
                    .unwrap_or(false);
                if ok {
                    HealthcheckResult::ok("database available")
                } else {
                    HealthcheckResult::warning("database unavailable")
                }
            }
        });
    }

    pub fn cluster_name(&self) -> &str {
        &self.config.cluster
    }

    pub fn database_name(&self) -> &str {
        &self.config.database
    }

    /// Connection options (for the first server) and the list of servers.
    pub fn options(&self) -> (Options, Vec<String>) {
        (self.options.clone(), self.config.servers.clone())
    }

    /// Run a function with a pooled connection. Servers are used in
    /// round-robin order for new connections.
    pub fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T, Error>) -> Result<T, Error> {
        let pooled = self.idle.lock().unwrap().pop();
        let mut conn = match pooled {
            Some(c) => c,
            None => {
                let i = self.next.fetch_add(1, Ordering::Relaxed) % self.config.servers.len();
                let mut options = self.options.clone();
                options.address = self.config.servers[i].clone();
                Connection::connect(&options)?
            }
        };
        let result = f(&mut conn);
        if !conn.is_broken() {
            let mut idle = self.idle.lock().unwrap();
            if idle.len() < self.config.max_open_conns / 2 + 1 {
                idle.push(conn);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{from_value, parse_str};

    #[test]
    fn configuration() {
        let got: Configuration = from_value(
            parse_str("servers: [ch1:9000, ch2:9000]\nmax-open-conns: 4\ndial-timeout: 2s\ntls:\n  enable: true\n")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got.servers, vec!["ch1:9000", "ch2:9000"]);
        assert_eq!(got.max_open_conns, 4);
        assert_eq!(*got.dial_timeout, std::time::Duration::from_secs(2));
        assert!(got.tls.enable);
        assert_eq!(got.database, "default");
        got.validate().unwrap();
        let bad = Configuration { servers: vec![], ..Default::default() };
        assert!(bad.validate().is_err());
    }
}
