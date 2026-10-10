// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Worker processing incoming flows. Port of `outlet/core/worker.go`.

use super::Inner;
use crate::clickhouse::{self, WorkerStatus};
use crate::kafkainput::{Done, ReceiveWorker, ScaleRequest};
use flowscope_pb::RawFlow;
use flowscope_schema::FlowMessage;
use prost::Message;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

/// A worker: one per Kafka consumer.
pub(crate) struct Worker {
    pub(super) inner: Arc<Inner>,
    pub(super) id: usize,
    pub(super) bf: FlowMessage,
    pub(super) cw: Box<dyn clickhouse::Worker>,
    pub(super) scale: SyncSender<ScaleRequest>,
}

impl Worker {
    pub(crate) fn new(inner: Arc<Inner>, id: usize, scale: SyncSender<ScaleRequest>) -> Worker {
        let mut bf = inner.deps.schema.new_flow_message();
        // Encode enriched flows to Protobuf only when the Kafka output is
        // enabled, so the ClickHouse-only path is unaffected.
        if inner.deps.kafka_output.as_ref().is_some_and(|k| k.enabled()) {
            bf.enable_protobuf();
        }
        let cw = inner.deps.clickhouse.new_worker(id, &bf);
        Worker { inner, id, bf, cw, scale }
    }

    /// Process a decoded flow: rate limit, enrich and send. Exposed to
    /// tests, which skip the decoding.
    pub(crate) fn finalize_flow(
        inner: &Inner,
        cw: &mut dyn clickhouse::Worker,
        scale: &SyncSender<ScaleRequest>,
        bf: &mut FlowMessage,
        rate_limit: u64,
        time_received: u64,
        done: Done,
    ) {
        let Some(ip) = bf.exporter_address else {
            bf.undo();
            return;
        };
        let exporter = flowscope_common::helpers::unmap_ip(&ip);
        inner.metrics.flows_received.with_label_values(&[&exporter]).inc();

        // Rate limiting
        let mut factor = 1.0;
        if rate_limit > 0 {
            let (allowed, f) = inner.rate_limiter.allow_one_message(ip, rate_limit, time_received);
            if !allowed {
                inner.metrics.flows_rate_limited.with_label_values(&[&exporter]).inc();
                bf.undo();
                return;
            }
            factor = f;
        }

        // Enrichment
        if inner.enrich_flow(bf, ip, &exporter) {
            bf.undo();
            return;
        }

        // Account for rate limiting in the sampling rate.
        if factor > 1.0 {
            bf.sampling_rate = (bf.sampling_rate as f64 * factor) as u64;
        }

        // Copy to HTTP clients, best effort.
        if inner.http_flow_clients.load(std::sync::atomic::Ordering::Relaxed) > 0
            && let Ok(json) = serde_json::to_vec(&bf.to_json())
        {
            let _ = inner.http_flow_tx.try_send(json);
        }

        // Finalize and send to ClickHouse.
        inner.metrics.flows_forwarded.with_label_values(&[&exporter]).inc();
        let status = cw.finalize_and_send(bf, done);
        // Export the enriched flow to Kafka, best effort. The flow is
        // finalized, so the Protobuf message is complete.
        if let Some(k) = &inner.deps.kafka_output
            && k.enabled()
        {
            let payload = bf.protobuf_message();
            if !payload.is_empty() {
                k.send(&exporter, payload);
            }
        }
        let request = match status {
            WorkerStatus::Overloaded => ScaleRequest::Increase,
            WorkerStatus::Underloaded => ScaleRequest::Decrease,
            WorkerStatus::Steady => ScaleRequest::Steady,
            WorkerStatus::Idle => return,
        };
        let _ = scale.send(request);
    }
}

impl ReceiveWorker for Worker {
    fn receive(&mut self, data: &[u8], done: Done) {
        let inner = &*self.inner;
        inner.metrics.raw_flows_received.inc();
        let raw = match RawFlow::decode(data) {
            Ok(r) => r,
            Err(e) => {
                // The Go version did not count this error and stopped the
                // worker.
                inner.metrics.raw_flows_errors.with_label_values(&["cannot decode protobuf"]).inc();
                tracing::debug!(worker = self.id, error = %e, "cannot decode raw flow");
                return;
            }
        };
        let (rate_limit, time_received) = (raw.rate_limit, raw.time_received);
        let cw = &mut *self.cw;
        let scale = &self.scale;
        let mut finalize = |bf: &mut FlowMessage| {
            Worker::finalize_flow(inner, cw, scale, bf, rate_limit, time_received, done);
        };
        if inner.deps.flow.decode(&raw, &mut self.bf, &mut finalize).is_err() {
            // The exporter may not be known yet.
            inner.metrics.raw_flows_errors.with_label_values(&["cannot decode payload"]).inc();
        }
    }

    fn shutdown(&mut self) {
        tracing::info!(worker = self.id, "flush final batch to ClickHouse");
        let deadline = Instant::now() + Duration::from_secs(10);
        let done = move || Instant::now() >= deadline;
        self.cw.flush(&mut self.bf, &done);
        self.cw.close();
        tracing::info!(worker = self.id, "worker stopped");
    }
}
