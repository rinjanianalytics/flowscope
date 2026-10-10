// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! The RIB of the BMP provider. Port of `outlet/routing/provider/bmp/rib.go`.
//!
//! Prefixes live in two prefix trees (IPv4-mapped prefixes are stored as
//! IPv4). Each prefix holds its routes in insertion order. A route is keyed
//! by its peer and its NLRI (family, route distinguisher, path ID). Route
//! attributes are interned: full tables share a few distinct values.

use super::parse::{Family, LargeCommunity};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixMap;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, RwLock};

/// What identifies a route for a prefix and a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Nlri {
    pub family: Family,
    pub path: u32,
    pub rd: u64,
}

/// Route attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RouteAttributes {
    pub asn: u32,
    pub as_path: Vec<u32>,
    pub communities: Vec<u32>,
    pub large_communities: Vec<LargeCommunity>,
}

/// A route of a prefix.
#[derive(Clone, Debug)]
pub struct Route {
    pub peer: u32,
    pub nlri: Nlri,
    pub next_hop: Option<Ipv6Addr>,
    pub attributes: Arc<RouteAttributes>,
    /// Prefix length, as an IPv6 prefix (IPv4 lengths are 96 more).
    pub prefix_len: u8,
}

/// A route to add.
#[derive(Clone, Debug)]
pub struct RawRoute {
    pub peer: u32,
    pub nlri: Nlri,
    pub next_hop: Option<Ipv6Addr>,
    pub attributes: RouteAttributes,
    pub prefix_len: u8,
}

#[derive(Default)]
struct Inner {
    v4: PrefixMap<Ipv4Net, Vec<Route>>,
    v6: PrefixMap<Ipv6Net, Vec<Route>>,
    attributes: HashSet<Arc<RouteAttributes>>,
}

impl Inner {
    fn routes_mut(&mut self, prefix: &IpNet) -> Option<&mut Vec<Route>> {
        match prefix {
            IpNet::V4(p) => self.v4.get_mut(p),
            IpNet::V6(p) => self.v6.get_mut(p),
        }
    }

    fn intern(&mut self, attrs: RouteAttributes) -> Arc<RouteAttributes> {
        if let Some(a) = self.attributes.get(&attrs) {
            return a.clone();
        }
        let a = Arc::new(attrs);
        self.attributes.insert(a.clone());
        a
    }

    /// Release attributes of a removed route.
    fn release(&mut self, attrs: Arc<RouteAttributes>) {
        // The pool and the given reference.
        if Arc::strong_count(&attrs) == 2 {
            self.attributes.remove(&attrs);
        }
    }

    fn remove_prefix(&mut self, prefix: &IpNet) {
        match prefix {
            IpNet::V4(p) => self.v4.remove(p),
            IpNet::V6(p) => self.v6.remove(p),
        };
    }
}

/// The RIB.
#[derive(Default)]
pub struct Rib {
    inner: RwLock<Inner>,
}

/// Turn an IPv4-mapped IPv6 prefix into an IPv4 prefix.
pub fn unmap_prefix(prefix: IpNet) -> IpNet {
    match prefix {
        IpNet::V6(p) if p.prefix_len() >= 96 => match p.addr().to_ipv4_mapped() {
            Some(v4) => IpNet::V4(Ipv4Net::new(v4, p.prefix_len() - 96).unwrap()),
            None => prefix,
        },
        _ => prefix,
    }
}

/// Prefix length as an IPv6 prefix.
pub fn prefix_len6(prefix: &IpNet) -> u8 {
    match prefix {
        IpNet::V4(p) => p.prefix_len() + 96,
        IpNet::V6(p) => p.prefix_len(),
    }
}

impl Rib {
    pub fn new() -> Rib {
        Rib::default()
    }

    /// Add or update a route. Returns the number of routes added (0 for an
    /// update) and whether the prefix is new.
    pub fn add_route(&self, prefix: IpNet, route: RawRoute) -> (usize, bool) {
        let prefix = unmap_prefix(prefix);
        let mut inner = self.inner.write().unwrap();
        let attributes = inner.intern(route.attributes);
        let new = Route {
            peer: route.peer,
            nlri: route.nlri,
            next_hop: route.next_hop,
            attributes,
            prefix_len: route.prefix_len,
        };
        let Some(routes) = inner.routes_mut(&prefix) else {
            match prefix {
                IpNet::V4(p) => inner.v4.insert(p, vec![new]),
                IpNet::V6(p) => inner.v6.insert(p, vec![new]),
            };
            return (1, true);
        };
        if let Some(existing) = routes.iter_mut().find(|r| r.peer == new.peer && r.nlri == new.nlri) {
            let old = std::mem::replace(existing, new);
            inner.release(old.attributes);
            return (0, false);
        }
        routes.push(new);
        (1, false)
    }

    /// Remove the route of a peer for a prefix. Returns the number of routes
    /// removed and whether the prefix was removed.
    pub fn remove_route(&self, prefix: IpNet, peer: u32, nlri: Nlri) -> (usize, bool) {
        let prefix = unmap_prefix(prefix);
        let mut inner = self.inner.write().unwrap();
        let Some(routes) = inner.routes_mut(&prefix) else { return (0, false) };
        let Some(idx) = routes.iter().position(|r| r.peer == peer && r.nlri == nlri) else { return (0, false) };
        let old = routes.remove(idx);
        let empty = routes.is_empty();
        inner.release(old.attributes);
        if empty {
            inner.remove_prefix(&prefix);
        }
        (1, empty)
    }

    /// Remove all the routes of a peer. Returns the number of routes and the
    /// number of prefixes removed.
    pub fn flush_peer(&self, peer: u32) -> (usize, usize) {
        let mut inner = self.inner.write().unwrap();
        let mut released = vec![];
        let mut routes_removed = 0;
        let mut empty_v4 = vec![];
        let mut empty_v6 = vec![];
        let mut flush = |routes: &mut Vec<Route>| {
            let before = routes.len();
            routes.retain(|r| {
                if r.peer == peer {
                    released.push(r.attributes.clone());
                    false
                } else {
                    true
                }
            });
            routes_removed += before - routes.len();
            routes.is_empty()
        };
        for (p, routes) in inner.v4.iter_mut() {
            if flush(routes) {
                empty_v4.push(p);
            }
        }
        for (p, routes) in inner.v6.iter_mut() {
            if flush(routes) {
                empty_v6.push(p);
            }
        }
        let prefixes_removed = empty_v4.len() + empty_v6.len();
        for p in empty_v4 {
            inner.v4.remove(&p);
        }
        for p in empty_v6 {
            inner.v6.remove(&p);
        }
        // Each released attribute was cloned once more above.
        for a in released {
            if Arc::strong_count(&a) == 2 {
                inner.attributes.remove(&a);
            }
        }
        (routes_removed, prefixes_removed)
    }

    /// Look up the most specific route for an address, preferring the route
    /// with the given next hop. Returns a copy of the attributes, the next
    /// hop and the prefix length (as an IPv6 prefix). No reference to the
    /// interned attributes escapes, so they can be released safely.
    pub fn lookup(&self, ip: Ipv6Addr, preferred: Option<Ipv6Addr>) -> Option<(RouteAttributes, Option<Ipv6Addr>, u8)> {
        let inner = self.inner.read().unwrap();
        let routes = match ip.to_ipv4_mapped() {
            Some(v4) => inner.v4.get_lpm(&Ipv4Net::new(v4, 32).unwrap()).map(|(_, r)| r),
            None => inner.v6.get_lpm(&Ipv6Net::new(ip, 128).unwrap()).map(|(_, r)| r),
        }?;
        let route = routes.iter().find(|r| r.next_hop == preferred).or(routes.first())?;
        Some(((*route.attributes).clone(), route.next_hop, route.prefix_len))
    }

    /// Visit all the routes.
    pub fn for_each(&self, mut f: impl FnMut(IpNet, &Route)) {
        let inner = self.inner.read().unwrap();
        for (p, routes) in inner.v4.iter() {
            for r in routes {
                f(IpNet::V4(p), r);
            }
        }
        for (p, routes) in inner.v6.iter() {
            for r in routes {
                f(IpNet::V6(p), r);
            }
        }
    }

    /// Number of distinct route attributes (for tests).
    pub fn attributes_count(&self) -> usize {
        self.inner.read().unwrap().attributes.len()
    }
}

/// Convert an address to an IPv6 one (IPv4 is mapped).
pub fn addr_to6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(peer: u32, path: u32, nh: &str, asn: u32) -> RawRoute {
        RawRoute {
            peer,
            nlri: Nlri { family: Family::IPV4_UNICAST, path, rd: 0 },
            next_hop: Some(addr_to6(nh.parse().unwrap())),
            attributes: RouteAttributes { asn, ..Default::default() },
            prefix_len: 120,
        }
    }

    #[test]
    fn add_update_remove() {
        let rib = Rib::new();
        let p: IpNet = "::ffff:192.0.2.0/120".parse().unwrap();
        assert_eq!(rib.add_route(p, route(1, 0, "192.0.2.1", 100)), (1, true));
        assert_eq!(rib.add_route(p, route(2, 0, "192.0.2.2", 200)), (1, false));
        assert_eq!(rib.add_route(p, route(1, 0, "192.0.2.3", 100)), (0, false));
        assert_eq!(rib.attributes_count(), 2);
        let ip = addr_to6("192.0.2.10".parse().unwrap());
        let (a, nh, plen) = rib.lookup(ip, None).unwrap();
        assert_eq!((a.asn, nh, plen), (100, Some(addr_to6("192.0.2.3".parse().unwrap())), 120));
        let (a, _, _) = rib.lookup(ip, Some(addr_to6("192.0.2.2".parse().unwrap()))).unwrap();
        assert_eq!(a.asn, 200);
        assert_eq!(rib.remove_route(p, 1, route(1, 0, "192.0.2.1", 0).nlri), (1, false));
        assert_eq!(rib.attributes_count(), 1);
        assert_eq!(rib.remove_route(p, 2, route(2, 0, "192.0.2.2", 0).nlri), (1, true));
        assert!(rib.lookup(ip, None).is_none());
        assert_eq!(rib.attributes_count(), 0);
    }

    #[test]
    fn flush_and_lpm() {
        let rib = Rib::new();
        let p24: IpNet = "::ffff:192.0.2.0/120".parse().unwrap();
        let p25: IpNet = "::ffff:192.0.2.0/121".parse().unwrap();
        rib.add_route(p24, route(1, 0, "192.0.2.1", 24));
        rib.add_route(p25, route(1, 0, "192.0.2.1", 25));
        rib.add_route(p25, route(2, 0, "192.0.2.1", 25));
        let lookup = |ip: &str| rib.lookup(addr_to6(ip.parse().unwrap()), None).map(|r| r.0.asn);
        assert_eq!(lookup("192.0.2.1"), Some(25));
        assert_eq!(lookup("192.0.2.200"), Some(24));
        assert_eq!(rib.flush_peer(1), (2, 1));
        assert_eq!(lookup("192.0.2.200"), None);
        assert_eq!(lookup("192.0.2.1"), Some(25));
        assert_eq!(rib.attributes_count(), 1);
    }
}
