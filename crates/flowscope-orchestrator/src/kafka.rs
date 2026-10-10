// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Kafka topic management: create the topics and keep their number of
//! partitions and their configuration in sync. Port of `orchestrator/kafka`.

use flowscope_common::helpers::TlsConfiguration;
use flowscope_common::migrated;
use flowscope_kafka::SaslConfiguration;
use rdkafka::admin::{
    AdminClient, AdminOptions, AlterConfig, ConfigSource, NewPartitions, NewTopic, ResourceSpecifier, TopicReplication,
};
use rdkafka::client::DefaultClientContext;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// Configuration of a managed topic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct TopicConfiguration {
    /// Number of partitions.
    pub num_partitions: i32,
    /// Replication factor.
    pub replication_factor: i32,
    /// Topic configuration overrides. Others are removed with a strict sync.
    pub config_entries: BTreeMap<String, Option<String>>,
    /// Remove the overrides not listed.
    pub config_entries_strict_sync: bool,
}

impl Default for TopicConfiguration {
    fn default() -> Self {
        TopicConfiguration {
            num_partitions: 1,
            replication_factor: 1,
            config_entries: BTreeMap::new(),
            config_entries_strict_sync: true,
        }
    }
}

/// The input topic (raw flows from the inlets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct InputConfiguration {
    pub topic: String,
    pub brokers: Vec<String>,
    pub tls: TlsConfiguration,
    pub sasl: SaslConfiguration,
    /// Create and update the topic.
    pub manage_topic: bool,
    pub topic_configuration: TopicConfiguration,
}

impl Default for InputConfiguration {
    fn default() -> Self {
        let common = flowscope_kafka::Configuration::default();
        InputConfiguration {
            topic: common.topic,
            brokers: common.brokers,
            tls: common.tls,
            sasl: common.sasl,
            manage_topic: true,
            topic_configuration: TopicConfiguration::default(),
        }
    }
}

fn migrate_input(map: &mut Map<String, Value>) -> Result<(), String> {
    flowscope_kafka::migrate_configuration(map)
}

migrated!(InputConfiguration, migrate_input);

/// The output topic (enriched flows from the outlets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct OutputConfiguration {
    pub topic: String,
    pub brokers: Vec<String>,
    pub tls: TlsConfiguration,
    pub sasl: SaslConfiguration,
    pub num_partitions: i32,
    pub replication_factor: i32,
    pub config_entries: BTreeMap<String, Option<String>>,
    pub config_entries_strict_sync: bool,
}

impl Default for OutputConfiguration {
    fn default() -> Self {
        let common = flowscope_kafka::Configuration::default();
        let t = TopicConfiguration::default();
        OutputConfiguration {
            topic: common.topic,
            brokers: common.brokers,
            tls: common.tls,
            sasl: common.sasl,
            num_partitions: t.num_partitions,
            replication_factor: t.replication_factor,
            config_entries: t.config_entries,
            config_entries_strict_sync: t.config_entries_strict_sync,
        }
    }
}

migrated!(OutputConfiguration, migrate_input);

impl InputConfiguration {
    pub fn common(&self) -> flowscope_kafka::Configuration {
        flowscope_kafka::Configuration {
            topic: self.topic.clone(),
            brokers: self.brokers.clone(),
            tls: self.tls.clone(),
            sasl: self.sasl.clone(),
        }
    }
}

impl OutputConfiguration {
    pub fn common(&self) -> flowscope_kafka::Configuration {
        flowscope_kafka::Configuration {
            topic: self.topic.clone(),
            brokers: self.brokers.clone(),
            tls: self.tls.clone(),
            sasl: self.sasl.clone(),
        }
    }

    fn topic_configuration(&self) -> TopicConfiguration {
        TopicConfiguration {
            num_partitions: self.num_partitions,
            replication_factor: self.replication_factor,
            config_entries: self.config_entries.clone(),
            config_entries_strict_sync: self.config_entries_strict_sync,
        }
    }
}

impl TopicConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if self.num_partitions < 1 || self.replication_factor < 1 {
            return Err("kafka topic partitions and replication factor should be at least 1".into());
        }
        Ok(())
    }
}

/// Whether the topic configuration should change: an entry differs or, with
/// a strict sync, an entry is not wanted. A null value means no value.
pub fn should_alter_configuration(
    target: &BTreeMap<String, Option<String>>,
    source: &BTreeMap<String, Option<String>>,
    strict: bool,
) -> bool {
    for (k, v) in target {
        if source.get(k) != Some(v) {
            return true;
        }
    }
    strict && target.len() != source.len()
}

/// A topic to manage.
struct Managed {
    config: flowscope_kafka::Configuration,
    topic: String,
    topic_config: TopicConfiguration,
}

/// The Kafka component.
pub struct Kafka {
    managed: Vec<Managed>,
}

impl Kafka {
    /// `None` when no topic is managed.
    pub fn new(
        input: &InputConfiguration,
        output: Option<&OutputConfiguration>,
        schema: &flowscope_schema::Component,
    ) -> Option<Kafka> {
        let mut managed = vec![];
        if input.manage_topic {
            managed.push(Managed {
                config: input.common(),
                topic: format!("{}-v{}", input.topic, flowscope_pb::VERSION),
                topic_config: input.topic_configuration.clone(),
            });
        }
        if let Some(output) = output {
            // Like the outlet: base name plus the schema hash.
            managed.push(Managed {
                config: output.common(),
                topic: format!("{}-{}", output.topic, schema.schema().protobuf_message_hash()),
                topic_config: output.topic_configuration(),
            });
        }
        if managed.is_empty() {
            tracing::info!("Kafka topic management disabled, skipping Kafka initialization");
            return None;
        }
        Some(Kafka { managed })
    }

    /// Create or update the topics.
    pub async fn start(&self) -> Result<(), String> {
        tracing::info!("starting Kafka component");
        for m in &self.managed {
            reconcile(m).await?;
        }
        tracing::info!("Kafka component stopped");
        Ok(())
    }
}

async fn reconcile(m: &Managed) -> Result<(), String> {
    let brokers = m.config.brokers.join(",");
    let topic = m.topic.as_str();
    let tc = &m.topic_config;
    let admin: AdminClient<DefaultClientContext> = m
        .config
        .client_config()
        .create()
        .map_err(|e| format!("unable to create Kafka client for topic creation: {e}"))?;
    let timeout = Duration::from_secs(30);
    let metadata =
        admin.inner().fetch_metadata(None, timeout).map_err(|e| format!("unable to get metadata for topics: {e}"))?;
    let opts = AdminOptions::new().operation_timeout(Some(timeout));
    let Some(existing) = metadata.topics().iter().find(|t| t.name() == topic) else {
        let entries: Vec<(String, String)> =
            tc.config_entries.iter().filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v))).collect();
        let mut new = NewTopic::new(topic, tc.num_partitions, TopicReplication::Fixed(tc.replication_factor));
        for (k, v) in &entries {
            new = new.set(k, v);
        }
        let result = admin.create_topics(&[new], &opts).await.map_err(|e| e.to_string())?;
        for r in result {
            r.map_err(|(t, e)| format!("unable to create topic {t:?}: {e}"))?;
        }
        tracing::info!(brokers, topic, "topic created");
        return Ok(());
    };

    let partitions = existing.partitions().len() as i32;
    if partitions > tc.num_partitions {
        tracing::warn!(
            brokers,
            topic,
            "cannot decrease the number of partitions (from {partitions} to {})",
            tc.num_partitions
        );
    } else if partitions < tc.num_partitions {
        let result = admin
            .create_partitions(&[NewPartitions::new(topic, tc.num_partitions as usize)], &opts)
            .await
            .map_err(|e| e.to_string())?;
        for r in result {
            r.map_err(|(t, e)| format!("unable to add more partitions to topic {t:?}: {e}"))?;
        }
        tracing::info!(brokers, topic, "number of partitions increased");
    }
    let replicas = existing.partitions().first().map(|p| p.replicas().len() as i32).unwrap_or(0);
    if replicas != tc.replication_factor {
        tracing::warn!(
            brokers,
            topic,
            "mismatch for replication factor: got {replicas}, want {}",
            tc.replication_factor
        );
    }

    let described = admin
        .describe_configs(&[ResourceSpecifier::Topic(topic)], &opts)
        .await
        .map_err(|e| format!("unable to get topic {topic:?} configuration: {e}"))?;
    let resource = described
        .into_iter()
        .next()
        .ok_or("no topic configuration")?
        .map_err(|e| format!("unable to get topic {topic:?} configuration: {e}"))?;
    let got: BTreeMap<String, Option<String>> = resource
        .entries
        .into_iter()
        .filter(|e| e.source == ConfigSource::DynamicTopic)
        .map(|e| (e.name, e.value))
        .collect();
    if should_alter_configuration(&tc.config_entries, &got, tc.config_entries_strict_sync) {
        // AlterConfigs replaces the whole set of overrides: keep the other
        // ones without a strict sync.
        let mut wanted: BTreeMap<String, Option<String>> =
            if tc.config_entries_strict_sync { BTreeMap::new() } else { got.clone() };
        wanted.extend(tc.config_entries.clone());
        let mut alter = AlterConfig::new(ResourceSpecifier::Topic(topic));
        for (k, v) in &wanted {
            if let Some(v) = v {
                alter = alter.set(k, v);
            }
        }
        let result = admin.alter_configs(&[alter], &opts).await.map_err(|e| e.to_string())?;
        for r in result {
            r.map_err(|(_, e)| format!("unable to set topic configuration for {topic:?}: {e}"))?;
        }
        tracing::info!(brokers, topic, "topic updated");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(entries: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
        entries.iter().map(|(k, v)| (k.to_string(), v.map(|s| s.to_string()))).collect()
    }

    #[test]
    fn should_alter() {
        let target = m(&[("retention.ms", Some("1000"))]);
        assert!(!should_alter_configuration(&target, &m(&[("retention.ms", Some("1000"))]), true));
        assert!(should_alter_configuration(&target, &m(&[("retention.ms", Some("2000"))]), true));
        assert!(should_alter_configuration(&target, &m(&[]), false));
        let source = m(&[("retention.ms", Some("1000")), ("segment.bytes", Some("1"))]);
        assert!(should_alter_configuration(&target, &source, true));
        assert!(!should_alter_configuration(&target, &source, false));
        // A null value does not crash (it did with Go).
        assert!(should_alter_configuration(&m(&[("x", None)]), &m(&[("x", Some("1"))]), true));
    }

    #[test]
    fn configuration() {
        use flowscope_common::config::{from_value, parse_str};
        let got: InputConfiguration = from_value(
            parse_str("topic: flows\nbrokers: [k:9092]\ntopic-configuration:\n  num-partitions: 8\n  config-entries: {retention.ms: \"86400000\"}\n")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got.topic_configuration.num_partitions, 8);
        assert_eq!(got.topic_configuration.config_entries["retention.ms"].as_deref(), Some("86400000"));
        assert!(got.manage_topic);
        let got: OutputConfiguration = from_value(parse_str("topic: enriched\nnum-partitions: 4\n").unwrap()).unwrap();
        assert_eq!(got.num_partitions, 4);
    }
}
