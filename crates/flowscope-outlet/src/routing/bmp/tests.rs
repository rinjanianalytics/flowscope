// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Replay the BMP captures of the Go tests and compare the RIB and the
//! metrics with the Go version (`tests/golden/testdata/bmp.json`, written by
//! `outlet/routing/provider/bmp/golden_dump_test.go`).

use super::*;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// TCP payloads of a pcap file, concatenated.
fn pcap_tcp_payload(path: &Path) -> Vec<u8> {
    let data = std::fs::read(path).unwrap();
    let le = u32::from_le_bytes(data[0..4].try_into().unwrap()) == 0xa1b2c3d4;
    let u32_at = |o: usize| {
        let b: [u8; 4] = data[o..o + 4].try_into().unwrap();
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let linktype = u32_at(20);
    let mut out = vec![];
    let mut offset = 24;
    while offset + 16 <= data.len() {
        let caplen = u32_at(offset + 8) as usize;
        let frame = &data[offset + 16..offset + 16 + caplen];
        offset += 16 + caplen;
        let mut d = match linktype {
            1 => &frame[14..],
            113 => &frame[16..],
            0 => &frame[4..],
            l => panic!("unsupported link type {l}"),
        };
        let proto;
        if d[0] >> 4 == 4 {
            let ihl = ((d[0] & 0xf) as usize) * 4;
            let total = u16::from_be_bytes([d[2], d[3]]) as usize;
            proto = d[9];
            d = &d[ihl..total];
        } else {
            let len = u16::from_be_bytes([d[4], d[5]]) as usize;
            proto = d[6];
            d = &d[40..40 + len];
        }
        assert_eq!(proto, 6, "TCP expected");
        let off = ((d[12] >> 4) as usize) * 4;
        out.extend_from_slice(&d[off..]);
    }
    out
}

/// Dump the RIB like the Go golden dumper.
fn dump_rib(p: &BmpProvider) -> BTreeMap<String, Vec<String>> {
    let peers = p.inner.peers.read().unwrap();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let list = |v: &[u32]| format!("[{}]", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "));
    p.inner.rib.for_each(|prefix, route| {
        let peer = peers.peers.iter().find(|(_, i)| i.reference == route.peer).and_then(|(k, _)| k.ip);
        let key = peer.map(|ip| unmap(&ip)).unwrap_or_else(|| "invalid IP".into());
        let a = &route.attributes;
        let large = a
            .large_communities
            .iter()
            .map(|l| format!("{{{} {} {}}}", l.asn, l.local_data1, l.local_data2))
            .collect::<Vec<_>>()
            .join(" ");
        out.entry(key).or_default().push(format!(
            "[{}] {} via {} {}/{} {} {} {} [{}] plen={}",
            route.nlri.family,
            prefix,
            route.next_hop.map(|ip| unmap(&ip)).unwrap_or_else(|| "invalid IP".into()),
            Rd(route.nlri.rd),
            route.nlri.path,
            a.asn,
            list(&a.as_path),
            list(&a.communities),
            large,
            route.prefix_len
        ));
    });
    for v in out.values_mut() {
        v.sort();
    }
    out
}

fn metrics(r: &Reporter) -> BTreeMap<String, String> {
    r.render_metrics()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("akvorado_outlet_routing_provider_bmp_"))
        .filter(|l| !["locked_duration", "buffer_size", "message_queue"].iter().any(|p| l.starts_with(p)))
        .filter_map(|l| l.rsplit_once(' ').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

#[test]
fn golden() {
    let golden: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(root().join("tests/golden/testdata/bmp.json")).unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let mut failures = vec![];
    for sc in &golden {
        let name = sc["name"].as_str().unwrap();
        let strings = |k: &str| -> Vec<String> {
            sc[k].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect()).unwrap_or_default()
        };
        let config = BmpConfiguration {
            listen: "127.0.0.1:0".into(),
            rds: strings("rds").iter().map(|s| s.parse().unwrap()).collect(),
            rts: strings("rts").iter().map(|s| s.parse().unwrap()).collect(),
            collect_asns: !sc["no_asns"].as_bool().unwrap_or(false),
            collect_as_paths: !sc["no_as_paths"].as_bool().unwrap_or(false),
            collect_communities: !sc["no_communities"].as_bool().unwrap_or(false),
            ..Default::default()
        };
        let r = Reporter::new_mock();
        let daemon = Daemon::new();
        let p = {
            let _guard = rt.enter();
            let p = BmpProvider::new(&r, config, &daemon).unwrap();
            p.start().unwrap();
            p
        };
        let addr = p.local_addr().unwrap();
        let mut conns: Vec<Option<std::net::TcpStream>> = vec![];
        let mut offset = std::time::Duration::ZERO;
        let mut snapshots = vec![];
        let sleep = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
        let snapshot = || {
            sleep(50);
            serde_json::json!({"rib": dump_rib(&p), "metrics": metrics(&r)})
        };
        for step in strings("steps") {
            let f: Vec<&str> = step.split_whitespace().collect();
            match f[0] {
                "dial" => {
                    conns.push(Some(std::net::TcpStream::connect(addr).unwrap()));
                    sleep(10);
                }
                "send" => {
                    let i: usize = f[1].parse().unwrap();
                    let payload = pcap_tcp_payload(&root().join("outlet/routing/provider/bmp/testdata").join(f[2]));
                    conns[i].as_mut().unwrap().write_all(&payload).unwrap();
                    sleep(30);
                }
                "close" => {
                    let i: usize = f[1].parse().unwrap();
                    conns[i] = None;
                    sleep(30);
                }
                "clock" => {
                    let d = flowscope_common::config::duration::parse(f[1]).unwrap();
                    offset += d;
                    p.inner.remove_stale_peers(Instant::now() + offset);
                    sleep(30);
                }
                "snapshot" => snapshots.push(snapshot()),
                s => panic!("unknown step {s}"),
            }
        }
        snapshots.push(snapshot());
        let expected = sc["snapshots"].as_array().unwrap();
        for (i, (got, want)) in snapshots.iter().zip(expected).enumerate() {
            let want = serde_json::json!({"rib": want["rib"], "metrics": want["metrics"]});
            if *got != want {
                let mut diff = vec![];
                for part in ["rib", "metrics"] {
                    let (g, w) = (got[part].as_object().unwrap(), want[part].as_object().unwrap());
                    for k in g.keys().chain(w.keys()).collect::<std::collections::BTreeSet<_>>() {
                        if g.get(k) != w.get(k) {
                            diff.push(format!("  {part} {k}:\n    go:   {:?}\n    rust: {:?}", w.get(k), g.get(k)));
                        }
                    }
                }
                failures.push(format!("{name} (snapshot {i}):\n{}", diff.join("\n")));
            }
        }
        daemon.terminate();
        drop(conns);
    }
    assert!(failures.is_empty(), "{} scenarios differ:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn invalid_lengths() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for length in [0u32, 5, (MAX_BMP_MESSAGE_SIZE + 1) as u32, 0xffffffff] {
        let r = Reporter::new_mock();
        let daemon = Daemon::new();
        let p = {
            let _guard = rt.enter();
            let p =
                BmpProvider::new(&r, BmpConfiguration { listen: "127.0.0.1:0".into(), ..Default::default() }, &daemon)
                    .unwrap();
            p.start().unwrap();
            p
        };
        let mut conn = std::net::TcpStream::connect(p.local_addr().unwrap()).unwrap();
        let mut header = vec![3, 0, 0, 0, 0, MSG_INITIATION];
        header[1..5].copy_from_slice(&length.to_be_bytes());
        conn.write_all(&header).unwrap();
        conn.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
        let mut buf = [0u8; 1];
        let got = std::io::Read::read(&mut conn, &mut buf);
        assert!(matches!(got, Ok(0) | Err(_)) && !matches!(&got, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(
            metrics(&r)
                .get(r#"errors_total{error="invalid BMP message length",exporter="127.0.0.1"}"#)
                .map(|s| s.as_str()),
            Some("1"),
            "length {length}"
        );
        daemon.terminate();
    }
}

#[test]
fn lookup() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let r = Reporter::new_mock();
    let p = BmpProvider::new(&r, BmpConfiguration::default(), &Daemon::new()).unwrap();
    let v4 = |s: &str| s.parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped();
    let rta = |asn| RouteAttributes { asn, as_path: vec![64200, asn], communities: vec![100], ..Default::default() };
    for (prefix, nh, asn) in [("192.0.2.0/27", "198.51.100.4", 174), ("192.0.2.0/27", "198.51.100.8", 1299)] {
        let prefix: IpNet = prefix.parse().unwrap();
        p.inner.rib.add_route(
            map_prefix(prefix),
            RawRoute {
                peer: 1,
                nlri: Nlri { family: Family::IPV4_UNICAST, path: asn, rd: 0 },
                next_hop: Some(v4(nh)),
                attributes: rta(asn),
                prefix_len: 123,
            },
        );
    }
    // Not active yet: no data.
    assert_eq!(p.inner.lookup(v4("192.0.2.2"), None), Ok(LookupResult::default()));
    p.inner.active.store(true, Ordering::Relaxed);
    let got = p.inner.lookup(v4("192.0.2.2"), Some(v4("198.51.100.8"))).unwrap();
    assert_eq!((got.asn, got.net_mask, got.next_hop), (1299, 27, Some(v4("198.51.100.8"))));
    let got = p.inner.lookup(v4("192.0.2.2"), None).unwrap();
    assert_eq!((got.asn, got.as_path), (174, vec![64200, 174]));
    assert_eq!(p.inner.lookup(v4("203.0.113.1"), None), Err("no route found".into()));
}
