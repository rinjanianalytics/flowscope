// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Mapping of NetFlow and IPFIX records to flows. Port of
//! `outlet/flow/decoder/netflow/decode.go`.

use super::NetflowDecoder;
use super::fields as f;
use super::packet::{DataField, FlowSet, V5Packet};
use super::templates::Exporter;
use crate::Options;
use crate::constants::*;
use crate::helpers::{decode_ip, parse_ethernet};
use flowscope_pb::{DecapsulationProtocol, TimestampSource};
use flowscope_schema::{ColumnGroup, ColumnKey as K, Direction, FlowMessage};
use std::collections::HashSet;
use std::net::Ipv4Addr;

/// Decode a big-endian unsigned number of 1 to 8 bytes. Other lengths give 0.
pub(crate) fn decode_unumber(b: &[u8]) -> u64 {
    if b.is_empty() || b.len() > 8 {
        return 0;
    }
    b.iter().fold(0u64, |acc, x| (acc << 8) | *x as u64)
}

/// Convert an uptime-relative timestamp to seconds since epoch.
fn uptime_to_unix(export_secs: u64, sys_uptime: u64, uptime: u64) -> u32 {
    let elapsed = (sys_uptime as u32).wrapping_sub(uptime as u32) as u64;
    (export_secs.wrapping_mul(1000).wrapping_sub(elapsed) / 1000) as u32
}

const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;

fn ntp_to_unix(ntp: u64) -> u32 {
    ((ntp >> 32).wrapping_sub(NTP_EPOCH_OFFSET)) as u32
}

fn is_all_zero_ip(b: &[u8]) -> bool {
    (b.len() == 4 || b.len() == 16) && b.iter().all(|x| *x == 0)
}

fn ipv4(v: u32) -> std::net::Ipv6Addr {
    Ipv4Addr::from(v).to_ipv6_mapped()
}

pub(crate) fn decode_v5(
    packet: &V5Packet,
    ts: u64,
    sys_uptime: u64,
    options: &Options,
    bf: &mut FlowMessage,
    finalize: &mut dyn FnMut(&mut FlowMessage),
) {
    let l3l4 = !bf.schema().is_disabled(ColumnGroup::L3L4);
    for record in &packet.records {
        bf.sampling_rate = packet.sampling_interval as u64;
        bf.in_if = record.input as u32;
        bf.out_if = record.output as u32;
        bf.src_addr = Some(ipv4(record.src_addr));
        bf.dst_addr = Some(ipv4(record.dst_addr));
        bf.next_hop = Some(ipv4(record.next_hop));
        bf.src_net_mask = record.src_mask;
        bf.dst_net_mask = record.dst_mask;
        bf.src_as = record.src_as as u32;
        bf.dst_as = record.dst_as as u32;
        bf.append_uint(K::Bytes, record.d_octets as u64);
        bf.append_uint(K::Packets, record.d_pkts as u64);
        bf.append_uint(K::EType, ETYPE_IPV4 as u64);
        bf.append_uint(K::Proto, record.proto as u64);
        bf.append_uint(K::SrcPort, record.src_port as u64);
        bf.append_uint(K::DstPort, record.dst_port as u64);
        if l3l4 {
            bf.append_uint(K::IPTos, record.tos as u64);
            bf.append_uint(K::TCPFlags, record.tcp_flags as u64);
        }
        if options.timestamp_source == TimestampSource::NetflowFirstSwitched {
            bf.time_received = uptime_to_unix(ts, sys_uptime, record.first as u64);
        }
        if bf.sampling_rate == 0 {
            bf.sampling_rate = 1;
        }
        finalize(bf);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_v9_ipfix(
    nd: &NetflowDecoder,
    version: u16,
    obs_domain_id: u32,
    flow_sets: &[FlowSet],
    exporter: &Exporter,
    ts: u64,
    sys_uptime: u64,
    options: &Options,
    key: &str,
    bf: &mut FlowMessage,
    finalize: &mut dyn FnMut(&mut FlowMessage),
) {
    for fs in flow_sets {
        match fs {
            FlowSet::OptionsData(records) => {
                for record in records {
                    let (mut sampling_rate, mut sampler_id, mut packet_interval, mut packet_space) =
                        (0u32, 0u64, 0u32, 0u32);
                    for field in &record.options {
                        let Some(v) = field.value else { continue };
                        if field.pen_provided {
                            continue;
                        }
                        match field.r#type {
                            f::SAMPLING_INTERVAL | f::SAMPLER_RANDOM_INTERVAL => {
                                sampling_rate = decode_unumber(v) as u32
                            }
                            f::SAMPLER_ID | f::SELECTOR_ID => sampler_id = decode_unumber(v),
                            f::SAMPLING_PACKET_INTERVAL => packet_interval = decode_unumber(v) as u32,
                            f::SAMPLING_PACKET_SPACE => packet_space = decode_unumber(v) as u32,
                            _ => {}
                        }
                    }
                    if packet_interval > 0 {
                        sampling_rate = packet_interval.wrapping_add(packet_space) / packet_interval;
                    }
                    if sampling_rate > 0 {
                        exporter.set_sampling_rate(version, obs_domain_id, sampler_id, sampling_rate);
                    }
                }
            }
            FlowSet::Data(records) => {
                for record in records {
                    decode_record(
                        nd,
                        version,
                        obs_domain_id,
                        exporter,
                        record,
                        ts,
                        sys_uptime,
                        options,
                        key,
                        bf,
                        finalize,
                    );
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Forward,
    Reverse,
}

#[allow(clippy::too_many_arguments)]
fn decode_record(
    nd: &NetflowDecoder,
    version: u16,
    obs_domain_id: u32,
    exporter: &Exporter,
    fields: &[DataField],
    ts: u64,
    sys_uptime: u64,
    options: &Options,
    key: &str,
    bf: &mut FlowMessage,
    finalize: &mut dyn FnMut(&mut FlowMessage),
) {
    let schema = bf.schema().clone();
    let nat = !schema.is_disabled(ColumnGroup::NAT);
    let l2 = !schema.is_disabled(ColumnGroup::L2);
    let l3l4 = !schema.is_disabled(ColumnGroup::L3L4);
    let need_decap = options.decapsulation_protocol != DecapsulationProtocol::None;
    // Field types present with the reverse PEN.
    let mut reverse_present: Option<HashSet<u16>> = None;

    for dir in [Dir::Forward, Dir::Reverse] {
        let (mut etype, mut dst_port, mut src_port) = (0u16, 0u16, 0u16);
        let (mut proto, mut icmp_type, mut icmp_code) = (0u8, 0u8, 0u8);
        let mut found_icmp_type_code = false;
        let mut decap_ok = false;
        let (mut first_switched, mut system_init) = (0u64, 0u64);
        let mut has_first_switched = false;
        let mut mpls_labels: Vec<u32> = Vec::with_capacity(5);
        // Systematic sampling stated in the record itself (RouterOS 7 does so):
        // samplingPacketInterval packets taken of every interval + space.
        let (mut packet_interval, mut packet_space) = (0u64, 0u64);

        for field in fields {
            let Some(v) = field.value else { continue };
            if field.pen_provided {
                if field.pen == f::REVERSE_PEN {
                    if dir == Dir::Forward {
                        reverse_present.get_or_insert_with(HashSet::new).insert(field.r#type);
                        continue;
                    }
                } else if field.pen == f::JUNIPER_PEN {
                    if field.r#type == f::COMMON_PROPERTIES_ID && v.len() == 2 && (v[0] & 0xfc) >> 2 == 0x02 {
                        if decode_unumber(v) & 0x03ff == 0 {
                            bf.append_uint(K::ForwardingStatus, 64);
                        } else {
                            bf.append_uint(K::ForwardingStatus, 128);
                        }
                    }
                    continue;
                } else {
                    continue;
                }
            } else if dir == Dir::Reverse && reverse_present.as_ref().is_some_and(|s| s.contains(&field.r#type)) {
                continue;
            }

            match field.r#type {
                f::OCTET_DELTA_COUNT | f::POST_OCTET_DELTA_COUNT | f::INITIATOR_OCTETS | f::RESPONDER_OCTETS => {
                    bf.append_uint(K::Bytes, decode_unumber(v))
                }
                f::PACKET_DELTA_COUNT | f::POST_PACKET_DELTA_COUNT => {
                    let n = decode_unumber(v);
                    if dir == Dir::Reverse && n == 0 {
                        bf.undo();
                        return;
                    }
                    bf.append_uint(K::Packets, n);
                }
                f::SAMPLING_INTERVAL | f::SAMPLER_RANDOM_INTERVAL => bf.sampling_rate = decode_unumber(v),
                f::SAMPLER_ID | f::SELECTOR_ID => {
                    bf.sampling_rate = exporter.sampling_rate(version, obs_domain_id, decode_unumber(v)) as u64
                }
                f::SAMPLING_PACKET_INTERVAL => packet_interval = decode_unumber(v),
                f::SAMPLING_PACKET_SPACE => packet_space = decode_unumber(v),

                f::SOURCE_IPV4_ADDRESS => {
                    if !is_all_zero_ip(v) {
                        etype = ETYPE_IPV4;
                        bf.src_addr = decode_ip(v);
                    }
                }
                f::DESTINATION_IPV4_ADDRESS => {
                    if !is_all_zero_ip(v) {
                        etype = ETYPE_IPV4;
                        bf.dst_addr = decode_ip(v);
                    }
                }
                f::SOURCE_IPV6_ADDRESS => {
                    if !is_all_zero_ip(v) {
                        etype = ETYPE_IPV6;
                        bf.src_addr = decode_ip(v);
                    }
                }
                f::DESTINATION_IPV6_ADDRESS => {
                    if !is_all_zero_ip(v) {
                        etype = ETYPE_IPV6;
                        bf.dst_addr = decode_ip(v);
                    }
                }
                f::SOURCE_IPV4_PREFIX_LENGTH | f::SOURCE_IPV6_PREFIX_LENGTH => {
                    bf.src_net_mask = decode_unumber(v) as u8
                }
                f::DESTINATION_IPV4_PREFIX_LENGTH | f::DESTINATION_IPV6_PREFIX_LENGTH => {
                    bf.dst_net_mask = decode_unumber(v) as u8
                }
                f::IP_NEXT_HOP_IPV4_ADDRESS
                | f::BGP_NEXT_HOP_IPV4_ADDRESS
                | f::IP_NEXT_HOP_IPV6_ADDRESS
                | f::BGP_NEXT_HOP_IPV6_ADDRESS => {
                    if !is_all_zero_ip(v) {
                        bf.next_hop = decode_ip(v);
                    }
                }

                f::SOURCE_TRANSPORT_PORT => {
                    src_port = decode_unumber(v) as u16;
                    bf.append_uint(K::SrcPort, src_port as u64);
                }
                f::DESTINATION_TRANSPORT_PORT => {
                    dst_port = decode_unumber(v) as u16;
                    bf.append_uint(K::DstPort, dst_port as u64);
                }
                f::PROTOCOL_IDENTIFIER => {
                    proto = decode_unumber(v) as u8;
                    bf.append_uint(K::Proto, proto as u64);
                }

                f::BGP_SOURCE_AS_NUMBER => bf.src_as = decode_unumber(v) as u32,
                f::BGP_DESTINATION_AS_NUMBER => bf.dst_as = decode_unumber(v) as u32,

                f::INGRESS_INTERFACE => bf.in_if = decode_unumber(v) as u32,
                f::EGRESS_INTERFACE => bf.out_if = decode_unumber(v) as u32,
                f::INGRESS_PHYSICAL_INTERFACE => {
                    if bf.in_if == 0 {
                        bf.in_if = decode_unumber(v) as u32;
                    }
                }
                f::EGRESS_PHYSICAL_INTERFACE => {
                    if bf.out_if == 0 {
                        bf.out_if = decode_unumber(v) as u32;
                    }
                }

                f::DATA_LINK_FRAME_SECTION => {
                    let l3_length = parse_ethernet(&schema, bf, options.decapsulation_protocol, v);
                    if l3_length > 0 {
                        bf.append_uint(K::Bytes, l3_length);
                        bf.append_uint(K::Packets, 1);
                        decap_ok = true;
                    }
                }

                f::MPLS_TOP_LABEL_STACK_SECTION..=f::MPLS_LABEL_STACK_SECTION10 => {
                    let uv = decode_unumber(v) >> 4;
                    if uv > 0 {
                        mpls_labels.push(uv as u32);
                    }
                }

                f::INGRESS_VRF_ID => bf.append_uint(K::IngressVRFID, decode_unumber(v)),
                f::EGRESS_VRF_ID => bf.append_uint(K::EgressVRFID, decode_unumber(v)),

                f::FORWARDING_STATUS => bf.append_uint(K::ForwardingStatus, decode_unumber(v)),
                f::FLOW_DIRECTION => match decode_unumber(v) {
                    0 => bf.append_uint(K::FlowDirection, Direction::Ingress as u64),
                    1 => bf.append_uint(K::FlowDirection, Direction::Egress as u64),
                    _ => {}
                },
                t => {
                    if options.timestamp_source == TimestampSource::NetflowFirstSwitched {
                        match t {
                            f::NFV9_FIRST_SWITCHED => {
                                first_switched = decode_unumber(v);
                                has_first_switched = true;
                            }
                            f::SYSTEM_INIT_TIME_MILLISECONDS => system_init = decode_unumber(v),
                            f::FLOW_START_SECONDS => bf.time_received = decode_unumber(v) as u32,
                            f::FLOW_START_MILLISECONDS => bf.time_received = (decode_unumber(v) / 1000) as u32,
                            f::FLOW_START_MICROSECONDS | f::FLOW_START_NANOSECONDS => {
                                bf.time_received = ntp_to_unix(decode_unumber(v))
                            }
                            _ => {}
                        }
                    }
                    if nat {
                        match t {
                            f::POST_NAT_SOURCE_IPV4_ADDRESS => bf.append_ipv6(K::SrcAddrNAT, decode_ip(v)),
                            f::POST_NAT_DESTINATION_IPV4_ADDRESS => bf.append_ipv6(K::DstAddrNAT, decode_ip(v)),
                            f::POST_NAPT_SOURCE_TRANSPORT_PORT => bf.append_uint(K::SrcPortNAT, decode_unumber(v)),
                            f::POST_NAPT_DESTINATION_TRANSPORT_PORT => bf.append_uint(K::DstPortNAT, decode_unumber(v)),
                            _ => {}
                        }
                    }
                    if l2 {
                        match t {
                            f::VLAN_ID | f::DOT1Q_VLAN_ID => {
                                if bf.src_vlan == 0 {
                                    bf.src_vlan = decode_unumber(v) as u16;
                                }
                            }
                            f::POST_VLAN_ID | f::POST_DOT1Q_VLAN_ID => {
                                if bf.dst_vlan == 0 {
                                    bf.dst_vlan = decode_unumber(v) as u16;
                                }
                            }
                            f::SOURCE_MAC_ADDRESS | f::POST_SOURCE_MAC_ADDRESS => {
                                bf.append_uint(K::SrcMAC, decode_unumber(v))
                            }
                            f::DESTINATION_MAC_ADDRESS | f::POST_DESTINATION_MAC_ADDRESS => {
                                bf.append_uint(K::DstMAC, decode_unumber(v))
                            }
                            _ => {}
                        }
                    }
                    if l3l4 {
                        match t {
                            f::IP_TTL | f::MINIMUM_TTL => bf.append_uint(K::IPTTL, decode_unumber(v)),
                            f::IP_CLASS_OF_SERVICE => bf.append_uint(K::IPTos, decode_unumber(v)),
                            f::FLOW_LABEL_IPV6 => bf.append_uint(K::IPv6FlowLabel, decode_unumber(v)),
                            f::TCP_CONTROL_BITS => bf.append_uint(K::TCPFlags, decode_unumber(v) & 0x0fff),
                            f::FRAGMENT_IDENTIFICATION => bf.append_uint(K::IPFragmentID, decode_unumber(v)),
                            f::FRAGMENT_OFFSET => bf.append_uint(K::IPFragmentOffset, decode_unumber(v)),
                            f::ICMP_TYPE_CODE_IPV4 | f::ICMP_TYPE_CODE_IPV6 => {
                                let tc = decode_unumber(v);
                                icmp_type = (tc >> 8) as u8;
                                icmp_code = (tc & 0xff) as u8;
                                found_icmp_type_code = true;
                            }
                            f::ICMP_TYPE_IPV4 | f::ICMP_TYPE_IPV6 => {
                                icmp_type = decode_unumber(v) as u8;
                                found_icmp_type_code = true;
                            }
                            f::ICMP_CODE_IPV4 | f::ICMP_CODE_IPV6 => {
                                icmp_code = decode_unumber(v) as u8;
                                found_icmp_type_code = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if l3l4 && (proto == PROTO_ICMPV4 || proto == PROTO_ICMPV6) {
            // Some exporters put the ICMP type and code in the destination port.
            if !found_icmp_type_code && src_port == 0 {
                icmp_type = (dst_port >> 8) as u8;
                icmp_code = (dst_port & 0xff) as u8;
            }
            if proto == PROTO_ICMPV4 {
                bf.append_uint(K::ICMPv4Type, icmp_type as u64);
                bf.append_uint(K::ICMPv4Code, icmp_code as u64);
            } else {
                bf.append_uint(K::ICMPv6Type, icmp_type as u64);
                bf.append_uint(K::ICMPv6Code, icmp_code as u64);
            }
        }
        if has_first_switched {
            if version == 9 {
                bf.time_received = uptime_to_unix(ts, sys_uptime, first_switched);
            } else if system_init > 0 {
                bf.time_received = (system_init.wrapping_add(first_switched) / 1000) as u32;
            }
        }
        bf.append_uint(K::EType, etype as u64);
        if !mpls_labels.is_empty() {
            bf.append_array_uint32(K::MPLSLabels, &mpls_labels);
        }
        if bf.sampling_rate == 0 && packet_interval > 0 {
            bf.sampling_rate = packet_interval.saturating_add(packet_space) / packet_interval;
        }
        if bf.sampling_rate == 0 {
            bf.sampling_rate = exporter.sampling_rate(version, obs_domain_id, 0) as u64;
        }
        let mut local_finalize = |bf: &mut FlowMessage| {
            if need_decap && !decap_ok {
                nd.metrics.errors.with_label_values(&[key, "non-encapsulated packet"]).inc();
                bf.undo();
            } else {
                finalize(bf);
            }
        };
        match (dir, reverse_present.is_some()) {
            (Dir::Forward, false) => {
                local_finalize(bf);
                break;
            }
            (Dir::Forward, true) => {
                local_finalize(bf);
                bf.reverse();
            }
            (Dir::Reverse, _) => {
                bf.reverse();
                local_finalize(bf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unumber() {
        assert_eq!(decode_unumber(&[]), 0);
        assert_eq!(decode_unumber(&[1]), 1);
        assert_eq!(decode_unumber(&[1, 2]), 0x102);
        assert_eq!(decode_unumber(&[1, 2, 3, 4, 5, 6, 7, 8]), 0x0102030405060708);
        assert_eq!(decode_unumber(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), 0);
    }

    #[test]
    fn timestamps() {
        // Exported at 1000s, uptime 10s, flow started at 8s of uptime.
        assert_eq!(uptime_to_unix(1000, 10_000, 8_000), 998);
        // Uptime wrapped around.
        assert_eq!(uptime_to_unix(1000, 1_000, u32::MAX as u64 - 999), 998);
        assert_eq!(ntp_to_unix((NTP_EPOCH_OFFSET + 1_700_000_000) << 32), 1_700_000_000);
    }
}
