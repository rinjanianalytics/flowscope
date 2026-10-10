// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! sFlow decoder. Port of `outlet/flow/decoder/sflow`.

pub mod packet;

use crate::constants::*;
use crate::helpers::{decode_ip, mac_to_u64, parse_ethernet, parse_ipv4, parse_ipv6};
use crate::{Decoder, Finalize, Input, Options, Sampler, exporter_key};
use flowscope_common::reporter::{CounterVec, Reporter};
use flowscope_pb::DecapsulationProtocol;
use flowscope_schema::{ColumnGroup, ColumnKey as K, FlowMessage, Schema};
use packet::{FlowRecord, SampleKind};
use std::time::Duration;

const INTERFACE_LOCAL: u32 = 0x3fffffff;
const INTERFACE_FORMAT_IFINDEX: u32 = 0;
const INTERFACE_FORMAT_DISCARD: u32 = 1;

/// sFlow discard reasons mapped to IPFIX forwarding status.
fn discard_reason_to_forwarding_status(reason: u32) -> u64 {
    match reason {
        0..=3 | 5..=7 => 131, // ICMP unreachable → unroutable
        4 => 133,             // fragmentation needed and DF set
        8 => 132,             // source host isolated → adjacency
        9 | 10 => 129,        // prohibited → ACL deny
        257 => 137,           // TTL exceeded → bad TTL
        258 => 130,           // ACL → ACL drop
        259 => 143,           // no buffer space
        260 => 139,           // RED → WRED
        261 => 138,           // traffic shaping → policer
        262 => 135,           // packet too big → bad total length
        263 => 131,           // source MAC is multicast → unroutable
        269 => 130,           // blackhole route → ACL drop
        302 => 140,           // uRPF → RPF
        _ => 128,             // dropped, unknown
    }
}

/// The sFlow decoder.
pub struct SflowDecoder {
    errors: CounterVec,
    stats: CounterVec,
    sample_records_stats_sum: CounterVec,
    sample_stats_sum: CounterVec,
    error_log: Sampler,
}

impl SflowDecoder {
    pub fn new(r: &Reporter) -> Self {
        let m = r.metrics("outlet/flow/decoder/sflow");
        SflowDecoder {
            errors: m.counter_vec(
                "errors_total",
                "Number of sFlow flows processed with errors.",
                &["exporter", "error"],
            ),
            stats: m.counter_vec("flows_total", "Number of sFlow flows processed.", &["exporter", "agent", "version"]),
            sample_records_stats_sum: m.counter_vec(
                "sample_records_sum",
                "Number of sFlow flows sample records.",
                &["exporter", "agent", "version", "type"],
            ),
            sample_stats_sum: m.counter_vec(
                "sample_sum",
                "Number of sFlow flows samples.",
                &["exporter", "agent", "version", "type"],
            ),
            error_log: Sampler::new(Duration::from_secs(30), 3),
        }
    }
}

fn parse_sampled_header(
    sch: &Schema,
    bf: &mut FlowMessage,
    decap: DecapsulationProtocol,
    protocol: u32,
    data: &[u8],
) -> u64 {
    match protocol {
        1 => parse_ethernet(sch, bf, decap, data),
        11 => parse_ipv4(sch, bf, decap, data),
        12 => parse_ipv6(sch, bf, decap, data),
        _ => 0,
    }
}

/// Format an agent address like Go's net.IP.String().
fn agent_string(ip: Option<&[u8]>) -> String {
    match ip {
        Some(b) if b.len() == 4 => std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]).to_string(),
        Some(b) if b.len() == 16 => {
            let v6 = std::net::Ipv6Addr::from(<[u8; 16]>::try_from(b).unwrap());
            match v6.to_ipv4_mapped() {
                Some(v4) => v4.to_string(),
                None => v6.to_string(),
            }
        }
        _ => "<nil>".into(),
    }
}

impl Decoder for SflowDecoder {
    fn name(&self) -> &'static str {
        "sflow"
    }

    fn decode(
        &self,
        input: &Input,
        options: &Options,
        bf: &mut FlowMessage,
        finalize: &mut Finalize,
    ) -> Result<usize, String> {
        let key = exporter_key(&input.source);
        let ts = input.time_received;
        let packet = match packet::parse(input.payload) {
            Ok(p) => p,
            Err(e) => {
                self.errors.with_label_values(&[&key, "sFlow decoding error"]).inc();
                if self.error_log.allow() {
                    tracing::error!(error = %e, exporter = key, "error while decoding sFlow");
                }
                return Err(format!("error while decoding sFlow: {e}"));
            }
        };
        let agent = agent_string(packet.agent_ip);
        let version = "5";
        self.stats.with_label_values(&[&key, &agent, version]).inc();
        for s in packet.samples.iter().flatten() {
            if s.kind == SampleKind::Drop {
                continue;
            }
            let name = s.kind.name();
            self.sample_stats_sum.with_label_values(&[&key, &agent, version, name]).inc();
            self.sample_records_stats_sum
                .with_label_values(&[&key, &agent, version, name])
                .inc_by(s.records_count as f64);
        }

        let schema = bf.schema().clone();
        let l2 = !schema.is_disabled(ColumnGroup::L2);
        let l3l4 = !schema.is_disabled(ColumnGroup::L3L4);
        let need_decap = options.decapsulation_protocol != DecapsulationProtocol::None;
        let agent_ip = packet.agent_ip.and_then(decode_ip);

        for s in packet.samples.iter().flatten() {
            // Only flow samples produce flows. The Go version also emitted an
            // empty flow for counter, drop and missing samples.
            let records: &[FlowRecord] = &s.records;
            let mut forwarding_status = 0u64;
            match s.kind {
                SampleKind::Flow => {
                    bf.sampling_rate = s.sampling_rate as u64;
                    if s.input >> 30 == INTERFACE_FORMAT_IFINDEX {
                        bf.in_if = s.input;
                    }
                    match s.output >> 30 {
                        INTERFACE_FORMAT_IFINDEX => bf.out_if = s.output,
                        INTERFACE_FORMAT_DISCARD => {
                            forwarding_status = discard_reason_to_forwarding_status(s.output & 0x3fffffff)
                        }
                        _ => {}
                    }
                }
                SampleKind::ExpandedFlow => {
                    bf.sampling_rate = s.sampling_rate as u64;
                    if s.input_format == INTERFACE_FORMAT_IFINDEX {
                        bf.in_if = s.input_value;
                    }
                    match s.output_format {
                        INTERFACE_FORMAT_IFINDEX => bf.out_if = s.output_value,
                        INTERFACE_FORMAT_DISCARD => {
                            forwarding_status = discard_reason_to_forwarding_status(s.output_value)
                        }
                        _ => {}
                    }
                }
                SampleKind::Counter | SampleKind::Drop => continue,
            }
            if bf.in_if == INTERFACE_LOCAL {
                bf.in_if = 0;
            }
            if bf.out_if == INTERFACE_LOCAL {
                bf.out_if = 0;
            }

            let has_ipv4 = records.iter().any(|r| matches!(r, FlowRecord::SampledIpv4 { .. }));
            let has_ipv6 = records.iter().any(|r| matches!(r, FlowRecord::SampledIpv6 { .. }));
            let has_ethernet = records.iter().any(|r| matches!(r, FlowRecord::SampledEthernet { .. }));
            let has_switch = records.iter().any(|r| matches!(r, FlowRecord::ExtendedSwitch { .. }));

            let mut l3_length = 0u64;
            for record in records {
                match record {
                    FlowRecord::SampledHeader { protocol, header } => {
                        let needs_ip = !(has_ipv4 || has_ipv6);
                        let needs_l2 = !(!l2 || (has_ethernet && has_switch));
                        if needs_ip || needs_l2 || l3l4 || need_decap {
                            let l =
                                parse_sampled_header(&schema, bf, options.decapsulation_protocol, *protocol, header);
                            if l > 0 {
                                l3_length = l;
                            }
                        }
                    }
                    FlowRecord::SampledIpv4 { length, protocol, src_ip, dst_ip, src_port, dst_port, tos } => {
                        if need_decap {
                            continue;
                        }
                        bf.src_addr = decode_ip(src_ip);
                        bf.dst_addr = decode_ip(dst_ip);
                        l3_length = *length as u64;
                        bf.append_uint(K::Proto, *protocol as u64);
                        bf.append_uint(K::SrcPort, *src_port as u64);
                        bf.append_uint(K::DstPort, *dst_port as u64);
                        bf.append_uint(K::EType, ETYPE_IPV4 as u64);
                        bf.append_uint(K::IPTos, *tos as u64);
                    }
                    FlowRecord::SampledIpv6 { length, protocol, src_ip, dst_ip, src_port, dst_port, priority } => {
                        if need_decap {
                            continue;
                        }
                        bf.src_addr = decode_ip(src_ip);
                        bf.dst_addr = decode_ip(dst_ip);
                        l3_length = *length as u64;
                        bf.append_uint(K::Proto, *protocol as u64);
                        bf.append_uint(K::SrcPort, *src_port as u64);
                        bf.append_uint(K::DstPort, *dst_port as u64);
                        bf.append_uint(K::EType, ETYPE_IPV6 as u64);
                        bf.append_uint(K::IPTos, *priority as u64);
                    }
                    FlowRecord::SampledEthernet { length, src_mac, dst_mac } => {
                        if need_decap {
                            continue;
                        }
                        if l3_length == 0 {
                            l3_length = (*length as u64).wrapping_sub(16);
                        }
                        if l2 {
                            bf.append_uint(K::SrcMAC, mac_to_u64(src_mac));
                            bf.append_uint(K::DstMAC, mac_to_u64(dst_mac));
                        }
                    }
                    FlowRecord::ExtendedSwitch { src_vlan, dst_vlan } => {
                        if need_decap {
                            continue;
                        }
                        if l2 {
                            if *src_vlan < 4096 {
                                bf.src_vlan = *src_vlan as u16;
                            }
                            if *dst_vlan < 4096 {
                                bf.dst_vlan = *dst_vlan as u16;
                            }
                        }
                    }
                    FlowRecord::ExtendedRouter { next_hop, src_mask_len, dst_mask_len } => {
                        if need_decap {
                            continue;
                        }
                        bf.src_net_mask = *src_mask_len as u8;
                        bf.dst_net_mask = *dst_mask_len as u8;
                        bf.next_hop = next_hop.and_then(decode_ip);
                    }
                    FlowRecord::ExtendedGateway { next_hop, r#as, src_as, as_path, communities } => {
                        if need_decap {
                            continue;
                        }
                        bf.next_hop = next_hop.and_then(decode_ip);
                        bf.dst_as = *r#as;
                        bf.src_as = *r#as;
                        if *src_as > 0 {
                            bf.src_as = *src_as;
                        }
                        if let Some(last) = as_path.last() {
                            bf.dst_as = *last;
                            bf.append_array_uint32(K::DstASPath, as_path);
                        }
                        if !communities.is_empty() {
                            bf.append_array_uint32(K::DstCommunities, communities);
                        }
                    }
                    FlowRecord::Other | FlowRecord::Missing => {}
                }
            }

            if l3_length > 0 {
                bf.append_uint(K::Bytes, l3_length);
            } else if need_decap {
                self.errors.with_label_values(&[&key, "non-encapsulated packet"]).inc();
                // Discard what was set for this sample. The Go version kept
                // it, leaking into the next flow.
                bf.undo();
                continue;
            }

            bf.exporter_address = agent_ip;
            bf.append_uint(K::Packets, 1);
            bf.append_uint(K::ForwardingStatus, forwarding_status);
            bf.time_received = ts as u32;
            finalize(bf);
        }
        Ok(packet.samples.len())
    }
}
