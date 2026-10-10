// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! BMP (RFC 7854, RFC 9069) and BGP (RFC 4271, RFC 4760, RFC 7911) parsing,
//! limited to what the BMP provider needs. The Go version relies on gobgp.
//! Errors in BGP updates are classified like gobgp does (RFC 7606), so the
//! same updates are ignored.

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use std::collections::HashMap;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

pub const HEADER_SIZE: usize = 6;
pub const PEER_HEADER_SIZE: usize = 42;

pub const MSG_ROUTE_MONITORING: u8 = 0;
pub const MSG_STATISTICS_REPORT: u8 = 1;
pub const MSG_PEER_DOWN_NOTIFICATION: u8 = 2;
pub const MSG_PEER_UP_NOTIFICATION: u8 = 3;
pub const MSG_INITIATION: u8 = 4;
pub const MSG_TERMINATION: u8 = 5;
pub const MSG_ROUTE_MIRRORING: u8 = 6;

pub const PEER_TYPE_GLOBAL: u8 = 0;
pub const PEER_TYPE_L3VPN: u8 = 1;
pub const PEER_TYPE_LOCAL_RIB: u8 = 3;

const PEER_FLAG_IPV6: u8 = 0x80;

/// The common BMP header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub length: u32,
    pub msg_type: u8,
}

pub fn parse_header(data: &[u8; HEADER_SIZE]) -> Header {
    Header { version: data[0], length: u32::from_be_bytes(data[1..5].try_into().unwrap()), msg_type: data[5] }
}

/// The per-peer header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerHeader {
    pub peer_type: u8,
    pub flags: u8,
    pub distinguisher: u64,
    /// Peer address (IPv4-mapped for IPv4). None for a Loc-RIB peer.
    pub address: Option<Ipv6Addr>,
    pub asn: u32,
    pub bgp_id: u32,
}

pub fn parse_peer_header(data: &[u8]) -> Result<PeerHeader, String> {
    if data.len() < PEER_HEADER_SIZE {
        return Err("invalid BMP Peer header length".into());
    }
    let peer_type = data[0];
    let flags = data[1];
    let address = if peer_type == PEER_TYPE_LOCAL_RIB {
        None
    } else if flags & PEER_FLAG_IPV6 != 0 {
        Some(Ipv6Addr::from(<[u8; 16]>::try_from(&data[10..26]).unwrap()))
    } else {
        Some(Ipv4Addr::new(data[22], data[23], data[24], data[25]).to_ipv6_mapped())
    };
    Ok(PeerHeader {
        peer_type,
        flags,
        distinguisher: u64::from_be_bytes(data[2..10].try_into().unwrap()),
        address,
        asn: u32::from_be_bytes(data[26..30].try_into().unwrap()),
        bgp_id: u32::from_be_bytes(data[30..34].try_into().unwrap()),
    })
}

/// Information TLVs from initiation and termination messages.
pub fn parse_info_tlvs(mut data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = vec![];
    while data.len() >= 4 {
        let t = u16::from_be_bytes([data[0], data[1]]);
        let l = u16::from_be_bytes([data[2], data[3]]) as usize;
        if data.len() < 4 + l {
            break;
        }
        out.push((t, data[4..4 + l].to_vec()));
        data = &data[4 + l..];
    }
    out
}

/// An address family (AFI, SAFI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Family(pub u32);

pub const AFI_IP: u16 = 1;
pub const AFI_IP6: u16 = 2;
pub const AFI_L2VPN: u16 = 25;
pub const AFI_LS: u16 = 16388;
pub const SAFI_UNICAST: u8 = 1;
pub const SAFI_MULTICAST: u8 = 2;
pub const SAFI_MPLS_LABEL: u8 = 4;
pub const SAFI_ENCAPSULATION: u8 = 7;
pub const SAFI_VPLS: u8 = 65;
pub const SAFI_EVPN: u8 = 70;
pub const SAFI_LS: u8 = 71;
pub const SAFI_SR_POLICY: u8 = 73;
pub const SAFI_MUP: u8 = 85;
pub const SAFI_MPLS_VPN: u8 = 128;
pub const SAFI_MPLS_VPN_MULTICAST: u8 = 129;
pub const SAFI_ROUTE_TARGET_CONSTRAINTS: u8 = 132;
pub const SAFI_FLOW_SPEC_UNICAST: u8 = 133;
pub const SAFI_FLOW_SPEC_VPN: u8 = 134;

impl Family {
    pub const IPV4_UNICAST: Family = Family::new(AFI_IP, SAFI_UNICAST);

    pub const fn new(afi: u16, safi: u8) -> Family {
        Family(((afi as u32) << 16) | safi as u32)
    }

    pub fn afi(self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub fn safi(self) -> u8 {
        self.0 as u8
    }

    /// Name, like gobgp. None when gobgp does not know the family.
    pub fn name(self) -> Option<&'static str> {
        Some(match (self.afi(), self.safi()) {
            (AFI_IP, SAFI_UNICAST) => "ipv4-unicast",
            (AFI_IP6, SAFI_UNICAST) => "ipv6-unicast",
            (AFI_IP, SAFI_MULTICAST) => "ipv4-multicast",
            (AFI_IP6, SAFI_MULTICAST) => "ipv6-multicast",
            (AFI_IP, SAFI_MPLS_LABEL) => "ipv4-labelled-unicast",
            (AFI_IP6, SAFI_MPLS_LABEL) => "ipv6-labelled-unicast",
            (AFI_IP, SAFI_MPLS_VPN) => "l3vpn-ipv4-unicast",
            (AFI_IP6, SAFI_MPLS_VPN) => "l3vpn-ipv6-unicast",
            (AFI_IP, SAFI_MPLS_VPN_MULTICAST) => "l3vpn-ipv4-multicast",
            (AFI_IP6, SAFI_MPLS_VPN_MULTICAST) => "l3vpn-ipv6-multicast",
            (AFI_L2VPN, SAFI_VPLS) => "l2vpn-vpls",
            (AFI_L2VPN, SAFI_EVPN) => "l2vpn-evpn",
            (AFI_IP, SAFI_ROUTE_TARGET_CONSTRAINTS) => "rtc",
            (AFI_IP, SAFI_ENCAPSULATION) => "ipv4-encap",
            (AFI_IP6, SAFI_ENCAPSULATION) => "ipv6-encap",
            (AFI_IP, SAFI_FLOW_SPEC_UNICAST) => "ipv4-flowspec",
            (AFI_IP, SAFI_FLOW_SPEC_VPN) => "l3vpn-ipv4-flowspec",
            (AFI_IP6, SAFI_FLOW_SPEC_UNICAST) => "ipv6-flowspec",
            (AFI_IP6, SAFI_FLOW_SPEC_VPN) => "l3vpn-ipv6-flowspec",
            (AFI_L2VPN, SAFI_FLOW_SPEC_VPN) => "l2vpn-flowspec",
            (AFI_LS, SAFI_LS) => "ls",
            (AFI_IP, SAFI_SR_POLICY) => "ipv4-srpolicy",
            (AFI_IP6, SAFI_SR_POLICY) => "ipv6-srpolicy",
            (AFI_IP, SAFI_MUP) => "ipv4-mup",
            (AFI_IP6, SAFI_MUP) => "ipv6-mup",
            _ => return None,
        })
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => f.write_str(n),
            None => write!(f, "{}-{}", self.afi(), self.safi()),
        }
    }
}

/// How to handle an error in an update, by increasing strength (RFC 7606).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorHandling {
    None,
    AttributeDiscard,
    TreatAsWithdraw,
    AfiSafiDisable,
    SessionReset,
}

impl ErrorHandling {
    /// Label used in metrics, like the Go version.
    pub fn label(self) -> &'static str {
        match self {
            ErrorHandling::None => "none",
            ErrorHandling::AttributeDiscard => "attribute-discard",
            ErrorHandling::TreatAsWithdraw => "treat-as-withdraw",
            ErrorHandling::AfiSafiDisable => "afi-safi",
            ErrorHandling::SessionReset => "session-reset",
        }
    }
}

/// An error in a BGP message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageError {
    pub handling: ErrorHandling,
    pub message: String,
}

impl MessageError {
    fn reset(message: impl Into<String>) -> Self {
        MessageError { handling: ErrorHandling::SessionReset, message: message.into() }
    }
}

const BGP_HEADER_SIZE: usize = 19;
const BGP_MSG_OPEN: u8 = 1;
const BGP_MSG_UPDATE: u8 = 2;

/// Split a BGP message: type, body and total length.
fn bgp_message(data: &[u8]) -> Result<(u8, &[u8], usize), MessageError> {
    if data.len() < BGP_HEADER_SIZE {
        return Err(MessageError::reset("not all BGP message header"));
    }
    let len = u16::from_be_bytes([data[16], data[17]]) as usize;
    if len < BGP_HEADER_SIZE || len > data.len() {
        return Err(MessageError::reset("unknown message type"));
    }
    Ok((data[18], &data[BGP_HEADER_SIZE..len], len))
}

/// ADD-PATH modes, as in the capability.
pub const ADD_PATH_RECEIVE: u8 = 1;
pub const ADD_PATH_SEND: u8 = 2;
pub const ADD_PATH_BOTH: u8 = 3;

/// The parts of an OPEN message we need: the ADD-PATH capability.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Open {
    pub add_path: Vec<(Family, u8)>,
}

fn parse_open_body(body: &[u8]) -> Result<Open, MessageError> {
    if body.len() < 10 {
        return Err(MessageError::reset("open message is too short"));
    }
    let mut params_len = body[9] as usize;
    let mut params = &body[10..];
    let mut extended = false;
    // RFC 9072: extended optional parameters length.
    if params_len == 255 && params.first() == Some(&255) {
        if params.len() < 3 {
            return Err(MessageError::reset("open message is too short"));
        }
        params_len = u16::from_be_bytes([params[1], params[2]]) as usize;
        params = &params[3..];
        extended = true;
    }
    if params.len() < params_len {
        return Err(MessageError::reset("optional parameters are too short"));
    }
    params = &params[..params_len];
    let mut open = Open::default();
    let hdr = if extended { 3 } else { 2 };
    while params.len() >= hdr {
        let ptype = params[0];
        let plen = if extended { u16::from_be_bytes([params[1], params[2]]) as usize } else { params[1] as usize };
        if params.len() < hdr + plen {
            return Err(MessageError::reset("optional parameter is too short"));
        }
        let mut caps = &params[hdr..hdr + plen];
        params = &params[hdr + plen..];
        if ptype != 2 {
            continue;
        }
        while caps.len() >= 2 {
            let (code, clen) = (caps[0], caps[1] as usize);
            if caps.len() < 2 + clen {
                return Err(MessageError::reset("capability is too short"));
            }
            let value = &caps[2..2 + clen];
            caps = &caps[2 + clen..];
            if code == 69 {
                for t in value.chunks_exact(4) {
                    open.add_path.push((Family::new(u16::from_be_bytes([t[0], t[1]]), t[2]), t[3]));
                }
            }
        }
    }
    Ok(open)
}

/// The OPEN messages of a peer up notification.
pub fn parse_peer_up(body: &[u8]) -> Result<(Open, Open), String> {
    if body.len() < 20 {
        return Err(format!("BMP Peer Up Notification body too short: {} bytes", body.len()));
    }
    let data = &body[20..];
    let (t, sent_body, len) = bgp_message(data).map_err(|e| e.message)?;
    if t != BGP_MSG_OPEN {
        return Err("sent message is not an OPEN".into());
    }
    let sent = parse_open_body(sent_body).map_err(|e| e.message)?;
    let (t, received_body, _) = bgp_message(&data[len..]).map_err(|e| e.message)?;
    if t != BGP_MSG_OPEN {
        return Err("received message is not an OPEN".into());
    }
    let received = parse_open_body(received_body).map_err(|e| e.message)?;
    Ok((sent, received))
}

/// Families for which the peer sends ADD-PATH and the station receives it,
/// from the sent and received OPEN messages.
pub fn add_path_families(sent: &Open, received: &Open) -> Vec<Family> {
    let received: HashMap<Family, u8> = received.add_path.iter().copied().collect();
    let mut out = vec![];
    for (family, mode) in &sent.add_path {
        let r = received.get(family).copied().unwrap_or(0);
        if (r == ADD_PATH_BOTH || r == ADD_PATH_SEND) && (*mode == ADD_PATH_BOTH || *mode == ADD_PATH_RECEIVE) {
            out.push(*family);
        }
    }
    out
}

/// A large community.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LargeCommunity {
    pub asn: u32,
    pub local_data1: u32,
    pub local_data2: u32,
}

/// A destination in an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nlri {
    /// Plain or labeled prefix: the route distinguisher comes from the peer.
    Prefix(IpNet),
    /// VPN or EVPN IP prefix route, with its route distinguisher.
    WithRd(u64, IpNet),
    /// A destination of a family not handled.
    Unhandled,
}

/// A destination with its path identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathNlri {
    pub nlri: Nlri,
    pub path_id: u32,
}

/// Content of a MP_REACH_NLRI or MP_UNREACH_NLRI attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MpNlri {
    /// MP_REACH_NLRI (true) or MP_UNREACH_NLRI (false).
    pub reach: bool,
    pub family: Family,
    /// Next hop (IPv4 mapped to IPv6), only for MP_REACH_NLRI.
    pub next_hop: Option<Ipv6Addr>,
    pub paths: Vec<PathNlri>,
}

/// The attributes of an update the provider uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub withdrawn: Vec<PathNlri>,
    pub nlri: Vec<PathNlri>,
    pub next_hop: Option<Ipv6Addr>,
    pub as_path: Option<Vec<u32>>,
    pub communities: Option<Vec<u32>>,
    pub large_communities: Option<Vec<LargeCommunity>>,
    pub extended_communities: Vec<[u8; 8]>,
    /// MP_REACH_NLRI and MP_UNREACH_NLRI, in order of appearance.
    pub mp: Vec<MpNlri>,
}

/// Options to parse an update.
#[derive(Clone, Debug, Default)]
pub struct UpdateOptions {
    /// Families with ADD-PATH path identifiers.
    pub add_path: Vec<Family>,
    /// AS numbers are encoded on two bytes in AS_PATH.
    pub two_byte_as: bool,
}

/// Result of parsing a route monitoring message.
pub enum RouteMonitoring {
    /// Not an UPDATE message.
    Other,
    /// An update, possibly with an error that does not prevent using it.
    Update(Update),
}

/// Parse the BGP message of a route monitoring message. An error with a
/// handling stronger than "attribute discard" means the update should be
/// ignored.
pub fn parse_route_monitoring(data: &[u8], options: &UpdateOptions) -> Result<RouteMonitoring, MessageError> {
    let (t, body, _) = bgp_message(data)?;
    if t != BGP_MSG_UPDATE {
        return Ok(RouteMonitoring::Other);
    }
    parse_update(body, options).map(RouteMonitoring::Update)
}

/// Read a prefix encoded as a length in bits followed by the significant
/// bytes. `max_bits` is 32 or 128.
fn read_prefix(data: &[u8], max_bits: u8) -> Result<(IpNet, usize), String> {
    let bits = *data.first().ok_or("prefix length is missing")?;
    if bits > max_bits {
        return Err(format!("prefix length {bits} is too long"));
    }
    let nbytes = (bits as usize).div_ceil(8);
    if data.len() < 1 + nbytes {
        return Err("prefix is short".into());
    }
    Ok((prefix_from_bytes(&data[1..1 + nbytes], bits, max_bits)?, 1 + nbytes))
}

fn prefix_from_bytes(bytes: &[u8], bits: u8, max_bits: u8) -> Result<IpNet, String> {
    let net = if max_bits == 32 {
        let mut a = [0u8; 4];
        a[..bytes.len()].copy_from_slice(bytes);
        IpNet::V4(Ipv4Net::new(Ipv4Addr::from(a), bits).map_err(|e| e.to_string())?.trunc())
    } else {
        let mut a = [0u8; 16];
        a[..bytes.len()].copy_from_slice(bytes);
        IpNet::V6(Ipv6Net::new(Ipv6Addr::from(a), bits).map_err(|e| e.to_string())?.trunc())
    };
    Ok(net)
}

/// Skip MPLS labels in a prefix: 3 bytes each, until the bottom of stack.
/// Returns the number of bytes used by labels.
fn skip_labels(data: &[u8], available_bits: usize) -> usize {
    let mut used = 0;
    while (used + 3) * 8 <= available_bits && data.len() >= used + 3 {
        let label = u32::from_be_bytes([0, data[used], data[used + 1], data[used + 2]]);
        used += 3;
        // Bottom of stack, or the withdrawal values from RFC 8277.
        if label & 1 == 1 || label == 0x800000 || label == 0 {
            break;
        }
    }
    used
}

/// Read one destination of a family. Returns the NLRI and its length.
fn read_nlri(family: Family, data: &[u8]) -> Result<(Nlri, usize), String> {
    let max_bits = if family.afi() == AFI_IP6 { 128 } else { 32 };
    match (family.afi(), family.safi()) {
        (AFI_IP | AFI_IP6, SAFI_UNICAST | SAFI_MULTICAST) => {
            let (p, n) = read_prefix(data, max_bits)?;
            Ok((Nlri::Prefix(p), n))
        }
        (AFI_IP | AFI_IP6, SAFI_MPLS_LABEL) => {
            let bits = *data.first().ok_or("prefix length is missing")? as usize;
            let nbytes = bits.div_ceil(8);
            if data.len() < 1 + nbytes {
                return Err("labeled prefix is short".into());
            }
            let labels = skip_labels(&data[1..], bits);
            let pbits = bits - labels * 8;
            if pbits > max_bits as usize {
                return Err("labeled prefix is too long".into());
            }
            let p = prefix_from_bytes(&data[1 + labels..1 + nbytes], pbits as u8, max_bits)?;
            Ok((Nlri::Prefix(p), 1 + nbytes))
        }
        (AFI_IP | AFI_IP6, SAFI_MPLS_VPN | SAFI_MPLS_VPN_MULTICAST) => {
            let bits = *data.first().ok_or("prefix length is missing")? as usize;
            let nbytes = bits.div_ceil(8);
            if data.len() < 1 + nbytes {
                return Err("VPN prefix is short".into());
            }
            let labels = skip_labels(&data[1..], bits);
            if bits < (labels + 8) * 8 {
                return Err("VPN prefix is too short for a route distinguisher".into());
            }
            let rd = u64::from_be_bytes(data[1 + labels..1 + labels + 8].try_into().unwrap());
            let pbits = bits - (labels + 8) * 8;
            if pbits > max_bits as usize {
                return Err("VPN prefix is too long".into());
            }
            let p = prefix_from_bytes(&data[1 + labels + 8..1 + nbytes], pbits as u8, max_bits)?;
            Ok((Nlri::WithRd(rd, p), 1 + nbytes))
        }
        (AFI_L2VPN, SAFI_EVPN) => {
            if data.len() < 2 || data.len() < 2 + data[1] as usize {
                return Err("EVPN route is short".into());
            }
            let (rtype, len) = (data[0], data[1] as usize);
            let route = &data[2..2 + len];
            // Type 5: IP prefix route (RFC 9136).
            if rtype == 5 && (len == 34 || len == 58) {
                let rd = u64::from_be_bytes(route[0..8].try_into().unwrap());
                let bits = route[22];
                let (addr, max) = if len == 34 { (&route[23..27], 32) } else { (&route[23..39], 128) };
                if bits > max {
                    return Err("EVPN prefix is too long".into());
                }
                let p = prefix_from_bytes(addr, bits, max)?;
                return Ok((Nlri::WithRd(rd, p), 2 + len));
            }
            Ok((Nlri::Unhandled, 2 + len))
        }
        (AFI_L2VPN, SAFI_VPLS) => {
            if data.len() < 2 || data.len() < 2 + u16::from_be_bytes([data[0], data[1]]) as usize {
                return Err("VPLS route is short".into());
            }
            Ok((Nlri::Unhandled, 2 + u16::from_be_bytes([data[0], data[1]]) as usize))
        }
        (AFI_IP | AFI_IP6, SAFI_ENCAPSULATION | SAFI_SR_POLICY) | (AFI_IP, SAFI_ROUTE_TARGET_CONSTRAINTS) => {
            let bits = *data.first().ok_or("length is missing")? as usize;
            let n = 1 + bits.div_ceil(8);
            if data.len() < n {
                return Err("route is short".into());
            }
            Ok((Nlri::Unhandled, n))
        }
        (AFI_IP | AFI_IP6 | AFI_L2VPN, SAFI_FLOW_SPEC_UNICAST | SAFI_FLOW_SPEC_VPN) => {
            let first = *data.first().ok_or("length is missing")? as usize;
            let (hdr, len) = if first >= 0xf0 {
                if data.len() < 2 {
                    return Err("flowspec length is short".into());
                }
                (2, ((first & 0x0f) << 8) | data[1] as usize)
            } else {
                (1, first)
            };
            if data.len() < hdr + len {
                return Err("flowspec route is short".into());
            }
            Ok((Nlri::Unhandled, hdr + len))
        }
        (AFI_LS, SAFI_LS) => {
            if data.len() < 4 || data.len() < 4 + u16::from_be_bytes([data[2], data[3]]) as usize {
                return Err("link-state route is short".into());
            }
            Ok((Nlri::Unhandled, 4 + u16::from_be_bytes([data[2], data[3]]) as usize))
        }
        (AFI_IP | AFI_IP6, SAFI_MUP) => {
            if data.len() < 4 || data.len() < 4 + data[3] as usize {
                return Err("MUP route is short".into());
            }
            Ok((Nlri::Unhandled, 4 + data[3] as usize))
        }
        _ => Err(format!("unknown AFI/SAFI {}/{}", family.afi(), family.safi())),
    }
}

/// Read a list of destinations of a family.
fn read_nlris(family: Family, mut data: &[u8], add_path: bool) -> Result<Vec<PathNlri>, String> {
    let mut out = vec![];
    while !data.is_empty() {
        let mut path_id = 0;
        if add_path {
            if data.len() < 4 {
                return Err("addpath length is incorrect".into());
            }
            path_id = u32::from_be_bytes(data[..4].try_into().unwrap());
            data = &data[4..];
        }
        let (nlri, n) = read_nlri(family, data)?;
        data = &data[n..];
        out.push(PathNlri { nlri, path_id });
    }
    Ok(out)
}

/// Error handling for an attribute that fails to decode, like gobgp.
fn handling_for_attribute(t: u8) -> ErrorHandling {
    match t {
        1..=5 | 8..=10 | 16 | 17 | 18 | 22 | 25 | 32 => ErrorHandling::TreatAsWithdraw,
        14 | 15 => ErrorHandling::AfiSafiDisable,
        _ => ErrorHandling::AttributeDiscard,
    }
}

/// Flatten an AS path: the first AS of a set is used, confederation
/// sequences are regular sequences.
fn parse_as_path(mut data: &[u8], as_size: usize) -> Result<Vec<u32>, String> {
    let mut out = vec![];
    while !data.is_empty() {
        if data.len() < 2 {
            return Err("AS PATH header is short".into());
        }
        let (seg_type, count) = (data[0], data[1] as usize);
        if seg_type == 0 || seg_type > 4 {
            return Err("unknown AS_PATH seg type".into());
        }
        if count == 0 {
            return Err("AS PATH segment has zero AS count".into());
        }
        let len = count * as_size;
        if data.len() < 2 + len {
            return Err("seg length is short".into());
        }
        let mut asns = data[2..2 + len].chunks_exact(as_size).map(|c| {
            if as_size == 4 {
                u32::from_be_bytes(c.try_into().unwrap())
            } else {
                u16::from_be_bytes([c[0], c[1]]) as u32
            }
        });
        match seg_type {
            // AS_SET and AS_CONFED_SET
            1 | 4 => out.extend(asns.next()),
            _ => out.extend(asns),
        }
        data = &data[2 + len..];
    }
    Ok(out)
}

fn parse_mp(t: u8, value: &[u8], options: &UpdateOptions) -> Result<MpNlri, String> {
    if value.len() < 3 {
        return Err("mpreach header length is short".into());
    }
    let family = Family::new(u16::from_be_bytes([value[0], value[1]]), value[2]);
    let mut rest = &value[3..];
    let mut next_hop = None;
    if t == 14 {
        let nhlen = *rest.first().ok_or("mpreach nexthop length is short")? as usize;
        if rest.len() < 1 + nhlen {
            return Err("mpreach nexthop length is short".into());
        }
        let mut nh = &rest[1..1 + nhlen];
        rest = &rest[1 + nhlen..];
        // VPN next hops start with a zero route distinguisher.
        if matches!(nhlen, 12 | 24 | 48) {
            nh = &nh[8..];
        }
        next_hop = match nh.len() {
            0 => {
                if !matches!(family.safi(), SAFI_FLOW_SPEC_UNICAST | SAFI_FLOW_SPEC_VPN | 241) {
                    return Err("mpreach nexthop length is zero for a family that requires a next hop".into());
                }
                None
            }
            4 => Some(Ipv4Addr::new(nh[0], nh[1], nh[2], nh[3]).to_ipv6_mapped()),
            // Global address, possibly followed by a link-local one (with
            // its own route distinguisher for VPN).
            16 | 32 | 40 => Some(Ipv6Addr::from(<[u8; 16]>::try_from(&nh[..16]).unwrap())),
            _ => return Err("mpreach nexthop length is incorrect".into()),
        };
        if rest.is_empty() {
            return Err("no skip byte".into());
        }
        rest = &rest[1..];
    }
    if family.name().is_none() {
        return Err(format!("unknown AFI/SAFI {}/{}", family.afi(), family.safi()));
    }
    let paths = read_nlris(family, rest, options.add_path.contains(&family))?;
    Ok(MpNlri { reach: t == 14, family, next_hop, paths })
}

fn parse_update(data: &[u8], options: &UpdateOptions) -> Result<Update, MessageError> {
    if data.len() < 2 {
        return Err(MessageError::reset("message length isn't enough for withdrawn route length"));
    }
    let wlen = u16::from_be_bytes([data[0], data[1]]) as usize;
    let data = &data[2..];
    if data.len() < wlen {
        return Err(MessageError::reset("withdrawn route length exceeds message length"));
    }
    let v4_add_path = options.add_path.contains(&Family::IPV4_UNICAST);
    let mut update = Update {
        withdrawn: read_nlris(Family::IPV4_UNICAST, &data[..wlen], v4_add_path).map_err(MessageError::reset)?,
        ..Default::default()
    };
    let data = &data[wlen..];
    if data.len() < 2 {
        return Err(MessageError::reset("message length isn't enough for path total attribute length"));
    }
    let alen = u16::from_be_bytes([data[0], data[1]]) as usize;
    let data = &data[2..];
    if data.len() < alen {
        return Err(MessageError::reset("path total attribute length exceeds message length"));
    }
    let (mut attrs, nlri) = data.split_at(alen);

    let mut strongest: Option<MessageError> = None;
    let mut record = |e: MessageError| {
        if strongest.as_ref().is_none_or(|s| e.handling > s.handling) {
            strongest = Some(e);
        }
    };
    let as_size = if options.two_byte_as { 2 } else { 4 };
    while !attrs.is_empty() {
        if attrs.len() < 3 {
            record(MessageError {
                handling: ErrorHandling::TreatAsWithdraw,
                message: "insufficient data to decode".into(),
            });
            break;
        }
        let (flags, t) = (attrs[0], attrs[1]);
        let (hdr, len) = if flags & 0x10 != 0 {
            if attrs.len() < 4 {
                record(MessageError {
                    handling: ErrorHandling::TreatAsWithdraw,
                    message: "attribute length is short".into(),
                });
                break;
            }
            (4, u16::from_be_bytes([attrs[2], attrs[3]]) as usize)
        } else {
            (3, attrs[2] as usize)
        };
        if attrs.len() < hdr + len {
            record(MessageError {
                handling: ErrorHandling::TreatAsWithdraw,
                message: "path attribute length exceeds path attributes boundary".into(),
            });
            return Err(strongest.unwrap());
        }
        let value = &attrs[hdr..hdr + len];
        attrs = &attrs[hdr + len..];
        let result: Result<(), String> = (|| {
            match t {
                // NEXT_HOP
                3 => {
                    update.next_hop = Some(match value.len() {
                        4 => Ipv4Addr::new(value[0], value[1], value[2], value[3]).to_ipv6_mapped(),
                        16 => Ipv6Addr::from(<[u8; 16]>::try_from(value).unwrap()),
                        _ => return Err("nexthop length isn't correct".into()),
                    })
                }
                // AS_PATH
                2 => update.as_path = Some(parse_as_path(value, as_size)?),
                // COMMUNITIES
                8 => {
                    if value.len() % 4 != 0 {
                        return Err("communities length isn't correct".into());
                    }
                    update.communities =
                        Some(value.chunks_exact(4).map(|c| u32::from_be_bytes(c.try_into().unwrap())).collect());
                }
                // EXTENDED_COMMUNITIES
                16 => {
                    if value.len() % 8 != 0 {
                        return Err("extended communities length isn't correct".into());
                    }
                    update.extended_communities = value.chunks_exact(8).map(|c| c.try_into().unwrap()).collect();
                }
                // LARGE_COMMUNITY
                32 => {
                    if value.len() % 12 != 0 {
                        return Err("large communities length isn't correct".into());
                    }
                    update.large_communities = Some(
                        value
                            .chunks_exact(12)
                            .map(|c| LargeCommunity {
                                asn: u32::from_be_bytes(c[0..4].try_into().unwrap()),
                                local_data1: u32::from_be_bytes(c[4..8].try_into().unwrap()),
                                local_data2: u32::from_be_bytes(c[8..12].try_into().unwrap()),
                            })
                            .collect(),
                    );
                }
                14 | 15 => update.mp.push(parse_mp(t, value, options)?),
                // ORIGIN, MED, LOCAL_PREF, ORIGINATOR_ID: fixed sizes.
                1 if value.len() != 1 => return Err("origin length isn't correct".into()),
                4 | 5 | 9 if value.len() != 4 => return Err("attribute length isn't correct".into()),
                10 if value.len() % 4 != 0 => return Err("cluster list length isn't correct".into()),
                _ => {}
            }
            Ok(())
        })();
        if let Err(message) = result {
            record(MessageError { handling: handling_for_attribute(t), message });
        }
    }

    match read_nlris(Family::IPV4_UNICAST, nlri, v4_add_path) {
        Ok(n) => update.nlri = n,
        Err(e) => return Err(MessageError::reset(e)),
    }
    match strongest {
        Some(e) if e.handling > ErrorHandling::AttributeDiscard => Err(e),
        _ => Ok(update),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_path() {
        // SEQ(1, 2) SET(3, 4) CONFED_SEQ(5)
        let data = [2, 2, 0, 0, 0, 1, 0, 0, 0, 2, 1, 2, 0, 0, 0, 3, 0, 0, 0, 4, 3, 1, 0, 0, 0, 5];
        assert_eq!(parse_as_path(&data, 4).unwrap(), vec![1, 2, 3, 5]);
        assert!(parse_as_path(&[2, 0], 4).is_err());
        assert_eq!(parse_as_path(&[2, 2, 0, 1, 0, 2], 2).unwrap(), vec![1, 2]);
    }

    #[test]
    fn prefixes() {
        assert_eq!(read_prefix(&[24, 192, 0, 2], 32).unwrap(), ("192.0.2.0/24".parse().unwrap(), 4));
        assert_eq!(read_prefix(&[0], 32).unwrap(), ("0.0.0.0/0".parse().unwrap(), 1));
        assert!(read_prefix(&[33, 1, 2, 3, 4, 5], 32).is_err());
        assert!(read_prefix(&[24, 192, 0], 32).is_err());
        // VPN: label 16 (BoS), RD 0:65017:101, 192.0.2.0/24
        let mut vpn = vec![24 + 24 + 64, 0x00, 0x01, 0x01, 0, 0, 0xfd, 0xf9, 0, 0, 0, 101, 192, 0, 2];
        let (n, len) = read_nlri(Family::new(AFI_IP, SAFI_MPLS_VPN), &vpn).unwrap();
        assert_eq!(len, vpn.len());
        assert_eq!(n, Nlri::WithRd(0xfdf9_0000_0065, "192.0.2.0/24".parse().unwrap()));
        vpn[0] = 200;
        assert!(read_nlri(Family::new(AFI_IP, SAFI_MPLS_VPN), &vpn).is_err());
    }

    #[test]
    fn family_names() {
        assert_eq!(Family::new(AFI_L2VPN, SAFI_VPLS).to_string(), "l2vpn-vpls");
        assert_eq!(Family::new(AFI_IP6, SAFI_MPLS_VPN).to_string(), "l3vpn-ipv6-unicast");
        assert_eq!(Family::new(99, 1).name(), None);
    }

    #[test]
    fn broken_attributes() {
        let opts = UpdateOptions::default();
        // A communities attribute with a bad length is "treat as withdraw".
        let body = [0, 0, 0, 5, 0xc0, 8, 2, 0, 1];
        assert_eq!(parse_update(&body, &opts).unwrap_err().handling, ErrorHandling::TreatAsWithdraw);
        // An unknown optional attribute is skipped.
        let body = [0, 0, 0, 5, 0xc0, 99, 2, 0, 1, 24, 192, 0, 2];
        let u = parse_update(&body, &opts).unwrap();
        assert_eq!(u.nlri, vec![PathNlri { nlri: Nlri::Prefix("192.0.2.0/24".parse().unwrap()), path_id: 0 }]);
        // Truncated withdrawn routes reset the session.
        assert_eq!(parse_update(&[0, 9, 1], &opts).unwrap_err().handling, ErrorHandling::SessionReset);
    }
}
