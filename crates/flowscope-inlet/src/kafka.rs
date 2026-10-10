// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Kafka producer for the inlet. Port of `inlet/kafka`.

use crate::config::KafkaConfiguration;
use flowscope_common::reporter::{CounterVec, Reporter};
use flowscope_kafka::LoadBalanceAlgorithm;
use rdkafka::ClientContext;
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::{DeliveryResult, Message};
use rdkafka::producer::{BaseRecord, Producer, ProducerContext, ThreadedProducer};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Metrics {
    messages_sent: CounterVec,
    bytes_sent: CounterVec,
    errors: CounterVec,
}

/// Context receiving delivery reports.
struct Context {
    metrics: Metrics,
    topic: String,
    last_error_log: std::sync::Mutex<Option<Instant>>,
}

impl ClientContext for Context {}

impl ProducerContext for Context {
    /// The exporter, for metrics.
    type DeliveryOpaque = Box<String>;

    fn delivery(&self, result: &DeliveryResult<'_>, exporter: Self::DeliveryOpaque) {
        match result {
            Ok(message) => {
                let size = message.payload().map(|p| p.len()).unwrap_or(0);
                self.metrics.bytes_sent.with_label_values(&[&exporter]).inc_by(size as f64);
                self.metrics.messages_sent.with_label_values(&[&exporter]).inc();
            }
            Err((err, message)) => {
                let label = match err {
                    KafkaError::MessageProduction(code) => code.to_string(),
                    _ => "unknown".into(),
                };
                self.metrics.errors.with_label_values(&[&label]).inc();
                // Log at most 3 errors every 10 seconds.
                let mut last = self.last_error_log.lock().unwrap();
                if last.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
                    *last = Some(Instant::now());
                    tracing::error!(error = %err, topic = %self.topic, partition = message.partition(),
                        offset = message.offset(), "Kafka producer error");
                }
            }
        }
    }
}

/// The Kafka producer.
#[derive(Clone)]
pub struct KafkaProducer {
    producer: Arc<ThreadedProducer<Context>>,
    topic: String,
    load_balance: LoadBalanceAlgorithm,
}

impl KafkaProducer {
    /// Create the producer. Connection happens in the background.
    pub fn new(r: &Reporter, config: &KafkaConfiguration) -> Result<Self, String> {
        let m = r.metrics("inlet/kafka");
        let metrics = Metrics {
            messages_sent: m.counter_vec(
                "sent_messages_total",
                "Number of messages sent from a given exporter.",
                &["exporter"],
            ),
            bytes_sent: m.counter_vec("sent_bytes_total", "Number of bytes sent from a given exporter.", &["exporter"]),
            errors: m.counter_vec("errors_total", "Number of errors when sending.", &["error"]),
        };
        let topic = format!("{}-v{}", config.topic, flowscope_pb::VERSION);
        let mut client = config.common().client_config();
        client
            .set("compression.codec", config.compression_codec.as_str())
            .set("queue.buffering.max.messages", config.queue_size.to_string())
            // Java-compatible partitioning, like franz-go, so records with the
            // same key land on the same partition with Go and Rust inlets.
            .set("partitioner", "murmur2_random")
            .set("linger.ms", "5");
        let context = Context { metrics, topic: topic.clone(), last_error_log: std::sync::Mutex::new(None) };
        let producer: ThreadedProducer<Context> =
            client.create_with_context(context).map_err(|e| format!("unable to create Kafka client: {e}"))?;
        Ok(KafkaProducer { producer: Arc::new(producer), topic, load_balance: config.load_balance })
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Send a payload. Blocks while the local queue is full, like franz-go
    /// with a bounded buffer.
    pub fn send(&self, exporter: &str, payload: &[u8]) {
        let key = self.load_balance.record_key(exporter);
        let mut record =
            BaseRecord::with_opaque_to(&self.topic, Box::new(exporter.to_string())).key(&key[..]).payload(payload);
        loop {
            match self.producer.send(record) {
                Ok(()) => return,
                Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), r)) => {
                    record = r;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err((err, r)) => {
                    let exporter = r.delivery_opaque;
                    drop(exporter);
                    tracing::error!(error = %err, topic = %self.topic, "cannot queue Kafka record");
                    return;
                }
            }
        }
    }

    /// Flush pending records.
    pub fn flush(&self, timeout: Duration) {
        if let Err(e) = self.producer.flush(timeout) {
            tracing::warn!(error = %e, "unable to flush Kafka producer");
        }
    }
}
