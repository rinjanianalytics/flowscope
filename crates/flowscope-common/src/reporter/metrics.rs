// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A small Prometheus metrics registry. Port of `common/reporter/metrics`.
//!
//! It provides counters, gauges, function-backed metrics, labeled vectors,
//! histograms and summaries with quantiles over a sliding window (the Rust
//! Prometheus crates do not have those), and renders them in the text
//! exposition format. Registering a metric that already exists returns the
//! existing one, like the Go factory.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// An atomic f64.
#[derive(Default, Debug)]
struct AtomicF64(AtomicU64);

impl AtomicF64 {
    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    fn add(&self, v: f64) {
        let mut old = self.0.load(Ordering::Relaxed);
        loop {
            let new = (f64::from_bits(old) + v).to_bits();
            match self.0.compare_exchange_weak(old, new, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return,
                Err(x) => old = x,
            }
        }
    }
}

/// A monotonic counter.
#[derive(Clone, Default, Debug)]
pub struct Counter(Arc<AtomicF64>);

impl Counter {
    pub fn inc(&self) {
        self.0.add(1.0)
    }
    pub fn inc_by(&self, v: f64) {
        if v > 0.0 {
            self.0.add(v)
        }
    }
    pub fn get(&self) -> f64 {
        self.0.get()
    }
}

/// A gauge.
#[derive(Clone, Default, Debug)]
pub struct Gauge(Arc<AtomicF64>);

impl Gauge {
    pub fn set(&self, v: f64) {
        self.0.set(v)
    }
    pub fn add(&self, v: f64) {
        self.0.add(v)
    }
    pub fn sub(&self, v: f64) {
        self.0.add(-v)
    }
    pub fn inc(&self) {
        self.0.add(1.0)
    }
    pub fn dec(&self) {
        self.0.add(-1.0)
    }
    pub fn get(&self) -> f64 {
        self.0.get()
    }
}

/// Default histogram buckets, same as Go's prometheus.DefBuckets.
pub const DEFAULT_BUCKETS: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

#[derive(Debug)]
struct HistogramInner {
    buckets: Vec<f64>,
    counts: Vec<AtomicU64>,
    sum: AtomicF64,
    count: AtomicU64,
}

/// A histogram.
#[derive(Clone, Debug)]
pub struct Histogram(Arc<HistogramInner>);

impl Histogram {
    fn new(buckets: &[f64]) -> Self {
        let buckets = if buckets.is_empty() { DEFAULT_BUCKETS.to_vec() } else { buckets.to_vec() };
        let counts = buckets.iter().map(|_| AtomicU64::new(0)).collect();
        Histogram(Arc::new(HistogramInner { buckets, counts, sum: AtomicF64::default(), count: AtomicU64::new(0) }))
    }

    pub fn observe(&self, v: f64) {
        if let Some(i) = self.0.buckets.iter().position(|b| v <= *b) {
            self.0.counts[i].fetch_add(1, Ordering::Relaxed);
        }
        self.0.sum.add(v);
        self.0.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 {
        self.0.count.load(Ordering::Relaxed)
    }
}

/// Number of samples kept per age bucket of a summary.
const SUMMARY_RESERVOIR: usize = 1024;
/// Number of age buckets of a summary, like Go's DefAgeBuckets.
const SUMMARY_AGE_BUCKETS: u32 = 5;

#[derive(Debug)]
struct AgeBucket {
    start: Instant,
    seen: u64,
    samples: Vec<f64>,
}

#[derive(Debug)]
struct SummaryInner {
    quantiles: Vec<f64>,
    max_age: Duration,
    window: Mutex<VecDeque<AgeBucket>>,
    rng: Mutex<u64>,
    sum: AtomicF64,
    count: AtomicU64,
}

/// A summary with quantiles computed over a sliding window. Each age bucket
/// keeps a uniform sample of its observations (reservoir sampling), so
/// quantiles are approximate for high rates.
#[derive(Clone, Debug)]
pub struct Summary(Arc<SummaryInner>);

impl Summary {
    fn new(quantiles: &[f64], max_age: Duration) -> Self {
        let max_age = if max_age.is_zero() { Duration::from_secs(600) } else { max_age };
        let mut quantiles = quantiles.to_vec();
        quantiles.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Summary(Arc::new(SummaryInner {
            quantiles,
            max_age,
            window: Mutex::new(VecDeque::new()),
            rng: Mutex::new(0x9e3779b97f4a7c15),
            sum: AtomicF64::default(),
            count: AtomicU64::new(0),
        }))
    }

    fn bucket_duration(&self) -> Duration {
        self.0.max_age / SUMMARY_AGE_BUCKETS
    }

    fn expire(&self, window: &mut VecDeque<AgeBucket>, now: Instant) {
        while window.front().is_some_and(|b| now.duration_since(b.start) > self.0.max_age) {
            window.pop_front();
        }
    }

    pub fn observe(&self, v: f64) {
        self.0.sum.add(v);
        self.0.count.fetch_add(1, Ordering::Relaxed);
        if self.0.quantiles.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut window = self.0.window.lock().unwrap();
        self.expire(&mut window, now);
        let need_new = window.back().is_none_or(|b| now.duration_since(b.start) >= self.bucket_duration());
        if need_new {
            window.push_back(AgeBucket { start: now, seen: 0, samples: Vec::with_capacity(64) });
        }
        let bucket = window.back_mut().unwrap();
        bucket.seen += 1;
        if bucket.samples.len() < SUMMARY_RESERVOIR {
            bucket.samples.push(v);
        } else {
            // Algorithm R with a xorshift generator.
            let mut rng = self.0.rng.lock().unwrap();
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            let j = *rng % bucket.seen;
            if (j as usize) < SUMMARY_RESERVOIR {
                bucket.samples[j as usize] = v;
            }
        }
    }

    /// Current quantiles as (quantile, value). NaN when there is no data.
    pub fn quantiles(&self) -> Vec<(f64, f64)> {
        let now = Instant::now();
        let mut window = self.0.window.lock().unwrap();
        self.expire(&mut window, now);
        // Weight each sample by the share of observations it stands for.
        let mut samples: Vec<(f64, f64)> = Vec::new();
        for b in window.iter() {
            if b.samples.is_empty() {
                continue;
            }
            let w = b.seen as f64 / b.samples.len() as f64;
            samples.extend(b.samples.iter().map(|v| (*v, w)));
        }
        drop(window);
        samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let total: f64 = samples.iter().map(|s| s.1).sum();
        self.0
            .quantiles
            .iter()
            .map(|q| {
                if samples.is_empty() {
                    return (*q, f64::NAN);
                }
                let target = q * total;
                let mut acc = 0.0;
                for (v, w) in &samples {
                    acc += w;
                    if acc >= target {
                        return (*q, *v);
                    }
                }
                (*q, samples.last().unwrap().0)
            })
            .collect()
    }
}

/// Labeled metrics of the same kind.
#[derive(Clone)]
pub struct Vector<T> {
    labels: Arc<Vec<String>>,
    children: Arc<RwLock<BTreeMap<Vec<String>, T>>>,
    make: Arc<dyn Fn() -> T + Send + Sync>,
}

impl<T> std::fmt::Debug for Vector<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vector").field("labels", &self.labels).finish_non_exhaustive()
    }
}

impl<T: Clone> Vector<T> {
    fn new(labels: &[&str], make: impl Fn() -> T + Send + Sync + 'static) -> Self {
        Vector {
            labels: Arc::new(labels.iter().map(|s| s.to_string()).collect()),
            children: Arc::new(RwLock::new(BTreeMap::new())),
            make: Arc::new(make),
        }
    }

    /// Get or create the metric for the provided label values.
    pub fn with_label_values(&self, values: &[&str]) -> T {
        assert_eq!(values.len(), self.labels.len(), "wrong number of label values");
        let key: Vec<String> = values.iter().map(|s| s.to_string()).collect();
        if let Some(m) = self.children.read().unwrap().get(&key) {
            return m.clone();
        }
        let mut children = self.children.write().unwrap();
        children.entry(key).or_insert_with(|| (self.make)()).clone()
    }

    /// Remove the metric for the provided label values.
    pub fn remove_label_values(&self, values: &[&str]) -> bool {
        let key: Vec<String> = values.iter().map(|s| s.to_string()).collect();
        self.children.write().unwrap().remove(&key).is_some()
    }

    /// Remove all metrics.
    pub fn reset(&self) {
        self.children.write().unwrap().clear();
    }
}

pub type CounterVec = Vector<Counter>;
pub type GaugeVec = Vector<Gauge>;
pub type HistogramVec = Vector<Histogram>;
pub type SummaryVec = Vector<Summary>;

type ValueFn = Arc<dyn Fn() -> f64 + Send + Sync>;

#[derive(Clone)]
enum Metric {
    Counter(Counter),
    CounterFunc(ValueFn),
    CounterVec(CounterVec),
    Gauge(Gauge),
    GaugeFunc(ValueFn),
    GaugeVec(GaugeVec),
    Histogram(Histogram),
    HistogramVec(HistogramVec),
    Summary(Summary),
    SummaryVec(SummaryVec),
}

impl Metric {
    fn type_name(&self) -> &'static str {
        match self {
            Metric::Counter(_) | Metric::CounterFunc(_) | Metric::CounterVec(_) => "counter",
            Metric::Gauge(_) | Metric::GaugeFunc(_) | Metric::GaugeVec(_) => "gauge",
            Metric::Histogram(_) | Metric::HistogramVec(_) => "histogram",
            Metric::Summary(_) | Metric::SummaryVec(_) => "summary",
        }
    }
}

/// One sample of a custom collector.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

/// A family produced by a custom collector.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub name: String,
    pub help: String,
    /// "counter", "gauge" or "untyped".
    pub kind: &'static str,
    pub samples: Vec<Sample>,
}

/// A custom collector, like a Go prometheus.Collector.
pub trait Collector: Send + Sync {
    fn collect(&self) -> Vec<Family>;
}

/// A custom collector with the prefix of its families.
type PrefixedCollector = (String, Arc<dyn Collector>);

struct Entry {
    help: String,
    metric: Metric,
}

/// A metrics registry.
#[derive(Clone, Default)]
pub struct Registry {
    entries: Arc<RwLock<BTreeMap<String, Entry>>>,
    collectors: Arc<RwLock<Vec<PrefixedCollector>>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").finish_non_exhaustive()
    }
}

macro_rules! register {
    ($self:ident, $name:expr, $help:expr, $variant:ident, $make:expr) => {{
        let name = $name.to_string();
        if let Some(e) = $self.entries.read().unwrap().get(&name) {
            match &e.metric {
                Metric::$variant(m) => return m.clone(),
                _ => panic!("metric {name} already registered with another type"),
            }
        }
        let mut entries = $self.entries.write().unwrap();
        let entry = entries
            .entry(name.clone())
            .or_insert_with(|| Entry { help: $help.to_string(), metric: Metric::$variant($make) });
        match &entry.metric {
            Metric::$variant(m) => m.clone(),
            _ => panic!("metric {name} already registered with another type"),
        }
    }};
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn counter(&self, name: &str, help: &str) -> Counter {
        register!(self, name, help, Counter, Counter::default())
    }

    pub fn counter_vec(&self, name: &str, help: &str, labels: &[&str]) -> CounterVec {
        register!(self, name, help, CounterVec, Vector::new(labels, Counter::default))
    }

    pub fn counter_func(&self, name: &str, help: &str, f: impl Fn() -> f64 + Send + Sync + 'static) {
        let f: ValueFn = Arc::new(f);
        let _: ValueFn = (|| register!(self, name, help, CounterFunc, f))();
    }

    pub fn gauge(&self, name: &str, help: &str) -> Gauge {
        register!(self, name, help, Gauge, Gauge::default())
    }

    pub fn gauge_vec(&self, name: &str, help: &str, labels: &[&str]) -> GaugeVec {
        register!(self, name, help, GaugeVec, Vector::new(labels, Gauge::default))
    }

    pub fn gauge_func(&self, name: &str, help: &str, f: impl Fn() -> f64 + Send + Sync + 'static) {
        let f: ValueFn = Arc::new(f);
        let _: ValueFn = (|| register!(self, name, help, GaugeFunc, f))();
    }

    pub fn histogram(&self, name: &str, help: &str, buckets: &[f64]) -> Histogram {
        register!(self, name, help, Histogram, Histogram::new(buckets))
    }

    pub fn histogram_vec(&self, name: &str, help: &str, buckets: &[f64], labels: &[&str]) -> HistogramVec {
        let buckets = buckets.to_vec();
        register!(self, name, help, HistogramVec, Vector::new(labels, move || Histogram::new(&buckets)))
    }

    pub fn summary(&self, name: &str, help: &str, quantiles: &[f64], max_age: Duration) -> Summary {
        register!(self, name, help, Summary, Summary::new(quantiles, max_age))
    }

    pub fn summary_vec(
        &self,
        name: &str,
        help: &str,
        quantiles: &[f64],
        max_age: Duration,
        labels: &[&str],
    ) -> SummaryVec {
        let quantiles = quantiles.to_vec();
        register!(self, name, help, SummaryVec, Vector::new(labels, move || Summary::new(&quantiles, max_age)))
    }

    /// Register a custom collector. The prefix is prepended to the names of
    /// the families it produces.
    pub fn register_collector(&self, prefix: &str, c: Arc<dyn Collector>) {
        self.collectors.write().unwrap().push((prefix.to_string(), c));
    }

    /// Unregister a custom collector.
    pub fn unregister_collector(&self, c: &Arc<dyn Collector>) {
        self.collectors.write().unwrap().retain(|(_, x)| !Arc::ptr_eq(x, c));
    }

    /// Remove a metric.
    pub fn unregister(&self, name: &str) {
        self.entries.write().unwrap().remove(name);
    }

    /// Render all metrics in the Prometheus text format.
    pub fn render(&self) -> String {
        let mut families: BTreeMap<String, String> = BTreeMap::new();
        for (name, entry) in self.entries.read().unwrap().iter() {
            let mut out = String::new();
            render_entry(&mut out, name, entry);
            families.insert(name.clone(), out);
        }
        for (prefix, c) in self.collectors.read().unwrap().iter() {
            for family in c.collect() {
                let name = format!("{prefix}{}", family.name);
                let mut out = String::new();
                let _ = writeln!(out, "# HELP {name} {}", escape_help(&family.help));
                let _ = writeln!(out, "# TYPE {name} {}", family.kind);
                for s in &family.samples {
                    let labels: Vec<(&str, &str)> = s.labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                    write_sample(&mut out, &name, &labels, s.value);
                }
                families.insert(name, out);
            }
        }
        families.into_values().collect()
    }
}

fn escape_help(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('"', "\\\"")
}

fn format_value(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 { "+Inf".into() } else { "-Inf".into() }
    } else {
        format!("{v}")
    }
}

fn write_sample(out: &mut String, name: &str, labels: &[(&str, &str)], value: f64) {
    out.push_str(name);
    if !labels.is_empty() {
        out.push('{');
        for (i, (k, v)) in labels.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "{k}=\"{}\"", escape_label(v));
        }
        out.push('}');
    }
    let _ = writeln!(out, " {}", format_value(value));
}

fn render_histogram(out: &mut String, name: &str, labels: &[(&str, &str)], h: &Histogram) {
    let mut cumulative = 0;
    for (i, b) in h.0.buckets.iter().enumerate() {
        cumulative += h.0.counts[i].load(Ordering::Relaxed);
        let le = format_value(*b);
        let mut l = labels.to_vec();
        l.push(("le", &le));
        write_sample(out, &format!("{name}_bucket"), &l, cumulative as f64);
    }
    let count = h.0.count.load(Ordering::Relaxed);
    let mut l = labels.to_vec();
    l.push(("le", "+Inf"));
    write_sample(out, &format!("{name}_bucket"), &l, count as f64);
    write_sample(out, &format!("{name}_sum"), labels, h.0.sum.get());
    write_sample(out, &format!("{name}_count"), labels, count as f64);
}

fn render_summary(out: &mut String, name: &str, labels: &[(&str, &str)], s: &Summary) {
    for (q, v) in s.quantiles() {
        let q = format_value(q);
        let mut l = labels.to_vec();
        l.push(("quantile", &q));
        write_sample(out, name, &l, v);
    }
    write_sample(out, &format!("{name}_sum"), labels, s.0.sum.get());
    write_sample(out, &format!("{name}_count"), labels, s.0.count.load(Ordering::Relaxed) as f64);
}

fn render_vec<T>(out: &mut String, _name: &str, v: &Vector<T>, mut f: impl FnMut(&mut String, &[(&str, &str)], &T)) {
    for (values, m) in v.children.read().unwrap().iter() {
        let mut labels: Vec<(&str, &str)> =
            v.labels.iter().map(|s| s.as_str()).zip(values.iter().map(|s| s.as_str())).collect();
        // Like the Go client, labels are sorted by name.
        labels.sort_by(|a, b| a.0.cmp(b.0));
        f(out, &labels, m);
    }
}

fn render_entry(out: &mut String, name: &str, entry: &Entry) {
    let _ = writeln!(out, "# HELP {name} {}", escape_help(&entry.help));
    let _ = writeln!(out, "# TYPE {name} {}", entry.metric.type_name());
    match &entry.metric {
        Metric::Counter(c) => write_sample(out, name, &[], c.get()),
        Metric::Gauge(g) => write_sample(out, name, &[], g.get()),
        Metric::CounterFunc(f) | Metric::GaugeFunc(f) => write_sample(out, name, &[], f()),
        Metric::CounterVec(v) => render_vec(out, name, v, |o, l, c| write_sample(o, name, l, c.get())),
        Metric::GaugeVec(v) => render_vec(out, name, v, |o, l, g| write_sample(o, name, l, g.get())),
        Metric::Histogram(h) => render_histogram(out, name, &[], h),
        Metric::HistogramVec(v) => render_vec(out, name, v, |o, l, h| render_histogram(o, name, l, h)),
        Metric::Summary(s) => render_summary(out, name, &[], s),
        Metric::SummaryVec(v) => render_vec(out, name, v, |o, l, s| render_summary(o, name, l, s)),
    }
}

/// Collector for process metrics (process_*), reading /proc on Linux.
pub struct ProcessCollector;

impl Collector for ProcessCollector {
    fn collect(&self) -> Vec<Family> {
        let mut families = Vec::new();
        let gauge = |name: &str, help: &str, value: f64, kind: &'static str| Family {
            name: name.into(),
            help: help.into(),
            kind,
            samples: vec![Sample { labels: vec![], value }],
        };
        #[cfg(target_os = "linux")]
        {
            let ticks = 100.0; // USER_HZ, constant on Linux
            let page = 4096.0;
            if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
                // Fields after the command name, which is in parentheses.
                if let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) {
                    let f: Vec<&str> = rest.split_whitespace().collect();
                    let num = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                    // f[0] is field 3 (state); utime is field 14, stime 15,
                    // starttime 22, vsize 23, rss 24.
                    let cpu = (num(11) + num(12)) / ticks;
                    families.push(gauge(
                        "process_cpu_seconds_total",
                        "Total user and system CPU time spent in seconds.",
                        cpu,
                        "counter",
                    ));
                    families.push(gauge(
                        "process_virtual_memory_bytes",
                        "Virtual memory size in bytes.",
                        num(20),
                        "gauge",
                    ));
                    families.push(gauge(
                        "process_resident_memory_bytes",
                        "Resident memory size in bytes.",
                        num(21) * page,
                        "gauge",
                    ));
                    if let Ok(boot) = boot_time() {
                        families.push(gauge(
                            "process_start_time_seconds",
                            "Start time of the process since unix epoch in seconds.",
                            boot + num(19) / ticks,
                            "gauge",
                        ));
                    }
                }
            }
            if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
                families.push(gauge(
                    "process_open_fds",
                    "Number of open file descriptors.",
                    fds.count() as f64,
                    "gauge",
                ));
            }
            if let Ok(limits) = std::fs::read_to_string("/proc/self/limits") {
                for line in limits.lines() {
                    if line.starts_with("Max open files")
                        && let Some(v) = line.split_whitespace().nth(3).and_then(|s| s.parse::<f64>().ok())
                    {
                        families.push(gauge("process_max_fds", "Maximum number of open file descriptors.", v, "gauge"));
                    }
                }
            }
        }
        families
    }
}

#[cfg(target_os = "linux")]
fn boot_time() -> Result<f64, ()> {
    let stat = std::fs::read_to_string("/proc/stat").map_err(|_| ())?;
    stat.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse().ok()).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render() {
        let r = Registry::new();
        let c = r.counter("akvorado_test_packets_total", "Packets.");
        c.inc();
        c.inc_by(2.0);
        // Same name returns the same counter.
        r.counter("akvorado_test_packets_total", "Packets.").inc();
        let v = r.gauge_vec("akvorado_test_queue", "Queue size.", &["worker"]);
        v.with_label_values(&["2"]).set(5.0);
        v.with_label_values(&["1"]).set(1.5);
        r.gauge_func("akvorado_test_uptime", "Up.", || 42.0);
        let h = r.histogram("akvorado_test_latency", "Latency.", &[0.1, 1.0]);
        h.observe(0.05);
        h.observe(0.5);
        h.observe(5.0);
        let got = r.render();
        let expected = "# HELP akvorado_test_latency Latency.
# TYPE akvorado_test_latency histogram
akvorado_test_latency_bucket{le=\"0.1\"} 1
akvorado_test_latency_bucket{le=\"1\"} 2
akvorado_test_latency_bucket{le=\"+Inf\"} 3
akvorado_test_latency_sum 5.55
akvorado_test_latency_count 3
# HELP akvorado_test_packets_total Packets.
# TYPE akvorado_test_packets_total counter
akvorado_test_packets_total 4
# HELP akvorado_test_queue Queue size.
# TYPE akvorado_test_queue gauge
akvorado_test_queue{worker=\"1\"} 1.5
akvorado_test_queue{worker=\"2\"} 5
# HELP akvorado_test_uptime Up.
# TYPE akvorado_test_uptime gauge
akvorado_test_uptime 42
";
        assert_eq!(got, expected);
    }

    #[test]
    fn summary() {
        let s = Summary::new(&[0.5, 0.9, 0.99], Duration::from_secs(600));
        for i in 1..=10000 {
            s.observe(i as f64);
        }
        let q = s.quantiles();
        assert_eq!(q.len(), 3);
        // Approximate quantiles from a 1024-sample reservoir.
        assert!((q[0].1 - 5000.0).abs() < 600.0, "{q:?}");
        assert!((q[1].1 - 9000.0).abs() < 400.0, "{q:?}");
        assert!(q[2].1 > 9600.0, "{q:?}");
        assert_eq!(s.0.count.load(Ordering::Relaxed), 10000);
    }

    #[test]
    #[should_panic]
    fn type_conflict() {
        let r = Registry::new();
        r.counter("x", "x");
        r.gauge("x", "x");
    }

    #[test]
    fn process() {
        let families = ProcessCollector.collect();
        if cfg!(target_os = "linux") {
            assert!(families.iter().any(|f| f.name == "process_resident_memory_bytes"));
        }
    }
}
