// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A BMP client announcing the configured routes for one peer. Port of
//! `demoexporter/bmp`.

use crate::config::{BmpConfiguration, RouteConfiguration};
use flowscope_common::reporter::{Counter, CounterVec, Reporter};
use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const BMP_ROUTE_MONITORING: u8 = 0;
const BMP_STATISTICS_REPORT: u8 = 1;
const BMP_PEER_UP: u8 = 3;
const BMP_INITIATION: u8 = 4;
const BGP_OPEN: u8 = 1;
const BGP_UPDATE: u8 = 2;
/// BGP identifiers of the simulated router and of its peer.
const LOCAL_ID: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
const PEER_ID: Ipv4Addr = Ipv4Addr::new(2, 2, 2, 2);

fn bmp_message(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + body.len());
    out.push(3); // version
    out.extend_from_slice(&((6 + body.len()) as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(body);
    out
}

fn bgp_message(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xff; 16];
    out.extend_from_slice(&((19 + body.len()) as u16).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(body);
    out
}

/// An address on 16 bytes. IPv4 addresses are at the end.
fn address(ip: IpAddr) -> [u8; 16] {
    match ip {
        IpAddr::V4(ip) => {
            let mut out = [0; 16];
            out[12..].copy_from_slice(&ip.octets());
            out
        }
        IpAddr::V6(ip) => ip.octets(),
    }
}

/// The header naming the peer a message is about. AS paths use four bytes
/// for each AS.
fn peer_header(config: &BmpConfiguration) -> Vec<u8> {
    let mut out = Vec::with_capacity(42);
    out.push(0); // global instance peer
    out.push(if config.peer_ip.is_ipv6() { 0x80 } else { 0 });
    out.extend_from_slice(&[0; 8]); // distinguisher
    out.extend_from_slice(&address(config.peer_ip));
    out.extend_from_slice(&(config.peer_asn as u32).to_be_bytes());
    out.extend_from_slice(&PEER_ID.octets());
    out.extend_from_slice(&[0; 8]); // timestamp
    out
}

/// An OPEN message with the IPv4 and IPv6 unicast capabilities.
fn open(asn: u16, id: Ipv4Addr) -> Vec<u8> {
    let mut capabilities = vec![];
    for afi in [1u16, 2] {
        capabilities.extend_from_slice(&[1, 4]); // multiprotocol, 4 bytes
        capabilities.extend_from_slice(&afi.to_be_bytes());
        capabilities.extend_from_slice(&[0, 1]); // reserved, unicast
    }
    let mut body = vec![4]; // version
    body.extend_from_slice(&asn.to_be_bytes());
    body.extend_from_slice(&30u16.to_be_bytes()); // hold time
    body.extend_from_slice(&id.octets());
    body.push(capabilities.len() as u8 + 2);
    body.extend_from_slice(&[2, capabilities.len() as u8]); // capabilities parameter
    body.extend_from_slice(&capabilities);
    bgp_message(BGP_OPEN, &body)
}

fn attribute(out: &mut Vec<u8>, flags: u8, kind: u8, value: &[u8]) {
    if value.len() > 255 {
        out.extend_from_slice(&[flags | 0x10, kind]); // extended length
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    } else {
        out.extend_from_slice(&[flags, kind, value.len() as u8]);
    }
    out.extend_from_slice(value);
}

/// The UPDATE announcing the prefixes of a route for a family. `None` when
/// the route has no prefix of this family.
fn update(route: &RouteConfiguration, v4: bool) -> Option<Vec<u8>> {
    let prefixes: Vec<&IpNet> = route.prefixes.iter().filter(|p| p.addr().is_ipv4() == v4).collect();
    if prefixes.is_empty() {
        return None;
    }
    const TRANSITIVE: u8 = 0x40;
    const OPTIONAL: u8 = 0x80;
    let mut attributes = vec![];
    attribute(&mut attributes, TRANSITIVE, 1, &[1]); // ORIGIN: EGP
    let mut as_path = vec![];
    // A segment holds at most 255 AS numbers.
    for segment in route.as_path.chunks(255) {
        as_path.extend_from_slice(&[2, segment.len() as u8]); // AS_SEQUENCE
        for asn in segment {
            as_path.extend_from_slice(&asn.to_be_bytes());
        }
    }
    attribute(&mut attributes, TRANSITIVE, 2, &as_path);
    let mut reach = vec![];
    reach.extend_from_slice(&(if v4 { 1u16 } else { 2 }).to_be_bytes());
    reach.push(1); // unicast
    if v4 {
        reach.push(4);
        reach.extend_from_slice(&Ipv4Addr::new(192, 0, 2, 1).octets());
    } else {
        reach.push(16);
        reach.extend_from_slice(&"fe80::1".parse::<Ipv6Addr>().unwrap().octets());
    }
    reach.push(0); // reserved
    for prefix in prefixes {
        let bits = prefix.prefix_len();
        reach.push(bits);
        reach.extend_from_slice(&address(prefix.network())[if v4 { 12 } else { 0 }..][..bits.div_ceil(8) as usize]);
    }
    attribute(&mut attributes, OPTIONAL, 14, &reach); // MP_REACH_NLRI
    if !route.communities.is_empty() {
        let value: Vec<u8> = route.communities.iter().flat_map(|c| c.0.to_be_bytes()).collect();
        attribute(&mut attributes, OPTIONAL | TRANSITIVE, 8, &value);
    }
    if !route.large_communities.is_empty() {
        let value: Vec<u8> = route
            .large_communities
            .iter()
            .flat_map(|c| [c.0.to_be_bytes(), c.1.to_be_bytes(), c.2.to_be_bytes()].concat())
            .collect();
        attribute(&mut attributes, OPTIONAL | TRANSITIVE, 32, &value);
    }
    let mut body = vec![0, 0]; // no withdrawn route
    body.extend_from_slice(&(attributes.len() as u16).to_be_bytes());
    body.extend_from_slice(&attributes);
    Some(bgp_message(BGP_UPDATE, &body))
}

/// The messages sent when the session starts: who we are, the peer coming
/// up, then its routes.
pub fn session(config: &BmpConfiguration) -> Vec<Vec<u8>> {
    let mut messages = vec![];
    let mut tlvs = vec![];
    for (kind, text) in [(1u16, "Fake exporter"), (2, "fake.example.com")] {
        tlvs.extend_from_slice(&kind.to_be_bytes());
        tlvs.extend_from_slice(&(text.len() as u16).to_be_bytes());
        tlvs.extend_from_slice(text.as_bytes());
    }
    messages.push(bmp_message(BMP_INITIATION, &tlvs));
    let header = peer_header(config);
    let mut up = header.clone();
    up.extend_from_slice(&address(config.local_ip));
    up.extend_from_slice(&179u16.to_be_bytes()); // local port
    up.extend_from_slice(&47647u16.to_be_bytes()); // remote port
    up.extend_from_slice(&open(config.local_asn, LOCAL_ID));
    up.extend_from_slice(&open(config.peer_asn, PEER_ID));
    messages.push(bmp_message(BMP_PEER_UP, &up));
    for v4 in [true, false] {
        for route in &config.routes {
            if let Some(update) = update(route, v4) {
                messages.push(bmp_message(BMP_ROUTE_MONITORING, &[header.as_slice(), &update].concat()));
            }
        }
    }
    messages
}

/// An empty statistics report, sent to notice a closed connection.
pub fn statistics(config: &BmpConfiguration) -> Vec<u8> {
    bmp_message(BMP_STATISTICS_REPORT, &[peer_header(config).as_slice(), &[0, 0, 0, 0]].concat())
}

/// The BMP client.
pub struct Bmp {
    config: BmpConfiguration,
    connections: Counter,
    errors: CounterVec,
}

impl Bmp {
    pub fn new(r: &Reporter, config: BmpConfiguration) -> Bmp {
        let m = r.metrics("demoexporter/bmp");
        Bmp {
            config,
            connections: m.counter("connections_total", "Number of successful connections to target."),
            errors: m.counter_vec("errors_total", "Number of unsuccessful connections to target.", &["error"]),
        }
    }

    /// Tell if a target is configured.
    pub fn enabled(&self) -> bool {
        !self.config.target.is_empty()
    }

    /// Send the routes and keep the connection up, until it breaks or the
    /// token is cancelled.
    async fn connect(&self, token: &CancellationToken) {
        let mut stream = match TcpStream::connect(&self.config.target).await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::error!(error = %e, "cannot connect to target");
                self.errors.with_label_values(&["cannot connect"]).inc();
                return;
            }
        };
        self.connections.inc();
        if let Err(e) = stream.write_all(&session(&self.config).concat()).await {
            tracing::error!(error = %e, "cannot write BMP message to target");
            self.errors.with_label_values(&["cannot write"]).inc();
            return;
        }
        // The other end never writes. The only way to see a closed
        // connection is to write.
        let statistics = statistics(&self.config);
        loop {
            if let Err(e) = stream.write_all(&statistics).await {
                use std::io::ErrorKind::{BrokenPipe, ConnectionReset, UnexpectedEof};
                if matches!(e.kind(), BrokenPipe | ConnectionReset | UnexpectedEof) {
                    tracing::info!("remote closed connection");
                    self.errors.with_label_values(&["EOF"]).inc();
                } else {
                    tracing::error!(error = %e, "cannot write to remote");
                    self.errors.with_label_values(&["cannot write"]).inc();
                }
                return;
            }
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(self.config.stats_delay.as_std()) => {}
            }
        }
    }

    /// Connect again and again until the token is cancelled.
    pub async fn run(self, token: CancellationToken) -> Result<(), String> {
        tracing::info!("starting BMP component");
        while !token.is_cancelled() {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = self.connect(&token) => {}
            }
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tokio::time::sleep(self.config.retry_after.as_std()) => {}
            }
        }
        tracing::info!("BMP component stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Community, LargeCommunity};
    use flowscope_outlet::routing::bmp::parse::{
        self, Nlri, RouteMonitoring, UpdateOptions, parse_header, parse_peer_header, parse_route_monitoring,
    };
    use tokio::io::AsyncReadExt;

    fn config() -> BmpConfiguration {
        BmpConfiguration {
            routes: vec![
                RouteConfiguration {
                    prefixes: vec!["192.0.2.0/24".parse().unwrap(), "2a01:db8:cafe:1::/64".parse().unwrap()],
                    as_path: vec![64501],
                    communities: vec![Community((65401 << 16) + 10), Community((65401 << 16) + 12)],
                    large_communities: vec![LargeCommunity(65401, 100, 200)],
                },
                RouteConfiguration {
                    prefixes: vec!["198.38.120.0/23".parse().unwrap(), "203.0.113.128/25".parse().unwrap()],
                    as_path: vec![5511, 1299, 1299, 4_200_000_001],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// Split a stream in messages: type and body.
    fn split(mut data: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut messages = vec![];
        while !data.is_empty() {
            let header = parse_header(data[..6].try_into().unwrap());
            assert_eq!(header.version, 3);
            messages.push((header.msg_type, data[6..header.length as usize].to_vec()));
            data = &data[header.length as usize..];
        }
        messages
    }

    /// The parser of the outlet reads what the client writes.
    #[test]
    fn read_by_the_outlet() {
        let config = config();
        let messages = split(&[session(&config).concat(), statistics(&config)].concat());
        let kinds: Vec<u8> = messages.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            [
                BMP_INITIATION,
                BMP_PEER_UP,
                BMP_ROUTE_MONITORING,
                BMP_ROUTE_MONITORING,
                BMP_ROUTE_MONITORING,
                BMP_STATISTICS_REPORT
            ]
        );

        let tlvs = parse::parse_info_tlvs(&messages[0].1);
        assert_eq!(tlvs, [(1, b"Fake exporter".to_vec()), (2, b"fake.example.com".to_vec())]);

        let peer = parse_peer_header(&messages[1].1).unwrap();
        assert_eq!(peer.address, Some("2001:db8::2".parse().unwrap()));
        assert_eq!(peer.asn, 64497);
        assert_eq!(peer.flags & 0x20, 0, "AS numbers are on four bytes");
        parse::parse_peer_up(&messages[1].1[42..]).unwrap();

        let update = |index: usize| {
            let body = &messages[index].1;
            assert_eq!(parse_peer_header(body).unwrap(), peer);
            match parse_route_monitoring(&body[42..], &UpdateOptions::default()) {
                Ok(RouteMonitoring::Update(update)) => update,
                _ => panic!("message {index} is not a valid update"),
            }
        };
        let prefixes = |update: &parse::Update| -> Vec<String> {
            assert_eq!(update.mp.len(), 1);
            assert!(update.mp[0].reach);
            update.mp[0]
                .paths
                .iter()
                .map(|p| match &p.nlri {
                    Nlri::Prefix(prefix) => prefix.to_string(),
                    other => panic!("unexpected destination {other:?}"),
                })
                .collect()
        };

        // IPv4 routes first, then IPv6 ones.
        let first = update(2);
        assert_eq!(prefixes(&first), ["192.0.2.0/24"]);
        assert_eq!(first.as_path, Some(vec![64501]));
        assert_eq!(first.communities, Some(vec![(65401 << 16) + 10, (65401 << 16) + 12]));
        assert_eq!(
            first.large_communities,
            Some(vec![parse::LargeCommunity { asn: 65401, local_data1: 100, local_data2: 200 }])
        );
        assert_eq!(first.mp[0].next_hop, Some(Ipv4Addr::new(192, 0, 2, 1).to_ipv6_mapped()));

        let second = update(3);
        assert_eq!(prefixes(&second), ["198.38.120.0/23", "203.0.113.128/25"]);
        assert_eq!(second.as_path, Some(vec![5511, 1299, 1299, 4_200_000_001]));
        assert_eq!(second.communities, None);

        let third = update(4);
        assert_eq!(prefixes(&third), ["2a01:db8:cafe:1::/64"]);
        assert_eq!(third.mp[0].next_hop, Some("fe80::1".parse().unwrap()));
    }

    #[tokio::test]
    async fn connects_again() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = BmpConfiguration {
            target: listener.local_addr().unwrap().to_string(),
            retry_after: std::time::Duration::from_millis(10).into(),
            stats_delay: std::time::Duration::from_millis(10).into(),
            ..config()
        };
        let expected = session(&config).concat();
        let token = CancellationToken::new();
        let client = tokio::spawn(Bmp::new(&Reporter::new(&Default::default()), config).run(token.clone()));
        // The routes are sent again after the collector drops the client.
        for _ in 0..2 {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept()).await.unwrap().unwrap();
            let mut received = vec![0; expected.len()];
            stream.read_exact(&mut received).await.unwrap();
            assert_eq!(received, expected);
        }
        token.cancel();
        client.await.unwrap().unwrap();
    }
}
