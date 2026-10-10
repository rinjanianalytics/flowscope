// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Send packets to the UDP input and read them back from a real Kafka.
//! Needs `docker compose -f tests/interop/docker-compose.yml up -d`.

use flowscope_common::reporter::Reporter;
use flowscope_inlet::config::{InputConfiguration, InputType, KafkaConfiguration, UdpConfiguration};
use flowscope_inlet::kafka::KafkaProducer;
use flowscope_inlet::{UdpInput, send_function};
use flowscope_pb::{DecapsulationProtocol, Decoder, RawFlow, TimestampSource};
use prost::Message as _;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::{ClientConfig, Message};
use std::time::{Duration, Instant};

fn brokers() -> String {
    std::env::var("FLOWSCOPE_TEST_KAFKA").unwrap_or_else(|_| "localhost:19092".into())
}

#[test]
#[ignore = "needs Kafka"]
fn udp_to_kafka() {
    let r = Reporter::new_mock();
    let topic = format!("flows-test-{}", std::process::id());
    let kafka = KafkaConfiguration { topic: topic.clone(), brokers: vec![brokers()], ..Default::default() };
    let producer = KafkaProducer::new(&r, &kafka).unwrap();
    assert_eq!(producer.topic(), format!("{topic}-v5"));
    let input = InputConfiguration {
        decoder: Decoder::Sflow,
        timestamp_source: TimestampSource::NetflowFirstSwitched,
        decapsulation_protocol: DecapsulationProtocol::Gre,
        rate_limit: 500,
        use_src_addr_for_exporter_addr: true,
        config: InputType::Udp(UdpConfiguration { listen: "127.0.0.1:0".into(), workers: 2, receive_buffer: 0 }),
    };
    let InputType::Udp(udp) = &input.config else { unreachable!() };
    let running = UdpInput::start(&r, udp, send_function(&input, producer.clone())).unwrap();

    let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    for i in 0..20u8 {
        client.send_to(&[i; 100], running.local_addr()).unwrap();
    }
    std::thread::sleep(Duration::from_millis(200));
    running.stop();
    producer.flush(Duration::from_secs(10));

    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .set("group.id", format!("test-{}", std::process::id()))
        .set("auto.offset.reset", "earliest")
        .create()
        .unwrap();
    consumer.subscribe(&[&format!("{topic}-v5")]).unwrap();
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while got.len() < 20 && Instant::now() < deadline {
        if let Some(Ok(m)) = consumer.poll(Duration::from_millis(500)) {
            assert_eq!(m.key().map(|k| k.len()), Some(4), "random 4-byte key");
            got.push(RawFlow::decode(m.payload().unwrap()).unwrap());
        }
    }
    assert_eq!(got.len(), 20);
    got.sort_by_key(|f| f.payload[0]);
    for (i, f) in got.iter().enumerate() {
        assert_eq!(f.payload, vec![i as u8; 100]);
        assert_eq!(f.decoder, Decoder::Sflow as i32);
        assert_eq!(f.timestamp_source, TimestampSource::NetflowFirstSwitched as i32);
        assert_eq!(f.decapsulation_protocol, DecapsulationProtocol::Gre as i32);
        assert_eq!(f.rate_limit, 500);
        assert!(f.use_source_address);
        assert_eq!(f.source_address, std::net::Ipv4Addr::LOCALHOST.to_ipv6_mapped().octets());
    }
    let metrics = r.render_metrics();
    assert!(metrics.contains("akvorado_inlet_kafka_sent_messages_total{exporter=\"127.0.0.1\"} 20"), "{metrics}");
}
