// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end: raw flows in Kafka (as produced by the inlet) go through the
//! outlet workers and land in ClickHouse. Needs
//! `docker compose -f tests/interop/docker-compose.yml up -d`.

use flowscope_common::clickhousedb::{self, ClickHouseDb};
use flowscope_common::config::Duration;
use flowscope_common::reporter::Reporter;
use flowscope_outlet::clickhouse::{self, ClickHouse};
use flowscope_outlet::core::{Configuration, Core, Dependencies};
use flowscope_outlet::kafkainput::{self, KafkaInput};
use flowscope_outlet::metadata::{Metadata, mock::MockProvider as MetadataMock};
use flowscope_outlet::routing::{Routing, mock::MockProvider as RoutingMock};
use flowscope_schema::TableOption;
use prost::Message as _;
use rdkafka::ClientConfig;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

fn brokers() -> String {
    std::env::var("FLOWSCOPE_TEST_KAFKA").unwrap_or_else(|_| "localhost:19092".into())
}

fn clickhouse_server() -> String {
    std::env::var("FLOWSCOPE_TEST_CLICKHOUSE").unwrap_or_else(|_| "127.0.0.1:19000".into())
}

/// UDP payloads of an Ethernet/IPv4 pcap file.
fn pcap_udp_payloads(path: &Path) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap();
    let mut out = vec![];
    let mut offset = 24;
    while offset + 16 <= data.len() {
        let caplen = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
        let frame = &data[offset + 16..offset + 16 + caplen];
        offset += 16 + caplen;
        let ip = &frame[14..];
        let ihl = ((ip[0] & 0xf) as usize) * 4;
        let udp = &ip[ihl..];
        let len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
        out.push(udp[8..len].to_vec());
    }
    out
}

#[test]
#[ignore = "needs Kafka and ClickHouse"]
fn kafka_to_clickhouse() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let guard = rt.enter();
    let r = Reporter::new_mock();
    let id = std::process::id();
    let topic = format!("e2e-{id}");
    let database = format!("e2e_{id}");

    // Kafka topic
    let admin: AdminClient<_> = ClientConfig::new().set("bootstrap.servers", brokers()).create().unwrap();
    let results = rt
        .block_on(admin.create_topics(
            &[NewTopic::new(&format!("{topic}-v5"), 2, TopicReplication::Fixed(1))],
            &AdminOptions::new(),
        ))
        .unwrap();
    assert!(results.iter().all(|r| r.is_ok()), "{results:?}");

    // ClickHouse database and raw table. The real one uses the Null engine
    // and a materialized view, a MergeTree lets us read the rows back.
    let schema = Arc::new(flowscope_schema::Component::default_schema());
    let db = ClickHouseDb::new(clickhousedb::Configuration {
        servers: vec![clickhouse_server()],
        database: database.clone(),
        ..Default::default()
    })
    .unwrap();
    let table = format!("flows_{}_raw", schema.schema().clickhouse_hash());
    {
        let mut admin_options = db.options().0;
        admin_options.database = "default".into();
        let mut conn = flowscope_clickhouse::Connection::connect(&admin_options).unwrap();
        conn.execute(&format!("CREATE DATABASE {database}"), &[]).unwrap();
        let columns = schema
            .schema()
            .clickhouse_create_table(&[TableOption::SkipGeneratedColumns, TableOption::SkipAliasedColumns]);
        conn.execute(
            &format!("CREATE TABLE {database}.{table} ({columns}) ENGINE = MergeTree ORDER BY tuple()"),
            &[flowscope_clickhouse::Setting::new("allow_suspicious_low_cardinality_types", 1)],
        )
        .unwrap();
    }

    // The outlet
    let ch = Arc::new(ClickHouse::new(
        &r,
        clickhouse::Configuration { maximum_wait_time: Duration::from_millis(500), ..Default::default() },
        db.clone(),
    ));
    let input = Arc::new(
        KafkaInput::new(
            &r,
            kafkainput::Configuration {
                topic: topic.clone(),
                brokers: vec![brokers()],
                consumer_group: format!("e2e-{id}"),
                fetch_max_wait_time: Duration::from_millis(100),
                fetch_min_bytes: 1,
                ..Default::default()
            },
            Arc::new(|| false),
        )
        .unwrap(),
    );
    input.start().unwrap();
    let core = Core::new(
        &r,
        flowscope_common::config::from_value::<Configuration>(
            flowscope_common::config::parse_str("defaultsamplingrate: 100").unwrap(),
        )
        .unwrap(),
        Dependencies {
            flow: Arc::new(flowscope_decoder::Flow::new(&r)),
            metadata: Metadata::with_providers(&r, Default::default(), vec![Box::new(MetadataMock)]),
            routing: Arc::new(Routing::with_provider(&r, Box::new(RoutingMock::populated()))),
            networks: None,
            kafka_input: input.clone(),
            kafka_output: None,
            clickhouse: ch,
            schema: schema.clone(),
            runtime: rt.handle().clone(),
        },
    )
    .unwrap();
    core.start().unwrap();
    drop(guard);

    // Produce raw flows until the consumer is assigned and rows show up.
    let producer: BaseProducer = ClientConfig::new().set("bootstrap.servers", brokers()).create().unwrap();
    let packets = pcap_udp_payloads(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/netflow/nfv5.pcap"));
    let raw: Vec<Vec<u8>> = packets
        .iter()
        .map(|p| {
            flowscope_pb::RawFlow {
                time_received: 1_700_000_000,
                payload: p.clone(),
                source_address: std::net::Ipv4Addr::new(192, 0, 2, 142).to_ipv6_mapped().octets().to_vec(),
                decoder: flowscope_pb::Decoder::Netflow as i32,
                timestamp_source: flowscope_pb::TimestampSource::Input as i32,
                ..Default::default()
            }
            .encode_to_vec()
        })
        .collect();
    let count_rows = || -> u64 {
        db.with_conn(|c| c.query(&format!("SELECT count() FROM {table}"), &[]))
            .map(|r| r.iter_rows().next().map(|row| format!("{:?}", row[0])).unwrap_or_default())
            .ok()
            .and_then(|s| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().ok())
            .unwrap_or(0)
    };
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    while count_rows() == 0 {
        assert!(Instant::now() < deadline, "no rows in ClickHouse");
        for m in &raw {
            producer.send(BaseRecord::<[u8], [u8]>::to(&format!("{topic}-v5")).payload(m)).unwrap();
        }
        producer.flush(std::time::Duration::from_secs(5)).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Stop: the workers flush their last batch.
    core.stop();
    input.stop();
    let metrics = r.render_metrics();
    let forwarded: u64 = metrics
        .lines()
        .find(|l| l.starts_with("flowscope_outlet_core_forwarded_flows_total"))
        .and_then(|l| l.rsplit_once(' '))
        .map(|(_, v)| v.parse().unwrap())
        .unwrap();
    eprintln!("rows={} forwarded={forwarded}", count_rows());
    assert_eq!(count_rows(), forwarded, "{metrics}");
    let check = db
        .with_conn(|c| {
            c.query(
                &format!("SELECT any(ExporterName), any(InIfName), min(SamplingRate), countIf(Bytes = 0) FROM {table}"),
                &[],
            )
        })
        .unwrap();
    let row = format!("{:?}", check.iter_rows().next().unwrap());
    eprintln!("{row}");
    assert!(row.contains("192_0_2_142"), "{row}");
    assert!(row.contains("Gi0/0/"), "{row}");

    // Cleanup
    db.with_conn(|c| c.execute(&format!("DROP DATABASE {database}"), &[])).unwrap();
    rt.block_on(admin.delete_topics(&[&format!("{topic}-v5")], &AdminOptions::new())).unwrap();
}
