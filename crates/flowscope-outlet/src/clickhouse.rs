// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Flow export to ClickHouse. Port of `outlet/clickhouse`.
//!
//! The component is inert: it does not track the workers it creates. The
//! caller flushes them before shutting down. Workers are synchronous and meant
//! to run on their own thread.

use flowscope_clickhouse::{Connection, Options, Setting};
use flowscope_common::clickhousedb::ClickHouseDb;
use flowscope_common::config::Duration;
use flowscope_common::reporter::{Counter, CounterVec, Histogram, Reporter, Summary};
use flowscope_schema::FlowMessage;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// How a worker chooses a ClickHouse server for each batch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerSelectionAlgorithm {
    /// Pin each worker to a random server until its connection breaks.
    #[default]
    StickyRandom,
    /// Spread the batches of a worker across all servers.
    RoundRobin,
}

/// Configuration of the ClickHouse exporter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// How long to wait for flushing a batch on shutdown.
    pub grace_period: Duration,
    /// Maximum number of rows in one batch.
    pub maximum_batch_size: usize,
    /// Maximum time to wait before sending the current batch.
    pub maximum_wait_time: Duration,
    /// How each worker chooses a server.
    pub server_selection: ServerSelectionAlgorithm,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            grace_period: Duration::from_secs(60),
            maximum_batch_size: 50_000,
            maximum_wait_time: Duration::from_secs(5),
            server_selection: ServerSelectionAlgorithm::StickyRandom,
        }
    }
}

impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        if self.grace_period.as_secs() < 10 {
            return Err("clickhouse.grace-period should be at least 10s".into());
        }
        if self.maximum_batch_size < 1 {
            return Err("clickhouse.maximum-batch-size should be at least 1".into());
        }
        if self.maximum_wait_time.as_millis() < 100 {
            return Err("clickhouse.maximum-wait-time should be at least 100ms".into());
        }
        Ok(())
    }
}

const MINIMUM_BATCH_SIZE_DIVIDER: usize = 10;

/// Whether a worker is overloaded or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerStatus {
    /// Nothing was sent.
    Idle,
    /// Too much work, more workers would help.
    Overloaded,
    /// Not enough work.
    Underloaded,
    /// The right amount of work.
    Steady,
}

/// Tells when an operation should give up.
pub type Done<'a> = &'a (dyn Fn() -> bool + Sync);

/// A worker sending batches to ClickHouse.
pub trait Worker: Send {
    /// Finalize the current flow and send the batch if it is full or old
    /// enough.
    fn finalize_and_send(&mut self, bf: &mut FlowMessage, done: Done) -> WorkerStatus;
    /// Send the remaining flows. Retry until `done` returns true.
    fn flush(&mut self, bf: &mut FlowMessage, done: Done);
    /// Release connections.
    fn close(&mut self);
}

/// Something creating ClickHouse workers.
pub trait Component: Send + Sync {
    fn new_worker(&self, i: usize, bf: &FlowMessage) -> Box<dyn Worker>;
}

struct Metrics {
    flows: Summary,
    wait_time: Histogram,
    insert_time: Histogram,
    batches_sent: CounterVec,
    overloaded: Counter,
    underloaded: Counter,
    steady: Counter,
    errors: CounterVec,
}

/// The ClickHouse exporter.
pub struct ClickHouse {
    config: Configuration,
    minimum_batch_size: usize,
    db: Arc<ClickHouseDb>,
    metrics: Arc<Metrics>,
}

impl ClickHouse {
    pub fn new(r: &Reporter, config: Configuration, db: Arc<ClickHouseDb>) -> ClickHouse {
        let m = r.metrics("outlet/clickhouse");
        let w = config.maximum_wait_time.as_secs_f64();
        let metrics = Metrics {
            flows: m.summary(
                "flow_per_batch",
                "Number of flow per batch sent to ClickHouse.",
                std::time::Duration::from_secs(600),
            ),
            wait_time: m.histogram(
                "wait_time_seconds",
                "Time spent waiting before sending a batch to ClickHouse.",
                &[w * 0.1, w * 0.25, w * 0.5, w * 0.9, w * 1.1, w * 2.5, w * 5.0, w * 10.0],
            ),
            insert_time: m.histogram(
                "insert_time_seconds",
                "Time spent inserting data to ClickHouse.",
                &[0.01, 0.025, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 20.0, 60.0],
            ),
            batches_sent: m.counter_vec(
                "batches_sent_total",
                "Number of batches sent to each ClickHouse server.",
                &["server"],
            ),
            overloaded: m.counter("worker_overloaded_total", "Number of times a worker was overloaded."),
            underloaded: m.counter("worker_underloaded_total", "Number of times a worker was underloaded."),
            steady: m.counter("worker_steady_total", "Number of times a worker was in steady state."),
            errors: m.counter_vec("errors_total", "Errors while inserting into ClickHouse.", &["error"]),
        };
        ClickHouse {
            minimum_batch_size: config.maximum_batch_size / MINIMUM_BATCH_SIZE_DIVIDER,
            config,
            db,
            metrics: Arc::new(metrics),
        }
    }
}

impl Component for ClickHouse {
    fn new_worker(&self, i: usize, bf: &FlowMessage) -> Box<dyn Worker> {
        let (options, servers) = self.db.options();
        let w = self.config.maximum_wait_time.as_millis();
        let async_settings = vec![
            Setting::important("async_insert", "1"),
            Setting::important("wait_for_async_insert", "1"),
            Setting::new("async_insert_busy_timeout_max_ms", w),
        ];
        Box::new(RealWorker {
            config: self.config.clone(),
            minimum_batch_size: self.minimum_batch_size,
            metrics: self.metrics.clone(),
            worker: i,
            table: format!("flows_{}_raw", bf.schema().clickhouse_hash()),
            last: None,
            servers: servers.into_iter().map(|address| ServerConn { address, conn: None }).collect(),
            selection: match self.config.server_selection {
                // Stagger the starting server per worker so batches spread
                // across all servers from the first flush.
                ServerSelectionAlgorithm::RoundRobin => Selection::RoundRobin { next: i },
                ServerSelectionAlgorithm::StickyRandom => Selection::StickyRandom { current: None },
            },
            connect: Box::new(ensure_connected_with_metrics(
                Box::new(ensure_connected(options)),
                self.metrics.clone(),
                i,
            )),
            shuffle: Box::new(|n| {
                let mut v: Vec<usize> = (0..n).collect();
                v.shuffle(&mut rand::rng());
                v
            }),
            async_settings,
        })
    }
}

/// One ClickHouse server of a worker and its connection, opened lazily.
pub struct ServerConn {
    pub address: String,
    pub conn: Option<Connection>,
}

enum Selection {
    RoundRobin { next: usize },
    StickyRandom { current: Option<usize> },
}

type ConnectFn = Box<dyn FnMut(&mut ServerConn) -> Result<(), String> + Send>;

/// Make sure a server has a healthy connection, dialing again if needed.
fn ensure_connected(options: Options) -> impl FnMut(&mut ServerConn) -> Result<(), String> + Send {
    move |sc| {
        if let Some(conn) = &mut sc.conn {
            if conn.ping().is_ok() {
                return Ok(());
            }
            sc.conn = None;
        }
        let mut opts = options.clone();
        opts.address = sc.address.clone();
        let mut conn = Connection::connect(&opts).map_err(|e| format!("connect: {e}"))?;
        conn.ping().map_err(|e| format!("ping: {e}"))?;
        sc.conn = Some(conn);
        Ok(())
    }
}

/// Wrap a connect function to log and count errors.
fn ensure_connected_with_metrics(
    mut inner: ConnectFn,
    metrics: Arc<Metrics>,
    worker: usize,
) -> impl FnMut(&mut ServerConn) -> Result<(), String> + Send {
    move |sc| {
        let had_conn = sc.conn.is_some();
        match inner(sc) {
            Ok(()) => {
                if !had_conn {
                    tracing::info!(worker, server = %sc.address, "connected to ClickHouse server");
                }
                Ok(())
            }
            Err(e) => {
                let kind = if e.starts_with("ping") { "ping" } else { "connect" };
                tracing::error!(worker, server = %sc.address, error = %e, "failed to connect to ClickHouse server");
                metrics.errors.with_label_values(&[kind]).inc();
                Err(e)
            }
        }
    }
}

/// The worker sending to ClickHouse.
pub struct RealWorker {
    config: Configuration,
    minimum_batch_size: usize,
    metrics: Arc<Metrics>,
    worker: usize,
    table: String,
    last: Option<Instant>,
    servers: Vec<ServerConn>,
    selection: Selection,
    connect: ConnectFn,
    shuffle: Box<dyn FnMut(usize) -> Vec<usize> + Send>,
    async_settings: Vec<Setting>,
}

impl RealWorker {
    /// Pick (and connect) the server for the next batch.
    fn select_server(&mut self) -> Result<usize, String> {
        let n = self.servers.len();
        let mut last_err = String::from("no server");
        match &mut self.selection {
            Selection::RoundRobin { next } => {
                for _ in 0..n {
                    let idx = *next % n;
                    *next = (*next + 1) % n;
                    match (self.connect)(&mut self.servers[idx]) {
                        Ok(()) => return Ok(idx),
                        Err(e) => last_err = e,
                    }
                }
                Err(last_err)
            }
            Selection::StickyRandom { current } => {
                // Keep the pinned server while its connection is healthy.
                if let Some(idx) = *current {
                    if (self.connect)(&mut self.servers[idx]).is_ok() {
                        return Ok(idx);
                    }
                    *current = None;
                }
                for idx in (self.shuffle)(n) {
                    match (self.connect)(&mut self.servers[idx]) {
                        Ok(()) => {
                            *current = Some(idx);
                            return Ok(idx);
                        }
                        Err(e) => last_err = e,
                    }
                }
                Err(last_err)
            }
        }
    }

    /// One attempt to send the batch.
    fn try_send(&mut self, bf: &FlowMessage, settings: &[Setting]) -> Result<(), String> {
        let idx = self.select_server()?;
        let query = bf.insert_query(&self.table);
        let ncols = bf.batch_columns().len();
        let rows = bf.flow_count();
        let start = Instant::now();
        let sc = &mut self.servers[idx];
        let conn = sc.conn.as_mut().expect("connected server");
        if let Err(e) =
            conn.insert_raw(&query, settings, ncols, rows, |out, with_custom| bf.encode_columns(with_custom, out))
        {
            tracing::error!(worker = self.worker, server = %sc.address, flows = rows, async = !settings.is_empty(),
                error = %e, "cannot send batch to ClickHouse");
            self.metrics.errors.with_label_values(&["send"]).inc();
            if conn.is_broken() {
                sc.conn = None;
            }
            return Err(e.to_string());
        }
        self.metrics.insert_time.observe(start.elapsed().as_secs_f64());
        self.metrics.flows.observe(rows as f64);
        self.metrics.batches_sent.with_label_values(&[&sc.address]).inc();
        Ok(())
    }
}

/// Sleep for `d`, waking up early when `done` returns true.
fn sleep_unless_done(d: std::time::Duration, done: Done) {
    let end = Instant::now() + d;
    while !done() {
        let now = Instant::now();
        if now >= end {
            return;
        }
        std::thread::sleep((end - now).min(std::time::Duration::from_millis(50)));
    }
}

impl Worker for RealWorker {
    fn finalize_and_send(&mut self, bf: &mut FlowMessage, done: Done) -> WorkerStatus {
        bf.finalize();
        let now = Instant::now();
        let batch_size = bf.flow_count();
        let waited = self.last.map(|l| now - l);
        if batch_size >= self.config.maximum_batch_size || waited.is_none_or(|w| w >= *self.config.maximum_wait_time) {
            if let Some(w) = waited {
                self.metrics.wait_time.observe(w.as_secs_f64());
            }
            self.flush(bf, done);
            self.last = Some(Instant::now());
            if batch_size >= self.config.maximum_batch_size {
                self.metrics.overloaded.inc();
                return WorkerStatus::Overloaded;
            } else if batch_size <= self.minimum_batch_size {
                self.metrics.underloaded.inc();
                return WorkerStatus::Underloaded;
            }
            self.metrics.steady.inc();
            return WorkerStatus::Steady;
        }
        WorkerStatus::Idle
    }

    fn flush(&mut self, bf: &mut FlowMessage, done: Done) {
        if bf.flow_count() == 0 {
            return;
        }
        // Use async inserts for small batches.
        let settings = if bf.flow_count() <= self.minimum_batch_size { self.async_settings.clone() } else { vec![] };
        // Retry with an exponential backoff until it works or we are told
        // to stop. An attempt in progress is not interrupted: the connection
        // timeouts bound it.
        let mut interval = std::time::Duration::from_millis(20);
        loop {
            if self.try_send(bf, &settings).is_ok() {
                bf.clear();
                return;
            }
            if done() {
                break;
            }
            let jitter = 0.5 + rand::random::<f64>();
            sleep_unless_done(interval.mul_f64(jitter), done);
            if done() {
                break;
            }
            interval = (interval.mul_f64(1.5)).min(std::time::Duration::from_secs(30));
        }
        tracing::error!(worker = self.worker, flows = bf.flow_count(), "batch lost, cannot flush to ClickHouse");
        self.metrics.errors.with_label_values(&["flush"]).inc();
    }

    fn close(&mut self) {
        for sc in &mut self.servers {
            sc.conn = None;
        }
    }
}

/// Mock exporter calling a callback with each sent flow message.
pub struct Mock {
    callback: Arc<Mutex<dyn FnMut(FlowMessage) + Send>>,
}

impl Mock {
    pub fn new(callback: impl FnMut(FlowMessage) + Send + 'static) -> Mock {
        Mock { callback: Arc::new(Mutex::new(callback)) }
    }
}

impl Component for Mock {
    fn new_worker(&self, _i: usize, _bf: &FlowMessage) -> Box<dyn Worker> {
        Box::new(MockWorker { callback: self.callback.clone() })
    }
}

struct MockWorker {
    callback: Arc<Mutex<dyn FnMut(FlowMessage) + Send>>,
}

impl Worker for MockWorker {
    fn finalize_and_send(&mut self, bf: &mut FlowMessage, done: Done) -> WorkerStatus {
        self.flush(bf, done);
        WorkerStatus::Idle
    }

    fn flush(&mut self, bf: &mut FlowMessage, _done: Done) {
        (self.callback.lock().unwrap())(bf.clone());
        // Clear instead of finalizing.
        bf.clear();
    }

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(
        selection: Selection,
        servers: &[&str],
        up: &'static [&'static str],
    ) -> (RealWorker, Arc<Mutex<Vec<String>>>) {
        let r = Reporter::new_mock();
        let db = ClickHouseDb::new(Default::default()).unwrap();
        let ch = ClickHouse::new(&r, Configuration::default(), db);
        let attempts = Arc::new(Mutex::new(vec![]));
        let a = attempts.clone();
        let w = RealWorker {
            config: ch.config.clone(),
            minimum_batch_size: ch.minimum_batch_size,
            metrics: ch.metrics.clone(),
            worker: 0,
            table: "flows_raw".into(),
            last: None,
            servers: servers.iter().map(|s| ServerConn { address: s.to_string(), conn: None }).collect(),
            selection,
            connect: Box::new(move |sc| {
                a.lock().unwrap().push(sc.address.clone());
                if up.contains(&sc.address.as_str()) { Ok(()) } else { Err("connect: down".into()) }
            }),
            shuffle: Box::new(|n| (0..n).rev().collect()),
            async_settings: vec![],
        };
        (w, attempts)
    }

    #[test]
    fn round_robin() {
        let (mut w, attempts) = worker(Selection::RoundRobin { next: 1 }, &["a", "b", "c"], &["a", "b", "c"]);
        let picks: Vec<usize> = (0..4).map(|_| w.select_server().unwrap()).collect();
        assert_eq!(picks, vec![1, 2, 0, 1]);
        assert_eq!(attempts.lock().unwrap().len(), 4);
    }

    #[test]
    fn round_robin_skips_down_servers() {
        let (mut w, _) = worker(Selection::RoundRobin { next: 0 }, &["a", "b", "c"], &["c"]);
        assert_eq!(w.select_server().unwrap(), 2);
        assert_eq!(w.select_server().unwrap(), 2);
        let (mut w, _) = worker(Selection::RoundRobin { next: 0 }, &["a", "b"], &[]);
        assert!(w.select_server().is_err());
    }

    #[test]
    fn sticky_random() {
        let (mut w, attempts) = worker(Selection::StickyRandom { current: None }, &["a", "b", "c"], &["a", "b"]);
        // Shuffle gives [2, 1, 0]: c is down, b is picked and kept.
        assert_eq!(w.select_server().unwrap(), 1);
        assert_eq!(w.select_server().unwrap(), 1);
        assert_eq!(*attempts.lock().unwrap(), vec!["c", "b", "b"]);
    }

    #[test]
    fn configuration() {
        use flowscope_common::config::{from_value, parse_str};
        let got: Configuration =
            from_value(parse_str("server-selection: round-robin\nmaximum-batch-size: 1000\n").unwrap()).unwrap();
        assert_eq!(got.server_selection, ServerSelectionAlgorithm::RoundRobin);
        assert_eq!(got.maximum_batch_size, 1000);
        got.validate().unwrap();
        assert!(Configuration { maximum_batch_size: 0, ..Default::default() }.validate().is_err());
    }
}
