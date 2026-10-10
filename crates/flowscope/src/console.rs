// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `console` command. Port of `cmd/akvorado/console.go`.

use crate::ServiceArgs;
use crate::service::{Service, parse_config};
use flowscope_common::clickhousedb::{self, ClickHouseDb};
use flowscope_common::config::{Duration, Validate};
use flowscope_common::{httpserver, reporter};
use flowscope_console::auth::{self, Authentication};
use flowscope_console::component::{ClickHouseDbQuery, Console, Dependencies};
use flowscope_console::config::{self, VisualizeOptions};
use flowscope_console::database::{self, Database};
use flowscope_console::query;
use flowscope_console::widgets::TopWidget;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration of the console service. The settings of the console
/// component are at the top level, like in Go.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct ConsoleConfiguration {
    pub reporting: reporter::Configuration,
    pub http: httpserver::Configuration,
    pub url_prefix: String,
    pub default_visualize_options: VisualizeOptions,
    pub homepage_top_widgets: Vec<TopWidget>,
    pub homepage_top_widgets_filter: query::Filter,
    pub homepage_graph_filter: query::Filter,
    pub homepage_graph_time_range: Duration,
    pub dimensions_limit: i64,
    pub branding: bool,
    #[serde(rename = "cache-ttl")]
    pub cache_ttl: Duration,
    pub clickhouse: clickhousedb::Configuration,
    pub auth: auth::Configuration,
    pub database: database::Configuration,
    pub schema: flowscope_schema::Configuration,
}

impl Default for ConsoleConfiguration {
    fn default() -> Self {
        let c = config::Configuration::default();
        ConsoleConfiguration {
            reporting: Default::default(),
            http: Default::default(),
            url_prefix: c.url_prefix,
            default_visualize_options: c.default_visualize_options,
            homepage_top_widgets: c.homepage_top_widgets,
            homepage_top_widgets_filter: c.homepage_top_widgets_filter,
            homepage_graph_filter: c.homepage_graph_filter,
            homepage_graph_time_range: c.homepage_graph_time_range,
            dimensions_limit: c.dimensions_limit,
            branding: c.branding,
            cache_ttl: c.cache_ttl,
            clickhouse: Default::default(),
            auth: Default::default(),
            database: Default::default(),
            schema: Default::default(),
        }
    }
}

impl ConsoleConfiguration {
    /// The configuration of the console component.
    fn console(&self) -> config::Configuration {
        config::Configuration {
            url_prefix: self.url_prefix.clone(),
            default_visualize_options: self.default_visualize_options.clone(),
            homepage_top_widgets: self.homepage_top_widgets.clone(),
            homepage_top_widgets_filter: self.homepage_top_widgets_filter.clone(),
            homepage_graph_filter: self.homepage_graph_filter.clone(),
            homepage_graph_time_range: self.homepage_graph_time_range,
            dimensions_limit: self.dimensions_limit,
            branding: self.branding,
            cache_ttl: self.cache_ttl,
        }
    }
}

impl Validate for ConsoleConfiguration {
    fn validate(&self) -> Result<(), String> {
        self.http.validate()?;
        self.console().validate()?;
        self.clickhouse.validate()?;
        self.auth.validate()?;
        self.database.validate()
    }
}

pub async fn run(args: ServiceArgs) -> anyhow::Result<()> {
    let (config, _) = parse_config::<ConsoleConfiguration>(&args, "console").await?;
    let err = anyhow::Error::msg;
    let service = Service::new("console", &config.reporting, config.http.clone()).await?;
    let r = &service.reporter;
    let clickhouse_db = ClickHouseDb::new(config.clickhouse.clone()).map_err(err)?;
    let schema = flowscope_schema::Component::new(config.schema.clone()).map_err(err)?;
    let auth = Authentication::new(config.auth.clone());
    // The database is opened when starting.
    if args.check {
        Console::new(r, config.console(), dependencies(&clickhouse_db, &schema, auth, None).await?).map_err(err)?;
        return Ok(());
    }
    let database =
        Database::new(config.database.clone()).await.map_err(|e| err(format!("unable to start database: {e}")))?;
    let console = Console::new(r, config.console(), dependencies(&clickhouse_db, &schema, auth, Some(database)).await?)
        .map_err(err)?;
    service.http.add_api(console.router(&service.http));
    let (assets, fallback) = flowscope_console::assets::routers(&console);
    service.http.add_handler("/assets/docs/", assets);
    service.http.add_handler("/", fallback);

    service.start().await?;
    clickhouse_db.start(r);
    console.spawn_refresh(service.daemon.token());
    tracing::info!(url_prefix = %config.console().url_prefix(), "console started");
    service.wait().await
}

async fn dependencies(
    clickhouse_db: &Arc<ClickHouseDb>,
    schema: &flowscope_schema::Component,
    auth: Authentication,
    database: Option<Database>,
) -> anyhow::Result<Dependencies> {
    let database = match database {
        Some(d) => d,
        // For the check mode, an in-memory database.
        None => Database::new(database::Configuration::default()).await.map_err(anyhow::Error::msg)?,
    };
    Ok(Dependencies {
        clickhouse: Arc::new(ClickHouseDbQuery(clickhouse_db.clone())),
        database_name: clickhouse_db.database_name().to_string(),
        schema: schema.schema().clone(),
        auth,
        database,
        clock: Arc::new(|| chrono::Utc::now().fixed_offset()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};

    #[test]
    fn default_is_valid() {
        ConsoleConfiguration::default().validate().unwrap();
    }

    #[test]
    fn parse() {
        let got: ConsoleConfiguration = from_value(
            parse_str(
                "homepage-top-widgets: [src-as, dst-port]
default-visualize-options:
  graph-type: lines
  dimensions: [ExporterName]
database:
  driver: mysql
  dsn: flowscope:flowscope@tcp(mysql:3306)/flowscope
auth:
  headers:
    login: X-User
clickhouse:
  servers: [clickhouse:9000]
cache-ttl: 30m
",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(got.homepage_top_widgets, vec![TopWidget::SrcAS, TopWidget::DstPort]);
        assert_eq!(got.default_visualize_options.graph_type, "lines");
        assert_eq!(got.database.driver, "mysql");
        assert_eq!(got.auth.headers.login, "X-User");
        assert_eq!(got.auth.headers.name, "Remote-Name");
        assert_eq!(*got.cache_ttl, std::time::Duration::from_secs(1800));
        got.validate().unwrap();
    }
}
