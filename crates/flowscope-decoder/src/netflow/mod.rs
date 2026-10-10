// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! NetFlow v5, NetFlow v9 and IPFIX decoder. Port of
//! `outlet/flow/decoder/netflow`.

mod decode;
pub mod fields;
pub mod packet;
pub mod templates;

use crate::{Decoder, Finalize, Input, Options, Sampler, exporter_key};
use flowscope_common::reporter::{CounterVec, Reporter};
use flowscope_pb::{DecapsulationProtocol, TimestampSource};
use flowscope_schema::FlowMessage;
use packet::FlowSet;
use std::time::Duration;
use templates::Collection;

pub(crate) struct Metrics {
    pub errors: CounterVec,
    pub packets: CounterVec,
    pub records: CounterVec,
    pub sets: CounterVec,
}

/// The NetFlow decoder.
pub struct NetflowDecoder {
    pub(crate) metrics: Metrics,
    collection: Collection,
    error_log: Sampler,
}

impl NetflowDecoder {
    pub fn new(r: &Reporter) -> Self {
        let m = r.metrics("outlet/flow/decoder/netflow");
        let templates = m.counter_vec(
            "templates_total",
            "Number of NetFlow templates received.",
            &["exporter", "version", "obs_domain_id", "template_id", "type"],
        );
        NetflowDecoder {
            metrics: Metrics {
                errors: m.counter_vec("errors_total", "Number of NetFlow errors processed.", &["exporter", "error"]),
                packets: m.counter_vec(
                    "packets_total",
                    "Number of NetFlow packets received.",
                    &["exporter", "version"],
                ),
                sets: m.counter_vec(
                    "sets_total",
                    "Number of NetFlow flowsets received.",
                    &["exporter", "version", "type"],
                ),
                records: m.counter_vec(
                    "records_total",
                    "Number of NetFlow records received.",
                    &["exporter", "version", "type"],
                ),
            },
            collection: Collection::new(templates),
            error_log: Sampler::new(Duration::from_secs(30), 3),
        }
    }

    /// Templates and sampling rates, for persistence.
    pub fn state(&self) -> serde_json::Value {
        self.collection.to_json()
    }

    /// Restore templates and sampling rates.
    pub fn restore(&self, state: &serde_json::Value) -> Result<(), String> {
        self.collection.restore_json(state)
    }

    /// Access to the per-exporter state, for tests.
    pub fn exporter(&self, key: &str) -> std::sync::Arc<templates::Exporter> {
        self.collection.get(key)
    }
}

impl Decoder for NetflowDecoder {
    fn name(&self) -> &'static str {
        "netflow"
    }

    fn decode(
        &self,
        input: &Input,
        options: &Options,
        bf: &mut FlowMessage,
        finalize: &mut Finalize,
    ) -> Result<usize, String> {
        if input.payload.len() < 2 {
            return Err("payload too small".into());
        }
        let key = exporter_key(&input.source);
        let exporter = self.collection.get(&key);
        let version = u16::from_be_bytes([input.payload[0], input.payload[1]]);
        let mut ts = input.time_received;
        let mut sys_uptime = 0u64;
        let source = input.source;
        let ts_from_packet =
            matches!(options.timestamp_source, TimestampSource::NetflowPacket | TimestampSource::NetflowFirstSwitched);

        let version_str;
        let flow_sets: Vec<FlowSet>;
        match version {
            5 => {
                if options.decapsulation_protocol != DecapsulationProtocol::None {
                    self.metrics.errors.with_label_values(&[&key, "non-encapsulated packet"]).inc();
                    return Ok(0);
                }
                let packet = match packet::parse_v5(&input.payload[2..]) {
                    Ok(p) => p,
                    Err(e) => {
                        self.metrics.errors.with_label_values(&[&key, "NetFlow v5 decoding error"]).inc();
                        if self.error_log.allow() {
                            tracing::error!(error = %e, exporter = key, "error while decoding NetFlow v5");
                        }
                        return Err(format!("NetFlow v5 decoding error: {e}"));
                    }
                };
                version_str = "5";
                self.metrics.sets.with_label_values(&[&key, version_str, "PDU"]).inc();
                self.metrics.records.with_label_values(&[&key, version_str, "PDU"]).inc_by(packet.records.len() as f64);
                if ts_from_packet {
                    ts = packet.unix_secs as u64;
                    sys_uptime = packet.sys_uptime as u64;
                }
                let mut f = |bf: &mut FlowMessage| {
                    if bf.time_received == 0 {
                        bf.time_received = ts as u32;
                    }
                    bf.exporter_address = Some(source);
                    finalize(bf);
                };
                decode::decode_v5(&packet, ts, sys_uptime, options, bf, &mut f);
                self.metrics.packets.with_label_values(&[&key, version_str]).inc();
                return Ok(0);
            }
            9 | 10 => {
                let (packet, _missing) = match packet::parse(input.payload, exporter.as_ref()) {
                    Ok(p) => p,
                    Err(e) => {
                        let what = if version == 9 { "NetFlow v9" } else { "IPFIX" };
                        if self.error_log.allow() {
                            tracing::error!(error = %e, exporter = key, "error while decoding {what}");
                        }
                        self.metrics.errors.with_label_values(&[&key, &format!("{what} decoding error")]).inc();
                        return Err(format!("{what} decoding error: {e}"));
                    }
                };
                version_str = if version == 9 { "9" } else { "10" };
                if version == 9 && ts_from_packet {
                    ts = packet.header.unix_seconds as u64;
                    sys_uptime = packet.header.system_uptime as u64;
                }
                if version == 10 && options.timestamp_source == TimestampSource::NetflowPacket {
                    ts = packet.header.unix_seconds as u64;
                }
                let mut f = |bf: &mut FlowMessage| {
                    if bf.time_received == 0 {
                        bf.time_received = ts as u32;
                    }
                    bf.exporter_address = Some(source);
                    finalize(bf);
                };
                decode::decode_v9_ipfix(
                    self,
                    version,
                    packet.header.obs_domain_id,
                    &packet.flow_sets,
                    &exporter,
                    ts,
                    sys_uptime,
                    options,
                    &key,
                    bf,
                    &mut f,
                );
                flow_sets = packet.flow_sets;
            }
            _ => {
                if self.error_log.allow() {
                    tracing::warn!(exporter = key, "unknown NetFlow version {version}");
                }
                self.metrics.packets.with_label_values(&[&key, "unknown"]).inc();
                return Err("unkown NetFlow version".into());
            }
        }
        self.metrics.packets.with_label_values(&[&key, version_str]).inc();

        let mut nb = 0;
        for fs in &flow_sets {
            let (kind, count) = match fs {
                FlowSet::Template(r) => ("TemplateFlowSet", r.len()),
                FlowSet::IpfixOptionsTemplate(r) => ("OptionsTemplateFlowSet", r.len()),
                FlowSet::Nfv9OptionsTemplate(r) => ("OptionsTemplateFlowSet", r.len()),
                FlowSet::OptionsData(r) => ("OptionsDataFlowSet", r.len()),
                FlowSet::Data(r) => {
                    nb += r.len();
                    ("DataFlowSet", r.len())
                }
                FlowSet::Raw => continue,
            };
            self.metrics.sets.with_label_values(&[&key, version_str, kind]).inc();
            self.metrics.records.with_label_values(&[&key, version_str, kind]).inc_by(count as f64);
        }
        Ok(nb)
    }
}
