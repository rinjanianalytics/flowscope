// SPDX-FileCopyrightText: 2024 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Port of `outlet/routing/provider/bioris/root_test.go`, plus an interop
//! test against a real bio-rd RIS server.

use super::pb::bio::ris::routing_information_service_server::{
    RoutingInformationService, RoutingInformationServiceServer,
};
use super::*;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::pin::Pin;
use tonic::{Request, Response, Status};

fn v4(s: &str) -> Ipv6Addr {
    s.parse::<Ipv4Addr>().unwrap().to_ipv6_mapped()
}

fn ris(addr: &str) -> Arc<RisInstanceRuntime> {
    Arc::new(RisInstanceRuntime { config: RisInstance { grpc_addr: addr.into(), ..Default::default() }, client: None })
}

fn metrics(r: &Reporter) -> BTreeMap<String, String> {
    r.render_metrics()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("akvorado_outlet_routing_provider_bioris_"))
        .filter_map(|l| l.rsplit_once(' ').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

#[test]
fn choose_router() {
    let r = Reporter::new_mock();
    let p = BiorisProvider::new(&r, BiorisConfiguration::default(), &Daemon::new()).unwrap();
    assert_eq!(p.inner.choose_router(v4("10.0.0.0")).unwrap_err(), Error::NoRouter);

    let (ris1, ris2, ris3) = (ris("ris1"), ris("ris2"), ris("ris3"));
    p.with_routers(HashMap::from([
        (v4("10.0.0.1"), vec![ris1.clone(), ris3.clone()]),
        (v4("10.0.0.2"), vec![ris2.clone()]),
        (v4("10.0.0.3"), vec![ris1.clone(), ris3.clone()]),
        (v4("10.0.0.4"), vec![ris2.clone()]),
        (v4("10.0.0.5"), vec![ris1.clone()]),
    ]));
    let name = |r: &Arc<RisInstanceRuntime>| r.config.grpc_addr.clone();
    for _ in 0..10 {
        let (router, ris) = p.inner.choose_router(v4("10.0.0.1")).unwrap();
        assert_eq!(router, v4("10.0.0.1"));
        assert!(["ris1", "ris3"].contains(&name(&ris).as_str()));
        let (router, ris) = p.inner.choose_router(v4("10.0.0.2")).unwrap();
        assert_eq!((router, name(&ris)), (v4("10.0.0.2"), "ris2".into()));
        // Fallback: the lowest router.
        let (router, ris) = p.inner.choose_router(v4("9.9.9.9")).unwrap();
        assert_eq!(router, v4("10.0.0.1"));
        assert!(["ris1", "ris3"].contains(&name(&ris).as_str()));
    }
}

fn prefix(ip: &str, len: u32) -> Option<pb::Prefix> {
    let mut p = host_prefix(match ip.parse::<IpAddr>().unwrap() {
        IpAddr::V4(v) => v.to_ipv6_mapped(),
        IpAddr::V6(v) => v,
    });
    p.length = len;
    Some(p)
}

fn bgp_path() -> pb::BgpPath {
    pb::BgpPath {
        communities: vec![123],
        large_communities: vec![pb::LargeCommunity { global_administrator: 0, data_part1: 123, data_part2: 456 }],
        as_path: vec![pb::AsPathSegment { as_sequence: false, asns: vec![123, 456] }],
        ..Default::default()
    }
}

#[test]
fn lpm_response_conversion() {
    let full = LookupResult {
        asn: 456,
        as_path: vec![123, 456],
        communities: vec![123],
        large_communities: vec![LargeCommunity { asn: 0, local_data1: 123, local_data2: 456 }],
        ..Default::default()
    };
    let path = |bgp: Option<pb::BgpPath>| pb::Path { bgp_path: bgp, ..Default::default() };
    let route = |pfx, paths| pb::Route { pfx, paths };
    let cases: Vec<(&str, pb::LpmResponse, Result<LookupResult, Error>)> = vec![
        ("without route", pb::LpmResponse::default(), Err(Error::NoRouteFound)),
        ("with empty path", pb::LpmResponse { routes: vec![route(prefix("::", 0), vec![])] }, Err(Error::NoPathFound)),
        (
            "with nil path",
            pb::LpmResponse { routes: vec![route(prefix("::", 0), vec![path(None)])] },
            Err(Error::NoPathFound),
        ),
        (
            "default route and more specific, content",
            pb::LpmResponse {
                routes: vec![
                    route(prefix("::", 0), vec![path(None)]),
                    route(prefix("2001:db8::", 32), vec![path(Some(bgp_path()))]),
                ],
            },
            Ok(LookupResult { net_mask: 32, ..full.clone() }),
        ),
        (
            "default route and more specific, no content",
            pb::LpmResponse {
                routes: vec![
                    route(prefix("::", 0), vec![path(None)]),
                    route(prefix("2001:db8::", 32), vec![path(Some(pb::BgpPath::default()))]),
                ],
            },
            Ok(LookupResult { net_mask: 32, ..Default::default() }),
        ),
        (
            "default route, multiple paths",
            pb::LpmResponse {
                routes: vec![route(prefix("::", 0), vec![path(Some(bgp_path())), path(Some(pb::BgpPath::default()))])],
            },
            Ok(LookupResult { net_mask: 0, ..full.clone() }),
        ),
        (
            "IPv4 next hop",
            pb::LpmResponse {
                routes: vec![route(
                    prefix("192.0.2.0", 24),
                    vec![path(Some(pb::BgpPath {
                        next_hop: prefix("198.51.100.1", 32).unwrap().address,
                        ..bgp_path()
                    }))],
                )],
            },
            Ok(LookupResult { net_mask: 24, next_hop: Some(v4("198.51.100.1")), ..full.clone() }),
        ),
    ];
    for (name, lpm, expected) in cases {
        assert_eq!(lpm_response_to_lookup_result(&lpm), expected, "{name}");
    }
}

/// A RIS server answering with a fixed route.
struct MockRis;

type Stream<T> = Pin<Box<dyn futures::Stream<Item = Result<T, Status>> + Send>>;

#[tonic::async_trait]
impl RoutingInformationService for MockRis {
    async fn lpm(&self, request: Request<pb::LpmRequest>) -> Result<Response<pb::LpmResponse>, Status> {
        let req = request.into_inner();
        if req.router != "127.0.0.1" || req.pfx != prefix("2001:db8:1::10", 128) {
            return Err(Status::not_found(format!("unexpected request {req:?}")));
        }
        let bgp = pb::BgpPath {
            next_hop: prefix("2001:db8::7", 128).unwrap().address,
            as_path: vec![
                pb::AsPathSegment { as_sequence: true, asns: vec![65017, 65013] },
                pb::AsPathSegment { as_sequence: true, asns: vec![174, 174, 174] },
            ],
            communities: vec![4260954122, 4260954132],
            large_communities: vec![pb::LargeCommunity { global_administrator: 65017, data_part1: 300, data_part2: 4 }],
            ..Default::default()
        };
        Ok(Response::new(pb::LpmResponse {
            routes: vec![pb::Route {
                pfx: prefix("2001:db8:1::", 64),
                paths: vec![pb::Path { bgp_path: Some(bgp), ..Default::default() }],
            }],
        }))
    }
    async fn get(&self, _: Request<pb::GetRequest>) -> Result<Response<pb::GetResponse>, Status> {
        Err(Status::unimplemented(""))
    }
    async fn get_routers(&self, _: Request<pb::GetRoutersRequest>) -> Result<Response<pb::GetRoutersResponse>, Status> {
        Ok(Response::new(pb::GetRoutersResponse {
            routers: vec![pb::Router { sys_name: "r1".into(), vrf_ids: vec![0], address: "127.0.0.1".into() }],
        }))
    }
    async fn get_longer(&self, _: Request<pb::GetLongerRequest>) -> Result<Response<pb::GetLongerResponse>, Status> {
        Err(Status::unimplemented(""))
    }
    type ObserveRIBStream = Stream<pb::RibUpdate>;
    async fn observe_rib(&self, _: Request<pb::ObserveRibRequest>) -> Result<Response<Self::ObserveRIBStream>, Status> {
        Err(Status::unimplemented(""))
    }
    type DumpRIBStream = Stream<pb::DumpRibReply>;
    async fn dump_rib(&self, _: Request<pb::DumpRibRequest>) -> Result<Response<Self::DumpRIBStream>, Status> {
        Err(Status::unimplemented(""))
    }
}

/// The lookup the Go test does, and its expected result.
async fn check_lookup(addr: &str, r: &Reporter, p: &BiorisProvider) {
    p.start().unwrap();
    for _ in 0..100 {
        if !p.inner.routers.read().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let got = p.lookup("2001:db8:1::10".parse().unwrap(), None, "2001:db8::7".parse().unwrap()).await.unwrap();
    assert_eq!(
        got,
        LookupResult {
            net_mask: 64,
            asn: 174,
            as_path: vec![65017, 65013, 174, 174, 174],
            communities: vec![4260954122, 4260954132],
            large_communities: vec![LargeCommunity { asn: 65017, local_data1: 300, local_data2: 4 }],
            next_hop: Some("2001:db8::7".parse().unwrap()),
        }
    );
    let got: BTreeMap<String, String> = metrics(r).into_iter().filter(|(k, _)| !k.starts_with("grpc_client")).collect();
    let l = |m: &str| format!(r#"{m}{{ris="{addr}",router="127.0.0.1"}}"#);
    let expected = BTreeMap::from([
        (format!(r#"connection_up{{ris="{addr}"}}"#), "1".to_string()),
        (format!(r#"known_routers_total{{ris="{addr}"}}"#), "1".to_string()),
        (l("lpm_request_errors_total"), "0".to_string()),
        (l("lpm_success_requests_total"), "1".to_string()),
        (l("lpm_request_timeouts_total"), "0".to_string()),
        (l("lpm_requests_total"), "1".to_string()),
        (l("router_agentid_requests_total"), "0".to_string()),
        (l("router_fallback_requests_total"), "1".to_string()),
    ]);
    assert_eq!(got, expected);
}

#[tokio::test]
async fn bioris_mock_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(RoutingInformationServiceServer::new(MockRis))
            .serve_with_incoming(tokio_stream_from(listener)),
    );
    let r = Reporter::new_mock();
    let config = BiorisConfiguration {
        ris_instances: vec![RisInstance { grpc_addr: addr.clone(), ..Default::default() }],
        ..Default::default()
    };
    let p = BiorisProvider::new(&r, config, &Daemon::new()).unwrap();
    check_lookup(&addr, &r, &p).await;
    // Unary RPC metrics, like go-grpc-prometheus.
    let m = metrics(&r);
    let g = |k: &str| m.get(k).cloned().unwrap_or_default();
    assert_eq!(
        g(
            r#"grpc_client_handled_total{grpc_code="Ok",grpc_method="LPM",grpc_service="bio.ris.RoutingInformationService",grpc_type="unary"}"#
        ),
        "1"
    );
}

/// Incoming connections of a listener as a stream, for tonic.
fn tokio_stream_from(
    listener: tokio::net::TcpListener,
) -> impl futures::Stream<Item = Result<tokio::net::TcpStream, std::io::Error>> {
    futures::stream::unfold(listener, |l| async move {
        let r = l.accept().await.map(|(s, _)| s);
        Some((r, l))
    })
}

#[tokio::test]
async fn non_working_bioris() {
    let r = Reporter::new_mock();
    let config = BiorisConfiguration {
        ris_instances: vec![
            RisInstance { grpc_addr: "ris.invalid:1000".into(), ..Default::default() },
            RisInstance { grpc_addr: "127.0.0.1:1".into(), ..Default::default() },
        ],
        refresh_timeout: Duration::from_secs(2),
        ..Default::default()
    };
    let p = BiorisProvider::new(&r, config, &Daemon::new()).unwrap();
    p.inner.refresh().await;
    let m = metrics(&r);
    assert_eq!(m.get(r#"connection_up{ris="ris.invalid:1000"}"#).map(|s| s.as_str()), Some("0"));
    assert_eq!(m.get(r#"connection_up{ris="127.0.0.1:1"}"#).map(|s| s.as_str()), Some("0"));
    assert_eq!(
        m.get(r#"grpc_client_started_total{grpc_method="GetRouters",grpc_service="bio.ris.RoutingInformationService",grpc_type="unary"}"#)
            .map(|s| s.as_str()),
        Some("2")
    );
    let lookup = p.lookup(v4("192.0.2.1"), None, v4("192.0.2.2")).await;
    assert_eq!(lookup, Err("no router".into()));
}

/// Against a real bio-rd RIS server, started by `tests/golden/bioris`
/// with the BMP captures of the Go tests.
#[tokio::test]
#[ignore = "needs Go"]
async fn bioris_interop() {
    use tokio::io::AsyncBufReadExt;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = tokio::process::Command::new("go")
        .args(["run", "./tests/golden/bioris"])
        .current_dir(&root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("go run");
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let addr = tokio::time::timeout(std::time::Duration::from_secs(120), lines.next_line())
        .await
        .expect("RIS server started")
        .unwrap()
        .unwrap();
    let r = Reporter::new_mock();
    let config = BiorisConfiguration {
        ris_instances: vec![RisInstance { grpc_addr: addr.clone(), ..Default::default() }],
        timeout: Duration::from_secs(2),
        ..Default::default()
    };
    let p = BiorisProvider::new(&r, config, &Daemon::new()).unwrap();
    check_lookup(&addr, &r, &p).await;
    drop(child.stdin.take());
    let _ = child.wait().await;
}
