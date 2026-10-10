// SPDX-FileCopyrightText: 2024 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Port of the Go gNMI provider tests, plus a mock gNMI server.

use super::events::{Event, convert_speed, parse_path, subscribe_response_to_events};
use super::pb::gnmi::{self, typed_value::Value as V};
use super::*;
use crate::metadata::Interface;
use crate::metadata::gnmi_config::{IfSpeedPathUnit, default_models};
use pretty_assertions::assert_eq;

fn elem(name: &str, keys: &[(&str, &str)]) -> gnmi::PathElem {
    gnmi::PathElem { name: name.into(), key: keys.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
}

fn path(elems: Vec<gnmi::PathElem>) -> Option<gnmi::Path> {
    Some(gnmi::Path { elem: elems, ..Default::default() })
}

fn update(p: Vec<gnmi::PathElem>, v: V) -> gnmi::Update {
    gnmi::Update { path: path(p), val: Some(gnmi::TypedValue { value: Some(v) }), ..Default::default() }
}

fn response(prefix: Option<gnmi::Path>, updates: Vec<gnmi::Update>) -> gnmi::SubscribeResponse {
    gnmi::SubscribeResponse {
        response: Some(gnmi::subscribe_response::Response::Update(gnmi::Notification {
            prefix,
            update: updates,
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn ev(path: &str, keys: &str, value: &str) -> Event {
    Event { path: path.into(), keys: keys.into(), value: value.into() }
}

fn json(s: &str) -> V {
    V::JsonVal(s.as_bytes().to_vec())
}

#[test]
fn subscribe_response_to_events_cases() {
    let sort = |mut v: Vec<Event>| {
        v.sort_by(|a, b| (&a.keys, &a.path).cmp(&(&b.keys, &b.path)));
        v
    };
    let cases: Vec<(&str, gnmi::SubscribeResponse, Vec<Event>)> = vec![
        (
            "sync response",
            gnmi::SubscribeResponse {
                response: Some(gnmi::subscribe_response::Response::SyncResponse(true)),
                ..Default::default()
            },
            vec![],
        ),
        (
            "string value",
            response(
                None,
                vec![update(
                    vec![elem("system", &[]), elem("name", &[]), elem("host-name", &[])],
                    V::StringVal("router1".into()),
                )],
            ),
            vec![ev("/system/name/host-name", "", "router1")],
        ),
        (
            "int value",
            response(
                None,
                vec![update(vec![elem("interface", &[("name", "eth1")]), elem("ifindex", &[])], V::IntVal(42))],
            ),
            vec![ev("/interface/ifindex", "name=eth1", "42")],
        ),
        (
            "uint value",
            response(
                None,
                vec![update(
                    vec![elem("interface", &[("name", "eth1")]), elem("counters", &[]), elem("in-octets", &[])],
                    V::UintVal(123456),
                )],
            ),
            vec![ev("/interface/counters/in-octets", "name=eth1", "123456")],
        ),
        (
            "ascii value",
            response(None, vec![update(vec![elem("system", &[]), elem("version", &[])], V::AsciiVal("v1.0".into()))]),
            vec![ev("/system/version", "", "v1.0")],
        ),
        (
            "unsupported value type is skipped",
            response(
                None,
                vec![
                    update(vec![elem("bool-leaf", &[])], V::BoolVal(true)),
                    update(vec![elem("string-leaf", &[])], V::StringVal("kept".into())),
                ],
            ),
            vec![ev("/string-leaf", "", "kept")],
        ),
        (
            "json_ietf value with nested map",
            response(
                None,
                vec![update(
                    vec![elem("system", &[])],
                    V::JsonIetfVal(br#"{"config":{"hostname":"router1"}}"#.to_vec()),
                )],
            ),
            vec![ev("/system/config/hostname", "", "router1")],
        ),
        (
            "json value with array",
            response(
                None,
                vec![update(
                    vec![elem("interfaces", &[])],
                    json(
                        r#"{"interface":[{"name":"eth1","state":{"ifindex":100,"description":"first"}},{"name":"eth2","state":{"ifindex":101,"description":"second"}}]}"#,
                    ),
                )],
            ),
            vec![
                ev("/interfaces/interface/name", "name=eth1", "eth1"),
                ev("/interfaces/interface/state/description", "name=eth1", "first"),
                ev("/interfaces/interface/state/ifindex", "name=eth1", "100"),
                ev("/interfaces/interface/name", "name=eth2", "eth2"),
                ev("/interfaces/interface/state/description", "name=eth2", "second"),
                ev("/interfaces/interface/state/ifindex", "name=eth2", "101"),
            ],
        ),
        (
            "json array with lag-speed",
            response(
                None,
                vec![update(
                    vec![elem("interfaces", &[])],
                    json(
                        r#"{"interface":[{"name":"po1","aggregation":{"state":{"lag-speed":40000}}},{"name":"po126","aggregation":{"state":{"lag-speed":25000}}}]}"#,
                    ),
                )],
            ),
            vec![
                ev("/interfaces/interface/aggregation/state/lag-speed", "name=po1", "40000"),
                ev("/interfaces/interface/name", "name=po1", "po1"),
                ev("/interfaces/interface/aggregation/state/lag-speed", "name=po126", "25000"),
                ev("/interfaces/interface/name", "name=po126", "po126"),
            ],
        ),
        (
            "json nested arrays with combined keys",
            response(
                None,
                vec![update(
                    vec![elem("interfaces", &[])],
                    json(
                        r#"{"interface":[{"name":"eth1/4","subinterfaces":{"subinterface":[{"index":1,"state":{"name":"eth1/4.1","description":"4th interface","ifindex":105}}]}}]}"#,
                    ),
                )],
            ),
            vec![
                ev("/interfaces/interface/name", "name=eth1/4", "eth1/4"),
                ev("/interfaces/interface/subinterfaces/subinterface/index", "name=eth1/4,index=1", "1"),
                ev(
                    "/interfaces/interface/subinterfaces/subinterface/state/description",
                    "name=eth1/4,index=1",
                    "4th interface",
                ),
                ev("/interfaces/interface/subinterfaces/subinterface/state/ifindex", "name=eth1/4,index=1", "105"),
                ev("/interfaces/interface/subinterfaces/subinterface/state/name", "name=eth1/4,index=1", "eth1/4.1"),
            ],
        ),
        (
            "path keys combined with json array keys",
            response(
                None,
                vec![update(
                    vec![elem("interface", &[("name", "eth1/4")]), elem("subinterface", &[])],
                    V::JsonIetfVal(br#"[{"index":1,"state":{"name":"eth1/4.1"}}]"#.to_vec()),
                )],
            ),
            vec![
                ev("/interface/subinterface/index", "name=eth1/4,index=1", "1"),
                ev("/interface/subinterface/state/name", "name=eth1/4,index=1", "eth1/4.1"),
            ],
        ),
        (
            "json array with non-map elements are skipped",
            response(None, vec![update(vec![elem("something", &[])], json(r#"["just a string",42]"#))]),
            vec![],
        ),
        (
            "invalid json is skipped",
            response(
                None,
                vec![
                    update(vec![elem("bad", &[])], json("{invalid")),
                    update(vec![elem("good", &[])], V::StringVal("ok".into())),
                ],
            ),
            vec![ev("/good", "", "ok")],
        ),
        (
            "prefix is prepended",
            response(
                path(vec![elem("interfaces", &[])]),
                vec![update(
                    vec![elem("interface", &[("name", "eth1")]), elem("state", &[]), elem("description", &[])],
                    V::StringVal("uplink".into()),
                )],
            ),
            vec![ev("/interfaces/interface/state/description", "name=eth1", "uplink")],
        ),
        (
            "namespace prefix is stripped",
            response(
                None,
                vec![update(
                    vec![elem("srl_nokia-interfaces:interface", &[("name", "ethernet-1/1")]), elem("description", &[])],
                    V::StringVal("first".into()),
                )],
            ),
            vec![ev("/interface/description", "name=ethernet-1/1", "first")],
        ),
        (
            "multiple path keys",
            response(
                None,
                vec![update(
                    vec![
                        elem("interface", &[("name", "ethernet-1/4")]),
                        elem("subinterface", &[("index", "1")]),
                        elem("name", &[]),
                    ],
                    V::StringVal("ethernet-1/4.1".into()),
                )],
            ),
            vec![ev("/interface/subinterface/name", "name=ethernet-1/4,index=1", "ethernet-1/4.1")],
        ),
    ];
    for (name, resp, expected) in cases {
        assert_eq!(sort(subscribe_response_to_events(&resp)), sort(expected), "{name}");
    }
}

fn model(name: &str) -> Model {
    default_models().into_iter().find(|m| m.name == name).unwrap()
}

#[test]
fn state_update() {
    let model = model("Nokia SR Linux");
    let mut state = ExporterState::default();
    state.update(&[], &model);
    assert_eq!(state, ExporterState::default());
    let events: Vec<Event> = [
        ("/interface/description", "name=ethernet-1/1", "1st interface"),
        ("/interface/description", "name=ethernet-1/2", "2nd interface"),
        ("/interface/description", "name=ethernet-1/3", "3rd interface"),
        ("/interface/description", "name=lag1", "lag interface"),
        ("/interface/ethernet/port-speed", "name=ethernet-1/1", "100G"),
        ("/interface/ethernet/port-speed", "name=ethernet-1/2", "100G"),
        ("/interface/ethernet/port-speed", "name=ethernet-1/3", "10G"),
        ("/interface/ethernet/port-speed", "name=ethernet-1/4", "25G"),
        ("/interface/ethernet/port-speed", "name=mgmt0", "1G"),
        ("/interface/lag/lag-speed", "name=lag1", "100000000"),
        ("/interface/subinterface/description", "name=ethernet-1/4,index=1", "4th interface"),
        ("/interface/subinterface/name", "name=ethernet-1/4,index=1", "ethernet-1/4.1"),
        ("/interface/subinterface/name", "name=mgmt0,index=0", "mgmt0.0"),
        ("/interface/ifindex", "name=ethernet-1/1", "100"),
        ("/interface/ifindex", "name=ethernet-1/2", "101"),
        ("/interface/ifindex", "name=ethernet-1/3", "102"),
        ("/interface/ifindex", "name=ethernet-1/4", "103"),
        ("/interface/subinterface/ifindex", "name=ethernet-1/4,index=1", "105"),
        ("/interface/ifindex", "name=lag1", "106"),
        ("/system/name/host-name", "", "srlinux"),
    ]
    .iter()
    .map(|(p, k, v)| ev(p, k, v))
    .collect();
    state.update(&events, &model);
    let iface = |name: &str, description: &str, speed| Interface {
        name: name.into(),
        description: description.into(),
        speed,
        ..Default::default()
    };
    assert_eq!(
        state,
        ExporterState {
            name: "srlinux".into(),
            interfaces: HashMap::from([
                (100, iface("ethernet-1/1", "1st interface", 100_000)),
                (101, iface("ethernet-1/2", "2nd interface", 100_000)),
                (102, iface("ethernet-1/3", "3rd interface", 10_000)),
                (103, iface("ethernet-1/4", "", 25_000)),
                (105, iface("ethernet-1/4.1", "4th interface", 25_000)),
                (106, iface("lag1", "lag interface", 100)),
            ]),
        }
    );
}

#[test]
fn speeds() {
    use IfSpeedPathUnit::*;
    let cases: &[(&str, IfSpeedPathUnit, Option<u64>)] = &[
        ("1000", Bps, Some(0)),
        ("1000000", Bps, Some(1)),
        ("100000000", Bps, Some(100)),
        // Above 32 bits: a 100G LAG in bps (an error with Go).
        ("100000000000", Bps, Some(100_000)),
        ("-43874", Bps, None),
        ("nope", Bps, None),
        ("1", Mbps, Some(1)),
        ("-1", Mbps, None),
        ("2500", Mbps, Some(2500)),
        ("10G", Human, Some(10_000)),
        ("25G", Human, Some(25_000)),
        ("100M", Human, Some(100)),
        ("100000K", Human, Some(100)),
        ("100000S", Human, None),
        ("&G", Human, None),
        ("1T", Human, Some(1_000_000)),
        ("", Human, None),
        ("SPEED_100GB", Ethernet, Some(100_000)),
        ("SPEED_25GB", Ethernet, Some(25_000)),
        ("SPEED_10MB", Ethernet, Some(10)),
        ("SPEED_10M", Ethernet, None),
        ("10MB", Ethernet, None),
        ("SPEED_UNKNOWN", Ethernet, Some(0)),
        ("SPEED_XXGB", Ethernet, None),
        // The Go version panicked.
        ("SPEED_B", Ethernet, None),
    ];
    for (value, unit, expected) in cases {
        assert_eq!(convert_speed(value, *unit).ok(), *expected, "{value} {unit:?}");
    }
}

#[test]
fn paths() {
    let p = parse_path("/interfaces/interface[name=eth1]/state/ifindex").unwrap();
    assert_eq!(p.origin, "");
    assert_eq!(p.elem.len(), 4);
    assert_eq!(p.elem[1].key.get("name").map(|s| s.as_str()), Some("eth1"));
    let p = parse_path("openconfig:/a/b[k1=v/1][k2=v2]").unwrap();
    assert_eq!(p.origin, "openconfig");
    assert_eq!(p.elem[1].key.len(), 2);
    assert_eq!(p.elem[1].key["k1"], "v/1");
    assert!(parse_path("/a[b=c").is_err());
}

/// Requests received by the mock server, with the username sent.
type Requests = Arc<Mutex<Vec<(gnmi::SubscribeRequest, Option<String>)>>>;

/// A gNMI server answering SubscribeOnce with an OpenConfig model.
#[derive(Clone, Default)]
struct MockGnmi {
    requests: Requests,
}

type Stream<T> = std::pin::Pin<Box<dyn futures::Stream<Item = Result<T, tonic::Status>> + Send>>;

#[tonic::async_trait]
impl pb::gnmi::g_nmi_server::GNmi for MockGnmi {
    async fn capabilities(
        &self,
        _: tonic::Request<gnmi::CapabilityRequest>,
    ) -> Result<tonic::Response<gnmi::CapabilityResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn get(
        &self,
        _: tonic::Request<gnmi::GetRequest>,
    ) -> Result<tonic::Response<gnmi::GetResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    async fn set(
        &self,
        _: tonic::Request<gnmi::SetRequest>,
    ) -> Result<tonic::Response<gnmi::SetResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(""))
    }
    type SubscribeStream = Stream<gnmi::SubscribeResponse>;
    async fn subscribe(
        &self,
        request: tonic::Request<tonic::Streaming<gnmi::SubscribeRequest>>,
    ) -> Result<tonic::Response<Self::SubscribeStream>, tonic::Status> {
        let user = request.metadata().get("username").map(|v| v.to_str().unwrap().to_string());
        let mut stream = request.into_inner();
        let req = stream.message().await?.ok_or_else(|| tonic::Status::invalid_argument("no request"))?;
        self.requests.lock().unwrap().push((req.clone(), user));
        let Some(gnmi::subscribe_request::Request::Subscribe(list)) = req.request else {
            return Err(tonic::Status::invalid_argument("not a subscription"));
        };
        let first = list.subscription.first().and_then(|s| s.path.clone()).unwrap_or_default();
        let sync = gnmi::SubscribeResponse {
            response: Some(gnmi::subscribe_response::Response::SyncResponse(true)),
            ..Default::default()
        };
        // Only the OpenConfig model with JSON exists here.
        let names: Vec<&str> = first.elem.iter().map(|e| e.name.as_str()).collect();
        if names != ["system", "config", "hostname"] || list.encoding != Encoding::Json as i32 {
            return Ok(tonic::Response::new(Box::pin(futures::stream::iter([Ok(sync)]))));
        }
        let data = response(
            None,
            vec![
                update(
                    vec![elem("system", &[]), elem("config", &[]), elem("hostname", &[])],
                    V::StringVal("router1".into()),
                ),
                update(
                    vec![elem("interfaces", &[])],
                    json(
                        r#"{"interface":[
                            {"name":"eth1","state":{"ifindex":100,"description":"uplink"},"ethernet":{"state":{"port-speed":"SPEED_100GB"}}},
                            {"name":"eth2","state":{"ifindex":101},"ethernet":{"state":{"port-speed":"SPEED_10GB"}},
                             "subinterfaces":{"subinterface":[{"index":5,"state":{"name":"eth2.5","ifindex":105,"description":"customer"}}]}}]}"#,
                    ),
                ),
            ],
        );
        Ok(tonic::Response::new(Box::pin(futures::stream::iter([Ok(data), Ok(sync)]))))
    }
}

#[tokio::test]
async fn mock_gnmi_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock = MockGnmi::default();
    let incoming = futures::stream::unfold(listener, |l| async move { Some((l.accept().await.map(|(s, _)| s), l)) });
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(pb::gnmi::g_nmi_server::GNmiServer::new(mock.clone()))
            .serve_with_incoming(incoming),
    );

    let r = Reporter::new_mock();
    let daemon = Daemon::new();
    let config: GnmiConfiguration = flowscope_common::config::from_value(
        flowscope_common::config::parse_str(&format!(
            "ports: {{'::/0': {port}}}\nset-target: {{'::/0': true}}\nminimal-refresh-interval: 1s\n\
             authentication-parameters: {{'::/0': {{username: admin, password: secret, tls: {{enable: false}}}}}}\n"
        ))
        .unwrap(),
    )
    .unwrap();
    let p = GnmiProvider::new(&r, config, &daemon).unwrap();
    let exporter = "127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped();
    let query = |if_index| p.query(Query { exporter_ip: exporter, if_index });
    let got = match tokio::time::timeout(Duration::from_secs(10), query(100)).await {
        Ok(got) => got.unwrap(),
        Err(_) => panic!("not ready: {}\n{:?}", r.render_metrics(), mock.requests.lock().unwrap().len()),
    };
    assert_eq!(
        got,
        Answer {
            found: true,
            exporter: Exporter { name: "router1".into(), ..Default::default() },
            interface: Interface {
                name: "eth1".into(),
                description: "uplink".into(),
                speed: 100_000,
                ..Default::default()
            },
        }
    );
    let got = query(105).await.unwrap();
    assert_eq!(
        got.interface,
        Interface { name: "eth2.5".into(), description: "customer".into(), speed: 10_000, ..Default::default() }
    );
    assert_eq!(query(999).await.unwrap(), Answer::default());

    // Detection tried the other models and JSON IETF first. Credentials
    // and target are sent.
    let requests = mock.requests.lock().unwrap().clone();
    assert!(requests.len() >= 3, "{} requests", requests.len());
    assert!(requests.iter().all(|(_, user)| user.as_deref() == Some("admin")));
    let last = &requests.last().unwrap().0;
    let Some(gnmi::subscribe_request::Request::Subscribe(list)) = &last.request else { panic!() };
    assert_eq!(list.prefix.as_ref().map(|p| p.target.as_str()), Some("127.0.0.1"));
    assert_eq!(list.mode, gnmi::subscription_list::Mode::Once as i32);

    let m: std::collections::BTreeMap<String, String> = r
        .render_metrics()
        .lines()
        .filter_map(|l| l.strip_prefix("flowscope_outlet_metadata_provider_gnmi_"))
        .filter_map(|l| l.rsplit_once(' ').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    assert_eq!(m.get(r#"model_info{exporter="127.0.0.1",model="OpenConfig"}"#).map(|s| s.as_str()), Some("1"));
    assert_eq!(m.get(r#"encoding_info{encoding="json",exporter="127.0.0.1"}"#).map(|s| s.as_str()), Some("1"));
    assert_eq!(m.get(r#"collector_ready_info{exporter="127.0.0.1"}"#).map(|s| s.as_str()), Some("1"));
    assert_eq!(m.get("collector_count").map(|s| s.as_str()), Some("1"));
    daemon.terminate();
}

#[tokio::test]
async fn unreachable_exporter() {
    let r = Reporter::new_mock();
    let daemon = Daemon::new();
    let config: GnmiConfiguration = flowscope_common::config::from_value(
        flowscope_common::config::parse_str("ports: {'::/0': 1}\ntimeout: 200ms\n").unwrap(),
    )
    .unwrap();
    let p = GnmiProvider::new(&r, config, &daemon).unwrap();
    let exporter = "127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped();
    // The caller bounds the wait: the collector never becomes ready.
    let got =
        tokio::time::timeout(Duration::from_millis(500), p.query(Query { exporter_ip: exporter, if_index: 1 })).await;
    assert!(got.is_err());
    let rendered = r.render_metrics();
    assert!(rendered.contains(r#"errors_total{error="not ready",exporter="127.0.0.1"} 1"#), "{rendered}");
    assert!(rendered.contains(r#"errors_total{error="cannot create client",exporter="127.0.0.1"}"#), "{rendered}");
    daemon.terminate();
}

#[tokio::test]
async fn mock_gnmi_server_tls() {
    // The test server needs a process-wide crypto provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let identity = tonic::transport::Identity::from_pem(cert.cert.pem(), cert.signing_key.serialize_pem());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let incoming = futures::stream::unfold(listener, |l| async move { Some((l.accept().await.map(|(s, _)| s), l)) });
    tokio::spawn(
        tonic::transport::Server::builder()
            .tls_config(tonic::transport::ServerTlsConfig::new().identity(identity))
            .unwrap()
            .add_service(pb::gnmi::g_nmi_server::GNmiServer::new(MockGnmi::default()))
            .serve_with_incoming(incoming),
    );
    let r = Reporter::new_mock();
    let daemon = Daemon::new();
    let config: GnmiConfiguration = flowscope_common::config::from_value(
        flowscope_common::config::parse_str(&format!(
            "ports: {{'::/0': {port}}}\nauthentication-parameters: {{'::/0': {{tls: {{enable: true, skip-verify: true}}}}}}\n"
        ))
        .unwrap(),
    )
    .unwrap();
    let p = GnmiProvider::new(&r, config, &daemon).unwrap();
    let exporter = "127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped();
    let got = tokio::time::timeout(Duration::from_secs(10), p.query(Query { exporter_ip: exporter, if_index: 100 }))
        .await
        .expect("ready over TLS")
        .unwrap();
    assert_eq!(got.interface.name, "eth1");
    daemon.terminate();
}
