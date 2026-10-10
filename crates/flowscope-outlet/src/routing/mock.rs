// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Mock provider with a static RIB, for tests. The routes are the ones from
//! `PopulateRIB()` in `outlet/routing/provider/bmp/tests.go`.

use super::{LargeCommunity, LookupResult, Provider};
use futures::future::BoxFuture;
use ipnet::Ipv6Net;
use std::net::Ipv6Addr;

/// A route of the mock RIB.
#[derive(Clone, Debug)]
pub struct MockRoute {
    pub prefix: Ipv6Net,
    pub next_hop: Ipv6Addr,
    pub result: LookupResult,
}

/// Provider looking up a static list of routes. Like the BMP RIB, the
/// longest prefix wins and, among its routes, the one with the preferred
/// next hop, or the first one.
pub struct MockProvider {
    pub routes: Vec<MockRoute>,
}

fn v4(s: &str) -> Ipv6Addr {
    s.parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped()
}

impl MockProvider {
    /// The routes of `PopulateRIB()`.
    pub fn populated() -> MockProvider {
        let route = |prefix: &str,
                     plen: u8,
                     nh: &str,
                     asn: u32,
                     as_path: &[u32],
                     communities: &[u32],
                     lc: &[LargeCommunity]| {
            let (addr, _) = prefix.split_once('/').unwrap();
            MockRoute {
                prefix: Ipv6Net::new(v4(addr), plen).unwrap().trunc(),
                next_hop: v4(nh),
                result: LookupResult {
                    asn,
                    as_path: as_path.to_vec(),
                    communities: communities.to_vec(),
                    large_communities: lc.to_vec(),
                    net_mask: plen - 96,
                    next_hop: Some(v4(nh)),
                },
            }
        };
        let lc = [LargeCommunity { asn: 64200, local_data1: 2, local_data2: 3 }];
        MockProvider {
            routes: vec![
                route("192.0.2.0/27", 123, "198.51.100.4", 174, &[64200, 1299, 174], &[100, 200, 400], &lc),
                route("192.0.2.0/27", 123, "198.51.100.8", 174, &[64200, 174, 174, 174], &[100], &[]),
                route("192.0.2.128/27", 123, "198.51.100.8", 1299, &[64200, 1299], &[500], &[]),
                route("1.0.0.0/24", 120, "198.51.100.8", 65300, &[], &[], &[]),
                route("192.168.144.0/21", 117, "203.0.113.14", 1234, &[54321, 1234], &[], &[]),
                route("192.168.144.0/22", 118, "203.0.113.15", 1234, &[1234], &[], &[]),
                route("192.168.148.0/22", 118, "203.0.113.15", 1234, &[1234], &[], &[]),
                route("192.168.148.1/32", 128, "203.0.113.14", 1234, &[1234], &[], &[]),
            ],
        }
    }
}

impl Provider for MockProvider {
    fn lookup(
        &self,
        ip: Ipv6Addr,
        nh: Option<Ipv6Addr>,
        _agent: Ipv6Addr,
    ) -> BoxFuture<'_, Result<LookupResult, String>> {
        let best = self.routes.iter().filter(|r| r.prefix.contains(&ip)).map(|r| r.prefix.prefix_len()).max();
        let result = best
            .and_then(|plen| {
                let mut candidates =
                    self.routes.iter().filter(|r| r.prefix.prefix_len() == plen && r.prefix.contains(&ip));
                let first = candidates.clone().next();
                candidates.find(|r| Some(r.next_hop) == nh).or(first).map(|r| r.result.clone())
            })
            .ok_or_else(|| "no route found".to_string());
        Box::pin(async move { result })
    }
}
