// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Port of `outlet/core/enricher_test.go`. Flows are given to the worker
//! after decoding (the Go tests use a gob decoder for that).

#![allow(clippy::type_complexity)]

use super::config::{AsnProvider, NetProvider};
use super::worker::Worker;
use super::*;
use crate::geoip::{self, GeoIp};
use crate::metadata::mock::MockProvider as MetadataMock;
use crate::networks;
use crate::routing::mock::MockProvider as RoutingMock;
use flowscope_common::config::{from_value, parse_str};
use flowscope_common::daemon::Daemon;
use flowscope_schema::{ColumnKey as K, FlowMessage, Value as V};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::net::Ipv6Addr;
use std::path::Path;

fn v4(s: &str) -> Ipv6Addr {
    s.parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped()
}

struct Harness {
    _rt: tokio::runtime::Runtime,
    r: Reporter,
    core: Core,
    sent: Arc<Mutex<Vec<FlowMessage>>>,
}

#[derive(Default)]
struct Options {
    config: &'static str,
    geoip: bool,
    networks: Option<networks::Configuration>,
}

fn geoip_path(db: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/geoip").join(db).to_string_lossy().to_string()
}

impl Harness {
    fn new(opts: Options) -> Harness {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let guard = rt.enter();
        let r = Reporter::new_mock();
        let daemon = Daemon::new();
        let metadata = Metadata::with_providers(&r, Default::default(), vec![Box::new(MetadataMock)]);
        let routing = Arc::new(Routing::with_provider(&r, Box::new(RoutingMock::populated())));
        let geoip = opts.geoip.then(|| {
            let g = GeoIp::new(
                &r,
                geoip::Configuration {
                    geo_database: [
                        "GeoLite2-City-Test.mmdb",
                        "ip_country_asn_sample.mmdb",
                        "ip_geolocation_sample.mmdb",
                        "dbip_city_sample.mmdb",
                    ]
                    .map(geoip_path)
                    .to_vec(),
                    asn_database: ["GeoLite2-ASN-Test.mmdb", "ip_country_asn_sample.mmdb", "dbip_asn_sample.mmdb"]
                        .map(geoip_path)
                        .to_vec(),
                    optional: false,
                },
            );
            g.start(&daemon).unwrap();
            g
        });
        let networks = (opts.geoip || opts.networks.is_some()).then(|| {
            let n = Networks::new(&r, opts.networks.unwrap_or_default(), geoip);
            n.rebuild();
            n
        });
        let sent = Arc::new(Mutex::new(vec![]));
        let s = sent.clone();
        let config: Configuration =
            from_value(parse_str(if opts.config.is_empty() { "{}" } else { opts.config }).unwrap()).unwrap();
        let core = Core::new(
            &r,
            config,
            Dependencies {
                flow: Arc::new(Flow::new(&r)),
                metadata,
                routing,
                networks,
                kafka_input: kafkainput::Mock::new(),
                kafka_output: None,
                clickhouse: Arc::new(clickhouse::Mock::new(move |bf| s.lock().unwrap().push(bf))),
                schema: Arc::new(flowscope_schema::Component::default_schema().enable_all_columns()),
                runtime: rt.handle().clone(),
            },
        )
        .unwrap();
        drop(guard);
        Harness { _rt: rt, r, core, sent }
    }

    /// Process a decoded flow and return what was sent to ClickHouse.
    fn process(&self, flow: impl FnOnce(&mut FlowMessage)) -> Option<FlowMessage> {
        let (tx, _rx) = std::sync::mpsc::sync_channel(10);
        let mut w = Worker::new(self.core.inner.clone(), 0, tx);
        w.bf.debug = true;
        flow(&mut w.bf);
        Worker::finalize_flow(&w.inner, &mut *w.cw, &w.scale, &mut w.bf, 0, 0, &|| false);
        self.sent.lock().unwrap().pop()
    }

    /// Metrics of the core component, without the prefix.
    fn metrics(&self, prefixes: &[&str]) -> BTreeMap<String, String> {
        self.r
            .render_metrics()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.strip_prefix("flowscope_outlet_core_"))
            .filter(|l| prefixes.iter().any(|p| l.starts_with(p)))
            .filter_map(|l| l.rsplit_once(' ').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect()
    }
}

/// What we expect from a flow.
#[derive(Debug, PartialEq, Default)]
struct Expected {
    sampling_rate: u64,
    in_if: u32,
    out_if: u32,
    src_vlan: u16,
    dst_vlan: u16,
    src_addr: Option<Ipv6Addr>,
    dst_addr: Option<Ipv6Addr>,
    next_hop: Option<Ipv6Addr>,
    src_as: u32,
    dst_as: u32,
    src_net_mask: u8,
    dst_net_mask: u8,
    other: BTreeMap<K, V>,
}

impl Expected {
    fn from(bf: &FlowMessage) -> Expected {
        Expected {
            sampling_rate: bf.sampling_rate,
            in_if: bf.in_if,
            out_if: bf.out_if,
            src_vlan: bf.src_vlan,
            dst_vlan: bf.dst_vlan,
            src_addr: bf.src_addr,
            dst_addr: bf.dst_addr,
            next_hop: bf.next_hop,
            src_as: bf.src_as,
            dst_as: bf.dst_as,
            src_net_mask: bf.src_net_mask,
            dst_net_mask: bf.dst_net_mask,
            other: bf.other_columns.clone(),
        }
    }

    /// The usual flow from 192.0.2.142 with interfaces 100 and 200.
    fn standard(sampling_rate: u64) -> Expected {
        Expected { sampling_rate, in_if: 100, out_if: 200, other: standard_columns(100, 200), ..Default::default() }
    }

    fn with(mut self, cols: &[(K, V)]) -> Expected {
        for (k, v) in cols {
            self.other.insert(*k, v.clone());
        }
        self
    }
}

fn s(v: &str) -> V {
    V::String(v.into())
}

fn standard_columns(in_if: u32, out_if: u32) -> BTreeMap<K, V> {
    BTreeMap::from([
        (K::ExporterName, s("192_0_2_142")),
        (K::InIfName, s(&format!("Gi0/0/{in_if}"))),
        (K::OutIfName, s(&format!("Gi0/0/{out_if}"))),
        (K::InIfDescription, s(&format!("Interface {in_if}"))),
        (K::OutIfDescription, s(&format!("Interface {out_if}"))),
        (K::InIfSpeed, V::U32(1000)),
        (K::OutIfSpeed, V::U32(1000)),
    ])
}

fn standard_flow(sampling_rate: u64) -> impl FnOnce(&mut FlowMessage) {
    move |bf| {
        bf.sampling_rate = sampling_rate;
        bf.exporter_address = Some(v4("192.0.2.142"));
        bf.in_if = 100;
        bf.out_if = 200;
    }
}

fn check(
    name: &str,
    opts: Options,
    flow: impl FnOnce(&mut FlowMessage),
    expected: Option<Expected>,
    metrics: &[(&str, &str)],
) {
    let h = Harness::new(opts);
    let got = h.process(flow);
    assert_eq!(got.as_ref().map(Expected::from), expected, "{name}");
    let mut want: BTreeMap<String, String> = BTreeMap::from([
        ("flows_http_clients".to_string(), "0".to_string()),
        (r#"received_flows_total{exporter="192.0.2.142"}"#.to_string(), "1".to_string()),
        // Raw flows are not decoded here.
        ("received_raw_flows_total".to_string(), "0".to_string()),
    ]);
    if expected.is_some() {
        want.insert(r#"forwarded_flows_total{exporter="192.0.2.142"}"#.into(), "1".into());
    }
    for (k, v) in metrics {
        want.insert(k.to_string(), v.to_string());
    }
    assert_eq!(h.metrics(&["flows_", "received_", "forwarded_"]), want, "{name}");
}

#[test]
fn enrich_no_rule() {
    check("no rule", Options::default(), standard_flow(1000), Some(Expected::standard(1000)), &[]);
}

#[test]
fn enrich_sampling_rates() {
    let map = "\n  192.0.2.0/24: 100\n  192.0.2.128/25: 500\n  192.0.2.141/32: 1000\n";
    let config: &'static str = Box::leak(format!("overridesamplingrate:{map}").into_boxed_str());
    check(
        "override sampling rate",
        Options { config, ..Default::default() },
        standard_flow(1000),
        Some(Expected::standard(500)),
        &[],
    );
    check(
        "default is one value",
        Options { config: "defaultsamplingrate: 500", ..Default::default() },
        standard_flow(0),
        Some(Expected::standard(500)),
        &[],
    );
    let config: &'static str = Box::leak(format!("defaultsamplingrate:{map}").into_boxed_str());
    check(
        "default is map",
        Options { config, ..Default::default() },
        standard_flow(0),
        Some(Expected::standard(500)),
        &[],
    );
    check(
        "no sampling rate",
        Options::default(),
        standard_flow(0),
        None,
        &[(r#"flows_errors_total{error="sampling rate missing",exporter="192.0.2.142"}"#, "1")],
    );
}

#[test]
fn enrich_exporter_rules() {
    check(
        "exporter rule",
        Options {
            config: r#"exporterclassifiers:
  - Exporter.Name startsWith "hello" && ClassifyRegion("europe")
  - Exporter.Name startsWith "192_" && ClassifyRegion("asia")
  - ClassifyRegion("other") && ClassifySite("unknown") && ClassifyTenant("alfred")
"#,
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[
            (K::ExporterRegion, s("asia")),
            (K::ExporterTenant, s("alfred")),
            (K::ExporterSite, s("unknown")),
        ])),
        &[],
    );
    check(
        "exporter rule setting name",
        Options {
            config: r#"exporterclassifiers:
  - Exporter.Name startsWith "192_" && SetName("router-192")
  - Exporter.Name startsWith "192_" && ClassifyRegion("asia")
"#,
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::ExporterName, s("router-192")), (K::ExporterRegion, s("asia"))])),
        &[],
    );
    check(
        "exporter rule not setting name",
        Options {
            config: "exporterclassifiers:\n  - Exporter.Name startsWith \"nothing\" && SetName(\"router-192\")\n",
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000)),
        &[],
    );
    check(
        "exporter rule with an error",
        Options {
            config: "exporterclassifiers:\n  - ClassifyTenant(\"alfred\")\n  - Exporter.Name > \"hello\"\n",
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::ExporterTenant, s("alfred"))])),
        &[],
    );
    check(
        "exporter rule with reject",
        Options { config: "exporterclassifiers: [\"Reject()\"]", ..Default::default() },
        standard_flow(1000),
        None,
        &[],
    );
}

#[test]
fn enrich_interface_rules() {
    check(
        "interface rule with reject",
        Options { config: "interfaceclassifiers: [\"Reject()\"]", ..Default::default() },
        standard_flow(1000),
        None,
        &[],
    );
    check(
        "interface rule with index",
        Options {
            config: r#"interfaceclassifiers:
  - Interface.Index == 100 && ClassifyProvider("index1")
  - Interface.Index == 200 && ClassifyProvider("index2")
"#,
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::InIfProvider, s("index1")), (K::OutIfProvider, s("index2"))])),
        &[],
    );
    check(
        "interface rule with rename",
        Options {
            config: r#"interfaceclassifiers:
  - Interface.Name == "Gi0/0/100" && SetName("eth100")
  - Interface.Name == "Gi0/0/200" && SetDescription("Super Speed")
"#,
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::InIfName, s("eth100")), (K::OutIfDescription, s("Super Speed"))])),
        &[],
    );
    let mut expected = Expected::standard(1000).with(&[(K::OutIfName, s("Gi0/0/200.300"))]);
    expected.src_vlan = 10;
    expected.dst_vlan = 300;
    check(
        "interface rule with VLAN",
        Options {
            config: "interfaceclassifiers:\n  - 'Interface.VLAN > 200 && SetName(Format(\"%s.%d\", Interface.Name, Interface.VLAN))'\n",
            ..Default::default()
        },
        |bf| {
            standard_flow(1000)(bf);
            bf.src_vlan = 10;
            bf.dst_vlan = 300;
        },
        Some(expected),
        &[],
    );
    check(
        "interface rule",
        Options {
            config: r#"interfaceclassifiers:
  - |
    Interface.Description startsWith "Transit:" &&
    ClassifyConnectivity("transit") &&
    ClassifyExternal() &&
    ClassifyProviderRegex(Interface.Description, "^Transit: ([^ ]+)", "$1")
  - ClassifyInternal()
"#,
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::InIfBoundary, V::U8(2)), (K::OutIfBoundary, V::U8(2))])),
        &[],
    );
    check(
        "configure twice boundary",
        Options {
            config: "interfaceclassifiers: [\"ClassifyInternal()\", \"ClassifyExternal()\"]",
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::InIfBoundary, V::U8(2)), (K::OutIfBoundary, V::U8(2))])),
        &[],
    );
    check(
        "configure twice provider",
        Options {
            config: "interfaceclassifiers: ['ClassifyProvider(\"telia\")', 'ClassifyProvider(\"cogent\")']",
            ..Default::default()
        },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[(K::InIfProvider, s("telia")), (K::OutIfProvider, s("telia"))])),
        &[],
    );
    let rules = r#"interfaceclassifiers:
  - ClassifyProvider("Othello")
  - ClassifyConnectivityRegex(Interface.Description, " (1\\d+)$", "P$1") && ClassifyExternal()
  - ClassifyInternal() && ClassifyConnectivity("core")
"#;
    check(
        "classify depending on description",
        Options { config: rules, ..Default::default() },
        standard_flow(1000),
        Some(Expected::standard(1000).with(&[
            (K::InIfConnectivity, s("p100")),
            (K::OutIfConnectivity, s("core")),
            (K::InIfProvider, s("othello")),
            (K::OutIfProvider, s("othello")),
            (K::InIfBoundary, V::U8(1)),
            (K::OutIfBoundary, V::U8(2)),
        ])),
        &[],
    );
    let mut expected = Expected {
        sampling_rate: 1000,
        in_if: 1010,
        out_if: 2010,
        other: standard_columns(1010, 2010),
        ..Default::default()
    }
    .with(&[
        (K::ExporterGroup, s("metadata group")),
        (K::ExporterRegion, s("metadata region")),
        (K::ExporterRole, s("metadata role")),
        (K::ExporterSite, s("metadata site")),
        (K::ExporterTenant, s("metadata tenant")),
        (K::InIfConnectivity, s("p1010")),
        (K::OutIfConnectivity, s("metadata connectivity")),
        (K::InIfProvider, s("othello")),
        (K::OutIfProvider, s("metadata provider")),
        (K::InIfBoundary, V::U8(1)),
        (K::OutIfBoundary, V::U8(1)),
    ]);
    expected.sampling_rate = 1000;
    check(
        "use metadata instead of classifier",
        Options { config: rules, ..Default::default() },
        |bf| {
            standard_flow(1000)(bf);
            bf.in_if = 1010;
            bf.out_if = 2010;
        },
        Some(expected),
        &[],
    );
}

#[test]
fn enrich_routing() {
    let mut expected = Expected::standard(1000).with(&[
        (K::DstASPath, V::ArrayU32(vec![64200, 1299, 174])),
        (K::SrcCommunities, V::ArrayU32(vec![500])),
        (K::DstCommunities, V::ArrayU32(vec![100, 200, 400])),
        (K::DstLargeCommunities, V::ArrayU128(vec![(64200u128 << 64) | (2 << 32) | 3])),
    ]);
    expected.src_addr = Some(v4("192.0.2.142"));
    expected.dst_addr = Some(v4("192.0.2.10"));
    expected.src_as = 1299;
    expected.dst_as = 174;
    expected.src_net_mask = 27;
    expected.dst_net_mask = 27;
    // The Go version leaves the next hop empty: a missing next hop in the
    // flow did not fall back to the one from the routing table.
    expected.next_hop = Some(v4("198.51.100.4"));
    check(
        "use data from routing",
        Options::default(),
        |bf| {
            standard_flow(1000)(bf);
            bf.src_addr = Some(v4("192.0.2.142"));
            bf.dst_addr = Some(v4("192.0.2.10"));
        },
        Some(expected),
        &[],
    );
}

#[test]
fn enrich_geoip() {
    let flow = |bf: &mut FlowMessage| {
        standard_flow(1000)(bf);
        bf.src_addr = Some(v4("67.43.156.77"));
        bf.dst_addr = Some(v4("203.0.113.5"));
    };
    let mut expected = Expected::standard(1000).with(&[(K::SrcCountry, s("BT"))]);
    expected.src_addr = Some(v4("67.43.156.77"));
    expected.dst_addr = Some(v4("203.0.113.5"));
    expected.src_as = 35908;
    check("use data from GeoIP", Options { geoip: true, ..Default::default() }, flow, Some(expected), &[]);

    let networks: networks::Configuration = from_value(
        parse_str(
            r#"networks:
  ::ffff:67.0.0.0/104: {name: customer1, role: customer, country: FR, city: Paris, asn: 65500}
  ::ffff:203.0.113.0/120: {name: servers, site: ams5, asn: 65401}
"#,
        )
        .unwrap(),
    )
    .unwrap();
    let mut expected = Expected::standard(1000).with(&[
        (K::SrcNetName, s("customer1")),
        (K::SrcNetRole, s("customer")),
        (K::SrcCountry, s("BT")),
        (K::SrcGeoCity, s("Paris")),
        (K::DstNetName, s("servers")),
        (K::DstNetSite, s("ams5")),
    ]);
    expected.src_addr = Some(v4("67.43.156.77"));
    expected.dst_addr = Some(v4("203.0.113.5"));
    expected.src_as = 35908;
    expected.dst_as = 65401;
    check(
        "merge network attributes and GeoIP",
        Options { geoip: true, networks: Some(networks), ..Default::default() },
        flow,
        Some(expected),
        &[],
    );
}

#[test]
fn enrich_errors() {
    check(
        "flow with missing interfaces",
        Options::default(),
        |bf| {
            standard_flow(1000)(bf);
            bf.in_if = 0;
            bf.out_if = 0;
        },
        None,
        &[(r#"flows_errors_total{error="input and output interfaces missing",exporter="192.0.2.142"}"#, "1")],
    );
    check(
        "flow with metadata missing",
        Options::default(),
        |bf| {
            standard_flow(1000)(bf);
            bf.in_if = 999;
            bf.out_if = 0;
        },
        None,
        &[(r#"flows_errors_total{error="metadata missing",exporter="192.0.2.142"}"#, "1")],
    );
}

fn with_providers(asn: Vec<AsnProvider>, net: Vec<NetProvider>) -> Harness {
    let h = Harness::new(Options::default());
    // The configuration is only read, it is fine to replace it in place.
    let inner = Arc::as_ptr(&h.core.inner) as *mut Inner;
    // SAFETY: no worker runs and no other reference uses the configuration.
    unsafe {
        (*inner).config.asn_providers = asn;
        (*inner).config.net_providers = net;
    }
    h
}

#[test]
fn get_as_number() {
    use AsnProvider::*;
    let cases: &[(u32, u32, u32, u8, &[AsnProvider], u32)] = &[
        (12322, 0, 0, 24, &[Flow], 12322),
        (65536, 0, 0, 24, &[Flow], 65536),
        (65536, 0, 0, 24, &[FlowExceptPrivate], 0),
        (4_200_000_121, 0, 0, 24, &[FlowExceptPrivate], 0),
        (65536, 0, 0, 24, &[FlowExceptPrivate, Flow], 65536),
        (12322, 0, 0, 24, &[FlowExceptPrivate], 12322),
        (12322, 0, 0, 0, &[FlowExceptDefaultRoute], 0),
        (12322, 174, 0, 24, &[Routing], 174),
        (12322, 0, 0, 24, &[Routing], 0),
        (0, 0, 65401, 24, &[Networks], 65401),
        (12322, 0, 65401, 24, &[Flow, Networks], 12322),
        (0, 65300, 65401, 24, &[Networks, Routing], 65401),
        (0, 65300, 0, 24, &[Networks, Routing], 65300),
    ];
    for (i, (flow_as, routing_as, net_as, mask, providers, expected)) in cases.iter().enumerate() {
        let h = with_providers(providers.to_vec(), vec![]);
        assert_eq!(h.core.inner.get_as_number(*flow_as, *routing_as, *net_as, *mask), *expected, "case {}", i + 1);
    }
}

#[test]
fn get_net_mask() {
    use NetProvider::*;
    let cases: &[(u8, u8, &[NetProvider], u8)] = &[
        (0, 0, &[Flow], 0),
        (32, 0, &[Flow], 32),
        (0, 16, &[Flow], 0),
        (0, 0, &[Routing], 0),
        (32, 12, &[Routing], 12),
        (0, 16, &[Routing], 16),
        (24, 0, &[Routing], 0),
        (0, 0, &[Routing, Flow], 0),
        (12, 0, &[Routing, Flow], 12),
        (0, 13, &[Routing, Flow], 13),
        (12, 24, &[Routing, Flow], 24),
        (0, 0, &[Flow, Routing], 0),
        (12, 0, &[Flow, Routing], 12),
        (0, 13, &[Flow, Routing], 13),
        (12, 24, &[Flow, Routing], 12),
    ];
    for (i, (flow, routing, providers, expected)) in cases.iter().enumerate() {
        let h = with_providers(vec![], providers.to_vec());
        assert_eq!(h.core.inner.get_net_mask(*flow, *routing), *expected, "case {}", i + 1);
    }
}

#[test]
fn get_next_hop() {
    use NetProvider::*;
    let u = Some(Ipv6Addr::UNSPECIFIED);
    let nh1: Option<Ipv6Addr> = Some("2001:db8::1".parse().unwrap());
    let nh2: Option<Ipv6Addr> = Some("2001:db8::2".parse().unwrap());
    let cases: &[(Option<Ipv6Addr>, Option<Ipv6Addr>, &[NetProvider], Option<Ipv6Addr>)] = &[
        (u, u, &[Flow], u),
        (nh1, u, &[Flow], nh1),
        (u, nh1, &[Flow], u),
        (u, u, &[Routing], u),
        (nh1, u, &[Routing], u),
        (u, nh1, &[Routing], nh1),
        (u, u, &[Routing, Flow], u),
        (nh1, u, &[Routing, Flow], nh1),
        (u, nh2, &[Routing, Flow], nh2),
        (nh1, nh2, &[Routing, Flow], nh2),
        (u, u, &[Flow, Routing], u),
        (nh1, u, &[Flow, Routing], nh1),
        (u, nh2, &[Flow, Routing], nh2),
        (nh1, nh2, &[Flow, Routing], nh1),
        // Missing next hops (Go returned the missing one from the flow).
        (None, nh2, &[Flow, Routing], nh2),
        (None, None, &[Flow, Routing], None),
        (nh1, None, &[Routing, Flow], nh1),
    ];
    for (i, (flow, routing, providers, expected)) in cases.iter().enumerate() {
        let h = with_providers(vec![], providers.to_vec());
        assert_eq!(h.core.inner.get_next_hop(*flow, *routing), *expected, "case {}", i + 1);
    }
}

#[test]
fn rate_limited_flows() {
    let h = Harness::new(Options::default());
    let (tx, _rx) = std::sync::mpsc::sync_channel(10);
    let mut w = Worker::new(h.core.inner.clone(), 0, tx);
    w.bf.debug = true;
    let mut sampling = vec![];
    for second in [1000u64, 1001] {
        for _ in 0..15 {
            standard_flow(1000)(&mut w.bf);
            Worker::finalize_flow(&w.inner, &mut *w.cw, &w.scale, &mut w.bf, 10, second, &|| false);
            if let Some(f) = h.sent.lock().unwrap().pop() {
                sampling.push(f.sampling_rate);
            }
        }
    }
    // 10 flows accepted each second, the second ones account for the
    // dropped flows of the first second.
    assert_eq!(sampling, [vec![1000; 10], vec![1500; 10]].concat());
    assert_eq!(
        h.metrics(&["flows_rate_limited"]),
        BTreeMap::from([(r#"flows_rate_limited_total{exporter="192.0.2.142"}"#.to_string(), "10".to_string())])
    );
}

#[test]
fn private_as() {
    for (asn, private) in
        [(0, true), (23456, true), (64496, true), (65551, true), (65552, false), (4_200_000_000, true), (174, false)]
    {
        assert_eq!(is_private_as(asn), private, "{asn}");
    }
}

#[test]
fn http_flows() {
    let h = Harness::new(Options::default());
    let listener = h._rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let router = h.core.router();
    h._rt.spawn(async move { axum::serve(listener, router).await });
    let client_count = || h.metrics(&["flows_http_clients"])["flows_http_clients"].clone();

    // Invalid limit
    let resp = h._rt.block_on(reqwest::get(format!("http://{addr}/api/v0/outlet/flows?limit=x"))).unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(h._rt.block_on(resp.text()).unwrap(), r#"{"message":"Invalid limit"}"#);

    for limit in [0usize, 4] {
        let url = if limit == 0 {
            format!("http://{addr}/api/v0/outlet/flows")
        } else {
            format!("http://{addr}/api/v0/outlet/flows?limit={limit}")
        };
        let mut resp = h._rt.block_on(reqwest::get(url)).unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(client_count(), "1");
        // The channel holds 10 flows, the others are skipped.
        for _ in 0..12 {
            assert!(
                h.process(|bf| {
                    standard_flow(1000)(bf);
                    bf.in_if = 434;
                    bf.out_if = 677;
                    bf.src_addr = Some(v4("67.43.156.77"));
                    bf.dst_addr = Some(v4("2.125.160.216"));
                })
                .is_some()
            );
        }
        let mut body = String::new();
        let wanted = if limit == 0 { 10 } else { limit };
        while body.lines().count() < wanted {
            let chunk = h._rt.block_on(resp.chunk()).unwrap().expect("more data");
            body.push_str(std::str::from_utf8(&chunk).unwrap());
        }
        if limit > 0 {
            // The response ends after the limit.
            assert!(h._rt.block_on(resp.chunk()).unwrap().is_none());
        }
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), wanted);
        let got: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(
            got,
            serde_json::json!({
                "TimeReceived": 0,
                "SamplingRate": 1000,
                "ExporterAddress": "::ffff:192.0.2.142",
                "SrcAddr": "::ffff:67.43.156.77",
                "DstAddr": "::ffff:2.125.160.216",
                "SrcAS": 0,
                "DstAS": 0,
                "InIf": 434,
                "OutIf": 677,
                "NextHop": "",
                "SrcNetMask": 0,
                "DstNetMask": 0,
                "SrcVlan": 0,
                "DstVlan": 0,
                "OtherColumns": {
                    "ExporterName": "192_0_2_142",
                    "InIfName": "Gi0/0/434",
                    "OutIfName": "Gi0/0/677",
                    "InIfDescription": "Interface 434",
                    "OutIfDescription": "Interface 677",
                    "InIfSpeed": 1000,
                    "OutIfSpeed": 1000,
                },
            })
        );
        drop(resp);
        // Drain what is left for the next client.
        while h.core.inner.http_flow_rx.try_lock().map(|mut rx| rx.try_recv().is_ok()).unwrap_or(true) {}
        for _ in 0..100 {
            if client_count() == "0" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(client_count(), "0");
    }
}

/// UDP payloads of an Ethernet/IPv4 pcap file.
fn pcap_udp_payloads(path: &Path) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap();
    assert_eq!(u32::from_le_bytes(data[0..4].try_into().unwrap()), 0xa1b2c3d4, "little-endian pcap expected");
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
fn decode_path() {
    use prost::Message;
    let h = Harness::new(Options { config: "defaultsamplingrate: 100", ..Default::default() });
    let input = kafkainput::Mock::new();
    let builder = h.core.worker_builder();
    kafkainput::Component::start_workers(&*input, builder).unwrap();
    let packets = pcap_udp_payloads(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/netflow/nfv5.pcap"));
    for p in &packets {
        let raw = flowscope_pb::RawFlow {
            time_received: 1_700_000_000,
            payload: p.clone(),
            source_address: v4("192.0.2.142").octets().to_vec(),
            use_source_address: false,
            decoder: flowscope_pb::Decoder::Netflow as i32,
            timestamp_source: flowscope_pb::TimestampSource::Input as i32,
            ..Default::default()
        };
        input.send(&raw.encode_to_vec());
    }
    // A broken message and a decoder we do not have.
    input.send(b"\xff\xff\xff");
    input.send(
        &flowscope_pb::RawFlow { decoder: 99, source_address: vec![1, 2, 3, 4], ..Default::default() }.encode_to_vec(),
    );

    let metrics = h.metrics(&["received_", "forwarded_", "raw_flows_errors", "flows_errors"]);
    assert_eq!(metrics["received_raw_flows_total"], (packets.len() + 2).to_string());
    assert_eq!(metrics[r#"raw_flows_errors_total{error="cannot decode protobuf"}"#], "1");
    assert_eq!(metrics[r#"raw_flows_errors_total{error="cannot decode payload"}"#], "1");
    let received: u64 = metrics[r#"received_flows_total{exporter="192.0.2.142"}"#].parse().unwrap();
    let forwarded: u64 =
        metrics.get(r#"forwarded_flows_total{exporter="192.0.2.142"}"#).map(|v| v.parse().unwrap()).unwrap_or(0);
    let errors: u64 = metrics
        .iter()
        .filter(|(k, _)| k.starts_with("flows_errors_total"))
        .map(|(_, v)| v.parse::<u64>().unwrap())
        .sum();
    assert!(received > 0);
    assert_eq!(received, forwarded + errors);
    assert_eq!(h.sent.lock().unwrap().len() as u64, forwarded);
    kafkainput::Component::stop_workers(&*input);
}

// A named value given as a number is refused, not stored as is. Port of
// Akvorado's TestNonStringToTextUnmarshallerHookFunc (b4d64af5), with the
// ASN providers as the named values.
#[test]
fn config_refuses_numbers_for_named_values() {
    let parse = |yaml: &str| from_value::<Configuration>(parse_str(yaml).unwrap()).map(|c| c.asn_providers);
    assert_eq!(parse("asn-providers: [routing, flow]").unwrap(), vec![AsnProvider::Routing, AsnProvider::Flow]);
    for bad in ["asn-providers: [42]", "asn-providers: [1]", "asn-providers: [third]"] {
        assert!(parse(bad).is_err(), "{bad} was accepted");
    }
}
