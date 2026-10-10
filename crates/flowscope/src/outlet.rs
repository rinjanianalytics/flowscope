// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! `outlet` command. Port of `cmd/akvorado/outlet.go`.

use crate::ServiceArgs;
use crate::service::{Service, parse_config};
use flowscope_common::clickhousedb::{self, ClickHouseDb};
use flowscope_common::config::{Validate, migrate, normalize_key};
use flowscope_common::{httpserver, migrated, reporter};
use flowscope_outlet::clickhouse::{self, ClickHouse};
use flowscope_outlet::core::{self, Core, Dependencies};
use flowscope_outlet::geoip::{self, GeoIp};
use flowscope_outlet::kafkainput::{self, KafkaInput};
use flowscope_outlet::kafkaoutput::{self, KafkaOutput};
use flowscope_outlet::metadata::{self, Metadata};
use flowscope_outlet::networks::{self, Networks};
use flowscope_outlet::routing::{self, Routing};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Configuration of the outlet service.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct OutletConfiguration {
    pub reporting: reporter::Configuration,
    pub http: httpserver::Configuration,
    pub metadata: metadata::Configuration,
    pub routing: routing::Configuration,
    pub kafka_input: kafkainput::Configuration,
    pub kafka_output: kafkaoutput::Configuration,
    pub networks: networks::Configuration,
    #[serde(rename = "geoip")]
    pub geoip: geoip::Configuration,
    #[serde(rename = "clickhousedb")]
    pub clickhouse_db: clickhousedb::Configuration,
    pub clickhouse: clickhouse::Configuration,
    pub flow: flowscope_decoder::Configuration,
    pub core: core::Configuration,
    pub schema: flowscope_schema::Configuration,
}

/// Field names of the metadata configuration, to dispatch the old `snmp`
/// section between the metadata component and its provider.
const METADATA_FIELDS: &[&str] = &[
    "CacheDuration",
    "CacheRefresh",
    "CacheCheckInterval",
    "CachePersistFile",
    "Providers",
    "Provider",
    "QueryTimeout",
    "InitialDelay",
    "Workers",
    "MaxBatchRequests",
];

/// Field names of the routing configuration.
const ROUTING_FIELDS: &[&str] = &["Provider"];

/// Move an old top-level section (`snmp`, `bmp`) into a component with a
/// provider: keys known by the component stay at its level, the other ones
/// go to the provider.
fn move_to_provider(
    map: &mut Map<String, Value>,
    old: &str,
    new: &str,
    provider: &str,
    component_fields: &[&str],
    skip: &[&str],
) -> Result<(), String> {
    let Some(old_key) = migrate::find_key(map, old) else { return Ok(()) };
    if let Some(new_key) = migrate::find_key(map, new) {
        return Err(format!("cannot have both {old_key:?} and {new_key:?}"));
    }
    let Some(Value::Object(section)) = map.remove(&old_key) else {
        return Err(format!("{old_key:?} should be a map"));
    };
    let is = |k: &str, names: &[&str]| names.iter().any(|n| normalize_key(n) == normalize_key(k));
    let mut component = Map::new();
    let mut provider_value = Map::new();
    for (k, v) in section {
        if is(&k, skip) {
            continue;
        }
        if is(&k, component_fields) {
            component.insert(k, v);
        } else {
            provider_value.insert(k, v);
        }
    }
    provider_value.insert("type".into(), Value::String(provider.into()));
    component.insert("provider".into(), Value::Object(provider_value));
    map.insert(new.into(), Value::Object(component));
    Ok(())
}

/// `snmp` became `metadata`, `bmp` became `routing` and `kafka` became
/// `kafka-input`.
fn migrate_outlet(map: &mut Map<String, Value>) -> Result<(), String> {
    move_to_provider(map, "Snmp", "metadata", "snmp", METADATA_FIELDS, &["PollerCoalesce", "Workers"])?;
    move_to_provider(map, "Bmp", "routing", "bmp", ROUTING_FIELDS, &[])?;
    migrate::rename(map, "Kafka", "kafka-input")?;
    Ok(())
}

migrated!(OutletConfiguration, migrate_outlet);

impl Validate for OutletConfiguration {
    fn validate(&self) -> Result<(), String> {
        self.http.validate()?;
        self.metadata.validate()?;
        self.routing.validate()?;
        self.kafka_input.validate()?;
        self.kafka_output.validate()?;
        self.networks.validate()?;
        self.clickhouse_db.validate()?;
        self.clickhouse.validate()?;
        self.core.validate()
    }
}

pub async fn run(args: ServiceArgs) -> anyhow::Result<()> {
    let (config, _) = parse_config::<OutletConfiguration>(&args, "outlet").await?;
    let service = Service::new("outlet", &config.reporting, config.http.clone()).await?;
    let r = &service.reporter;
    let daemon = &service.daemon;
    let err = anyhow::Error::msg;

    // Initialize the components.
    let schema = Arc::new(flowscope_schema::Component::new(config.schema.clone()).map_err(err)?);
    let flow = Arc::new(flowscope_decoder::Flow::new(r));
    let metadata = Metadata::new(r, config.metadata.clone(), daemon).map_err(err)?;
    let routing = Arc::new(Routing::new(r, &config.routing, daemon).map_err(err)?);
    let token = daemon.token();
    let kafka_input =
        Arc::new(KafkaInput::new(r, config.kafka_input.clone(), Arc::new(move || token.is_cancelled())).map_err(err)?);
    let geoip = GeoIp::new(r, config.geoip.clone());
    let networks = Networks::new(r, config.networks.clone(), Some(geoip.clone()));
    let clickhouse_db = ClickHouseDb::new(config.clickhouse_db.clone()).map_err(err)?;
    let clickhouse = Arc::new(ClickHouse::new(r, config.clickhouse.clone(), clickhouse_db.clone()));
    let kafka_output = KafkaOutput::new(r, config.kafka_output.clone(), &schema).map_err(err)?;
    let core = Core::new(
        r,
        config.core.clone(),
        Dependencies {
            flow: flow.clone(),
            metadata: metadata.clone(),
            routing: routing.clone(),
            networks: Some(networks.clone()),
            kafka_input: kafka_input.clone(),
            kafka_output: Some(kafka_output.clone()),
            clickhouse,
            schema,
            runtime: tokio::runtime::Handle::current(),
        },
    )
    .map_err(err)?;
    service.http.add_api(core.router());
    service.http.add_api(kafka_output.router());

    if args.check {
        return Ok(());
    }

    // Start the components.
    service.start().await?;
    clickhouse_db.start(r);
    if !config.flow.state_persist_file.is_empty()
        && let Err(e) = flow.restore_state(&config.flow.state_persist_file)
    {
        tracing::warn!(error = %e, "cannot restore decoders state, ignoring");
    }
    metadata.start(r, daemon);
    routing.start().map_err(err)?;
    {
        let k = kafka_input.clone();
        tokio::task::spawn_blocking(move || k.start()).await?.map_err(err)?;
    }
    kafka_output.start().map_err(err)?;
    geoip.start(daemon).map_err(err)?;
    networks.start(r, daemon).await;
    core.start().map_err(err)?;

    // Wait for the end, then stop in reverse order.
    daemon.terminated().await;
    tracing::info!("stopping outlet");
    {
        let (k, ko) = (kafka_input.clone(), kafka_output.clone());
        tokio::task::spawn_blocking(move || {
            core.stop();
            ko.stop();
            k.stop();
        })
        .await?;
    }
    if !config.flow.state_persist_file.is_empty()
        && let Err(e) = flow.save_state(&config.flow.state_persist_file)
    {
        tracing::warn!(error = %e, "cannot save decoders state");
    }
    service.wait().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};
    use pretty_assertions::assert_eq;

    #[test]
    fn default_is_valid() {
        OutletConfiguration::default().validate().unwrap();
        let c = OutletConfiguration::default();
        assert!(matches!(c.metadata.providers[..], [metadata::ProviderConfiguration::Snmp(_)]));
        assert!(matches!(c.routing.provider, routing::ProviderConfiguration::Bmp(_)));
    }

    #[test]
    fn snmp_and_bmp_migrations() {
        let got: OutletConfiguration = from_value(
            parse_str(
                "snmp:
  cache-duration: 2h
  poller-coalesce: 10
  workers: 4
  communities:
    ::/0: private
bmp:
  listen: 127.0.0.1:10179
kafka:
  topic: other
  brokers: [kafka:9092]
",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(*got.metadata.cache_duration, std::time::Duration::from_secs(7200));
        let metadata::ProviderConfiguration::Snmp(snmp) = &got.metadata.providers[0] else {
            panic!("SNMP provider expected, got {:?}", got.metadata.providers)
        };
        assert!(format!("{:?}", snmp.credentials).contains("private"), "{snmp:?}");
        let routing::ProviderConfiguration::Bmp(bmp) = &got.routing.provider else { panic!("BMP expected") };
        assert_eq!(bmp.listen, "127.0.0.1:10179");
        assert_eq!(got.kafka_input.topic, "other");
        assert_eq!(got.kafka_input.brokers, vec!["kafka:9092"]);
    }

    #[test]
    fn both_old_and_new() {
        assert!(from_value::<OutletConfiguration>(parse_str("snmp: {}\nmetadata: {}\n").unwrap()).is_err());
        assert!(from_value::<OutletConfiguration>(parse_str("bmp: {}\nrouting: {}\n").unwrap()).is_err());
    }
}
