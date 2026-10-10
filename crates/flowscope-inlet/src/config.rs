// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Inlet configuration. Port of `inlet/flow/config.go`,
//! `inlet/flow/input/*/config.go` and `inlet/kafka/config.go`.

use flowscope_common::config::{migrate, parametrized};
use flowscope_common::helpers::{TlsConfiguration, validate_listen};
use flowscope_common::migrated;
use flowscope_kafka::{CompressionCodec, LoadBalanceAlgorithm, SaslConfiguration};
use flowscope_pb::{DecapsulationProtocol, Decoder, TimestampSource};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Flow component configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FlowConfiguration {
    /// Inputs to receive flows from.
    pub inputs: Vec<InputConfiguration>,
}

impl Default for FlowConfiguration {
    fn default() -> Self {
        FlowConfiguration {
            inputs: vec![
                InputConfiguration { decoder: Decoder::Netflow, ..Default::default() },
                InputConfiguration { decoder: Decoder::Sflow, ..Default::default() },
            ],
        }
    }
}

impl FlowConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        for (i, input) in self.inputs.iter().enumerate() {
            input.validate().map_err(|e| format!("flow.inputs[{i}]: {e}"))?;
        }
        Ok(())
    }
}

/// An input. The type-specific fields (`listen` for UDP) sit next to the
/// common ones.
#[derive(Clone, Debug, PartialEq)]
pub struct InputConfiguration {
    /// Decoder to use.
    pub decoder: Decoder,
    /// Use the source address of the packet as the exporter address.
    pub use_src_addr_for_exporter_addr: bool,
    /// Where to get the timestamp from.
    pub timestamp_source: TimestampSource,
    /// Decapsulate flows with this protocol.
    pub decapsulation_protocol: DecapsulationProtocol,
    /// Per-exporter rate limit (flows/s). 0 means no limit.
    pub rate_limit: u64,
    /// Type-specific configuration.
    pub config: InputType,
}

impl Default for InputConfiguration {
    fn default() -> Self {
        InputConfiguration {
            decoder: Decoder::Unspecified,
            use_src_addr_for_exporter_addr: false,
            timestamp_source: TimestampSource::Input,
            decapsulation_protocol: DecapsulationProtocol::None,
            rate_limit: 0,
            config: InputType::Udp(UdpConfiguration::default()),
        }
    }
}

const OUTER_FIELDS: &[&str] =
    &["decoder", "use-src-addr-for-exporter-addr", "timestamp-source", "decapsulation-protocol", "rate-limit"];

/// The common fields of an input, for decoding.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct OuterFields {
    decoder: Decoder,
    use_src_addr_for_exporter_addr: bool,
    timestamp_source: TimestampSource,
    decapsulation_protocol: DecapsulationProtocol,
    rate_limit: u64,
}

impl Default for OuterFields {
    fn default() -> Self {
        let d = InputConfiguration::default();
        OuterFields {
            decoder: d.decoder,
            use_src_addr_for_exporter_addr: d.use_src_addr_for_exporter_addr,
            timestamp_source: d.timestamp_source,
            decapsulation_protocol: d.decapsulation_protocol,
            rate_limit: d.rate_limit,
        }
    }
}

impl Serialize for InputConfiguration {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = match serde_json::to_value(&self.config).map_err(serde::ser::Error::custom)? {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        // An unspecified decoder has no name: leave it out.
        if self.decoder != Decoder::Unspecified {
            map.insert("decoder".into(), Value::String(self.decoder.to_string()));
        }
        map.insert("use-src-addr-for-exporter-addr".into(), Value::Bool(self.use_src_addr_for_exporter_addr));
        map.insert("timestamp-source".into(), Value::String(self.timestamp_source.to_string()));
        map.insert("decapsulation-protocol".into(), Value::String(self.decapsulation_protocol.to_string()));
        map.insert("rate-limit".into(), Value::from(self.rate_limit));
        map.serialize(s)
    }
}

impl<'de> Deserialize<'de> for InputConfiguration {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut raw = Value::deserialize(d)?;
        let (t, rest) = parametrized::take_type(&mut raw).map_err(serde::de::Error::custom)?;
        let Value::Object(rest) = rest else { unreachable!() };
        let (outer, inner) = parametrized::split_outer(rest, OUTER_FIELDS);
        let outer: OuterFields =
            flowscope_common::config::from_value(Value::Object(outer)).map_err(serde::de::Error::custom)?;
        let config = InputType::from_type(&t, Value::Object(inner)).map_err(serde::de::Error::custom)?;
        Ok(InputConfiguration {
            decoder: outer.decoder,
            use_src_addr_for_exporter_addr: outer.use_src_addr_for_exporter_addr,
            timestamp_source: outer.timestamp_source,
            decapsulation_protocol: outer.decapsulation_protocol,
            rate_limit: outer.rate_limit,
            config,
        })
    }
}

impl InputConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if self.decoder == Decoder::Unspecified {
            return Err("decoder is required".into());
        }
        match &self.config {
            InputType::Udp(c) => c.validate(),
            InputType::File(c) => c.validate(),
        }
    }
}

/// Type of input.
#[derive(Clone, Debug, PartialEq)]
pub enum InputType {
    Udp(UdpConfiguration),
    File(FileConfiguration),
}

flowscope_common::parametrized!(InputType {
    "udp" => Udp(UdpConfiguration),
    "file" => File(FileConfiguration),
});

/// UDP input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct UdpConfiguration {
    /// Address to listen to.
    pub listen: String,
    /// Number of workers, each with its own socket.
    pub workers: usize,
    /// Size of the receive buffer of each socket. 0 keeps the system default.
    pub receive_buffer: u64,
}

impl Default for UdpConfiguration {
    fn default() -> Self {
        UdpConfiguration { listen: ":0".into(), workers: 1, receive_buffer: 0 }
    }
}

fn migrate_udp(map: &mut Map<String, Value>) -> Result<(), String> {
    migrate::remove(map, "QueueSize");
    Ok(())
}

migrated!(UdpConfiguration, migrate_udp);

impl UdpConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if self.listen.is_empty() {
            return Err("listen is required".into());
        }
        validate_listen(&self.listen)?;
        if !(1..=256).contains(&self.workers) {
            return Err(format!("workers should be between 1 and 256, not {}", self.workers));
        }
        Ok(())
    }
}

/// File input, replaying files in a loop. For tests and demos.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct FileConfiguration {
    /// Files to read.
    pub paths: Vec<String>,
    /// Stop after this number of flows. 0 means no limit.
    pub max_flows: u64,
}

impl FileConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        if self.paths.is_empty() || self.paths.iter().any(|p| p.is_empty()) {
            return Err("paths needs at least one non-empty path".into());
        }
        Ok(())
    }
}

/// Kafka configuration for the inlet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct KafkaConfiguration {
    pub topic: String,
    pub brokers: Vec<String>,
    pub tls: TlsConfiguration,
    pub sasl: SaslConfiguration,
    /// Compression codec for produced batches.
    pub compression_codec: CompressionCodec,
    /// Maximum number of buffered records.
    pub queue_size: usize,
    /// How records are spread over partitions.
    pub load_balance: LoadBalanceAlgorithm,
}

impl Default for KafkaConfiguration {
    fn default() -> Self {
        let common = flowscope_kafka::Configuration::default();
        KafkaConfiguration {
            topic: common.topic,
            brokers: common.brokers,
            tls: common.tls,
            sasl: common.sasl,
            compression_codec: CompressionCodec::Lz4,
            queue_size: 4096,
            load_balance: LoadBalanceAlgorithm::Random,
        }
    }
}

fn migrate_kafka(map: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["FlushInterval", "FlushBytes", "MaxMessageBytes"] {
        migrate::remove(map, k);
    }
    flowscope_kafka::migrate_configuration(map)
}

migrated!(KafkaConfiguration, migrate_kafka);

impl KafkaConfiguration {
    /// The common part of the configuration.
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
            return Err("kafka.queue-size should be at least 1".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::{from_value, parse_str};
    use pretty_assertions::assert_eq;

    #[test]
    fn inputs() {
        let got: FlowConfiguration = from_value(
            parse_str(
                "inputs:
  - type: udp
    decoder: netflow
    listen: 0.0.0.0:2055
    workers: 3
    queue-size: 100000
  - type: udp
    decoder: sflow
    listen: :6343
    UseSrcAddrForExporterAddr: true
    decapsulation-protocol: vxlan
    rate-limit: 1000
  - type: file
    decoder: netflow
    paths: [a.pcap, b.pcap]
    timestamp-source: netflow-packet
",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            got.inputs,
            vec![
                InputConfiguration {
                    decoder: Decoder::Netflow,
                    config: InputType::Udp(UdpConfiguration {
                        listen: "0.0.0.0:2055".into(),
                        workers: 3,
                        receive_buffer: 0
                    }),
                    ..Default::default()
                },
                InputConfiguration {
                    decoder: Decoder::Sflow,
                    use_src_addr_for_exporter_addr: true,
                    decapsulation_protocol: DecapsulationProtocol::Vxlan,
                    rate_limit: 1000,
                    config: InputType::Udp(UdpConfiguration { listen: ":6343".into(), workers: 1, receive_buffer: 0 }),
                    ..Default::default()
                },
                InputConfiguration {
                    decoder: Decoder::Netflow,
                    timestamp_source: TimestampSource::NetflowPacket,
                    config: InputType::File(FileConfiguration {
                        paths: vec!["a.pcap".into(), "b.pcap".into()],
                        max_flows: 0
                    }),
                    ..Default::default()
                },
            ]
        );
        got.validate().unwrap();
    }

    #[test]
    fn roundtrip() {
        let config = FlowConfiguration::default();
        let dumped = flowscope_common::config::dump(&config).unwrap();
        assert!(dumped.contains("type: udp"), "{dumped}");
        let back: FlowConfiguration = from_value(parse_str(&dumped).unwrap()).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn errors() {
        assert!(from_value::<FlowConfiguration>(parse_str("inputs: [{decoder: netflow}]").unwrap()).is_err());
        assert!(
            from_value::<FlowConfiguration>(parse_str("inputs: [{type: tcp, decoder: netflow}]").unwrap()).is_err()
        );
        let c: FlowConfiguration = from_value(parse_str("inputs: [{type: udp, listen: ':2055'}]").unwrap()).unwrap();
        assert!(c.validate().is_err(), "missing decoder");
        let c: FlowConfiguration =
            from_value(parse_str("inputs: [{type: udp, decoder: netflow, workers: 0}]").unwrap()).unwrap();
        assert!(c.validate().is_err(), "no workers");
    }

    #[test]
    fn kafka() {
        let got: KafkaConfiguration = from_value(
            parse_str("topic: flows\nbrokers: [kafka:9092]\ncompression-codec: zstd\nflush-interval: 1s\nload-balance: by-exporter\n")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got.compression_codec, CompressionCodec::Zstd);
        assert_eq!(got.load_balance, LoadBalanceAlgorithm::ByExporter);
        assert_eq!(got.queue_size, 4096);
        got.validate().unwrap();
    }
}
