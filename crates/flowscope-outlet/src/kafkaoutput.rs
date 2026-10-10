// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Export of enriched flows to Kafka, in parallel with ClickHouse. Port of
//! `outlet/kafkaoutput`. Disabled by default.
//!
//! Delivery is best-effort and at-most-once: when the producer queue is full
//! or a produce fails, records are dropped and counted, never retried.
//! [`KafkaOutput::send`] never blocks, so a slow Kafka output does not slow
//! down the ClickHouse path.
//!
//! The topic is the configured name suffixed with the hash of the Protobuf
//! message, so an incompatible schema change lands on a new topic.

use axum::Router;
use flowscope_common::config::{Duration, migrate};
use flowscope_common::helpers::TlsConfiguration;
use flowscope_common::migrated;
use flowscope_common::reporter::{Counter, CounterVec, Reporter};
use flowscope_kafka::{CompressionCodec, LoadBalanceAlgorithm, SaslConfiguration};
use flowscope_schema::Component as Schema;
use rdkafka::ClientContext;
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::DeliveryResult;
use rdkafka::producer::{BaseRecord, Producer, ProducerContext, ThreadedProducer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Configuration of the Kafka output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// Turn the Kafka output on.
    pub enabled: bool,
    pub topic: String,
    pub brokers: Vec<String>,
    pub tls: TlsConfiguration,
    pub sasl: SaslConfiguration,
    /// Compression to use.
    pub compression_codec: CompressionCodec,
    /// Maximum number of records in flight. When full, records are dropped.
    pub queue_size: usize,
    /// How records are spread over partitions.
    pub load_balance: LoadBalanceAlgorithm,
    /// How long to wait for buffered records on shutdown.
    pub shutdown_timeout: Duration,
}

impl Default for Configuration {
    fn default() -> Self {
        let common = flowscope_kafka::Configuration::default();
        Configuration {
            enabled: false,
            topic: "flows-enriched".into(),
            brokers: common.brokers,
            tls: common.tls,
            sasl: common.sasl,
            compression_codec: CompressionCodec::Lz4,
            queue_size: 4096,
            load_balance: LoadBalanceAlgorithm::Random,
            shutdown_timeout: Duration::from_secs(1),
        }
    }
}

fn migrate_output(map: &mut Map<String, Value>) -> Result<(), String> {
    migrate::remove(map, "Version");
    flowscope_kafka::migrate_configuration(map)
}

migrated!(Configuration, migrate_output);

impl Configuration {
    pub fn common(&self) -> flowscope_kafka::Configuration {
        flowscope_kafka::Configuration {
            topic: self.topic.clone(),
            brokers: self.brokers.clone(),
            tls: self.tls.clone(),
            sasl: self.sasl.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.common().validate()?;
        if self.queue_size < 1 {
            return Err("kafka-output.queue-size should be at least 1".into());
        }
        Ok(())
    }
}

struct Metrics {
    messages_sent: Counter,
    bytes_sent: Counter,
    dropped: Counter,
    errors: CounterVec,
}

struct Context {
    metrics: Arc<Metrics>,
    topic: String,
    last_error_log: Mutex<Option<Instant>>,
}

impl ClientContext for Context {}

impl ProducerContext for Context {
    type DeliveryOpaque = ();

    fn delivery(&self, result: &DeliveryResult<'_>, _: ()) {
        match result {
            Ok(m) => {
                self.metrics.messages_sent.inc();
                self.metrics.bytes_sent.inc_by(m.payload_len() as f64);
            }
            Err((err, _)) => {
                let label = match err {
                    KafkaError::MessageProduction(code) => code.to_string(),
                    _ => "unknown".into(),
                };
                self.metrics.errors.with_label_values(&[&label]).inc();
                let mut last = self.last_error_log.lock().unwrap();
                if last.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3)) {
                    *last = Some(Instant::now());
                    tracing::error!(error = %err, topic = %self.topic, "Kafka producer error");
                }
            }
        }
    }
}

/// The Kafka output.
pub struct KafkaOutput {
    config: Configuration,
    topic: String,
    proto: String,
    metrics: Arc<Metrics>,
    producer: Mutex<Option<Arc<ThreadedProducer<Context>>>>,
}

impl KafkaOutput {
    pub fn new(r: &Reporter, config: Configuration, schema: &Schema) -> Result<Arc<KafkaOutput>, String> {
        let m = r.metrics("outlet/kafkaoutput");
        let metrics = Arc::new(Metrics {
            messages_sent: m.counter("sent_messages_total", "Number of enriched flow messages sent to Kafka."),
            bytes_sent: m.counter("sent_bytes_total", "Number of bytes sent to Kafka."),
            dropped: m.counter(
                "dropped_messages_total",
                "Number of enriched flow messages dropped because the producer buffer was full.",
            ),
            errors: m.counter_vec("errors_total", "Number of errors when sending to Kafka.", &["error"]),
        });
        let s = schema.schema();
        Ok(Arc::new(KafkaOutput {
            topic: format!("{}-{}", config.topic, s.protobuf_message_hash()),
            proto: s.protobuf_definition(),
            config,
            metrics,
            producer: Mutex::new(None),
        }))
    }

    /// Whether the output is active.
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Routes of the component (only when enabled).
    pub fn router(self: &Arc<Self>) -> Router {
        if !self.config.enabled {
            return Router::new();
        }
        let this = self.clone();
        Router::new().route(
            "/api/v0/outlet/kafka-output/schema.proto",
            axum::routing::get(move || {
                let proto = this.proto.clone();
                async move { ([(axum::http::header::CONTENT_TYPE, "text/plain")], proto) }
            }),
        )
    }

    /// Create the producer.
    pub fn start(&self) -> Result<(), String> {
        if !self.config.enabled {
            return Ok(());
        }
        tracing::info!("starting Kafka output component");
        let mut client = self.config.common().client_config();
        client
            .set("compression.codec", self.config.compression_codec.as_str())
            .set("queue.buffering.max.messages", self.config.queue_size.to_string())
            .set("partitioner", "murmur2_random")
            .set("allow.auto.create.topics", "true")
            .set("linger.ms", "5");
        let context =
            Context { metrics: self.metrics.clone(), topic: self.topic.clone(), last_error_log: Mutex::new(None) };
        let producer: ThreadedProducer<Context> =
            client.create_with_context(context).map_err(|e| format!("unable to create Kafka client: {e}"))?;
        *self.producer.lock().unwrap() = Some(Arc::new(producer));
        Ok(())
    }

    /// Flush the buffered records and close the producer.
    pub fn stop(&self) {
        let Some(producer) = self.producer.lock().unwrap().take() else { return };
        tracing::info!("stopping Kafka output component");
        if !self.config.shutdown_timeout.is_zero()
            && let Err(e) = producer.flush(*self.config.shutdown_timeout)
        {
            tracing::warn!(error = %e, "cannot flush the remaining records");
        }
        tracing::info!("Kafka output component stopped");
    }

    /// Hand one enriched flow to the producer. Never blocks: when the queue
    /// is full, the record is dropped.
    pub fn send(&self, exporter: &str, payload: &[u8]) {
        let producer = self.producer.lock().unwrap().clone();
        let Some(producer) = producer else { return };
        let key = self.config.load_balance.record_key(exporter);
        let record = BaseRecord::to(&self.topic).key(&key[..]).payload(payload);
        match producer.send(record) {
            Ok(()) => {}
            Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), _)) => self.metrics.dropped.inc(),
            Err((err, _)) => {
                self.metrics.errors.with_label_values(&["unknown"]).inc();
                tracing::debug!(error = %err, topic = %self.topic, "cannot queue Kafka record");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_schema_suffix() {
        let r = Reporter::new_mock();
        let schema = Schema::default_schema();
        let c = KafkaOutput::new(&r, Configuration::default(), &schema).unwrap();
        assert_eq!(c.topic(), format!("flows-enriched-{}", schema.schema().protobuf_message_hash()));
    }

    #[test]
    fn disabled() {
        let r = Reporter::new_mock();
        let c = KafkaOutput::new(&r, Configuration::default(), &Schema::default_schema()).unwrap();
        assert!(!c.enabled());
        c.start().unwrap();
        c.send("k", b"dropped");
        c.stop();
    }

    #[tokio::test]
    async fn schema_http_handler() {
        use tower::ServiceExt;
        let r = Reporter::new_mock();
        let schema = Schema::default_schema();
        let c = KafkaOutput::new(&r, Configuration { enabled: true, ..Default::default() }, &schema).unwrap();
        let resp = c
            .router()
            .oneshot(
                axum::http::Request::get("/api/v0/outlet/kafka-output/schema.proto")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers()["content-type"], "text/plain");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(body, schema.schema().protobuf_definition());
        assert!(body.contains(&schema.schema().protobuf_message_hash()));
    }
}
