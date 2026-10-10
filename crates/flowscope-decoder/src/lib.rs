// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Flow decoders: NetFlow v5, NetFlow v9, IPFIX and sFlow. Port of
//! `outlet/flow`.
//!
//! The protocol parsers are written for FlowScope (the Go version relies on
//! goflow2) and produce the same flows as the Go version for the same input.

pub mod constants;
pub mod helpers;
pub mod netflow;
pub mod sflow;

use flowscope_common::reporter::{CounterVec, Reporter};
use flowscope_pb::{DecapsulationProtocol, Decoder as DecoderType, RawFlow, TimestampSource};
use flowscope_schema::FlowMessage;
use serde::{Deserialize, Serialize};
use std::net::Ipv6Addr;
use std::time::{Duration, Instant};

/// Decoder input.
#[derive(Clone, Debug)]
pub struct Input<'a> {
    /// When the packet was received (seconds since epoch).
    pub time_received: u64,
    pub payload: &'a [u8],
    /// Source address of the packet.
    pub source: Ipv6Addr,
}

/// Decoder options, from the inlet input configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Options {
    pub timestamp_source: TimestampSource,
    pub decapsulation_protocol: DecapsulationProtocol,
}

/// Function called for each decoded flow. It should finalize the flow.
pub type Finalize<'a> = dyn FnMut(&mut FlowMessage) + 'a;

/// A flow decoder.
pub trait Decoder: Send + Sync {
    /// Decode a packet. Returns the number of flows.
    fn decode(
        &self,
        input: &Input,
        options: &Options,
        bf: &mut FlowMessage,
        finalize: &mut Finalize,
    ) -> Result<usize, String>;
    fn name(&self) -> &'static str;
}

/// Exporter key: the source address formatted like Go's netip.
pub fn exporter_key(ip: &Ipv6Addr) -> String {
    flowscope_schema::format_ip(ip)
}

/// Log at most `burst` messages per period.
pub(crate) struct Sampler {
    state: std::sync::Mutex<(Instant, u32)>,
    period: Duration,
    burst: u32,
}

impl Sampler {
    pub(crate) fn new(period: Duration, burst: u32) -> Self {
        Sampler { state: std::sync::Mutex::new((Instant::now(), 0)), period, burst }
    }

    pub(crate) fn allow(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.0.elapsed() > self.period {
            *s = (Instant::now(), 0);
        }
        s.1 += 1;
        s.1 <= self.burst
    }
}

/// Configuration of the flow component.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct Configuration {
    /// File to save the decoders' state (templates) on stop.
    pub state_persist_file: String,
}

/// Current version of the state file.
const STATE_VERSION: u64 = 1;

/// The flow component: dispatch raw flows to the decoders.
pub struct Flow {
    netflow: netflow::NetflowDecoder,
    sflow: sflow::SflowDecoder,
    decoder_stats: CounterVec,
    decoder_errors: CounterVec,
    errors: Sampler,
}

impl Flow {
    pub fn new(r: &Reporter) -> Self {
        let m = r.metrics("outlet/flow");
        Flow {
            netflow: netflow::NetflowDecoder::new(r),
            sflow: sflow::SflowDecoder::new(r),
            decoder_stats: m.counter_vec("decoder_flows_total", "Decoder processed count.", &["name"]),
            decoder_errors: m.counter_vec("decoder_errors_total", "Decoder processed error count.", &["name"]),
            errors: Sampler::new(Duration::from_secs(30), 3),
        }
    }

    /// Decode a raw flow. `finalize` is called for each flow.
    pub fn decode(&self, raw: &RawFlow, bf: &mut FlowMessage, finalize: &mut Finalize) -> Result<(), String> {
        let decoder: &dyn Decoder = match DecoderType::try_from(raw.decoder) {
            Ok(DecoderType::Netflow) => &self.netflow,
            Ok(DecoderType::Sflow) => &self.sflow,
            _ => return Err(format!("decoder {} not available", raw.decoder)),
        };
        let source: [u8; 16] = match raw.source_address.len() {
            16 => raw.source_address.as_slice().try_into().unwrap(),
            4 => std::net::Ipv4Addr::new(
                raw.source_address[0],
                raw.source_address[1],
                raw.source_address[2],
                raw.source_address[3],
            )
            .to_ipv6_mapped()
            .octets(),
            _ => return Err("missing source address".into()),
        };
        let source = Ipv6Addr::from(source);
        let input = Input { time_received: raw.time_received, payload: &raw.payload, source };
        let options = Options {
            timestamp_source: TimestampSource::try_from(raw.timestamp_source).unwrap_or_default(),
            decapsulation_protocol: DecapsulationProtocol::try_from(raw.decapsulation_protocol).unwrap_or_default(),
        };
        let use_source = raw.use_source_address;
        let mut wrapped = |bf: &mut FlowMessage| {
            if use_source {
                bf.exporter_address = Some(source);
            }
            finalize(bf);
        };
        // A decoder bug should not take the outlet down.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decoder.decode(&input, &options, bf, &mut wrapped)
        }));
        match result {
            Ok(Ok(n)) => {
                self.decoder_stats.with_label_values(&[decoder.name()]).inc_by(n as f64);
                Ok(())
            }
            Ok(Err(e)) => {
                self.decoder_errors.with_label_values(&[decoder.name()]).inc();
                Err(format!("failed to decode flow: {e}"))
            }
            Err(panic) => {
                self.decoder_errors.with_label_values(&[decoder.name()]).inc();
                if self.errors.allow() {
                    let msg = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    tracing::error!(decoder = decoder.name(), panic = msg, "panic while decoding");
                }
                bf.undo();
                Ok(())
            }
        }
    }

    /// Save the decoders' state.
    pub fn save_state(&self, target: &str) -> Result<(), String> {
        let state = serde_json::json!({
            "Version": STATE_VERSION,
            "Decoders": {
                "netflow": self.netflow.state(),
                "sflow": {},
            },
        });
        let data = serde_json::to_vec(&state).map_err(|e| format!("unable to encode decoders' state: {e}"))?;
        let path = std::path::Path::new(target);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)
            .and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| format!("unable to write state file {target:?}: {e}"))
    }

    /// Restore the decoders' state.
    pub fn restore_state(&self, source: &str) -> Result<(), String> {
        let data = std::fs::read(source).map_err(|e| format!("unable to read state file {source:?}: {e}"))?;
        let state: serde_json::Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        if state.get("Version").and_then(|v| v.as_u64()) != Some(STATE_VERSION) {
            return Err("collection version mismatch".into());
        }
        if let Some(nf) = state.pointer("/Decoders/netflow") {
            self.netflow.restore(nf).map_err(|e| format!("unable to decode decoder' state (netflow): {e}"))?;
        }
        Ok(())
    }
}
