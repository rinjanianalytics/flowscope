// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Decoders must never panic, whatever the input. Feed truncated and altered
//! versions of the test packets.

use flowscope_common::reporter::Reporter;
use flowscope_decoder::{Decoder, Input, Options, netflow::NetflowDecoder, sflow::SflowDecoder};
use flowscope_pb::DecapsulationProtocol;
use flowscope_schema::{Component, FlowMessage};
use std::path::Path;

fn packets(dir: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(dir);
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "pcap") {
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        let mut offset = 24;
        while offset + 16 <= data.len() {
            let caplen = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
            offset += 16;
            let frame = &data[offset..offset + caplen];
            offset += caplen;
            // Skip Ethernet, IP and UDP headers roughly: the payload start
            // does not matter much for this test, take a few offsets.
            for start in [42, 62] {
                if frame.len() > start {
                    out.push(frame[start..].to_vec());
                }
            }
        }
    }
    out
}

fn run(decoder: &dyn Decoder, inputs: &[Vec<u8>]) {
    let schema = Component::default_schema().enable_all_columns();
    let mut bf = schema.new_flow_message();
    for decap in [DecapsulationProtocol::None, DecapsulationProtocol::Vxlan, DecapsulationProtocol::Srv6] {
        let options = Options { decapsulation_protocol: decap, ..Default::default() };
        for payload in inputs {
            let mut variants = vec![];
            for len in 0..payload.len().min(300) {
                variants.push(payload[..len].to_vec());
            }
            for i in (0..payload.len()).step_by(7) {
                let mut altered = payload.clone();
                altered[i] ^= 0xff;
                variants.push(altered);
            }
            for v in variants {
                let input = Input { time_received: 1, payload: &v, source: "::1".parse().unwrap() };
                let _ = decoder.decode(&input, &options, &mut bf, &mut |bf: &mut FlowMessage| bf.finalize());
                bf.clear();
            }
        }
    }
}

#[test]
fn netflow_never_panics() {
    let r = Reporter::new_mock();
    run(&NetflowDecoder::new(&r), &packets("tests/data/netflow"));
}

#[test]
fn sflow_never_panics() {
    let r = Reporter::new_mock();
    run(&SflowDecoder::new(&r), &packets("tests/data/sflow"));
}
