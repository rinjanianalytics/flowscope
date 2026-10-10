// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Common service startup: configuration, reporter, HTTP server, daemon.

use crate::ServiceArgs;
use flowscope_common::config::{self, Options, Validate};
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::{FLOWSCOPE_VERSION, compiler_version};
use flowscope_common::httpserver::{self, HttpServer, common_routes};
use flowscope_common::reporter::Reporter;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::time::Instant;

/// Parse the configuration of a service.
pub async fn parse_config<T>(args: &ServiceArgs, component: &str) -> anyhow::Result<(T, Vec<PathBuf>)>
where
    T: Default + Serialize + DeserializeOwned + Validate,
{
    let options = Options { path: args.config.clone().unwrap_or_default(), dump: args.dump };
    Ok(config::parse(&options, component).await?)
}

/// Shared pieces of a running service.
pub struct Service {
    pub reporter: Reporter,
    pub daemon: Daemon,
    pub http: HttpServer,
}

impl Service {
    /// Create the reporter, the daemon and the HTTP server with the common
    /// routes and metrics.
    pub async fn new(
        name: &str,
        reporting: &flowscope_common::reporter::Configuration,
        http: httpserver::Configuration,
    ) -> anyhow::Result<Service> {
        let reporter = Reporter::new(reporting);
        let daemon = Daemon::new();
        let server = HttpServer::new(&reporter, name, http).await.map_err(anyhow::Error::msg)?;
        let (metrics, api) = common_routes(&reporter, name, FLOWSCOPE_VERSION);
        server.add_handler(&format!("/api/v0/{name}/metrics"), metrics);
        server.add_api(api);
        more_metrics(&reporter);
        Ok(Service { reporter, daemon, http: server })
    }

    /// Start the HTTP server and watch signals.
    pub async fn start(&self) -> anyhow::Result<()> {
        self.http.start(&self.daemon).await.map_err(anyhow::Error::msg)?;
        self.daemon.watch_signals();
        Ok(())
    }

    /// Wait for the end and report the first error.
    pub async fn wait(self) -> anyhow::Result<()> {
        self.daemon.terminated().await;
        self.daemon.join().await;
        match self.daemon.error() {
            Some(e) => anyhow::bail!(e),
            None => Ok(()),
        }
    }
}

fn more_metrics(r: &Reporter) {
    let m = r.metrics("");
    let start = Instant::now();
    m.gauge_func("dropped_log_messages", "Number of log messages dropped.", || 0.0);
    m.gauge_vec("info", "FlowScope build information.", &["version", "compiler"])
        .with_label_values(&[FLOWSCOPE_VERSION, compiler_version()])
        .set(1.0);
    m.gauge_func("uptime_seconds", "Number of seconds the application is running.", move || {
        start.elapsed().as_secs_f64()
    });
}
