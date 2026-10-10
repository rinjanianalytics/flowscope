// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `orchestrator` command. Port of `cmd/akvorado/orchestrator.go`.

use crate::ServiceArgs;
use crate::console::ConsoleConfiguration;
use crate::inlet::InletConfiguration;
use crate::outlet::OutletConfiguration;
use crate::service::{Service, parse_config};
use flowscope_common::clickhousedb::{self, ClickHouseDb};
use flowscope_common::config::{Validate, migrate};
use flowscope_common::{httpserver, migrated, reporter};
use flowscope_orchestrator::Orchestrator;
use flowscope_orchestrator::clickhouse::{self, ClickHouse};
use flowscope_orchestrator::kafka::{self, Kafka};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Configuration of the orchestrator service, with the configurations of
/// the other services.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct OrchestratorConfiguration {
    /// Restart when a configuration file changes.
    pub automatic_restart: bool,
    pub reporting: reporter::Configuration,
    pub http: httpserver::Configuration,
    pub clickhouse: clickhouse::Configuration,
    #[serde(rename = "clickhousedb")]
    pub clickhouse_db: clickhousedb::Configuration,
    pub kafka: kafka::InputConfiguration,
    pub kafka_output: Option<kafka::OutputConfiguration>,
    pub schema: flowscope_schema::Configuration,
    pub inlet: Vec<InletConfiguration>,
    pub outlet: Vec<OutletConfiguration>,
    pub console: Vec<ConsoleConfiguration>,
    /// Demo exporter configurations, served as given (not ported yet).
    pub demo_exporter: Vec<Value>,
}

impl Default for OrchestratorConfiguration {
    fn default() -> Self {
        OrchestratorConfiguration {
            automatic_restart: true,
            reporting: Default::default(),
            http: Default::default(),
            clickhouse: Default::default(),
            clickhouse_db: Default::default(),
            kafka: Default::default(),
            kafka_output: None,
            schema: Default::default(),
            inlet: vec![InletConfiguration::default()],
            outlet: vec![OutletConfiguration::default()],
            console: vec![ConsoleConfiguration::default()],
            demo_exporter: vec![],
        }
    }
}

/// Entries of a section which may be a list or a single map.
fn entries<'a>(map: &'a mut Map<String, Value>, name: &str) -> Vec<&'a mut Map<String, Value>> {
    let Some(key) = migrate::find_key(map, name) else { return vec![] };
    let value = map.get_mut(&key).unwrap();
    if value.is_object() {
        *value = Value::Array(vec![value.take()]);
    }
    match value {
        Value::Array(items) => items.iter_mut().filter_map(|i| i.as_object_mut()).collect(),
        _ => vec![],
    }
}

/// Store a value under a name in each outlet configuration, creating one
/// when there is none.
fn migrate_to_outlet(map: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), String> {
    if migrate::find_key(map, "Outlet").is_none() {
        let mut outlet = Map::new();
        outlet.insert(name.into(), value);
        map.insert("outlet".into(), Value::Array(vec![Value::Object(outlet)]));
        return Ok(());
    }
    for outlet in entries(map, "Outlet") {
        if migrate::find_key(outlet, name).is_some() {
            return Err(format!("cannot have {name:?} both in \"outlet\" and in the orchestrator configuration"));
        }
        outlet.insert(name.into(), value.clone());
    }
    Ok(())
}

/// Older layouts of the configuration.
fn migrate_layout(map: &mut Map<String, Value>) -> Result<(), String> {
    // ClickHouse database settings moved from "clickhouse" to "clickhousedb".
    let has_db = migrate::find_key(map, "ClickHouseDB").is_some();
    if let Some(Value::Object(ch)) = migrate::get_mut(map, "ClickHouse") {
        let mut moved = Map::new();
        for field in ["Servers", "Cluster", "Database", "Username", "Password", "MaxOpenConns", "DialTimeout", "TLS"] {
            if let Some(k) = migrate::find_key(ch, field) {
                if has_db {
                    return Err(
                        "cannot have both \"ClickHouseDB\" and ClickHouse database settings in \"ClickHouse\"".into()
                    );
                }
                moved.insert(k.clone(), ch.remove(&k).unwrap());
            }
        }
        if !moved.is_empty() {
            map.insert("clickhousedb".into(), Value::Object(moved));
        }
    }

    // Outlet settings used to be in the inlet configuration.
    let has_outlet = migrate::find_key(map, "Outlet").is_some();
    let inlet_count = entries(map, "Inlet").len();
    let mut new_outlet: Option<Map<String, Value>> = None;
    for inlet in entries(map, "Inlet") {
        let mut moved = Map::new();
        for field in ["Metadata", "Routing", "Core", "Schema", "BMP", "SNMP"] {
            if let Some(k) = migrate::find_key(inlet, field) {
                if has_outlet {
                    return Err(format!(
                        "cannot have both \"inlet\" configuration with {field:?} field and \"outlet\" configuration"
                    ));
                }
                if inlet_count > 1 {
                    return Err(format!(
                        "cannot migrate {field:?} from \"inlet\" to \"outlet\" as there are several inlet configurations"
                    ));
                }
                moved.insert(k.clone(), inlet.remove(&k).unwrap());
            }
        }
        if !moved.is_empty() {
            new_outlet = Some(moved);
        }
    }
    if let Some(outlet) = new_outlet {
        map.insert("outlet".into(), Value::Array(vec![Value::Object(outlet)]));
    }

    // GeoIP moved from the inlet, then from the top level, to the outlet.
    let top_geoip = migrate::find_key(map, "GeoIP");
    let mut geoip: Option<Value> = None;
    for inlet in entries(map, "Inlet") {
        if let Some(k) = migrate::find_key(inlet, "GeoIP") {
            if top_geoip.is_some() {
                return Err("cannot have both \"geoip\" in inlet and orchestrator configuration".into());
            }
            let v = inlet.remove(&k).unwrap();
            geoip.get_or_insert(v);
        }
    }
    if geoip.is_none()
        && let Some(k) = top_geoip
    {
        geoip = map.remove(&k);
    }
    if let Some(v) = geoip {
        migrate_to_outlet(map, "geoip", v)?;
    }

    // Network attributes moved from "clickhouse" to the outlet.
    let mut networks = Map::new();
    if let Some(Value::Object(ch)) = migrate::get_mut(map, "ClickHouse") {
        for field in ["Networks", "NetworkSources", "NetworkSourcesTimeout"] {
            if let Some(k) = migrate::find_key(ch, field) {
                networks.insert(field.into(), ch.remove(&k).unwrap());
            }
        }
    }
    if !networks.is_empty() {
        migrate_to_outlet(map, "networks", Value::Object(networks))?;
    }
    // Missing services get their default configuration, which receives the
    // shared settings too.
    for service in ["inlet", "outlet", "console"] {
        if migrate::find_key(map, service).is_none() {
            map.insert(service.into(), Value::Array(vec![Value::Object(Map::new())]));
        }
    }
    share_settings(map);
    Ok(())
}

/// Copy some keys of a section (when present) into another one.
fn copy_keys(from: Option<&Value>, to: &mut Map<String, Value>, keys: &[&str]) {
    for k in keys {
        if let Some(existing) = migrate::find_key(to, k) {
            to.remove(&existing);
        }
        if let Some(Value::Object(from)) = from
            && let Some(fk) = migrate::find_key(from, k)
        {
            to.insert(fk.clone(), from[&fk].clone());
        }
    }
}

/// Whether a section has a key with something in it.
fn has(map: &Map<String, Value>, section: &[&str], key: &str) -> bool {
    let mut current = map;
    for s in section {
        match migrate::get(current, s) {
            Some(Value::Object(m)) => current = m,
            _ => return false,
        }
    }
    matches!(migrate::get(current, key), Some(Value::Array(a)) if !a.is_empty())
}

/// Give the other services the shared settings of the orchestrator: Kafka,
/// ClickHouse and the schema, unless they have their own.
fn share_settings(map: &mut Map<String, Value>) {
    const KAFKA_KEYS: &[&str] = &["Topic", "Brokers", "TLS", "SASL"];
    let kafka = migrate::get(map, "Kafka").cloned();
    let kafka_output = migrate::get(map, "KafkaOutput").cloned();
    let clickhouse_db = migrate::get(map, "ClickHouseDB").cloned();
    let schema = migrate::get(map, "Schema").cloned();
    let section = |entry: &mut Map<String, Value>, names: &[&str]| -> String {
        names.iter().find_map(|n| migrate::find_key(entry, n)).unwrap_or_else(|| names[0].to_lowercase())
    };

    for inlet in entries(map, "Inlet") {
        if !has(inlet, &["Kafka"], "Brokers") {
            let k = section(inlet, &["Kafka"]);
            let target = inlet.entry(k).or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(target) = target {
                copy_keys(kafka.as_ref(), target, KAFKA_KEYS);
            }
        }
    }
    for outlet in entries(map, "Outlet") {
        // The Go version tested a key that does not exist for the outlet,
        // so its own ClickHouse settings were always replaced.
        if !has(outlet, &["ClickHouseDB"], "Servers") && clickhouse_db.is_some() {
            let k = section(outlet, &["ClickHouseDB"]);
            outlet.insert(k, clickhouse_db.clone().unwrap());
        }
        let input = section(outlet, &["KafkaInput", "Kafka"]);
        if !has(outlet, &[input.as_str()], "Brokers") {
            let target = outlet.entry(input).or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(target) = target {
                copy_keys(kafka.as_ref(), target, KAFKA_KEYS);
            }
        }
        if !has(outlet, &["KafkaOutput"], "Brokers") {
            let k = section(outlet, &["KafkaOutput"]);
            let target = outlet.entry(k).or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(target) = target {
                if kafka_output.is_some() {
                    // The topic the orchestrator manages, as is.
                    copy_keys(kafka_output.as_ref(), target, KAFKA_KEYS);
                } else {
                    // The input cluster, but not the input topic.
                    copy_keys(kafka.as_ref(), target, &["Brokers", "TLS", "SASL"]);
                }
            }
        }
        let k = section(outlet, &["Schema"]);
        match &schema {
            Some(s) => {
                outlet.insert(k, s.clone());
            }
            None => {
                outlet.remove(&k);
            }
        }
    }
    for console in entries(map, "Console") {
        if !has(console, &["ClickHouse"], "Servers") && clickhouse_db.is_some() {
            let k = section(console, &["ClickHouse"]);
            console.insert(k, clickhouse_db.clone().unwrap());
        }
        let k = section(console, &["Schema"]);
        match &schema {
            Some(s) => {
                console.insert(k, s.clone());
            }
            None => {
                console.remove(&k);
            }
        }
    }
}

migrated!(OrchestratorConfiguration, migrate_layout);

impl Validate for OrchestratorConfiguration {
    /// The shared settings go to the services which do not set them: this
    /// has to be decided before the defaults fill everything.
    fn prepare(tree: &mut Value) -> Result<(), String> {
        if tree.is_null() {
            *tree = Value::Object(Map::new());
        }
        match tree {
            Value::Object(map) => migrate_layout(map),
            _ => Ok(()),
        }
    }

    fn validate(&self) -> Result<(), String> {
        self.http.validate()?;
        self.clickhouse.validate()?;
        self.clickhouse_db.validate()?;
        self.kafka.common().validate()?;
        self.kafka.topic_configuration.validate()?;
        for i in &self.inlet {
            i.validate()?;
        }
        for o in &self.outlet {
            o.validate()?;
        }
        for c in &self.console {
            c.validate()?;
        }
        Ok(())
    }
}

/// Modification times of the configuration files.
fn mtimes(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    paths.iter().map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).collect()
}

pub async fn run(args: ServiceArgs) -> anyhow::Result<()> {
    loop {
        let (config, paths) = parse_config::<OrchestratorConfiguration>(&args, "orchestrator").await?;
        if args.check {
            check(&config)?;
            return Ok(());
        }
        if !start(&args, config, paths).await? {
            return Ok(());
        }
        tracing::info!("restart on configuration change");
    }
}

fn check(config: &OrchestratorConfiguration) -> anyhow::Result<()> {
    let schema = flowscope_schema::Component::new(config.schema.clone()).map_err(anyhow::Error::msg)?;
    ClickHouse::new(&reporter::Reporter::new_mock(), config.clickhouse.clone(), None, Arc::new(schema))
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

/// Run the orchestrator. Returns true to restart.
async fn start(args: &ServiceArgs, config: OrchestratorConfiguration, paths: Vec<PathBuf>) -> anyhow::Result<bool> {
    let err = anyhow::Error::msg;
    let service = Service::new("orchestrator", &config.reporting, config.http.clone()).await?;
    let r = &service.reporter;
    let schema = Arc::new(flowscope_schema::Component::new(config.schema.clone()).map_err(err)?);
    let db = ClickHouseDb::new(config.clickhouse_db.clone()).map_err(err)?;
    let clickhouse = ClickHouse::new(r, config.clickhouse.clone(), Some(db.clone()), schema.clone()).map_err(err)?;
    let kafka = Kafka::new(&config.kafka, config.kafka_output.as_ref(), &schema);
    let orchestrator = Orchestrator::new();
    for c in &config.inlet {
        orchestrator.register_configuration("inlet", serde_json::to_value(c)?);
    }
    for c in &config.outlet {
        orchestrator.register_configuration("outlet", serde_json::to_value(c)?);
    }
    for c in &config.console {
        orchestrator.register_configuration("console", serde_json::to_value(c)?);
    }
    for c in &config.demo_exporter {
        orchestrator.register_configuration("demo-exporter", c.clone());
    }
    service.http.add_api(orchestrator.router());
    service.http.add_handler("/api/v0/orchestrator/clickhouse", clickhouse.router());

    service.start().await?;
    db.start(r);
    clickhouse.start(&service.daemon, service.http.local_addr().map(|a| a.port()).unwrap_or(0));
    if let Some(kafka) = kafka {
        kafka.start().await.map_err(err)?;
    }

    // Restart when a configuration file changes and is still valid.
    let restart = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if config.automatic_restart && !paths.is_empty() {
        tracing::info!(?paths, "watching loaded configuration files for changes");
        let daemon = service.daemon.clone();
        let token = daemon.token();
        let restart = restart.clone();
        let path = args.config.clone().unwrap_or_default();
        service.daemon.spawn_background(async move {
            let mut last = mtimes(&paths);
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
                let now = mtimes(&paths);
                if now == last {
                    continue;
                }
                last = now;
                let options = flowscope_common::config::Options { path: path.clone(), dump: false };
                match flowscope_common::config::parse::<OrchestratorConfiguration>(&options, "orchestrator").await {
                    Ok(_) => {
                        restart.store(true, std::sync::atomic::Ordering::Relaxed);
                        daemon.terminate();
                        return;
                    }
                    Err(e) => tracing::error!(error = %e, "cannot validate new configuration, not reloading"),
                }
            }
        });
    }
    service.wait().await?;
    Ok(restart.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};
    use pretty_assertions::assert_eq;

    fn parse(yaml: &str) -> Result<OrchestratorConfiguration, String> {
        from_value(parse_str(yaml).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    #[test]
    fn defaults() {
        let c = OrchestratorConfiguration::default();
        c.validate().unwrap();
        assert_eq!((c.inlet.len(), c.outlet.len(), c.console.len()), (1, 1, 1));
        assert!(c.automatic_restart);
    }

    #[test]
    fn shared_settings() {
        let c = parse(
            "kafka:\n  brokers: [k1:9092]\n  topic: flows2\nclickhousedb:\n  servers: [ch:9000]\n\
             schema:\n  enabled: [SrcVlan]\ninlet: [{}]\noutlet:\n  - {}\n  - kafka-input: {brokers: [other:9092]}\n\
             console: [{}]\n",
        )
        .unwrap();
        assert_eq!(c.inlet[0].kafka.brokers, vec!["k1:9092"]);
        assert_eq!(c.inlet[0].kafka.topic, "flows2");
        assert_eq!(c.outlet[0].kafka_input.brokers, vec!["k1:9092"]);
        assert_eq!(c.outlet[0].clickhouse_db.servers, vec!["ch:9000"]);
        assert_eq!(c.outlet[0].schema, c.schema);
        // Its own Kafka cluster is kept.
        assert_eq!(c.outlet[1].kafka_input.brokers, vec!["other:9092"]);
        // The output uses the input cluster, with its own topic.
        assert_eq!(c.outlet[0].kafka_output.brokers, vec!["k1:9092"]);
        assert_eq!(c.outlet[0].kafka_output.topic, "flows-enriched");
        assert_eq!(c.console[0].clickhouse.servers, vec!["ch:9000"]);
    }

    #[test]
    fn shared_settings_default_services() {
        let c = parse("kafka:\n  brokers: [k1:9092]\nclickhousedb:\n  servers: [ch:9000]\n").unwrap();
        assert_eq!(c.inlet[0].kafka.brokers, vec!["k1:9092"]);
        assert_eq!(c.outlet[0].kafka_input.brokers, vec!["k1:9092"]);
        assert_eq!(c.outlet[0].clickhouse_db.servers, vec!["ch:9000"]);
        assert_eq!(c.console[0].clickhouse.servers, vec!["ch:9000"]);
    }

    /// Through the loader, which merges the defaults: settings only given
    /// at the top must still reach the services.
    #[test]
    fn shared_settings_through_loader() {
        let tree = parse_str("kafka:\n  brokers: [k1:9092]\nclickhousedb:\n  servers: [ch:9000]\n").unwrap();
        let c: OrchestratorConfiguration =
            flowscope_common::config::decode(tree, "orchestrator", std::iter::empty()).unwrap();
        assert_eq!(c.inlet[0].kafka.brokers, vec!["k1:9092"]);
        assert_eq!(c.outlet[0].kafka_input.brokers, vec!["k1:9092"]);
        assert_eq!(c.outlet[0].clickhouse_db.servers, vec!["ch:9000"]);
    }

    #[test]
    fn migrations() {
        // ClickHouse database settings in "clickhouse", outlet settings in
        // the inlet, GeoIP and networks at the top.
        let c = parse(
            "clickhouse:\n  servers: [ch:9000]\n  networks: {192.0.2.0/24: {name: lab}}\n\
             inlet:\n  - core: {exporter-classifiers: ['ClassifyGroup(\"x\")']}\n    geoip: {optional: true}\n",
        )
        .unwrap();
        assert_eq!(c.clickhouse_db.servers, vec!["ch:9000"]);
        assert_eq!(c.outlet.len(), 1);
        assert_eq!(c.outlet[0].core.exporter_classifiers.len(), 1);
        assert!(c.outlet[0].geoip.optional);
        assert_eq!(c.outlet[0].networks.networks.len(), 1);
        assert!(parse("clickhouse: {servers: [a:9000]}\nclickhousedb: {servers: [b:9000]}\n").is_err());
        assert!(parse("inlet: [{core: {}}, {}]\n").is_err());
    }
}
