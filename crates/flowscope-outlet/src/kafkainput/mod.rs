// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Flow import from Kafka. Port of `outlet/kafkainput`.
//!
//! Each worker owns a consumer of the group and runs on its own thread. The
//! number of workers changes with the load reported by the workers.

pub mod scaler;

pub use scaler::ScaleRequest;

use flowscope_common::config::{Duration, migrate};
use flowscope_common::helpers::TlsConfiguration;
use flowscope_common::migrated;
use flowscope_common::reporter::{Counter, CounterVec, Reporter};
use flowscope_kafka::SaslConfiguration;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer};
use rdkafka::{ClientConfig, Message, Offset, TopicPartitionList};
use scaler::{ScalerCallbacks, ScalerConfiguration, run_scaler};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Configuration of the Kafka input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    pub topic: String,
    pub brokers: Vec<String>,
    pub tls: TlsConfiguration,
    pub sasl: SaslConfiguration,
    /// Consumer group to use.
    pub consumer_group: String,
    /// Minimum number of bytes to wait for before fetching.
    pub fetch_min_bytes: i32,
    /// Maximum time to wait for the minimum number of bytes.
    pub fetch_max_wait_time: Duration,
    /// Minimum number of workers.
    pub min_workers: usize,
    /// Maximum number of workers.
    pub max_workers: usize,
    /// Time between two increases of the number of workers.
    pub worker_increase_rate_limit: Duration,
    /// Time between two decreases of the number of workers.
    pub worker_decrease_rate_limit: Duration,
}

impl Default for Configuration {
    fn default() -> Self {
        let common = flowscope_kafka::Configuration::default();
        Configuration {
            topic: common.topic,
            brokers: common.brokers,
            tls: common.tls,
            sasl: common.sasl,
            consumer_group: "flowscope-outlet".into(),
            fetch_min_bytes: 1_000_000,
            fetch_max_wait_time: Duration::from_secs(1),
            min_workers: 1,
            // Too many workers for a single ClickHouse table is not good.
            max_workers: 8,
            worker_increase_rate_limit: Duration::from_secs(60),
            worker_decrease_rate_limit: Duration::from_secs(600),
        }
    }
}

fn migrate_kafka(map: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["MaxMessageBytes", "QueueSize", "Workers"] {
        migrate::remove(map, k);
    }
    flowscope_kafka::migrate_configuration(map)
}

migrated!(Configuration, migrate_kafka);

impl Configuration {
    /// The common part of the configuration.
    pub fn common(&self) -> flowscope_kafka::Configuration {
        flowscope_kafka::Configuration {
            topic: self.topic.clone(),
            brokers: self.brokers.clone(),
            tls: self.tls.clone(),
            sasl: self.sasl.clone(),
        }
    }

    /// Topic name with the version of the message.
    pub fn versioned_topic(&self) -> String {
        format!("{}-v{}", self.topic, flowscope_pb::VERSION)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.common().validate()?;
        if self.consumer_group.is_empty() || !self.consumer_group.is_ascii() {
            return Err("kafka.consumer-group should be a non-empty ASCII string".into());
        }
        if self.fetch_min_bytes < 1 {
            return Err("kafka.fetch-min-bytes should be at least 1".into());
        }
        if self.fetch_max_wait_time.as_millis() < 100 {
            return Err("kafka.fetch-max-wait-time should be at least 100ms".into());
        }
        if self.min_workers < 1 {
            return Err("kafka.min-workers should be at least 1".into());
        }
        if self.max_workers < self.min_workers {
            return Err("kafka.max-workers should be at least kafka.min-workers".into());
        }
        if self.worker_increase_rate_limit.as_secs() < 10 {
            return Err("kafka.worker-increase-rate-limit should be at least 10s".into());
        }
        if self.worker_decrease_rate_limit.as_secs() < 20
            || self.worker_decrease_rate_limit <= self.worker_increase_rate_limit
        {
            return Err(
                "kafka.worker-decrease-rate-limit should be at least 20s and more than the increase rate limit".into(),
            );
        }
        Ok(())
    }
}

/// Tells when a worker should stop.
pub type Done<'a> = &'a (dyn Fn() -> bool + Sync);

/// A worker processing messages from Kafka.
pub trait ReceiveWorker: Send {
    /// Process one message. Errors are handled by the worker.
    fn receive(&mut self, data: &[u8], done: Done);
    /// Flush and release resources.
    fn shutdown(&mut self);
}

/// Build a worker. It gets its number and a channel to request more or
/// fewer workers.
pub type WorkerBuilder = Arc<dyn Fn(usize, SyncSender<ScaleRequest>) -> Box<dyn ReceiveWorker> + Send + Sync>;

/// Something feeding workers with messages.
pub trait Component: Send + Sync {
    /// Start the initial workers. Call it only once.
    fn start_workers(&self, builder: WorkerBuilder) -> Result<(), String>;
    /// Stop all workers.
    fn stop_workers(&self);
}

struct Metrics {
    messages_received: CounterVec,
    fetches_received: CounterVec,
    bytes_received: CounterVec,
    errors_received: CounterVec,
    worker_increase: Counter,
    worker_decrease: Counter,
}

struct RunningWorker {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

struct Inner {
    config: Configuration,
    client_config: ClientConfig,
    metrics: Metrics,
    max_workers: AtomicUsize,
    workers: Mutex<Vec<RunningWorker>>,
    builder: Mutex<Option<WorkerBuilder>>,
    scale_tx: Mutex<Option<SyncSender<ScaleRequest>>>,
    scaler_stop: Arc<AtomicBool>,
    terminated: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Consumer used for the lag metric. It does not join the group.
    lag_consumer: Mutex<Option<BaseConsumer>>,
}

/// The Kafka input.
pub struct KafkaInput {
    inner: Arc<Inner>,
}

impl KafkaInput {
    /// Create the component. `terminated` tells when the daemon stops.
    pub fn new(
        r: &Reporter,
        config: Configuration,
        terminated: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<KafkaInput, String> {
        let mut client_config = config.common().client_config();
        client_config
            .set("group.id", &config.consumer_group)
            .set("fetch.min.bytes", config.fetch_min_bytes.to_string())
            .set("fetch.wait.max.ms", config.fetch_max_wait_time.as_millis().to_string())
            .set("auto.offset.reset", "latest")
            // Offsets are stored once a message is processed, and committed
            // every second.
            .set("enable.auto.commit", "true")
            .set("auto.commit.interval.ms", "1000")
            .set("enable.auto.offset.store", "false")
            .set("partition.assignment.strategy", "cooperative-sticky");
        let m = r.metrics("outlet/kafkainput");
        let metrics = Metrics {
            messages_received: m.counter_vec(
                "received_messages_total",
                "Number of messages received for a given worker.",
                &["worker"],
            ),
            fetches_received: m.counter_vec(
                "received_fetches_total",
                "Number of fetches received for a given worker.",
                &["worker"],
            ),
            bytes_received: m.counter_vec(
                "received_bytes_total",
                "Number of bytes received for a given worker.",
                &["worker"],
            ),
            errors_received: m.counter_vec(
                "received_errors_total",
                "Number of errors while handling received messages for a given worker.",
                &["worker"],
            ),
            worker_increase: m.counter("worker_increase_total", "Number of times a new worker was spawned."),
            worker_decrease: m.counter("worker_decrease_total", "Number of times a new worker was stopped."),
        };
        let inner = Arc::new(Inner {
            max_workers: AtomicUsize::new(config.max_workers),
            config,
            client_config,
            metrics,
            workers: Mutex::new(vec![]),
            builder: Mutex::new(None),
            scale_tx: Mutex::new(None),
            scaler_stop: Arc::new(AtomicBool::new(false)),
            terminated,
            lag_consumer: Mutex::new(None),
        });
        let i = inner.clone();
        m.gauge_func("workers", "Number of running workers.", move || i.workers.lock().unwrap().len() as f64);
        let i = inner.clone();
        m.gauge_func("max_workers", "Maximum number of running workers.", move || {
            i.max_workers.load(Ordering::Relaxed) as f64
        });
        let min = inner.config.min_workers as f64;
        m.gauge_func("min_workers", "Minimum number of running workers.", move || min);
        let i = inner.clone();
        m.gauge_func(
            "consumergroup_lag_messages",
            "Current consumer lag across all partitions (or -1 on errors).",
            move || match i.lag() {
                Ok(lag) => lag as f64,
                Err(e) => {
                    tracing::error!(error = %e, "lag metric refresh failed");
                    -1.0
                }
            },
        );
        Ok(KafkaInput { inner })
    }

    /// Check the topic exists and cap the number of workers to the number
    /// of partitions.
    pub fn start(&self) -> Result<(), String> {
        tracing::info!("starting Kafka component");
        let consumer: BaseConsumer =
            self.inner.client_config.create().map_err(|e| format!("unable to create Kafka admin client: {e}"))?;
        let topic = self.inner.config.versioned_topic();
        let metadata = consumer
            .fetch_metadata(Some(&topic), std::time::Duration::from_secs(10))
            .map_err(|e| format!("unable to get metadata for topics: {e}"))?;
        let partitions = metadata
            .topics()
            .iter()
            .find(|t| t.name() == topic && t.error().is_none())
            .map(|t| t.partitions().len())
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("unable find topic {topic:?}"))?;
        tracing::info!("topic {topic:?} has {partitions} partitions");
        if partitions < self.inner.config.max_workers {
            tracing::warn!("capping max workers from {} to {partitions}", self.inner.config.max_workers);
            self.inner.max_workers.store(partitions, Ordering::Relaxed);
        }
        *self.inner.lag_consumer.lock().unwrap() = Some(consumer);
        Ok(())
    }

    /// Stop the workers and the scaler.
    pub fn stop(&self) {
        tracing::info!("stopping Kafka component");
        self.inner.scaler_stop.store(true, Ordering::Relaxed);
        self.stop_workers();
        self.inner.lag_consumer.lock().unwrap().take();
        tracing::info!("Kafka component stopped");
    }
}

impl Inner {
    /// Sum of the lag of the group over all partitions of the topic.
    fn lag(&self) -> Result<i64, String> {
        let guard = self.lag_consumer.lock().unwrap();
        let Some(consumer) = guard.as_ref() else { return Err("not started".into()) };
        let timeout = std::time::Duration::from_millis(500);
        let topic = self.config.versioned_topic();
        let metadata = consumer.fetch_metadata(Some(&topic), timeout).map_err(|e| e.to_string())?;
        let mut tpl = TopicPartitionList::new();
        for t in metadata.topics().iter().filter(|t| t.name() == topic) {
            for p in t.partitions() {
                tpl.add_partition(&topic, p.id());
            }
        }
        if tpl.count() == 0 {
            return Err(format!("unable to find Kafka consumer group lag for topic {topic:?}"));
        }
        let committed = consumer.committed_offsets(tpl, timeout).map_err(|e| {
            format!("unable to fetch Kafka consumer group offsets {:?}: {e}", self.config.consumer_group)
        })?;
        let mut total = 0;
        for elem in committed.elements() {
            // Skip partitions without a commit (possibly unassigned).
            let Offset::Offset(offset) = elem.offset() else { continue };
            let (_, high) = consumer
                .fetch_watermarks(&topic, elem.partition(), timeout)
                .map_err(|e| format!("unable to compute Kafka group lag: {e}"))?;
            total += (high - offset).max(0);
        }
        Ok(total)
    }

    fn start_one_worker(self: &Arc<Self>) -> Result<(), String> {
        let mut workers = self.workers.lock().unwrap();
        let i = workers.len();
        tracing::info!(worker = i, "starting new client");
        let consumer: BaseConsumer = self.client_config.create().map_err(|e| {
            tracing::error!(worker = i, brokers = %self.config.brokers.join(","), error = %e, "unable to create new client");
            format!("unable to create Kafka client: {e}")
        })?;
        consumer.subscribe(&[&self.config.versioned_topic()]).map_err(|e| format!("unable to subscribe: {e}"))?;
        let builder = self.builder.lock().unwrap().clone().expect("builder set");
        let tx = self.scale_tx.lock().unwrap().clone().expect("scaler started");
        let receiver = builder(i, tx);
        let stop = Arc::new(AtomicBool::new(false));
        let this = self.clone();
        let s = stop.clone();
        let handle = std::thread::Builder::new()
            .name(format!("kafka-worker-{i}"))
            .spawn(move || this.run_worker(i, consumer, receiver, s))
            .map_err(|e| e.to_string())?;
        workers.push(RunningWorker { stop, handle });
        self.metrics.worker_increase.inc();
        Ok(())
    }

    fn stop_one_worker(&self) {
        let w = self.workers.lock().unwrap().pop();
        if let Some(w) = w {
            w.stop.store(true, Ordering::Relaxed);
            let _ = w.handle.join();
            self.metrics.worker_decrease.inc();
        }
    }

    fn run_worker(
        &self,
        i: usize,
        consumer: BaseConsumer,
        mut receiver: Box<dyn ReceiveWorker>,
        stop: Arc<AtomicBool>,
    ) {
        let worker = i.to_string();
        let messages = self.metrics.messages_received.with_label_values(&[&worker]);
        let bytes = self.metrics.bytes_received.with_label_values(&[&worker]);
        let fetches = self.metrics.fetches_received.with_label_values(&[&worker]);
        let errors = self.metrics.errors_received.with_label_values(&[&worker]);
        let terminated = self.terminated.clone();
        let done = move || stop.load(Ordering::Relaxed) || terminated();
        while !done() {
            match consumer.poll(std::time::Duration::from_millis(100)) {
                None => {}
                Some(Err(e)) => {
                    // librdkafka recovers from most errors by itself. The Go
                    // version stopped the worker for good.
                    errors.inc();
                    tracing::error!(worker = i, error = %e, "fetch error");
                }
                Some(Ok(msg)) => {
                    fetches.inc();
                    messages.inc();
                    let payload = msg.payload().unwrap_or_default();
                    bytes.inc_by(payload.len() as f64);
                    receiver.receive(payload, &done);
                    if let Err(e) = consumer.store_offset_from_message(&msg) {
                        tracing::debug!(worker = i, error = %e, "cannot store offset");
                    }
                }
            }
        }
        tracing::info!(worker = i, "stopping worker");
        if let Err(e) = consumer.commit_consumer_state(CommitMode::Sync) {
            tracing::debug!(worker = i, error = %e, "cannot commit marked partition offsets");
        }
        consumer.unsubscribe();
        drop(consumer);
        receiver.shutdown();
    }
}

impl Component for KafkaInput {
    fn start_workers(&self, builder: WorkerBuilder) -> Result<(), String> {
        *self.inner.builder.lock().unwrap() = Some(builder);
        let max_workers = self.inner.max_workers.load(Ordering::Relaxed);
        let i1 = self.inner.clone();
        let i2 = self.inner.clone();
        let i3 = self.inner.clone();
        let tx = run_scaler(
            ScalerConfiguration {
                min_workers: self.inner.config.min_workers,
                max_workers,
                increase_rate_limit: *self.inner.config.worker_increase_rate_limit,
                decrease_rate_limit: *self.inner.config.worker_decrease_rate_limit,
            },
            ScalerCallbacks {
                worker_count: Box::new(move || i1.workers.lock().unwrap().len()),
                increase_workers: Box::new(move |from, to| {
                    tracing::info!("increase number of workers from {from} to {to}");
                    for _ in from..to {
                        if let Err(e) = i2.start_one_worker() {
                            tracing::error!(error = %e, "cannot spawn a new worker");
                            return;
                        }
                    }
                }),
                decrease_workers: Box::new(move |from, to| {
                    tracing::info!("decrease number of workers from {from} to {to}");
                    for _ in to..from {
                        i3.stop_one_worker();
                    }
                }),
            },
            self.inner.scaler_stop.clone(),
        );
        *self.inner.scale_tx.lock().unwrap() = Some(tx);
        for _ in 0..self.inner.config.min_workers {
            self.inner.start_one_worker()?;
        }
        Ok(())
    }

    fn stop_workers(&self) {
        let workers: Vec<RunningWorker> = std::mem::take(&mut *self.inner.workers.lock().unwrap());
        for w in &workers {
            w.stop.store(true, Ordering::Relaxed);
        }
        for w in workers {
            let _ = w.handle.join();
        }
    }
}

/// Mock input: messages pushed with [`Mock::send`] go to the first worker.
pub struct Mock {
    workers: Mutex<Vec<Box<dyn ReceiveWorker>>>,
    scale_rx: Mutex<Option<std::sync::mpsc::Receiver<ScaleRequest>>>,
}

impl Mock {
    pub fn new() -> Arc<Mock> {
        Arc::new(Mock { workers: Mutex::new(vec![]), scale_rx: Mutex::new(None) })
    }

    /// Process a message with the first worker, synchronously.
    pub fn send(&self, data: &[u8]) {
        let mut workers = self.workers.lock().unwrap();
        let w = workers.first_mut().expect("workers started");
        w.receive(data, &|| false);
    }

    /// Scale requests sent so far.
    pub fn scale_requests(&self) -> Vec<ScaleRequest> {
        self.scale_rx.lock().unwrap().as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default()
    }
}

impl Component for Mock {
    fn start_workers(&self, builder: WorkerBuilder) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1024);
        *self.scale_rx.lock().unwrap() = Some(rx);
        self.workers.lock().unwrap().push(builder(0, tx));
        Ok(())
    }

    fn stop_workers(&self) {
        for mut w in self.workers.lock().unwrap().drain(..) {
            w.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};

    #[test]
    fn default_configuration() {
        Configuration::default().validate().unwrap();
    }

    #[test]
    fn configuration() {
        let got: Configuration = from_value(
            parse_str("topic: flows\nbrokers: [k:9092]\nworkers: 4\nqueue-size: 10\nmax-workers: 4\nversion: 2.8.1\n")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got.max_workers, 4);
        assert_eq!(got.brokers, vec!["k:9092"]);
        assert_eq!(got.versioned_topic(), "flows-v5");
    }
}
