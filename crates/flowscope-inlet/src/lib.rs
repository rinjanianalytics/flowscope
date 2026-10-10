// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Inlet service: receive flows from exporters and push them, undecoded, to
//! Kafka. Port of `inlet/`.

pub mod config;
#[cfg(target_os = "linux")]
mod ebpf;
mod file;
pub mod kafka;
mod udp;

use config::{FlowConfiguration, InputConfiguration, InputType, KafkaConfiguration};
use flowscope_common::daemon::Daemon;
use flowscope_common::reporter::Reporter;
use flowscope_pb::RawFlow;
use kafka::KafkaProducer;
use prost::Message;
use std::sync::Arc;
use std::time::Duration;

pub use file::FileInput;
pub use udp::UdpInput;

/// Function called by inputs for each received packet. The input fills the
/// time, payload and source address; the rest comes from the input
/// configuration.
pub type SendFn = Arc<dyn Fn(&str, &mut RawFlow) + Send + Sync>;

/// Build the send function for an input: complete the message with the input
/// settings, encode it and give it to the producer.
pub fn send_function(input: &InputConfiguration, producer: KafkaProducer) -> SendFn {
    let decoder = input.decoder as i32;
    let timestamp_source = input.timestamp_source as i32;
    let use_source_address = input.use_src_addr_for_exporter_addr;
    let decapsulation = input.decapsulation_protocol as i32;
    let rate_limit = input.rate_limit;
    Arc::new(move |exporter: &str, flow: &mut RawFlow| {
        flow.decoder = decoder;
        flow.timestamp_source = timestamp_source;
        flow.use_source_address = use_source_address;
        flow.decapsulation_protocol = decapsulation;
        flow.rate_limit = rate_limit;
        let buf = flow.encode_to_vec();
        producer.send(exporter, &buf);
    })
}

enum RunningInput {
    Udp(UdpInput),
    File(FileInput),
}

/// The inlet flow component.
pub struct Inlet {
    producer: KafkaProducer,
    inputs: Vec<RunningInput>,
}

impl Inlet {
    /// Create the Kafka producer and start the inputs.
    pub fn start(r: &Reporter, flow: &FlowConfiguration, kafka: &KafkaConfiguration) -> Result<Inlet, String> {
        if flow.inputs.is_empty() {
            return Err("no input configured".into());
        }
        tracing::info!("starting Kafka component");
        let producer = KafkaProducer::new(r, kafka)?;
        let mut inputs = Vec::new();
        for input in &flow.inputs {
            let send = send_function(input, producer.clone());
            let running = match &input.config {
                InputType::Udp(c) => RunningInput::Udp(UdpInput::start(r, c, send)?),
                InputType::File(c) => RunningInput::File(FileInput::start(c, send)?),
            };
            inputs.push(running);
        }
        Ok(Inlet { producer, inputs })
    }

    /// Run until the daemon terminates, then stop the inputs and flush Kafka.
    pub async fn run(self, daemon: Daemon) -> Result<(), String> {
        let token = daemon.token();
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    // A file input stops by itself on error.
                    if self.inputs.iter().any(|i| matches!(i, RunningInput::File(f) if f.is_finished())) {
                        break;
                    }
                }
            }
        }
        tracing::info!("stopping flow component");
        let producer = self.producer.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut result = Ok(());
            for input in self.inputs {
                match input {
                    RunningInput::Udp(u) => u.stop(),
                    RunningInput::File(f) => {
                        if let Err(e) = f.stop() {
                            result = Err(e);
                        }
                    }
                }
            }
            producer.flush(Duration::from_secs(10));
            result
        })
        .await
        .map_err(|e| e.to_string())?;
        tracing::info!("flow component stopped");
        result
    }
}
