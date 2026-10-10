// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Behaviors where the Rust decoders deliberately differ from the Go version.

use flowscope_common::reporter::Reporter;
use flowscope_decoder::{Decoder, Input, Options, netflow::NetflowDecoder, sflow::SflowDecoder};
use flowscope_pb::DecapsulationProtocol;
use flowscope_schema::{Component, FlowMessage};

fn decode(decoder: &dyn Decoder, payload: &[u8], options: Options) -> Vec<FlowMessage> {
    let schema = Component::default_schema().enable_all_columns();
    let mut bf = schema.new_flow_message();
    bf.debug = true;
    let mut flows = vec![];
    let input = Input { time_received: 1_700_000_000, payload, source: "::ffff:192.0.2.1".parse().unwrap() };
    decoder
        .decode(&input, &options, &mut bf, &mut |bf: &mut FlowMessage| {
            flows.push(bf.clone());
            bf.clear();
        })
        .unwrap();
    flows
}

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// An sFlow packet with the provided samples (format, body).
fn sflow_packet(samples: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut p = u32s(&[5, 1, 0xc0000201, 0, 1, 100, samples.len() as u32]);
    for (format, body) in samples {
        p.extend(u32s(&[*format, body.len() as u32]));
        p.extend(body);
    }
    p
}

/// A flow sample with one sampled IPv4 record.
fn flow_sample(input: u32) -> Vec<u8> {
    let mut record = u32s(&[1500, 6]);
    record.extend([192, 0, 2, 10, 198, 51, 100, 20]);
    record.extend(u32s(&[443, 33000, 0, 0]));
    let mut s = u32s(&[1, 0, 1024, 0, 0, input, 20, 1]);
    s.extend(u32s(&[3, record.len() as u32]));
    s.extend(record);
    s
}

#[test]
fn sflow_counter_samples_give_no_flow() {
    let r = Reporter::new_mock();
    let counter = u32s(&[1, 0, 1, 99, 4, 0]); // one unknown counter record of 4 bytes
    let packet = sflow_packet(&[(2, counter), (1, flow_sample(10)), (2, u32s(&[2, 0, 0]))]);
    let flows = decode(&SflowDecoder::new(&r), &packet, Options::default());
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].in_if, 10);
    assert_eq!(flows[0].sampling_rate, 1024);
}

#[test]
fn sflow_skipped_sample_does_not_leak() {
    // With decapsulation, a sampled IPv4 record (no header) cannot be
    // decapsulated: the sample is skipped. Its interface must not end in the
    // next flow, and its columns must not stay set.
    let r = Reporter::new_mock();
    // Input in the "multiple interfaces" format: not an ifIndex, so the
    // decoder does not set it.
    let mut header_sample = u32s(&[2, 0, 512, 0, 0, 0x8000_0002, 30, 1]);
    // Sampled header: Ethernet + IPv4 + UDP to port 4789 + VXLAN + inner frame.
    let mut frame = vec![0u8; 12];
    frame.extend([0x08, 0x00]);
    let mut inner_eth = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0x08, 0x00];
    let mut inner_ip = vec![0x45, 0, 0, 40, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 1, 10, 0, 0, 2];
    inner_ip.extend([0, 80, 0, 81, 0, 0, 0, 0, 0, 0, 0, 0, 0x50, 0x02, 0, 0, 0, 0, 0, 0]);
    inner_eth.extend(&inner_ip);
    let mut udp = vec![0x30, 0x39, 0x12, 0xb5, 0, 0, 0, 0];
    udp.extend([0x08, 0, 0, 0, 0, 0, 0, 0]);
    udp.extend(&inner_eth);
    let total = 20 + udp.len();
    let mut outer_ip =
        vec![0x45, 0, (total >> 8) as u8, total as u8, 0, 0, 0, 0, 64, 17, 0, 0, 192, 0, 2, 1, 192, 0, 2, 2];
    outer_ip.extend(&udp);
    frame.extend(&outer_ip);
    let mut record = u32s(&[1, frame.len() as u32, 0, frame.len() as u32]);
    let padded = frame.len().div_ceil(4) * 4;
    frame.resize(padded, 0);
    record.extend(&frame);
    header_sample.extend(u32s(&[1, record.len() as u32]));
    header_sample.extend(record);

    let packet = sflow_packet(&[(1, flow_sample(77)), (1, header_sample)]);
    let options = Options { decapsulation_protocol: DecapsulationProtocol::Vxlan, ..Default::default() };
    let flows = decode(&SflowDecoder::new(&r), &packet, options);
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].in_if, 0, "interface of the skipped sample leaked");
    assert_eq!(flows[0].out_if, 30);
    assert_eq!(flows[0].sampling_rate, 512);
    assert_eq!(flows[0].src_addr, Some("::ffff:10.0.0.1".parse().unwrap()));
}

#[test]
fn netflow_v5_short_packet() {
    let r = Reporter::new_mock();
    // Announce 3 records, carry 1.
    let mut p = vec![0, 5, 0, 3];
    p.extend(u32s(&[1000, 1_700_000_000, 0, 1]));
    p.extend([0, 0, 0, 0]);
    let mut rec = u32s(&[0xc0000201, 0xc0000202, 0]);
    rec.extend([0, 1, 0, 2]);
    rec.extend(u32s(&[10, 1000, 0, 0]));
    rec.extend([0, 80, 0, 81, 0, 0, 6, 0, 0, 0, 0, 0, 24, 24, 0, 0]);
    p.extend(rec);
    let flows = decode(&NetflowDecoder::new(&r), &p, Options::default());
    assert_eq!(flows.len(), 1);
}
