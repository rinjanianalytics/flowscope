// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Parsing of sampled packet headers (Ethernet, MPLS, IPv4, IPv6, L4) and
//! decapsulation. Port of `outlet/flow/decoder/helpers.go`.
//!
//! The parse functions return the L3 length. With decapsulation, only the
//! inner packet is used and the returned length is the inner one.

use crate::constants::*;
use flowscope_pb::DecapsulationProtocol as Decap;
use flowscope_schema::{ColumnGroup, ColumnKey as K, FlowMessage, Schema};
use std::net::{Ipv4Addr, Ipv6Addr};

fn be16(d: &[u8]) -> u16 {
    u16::from_be_bytes([d[0], d[1]])
}

fn be32(d: &[u8]) -> u32 {
    u32::from_be_bytes([d[0], d[1], d[2], d[3]])
}

/// Decode an IPv4 or IPv6 address into its IPv6 form. Other lengths give
/// no address.
pub fn decode_ip(b: &[u8]) -> Option<Ipv6Addr> {
    match b.len() {
        4 => Some(Ipv4Addr::new(b[0], b[1], b[2], b[3]).to_ipv6_mapped()),
        16 => Some(Ipv6Addr::from(<[u8; 16]>::try_from(b).unwrap())),
        _ => None,
    }
}

/// Parse an IPv4 header and what follows.
pub fn parse_ipv4(sch: &Schema, bf: &mut FlowMessage, decap: Decap, data: &[u8]) -> u64 {
    if data.len() < 20 {
        return 0;
    }
    let l3_length = be16(&data[2..4]) as u64;
    let frag_offset = be16(&data[6..8]) & 0x1fff;
    let proto = data[9];
    if decap == Decap::None {
        bf.append_uint(K::EType, ETYPE_IPV4 as u64);
        bf.src_addr = decode_ip(&data[12..16]);
        bf.dst_addr = decode_ip(&data[16..20]);
        if !sch.is_disabled(ColumnGroup::L3L4) {
            bf.append_uint(K::IPTos, data[1] as u64);
            bf.append_uint(K::IPTTL, data[8] as u64);
            bf.append_uint(K::IPFragmentID, be16(&data[4..6]) as u64);
            bf.append_uint(K::IPFragmentOffset, frag_offset as u64);
        }
        bf.append_uint(K::Proto, proto as u64);
    }
    let ihl = ((data[0] & 0xf) as usize) * 4;
    let rest = if data.len() >= ihl { &data[ihl..] } else { &data[..0] };
    if frag_offset == 0 {
        let inner = parse_l4(sch, bf, decap, rest, proto);
        if decap != Decap::None {
            return inner;
        }
        return l3_length;
    }
    if decap != Decap::None {
        return 0;
    }
    l3_length
}

/// Parse an IPv6 header and what follows.
pub fn parse_ipv6(sch: &Schema, bf: &mut FlowMessage, decap: Decap, data: &[u8]) -> u64 {
    if data.len() < 40 {
        return 0;
    }
    let l3_length = be16(&data[4..6]) as u64 + 40;
    let proto = data[6];
    if decap == Decap::None {
        bf.append_uint(K::EType, ETYPE_IPV6 as u64);
        bf.src_addr = decode_ip(&data[8..24]);
        bf.dst_addr = decode_ip(&data[24..40]);
        bf.append_uint(K::Proto, proto as u64);
        if !sch.is_disabled(ColumnGroup::L3L4) {
            bf.append_uint(K::IPTos, ((be16(&data[0..2]) & 0xff0) >> 4) as u64);
            bf.append_uint(K::IPTTL, data[7] as u64);
            bf.append_uint(K::IPv6FlowLabel, (be32(&data[0..4]) & 0xfffff) as u64);
        }
    }
    let inner = parse_l4(sch, bf, decap, &data[40..], proto);
    if decap != Decap::None {
        return inner;
    }
    l3_length
}

/// Parse the L4 header, or decapsulate.
pub fn parse_l4(sch: &Schema, bf: &mut FlowMessage, decap: Decap, mut data: &[u8], mut proto: u8) -> u64 {
    match decap {
        Decap::None => {}
        Decap::Vxlan => {
            if proto == PROTO_UDP && data.len() > 16 && be16(&data[2..4]) == PORT_VXLAN {
                return parse_ethernet(sch, bf, Decap::None, &data[16..]);
            }
            return 0;
        }
        Decap::Gre => {
            if proto == PROTO_GRE && data.len() > 4 {
                let flags_and_version = be16(&data[..2]);
                let gre_protocol = be16(&data[2..4]);
                if flags_and_version & 0x4fff != 0 {
                    return 0;
                }
                let skip = 4 + flags_and_version.count_ones() as usize * 4;
                if data.len() >= skip {
                    let inner = &data[skip..];
                    match gre_protocol {
                        ETYPE_IPV4 => return parse_ipv4(sch, bf, Decap::None, inner),
                        ETYPE_IPV6 => return parse_ipv6(sch, bf, Decap::None, inner),
                        _ => {}
                    }
                }
            }
            return 0;
        }
        Decap::Ipip => {
            return match proto {
                PROTO_IPV4 => parse_ipv4(sch, bf, Decap::None, data),
                PROTO_IPV6 => parse_ipv6(sch, bf, Decap::None, data),
                _ => 0,
            };
        }
        Decap::Srv6 => loop {
            match proto {
                PROTO_IPV4 => return parse_ipv4(sch, bf, Decap::None, data),
                PROTO_IPV6 => return parse_ipv6(sch, bf, Decap::None, data),
                PROTO_SRH => {
                    if data.len() < 8 || data[2] != 4 {
                        return 0;
                    }
                    let skip = 8 + data[1] as usize * 8;
                    if data.len() < skip {
                        return 0;
                    }
                    proto = data[0];
                    data = &data[skip..];
                }
                _ => return 0,
            }
        },
    }
    if (proto == PROTO_TCP || proto == PROTO_UDP) && data.len() > 4 {
        bf.append_uint(K::SrcPort, be16(&data[0..2]) as u64);
        bf.append_uint(K::DstPort, be16(&data[2..4]) as u64);
    }
    if !sch.is_disabled(ColumnGroup::L3L4) {
        match proto {
            PROTO_TCP if data.len() > 13 => bf.append_uint(K::TCPFlags, data[13] as u64),
            PROTO_ICMPV4 if data.len() > 2 => {
                bf.append_uint(K::ICMPv4Type, data[0] as u64);
                bf.append_uint(K::ICMPv4Code, data[1] as u64);
            }
            PROTO_ICMPV6 if data.len() > 2 => {
                bf.append_uint(K::ICMPv6Type, data[0] as u64);
                bf.append_uint(K::ICMPv6Code, data[1] as u64);
            }
            _ => {}
        }
    }
    0
}

/// MAC address as an integer.
pub fn mac_to_u64(mac: &[u8]) -> u64 {
    if mac.len() != 6 {
        return 0;
    }
    mac.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64)
}

/// Parse an Ethernet frame (with VLAN tags and MPLS labels).
pub fn parse_ethernet(sch: &Schema, bf: &mut FlowMessage, decap: Decap, data: &[u8]) -> u64 {
    if data.len() < 14 {
        return 0;
    }
    let l2 = !sch.is_disabled(ColumnGroup::L2);
    if l2 && decap == Decap::None {
        bf.append_uint(K::DstMAC, mac_to_u64(&data[0..6]));
        bf.append_uint(K::SrcMAC, mac_to_u64(&data[6..12]));
    }
    let mut ether_type = be16(&data[12..14]);
    let mut data = &data[14..];
    let mut vlan: u16 = 0;
    while ether_type == ETYPE_VLAN {
        if data.len() < 4 {
            return 0;
        }
        if l2 && decap == Decap::None {
            vlan = (((data[0] & 0xf) as u16) << 8) + data[1] as u16;
        }
        ether_type = be16(&data[2..4]);
        data = &data[4..];
    }
    if vlan != 0 {
        if bf.src_vlan == 0 {
            bf.src_vlan = vlan;
        }
        if bf.dst_vlan == 0 {
            bf.dst_vlan = vlan;
        }
    }
    if ether_type == ETYPE_MPLS {
        let mut labels: Vec<u32> = Vec::with_capacity(5);
        loop {
            if data.len() < 5 {
                return 0;
            }
            let label = u32::from_be_bytes([0, data[0], data[1], data[2]]) >> 4;
            let bottom = data[2] & 1;
            data = &data[4..];
            labels.push(label);
            if bottom == 1 {
                match label {
                    0 => ether_type = ETYPE_IPV4,
                    2 => ether_type = ETYPE_IPV6,
                    _ => {
                        if data.is_empty() {
                            return 0;
                        }
                        match data[0] >> 4 {
                            0x4 => ether_type = ETYPE_IPV4,
                            0x6 => ether_type = ETYPE_IPV6,
                            0x0 => {
                                if data.len() < 4 {
                                    return 0;
                                }
                                // Pseudowire control word, then Ethernet.
                                if be32(&data[0..4]) == 0 {
                                    if decap == Decap::None {
                                        bf.append_array_uint32(K::MPLSLabels, &labels);
                                    }
                                    return parse_ethernet(sch, bf, decap, &data[4..]);
                                }
                                return 0;
                            }
                            _ => return 0,
                        }
                    }
                }
                break;
            }
        }
        if !labels.is_empty() && decap == Decap::None {
            bf.append_array_uint32(K::MPLSLabels, &labels);
        }
    }
    match ether_type {
        ETYPE_IPV4 => parse_ipv4(sch, bf, decap, data),
        ETYPE_IPV6 => parse_ipv6(sch, bf, decap, data),
        _ => 0,
    }
}
