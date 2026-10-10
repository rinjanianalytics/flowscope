// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Compare the decoders with the Go implementation on the pcap files of the
//! Go tests. The expected output comes from
//! `go run ./tests/golden/decoders . > tests/golden/testdata/decoders.json`.

use flowscope_common::reporter::Reporter;
use flowscope_decoder::{Decoder, Input, Options, helpers, netflow::NetflowDecoder, sflow::SflowDecoder};
use flowscope_pb::{DecapsulationProtocol, TimestampSource};
use flowscope_schema::{Component, FlowMessage, format_ip};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Read packets from a classic pcap file.
fn pcap_frames(path: &Path) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap();
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let le = match magic {
        0xa1b2c3d4 | 0xa1b23c4d => true,
        0xd4c3b2a1 | 0x4d3cb2a1 => false,
        _ => panic!("{}: not a pcap file", path.display()),
    };
    let u32_at = |o: usize| {
        let b: [u8; 4] = data[o..o + 4].try_into().unwrap();
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    };
    let mut frames = Vec::new();
    let mut offset = 24;
    while offset + 16 <= data.len() {
        let caplen = u32_at(offset + 8) as usize;
        offset += 16;
        frames.push(data[offset..offset + caplen].to_vec());
        offset += caplen;
    }
    frames
}

/// UDP or TCP payload of an Ethernet frame.
fn l4_payload(frame: &[u8]) -> Vec<u8> {
    let mut etype = u16::from_be_bytes([frame[12], frame[13]]);
    let mut d = &frame[14..];
    while etype == 0x8100 || etype == 0x88a8 {
        etype = u16::from_be_bytes([d[2], d[3]]);
        d = &d[4..];
    }
    let (proto, l4) = match etype {
        0x0800 => {
            let ihl = ((d[0] & 0xf) as usize) * 4;
            let total = u16::from_be_bytes([d[2], d[3]]) as usize;
            (d[9], &d[ihl..total.min(d.len())])
        }
        0x86dd => {
            let len = u16::from_be_bytes([d[4], d[5]]) as usize;
            (d[6], &d[40..(40 + len).min(d.len())])
        }
        e => panic!("unexpected ethertype {e:#x}"),
    };
    match proto {
        17 => {
            let len = u16::from_be_bytes([l4[4], l4[5]]) as usize;
            l4[8..len.min(l4.len())].to_vec()
        }
        6 => {
            let off = ((l4[12] >> 4) as usize) * 4;
            l4[off..].to_vec()
        }
        p => panic!("unexpected protocol {p}"),
    }
}

fn ip(v: Option<std::net::Ipv6Addr>) -> String {
    v.map(|a| format_ip(&a)).unwrap_or_default()
}

fn capture(bf: &FlowMessage) -> Value {
    let columns: BTreeMap<String, Value> = bf.other_columns.iter().map(|(k, v)| (k.name(), v.to_json())).collect();
    json!({
        "time_received": bf.time_received,
        "sampling_rate": bf.sampling_rate,
        "exporter_address": ip(bf.exporter_address),
        "in_if": bf.in_if,
        "out_if": bf.out_if,
        "src_vlan": bf.src_vlan,
        "dst_vlan": bf.dst_vlan,
        "src_addr": ip(bf.src_addr),
        "dst_addr": ip(bf.dst_addr),
        "next_hop": ip(bf.next_hop),
        "src_as": bf.src_as,
        "dst_as": bf.dst_as,
        "src_net_mask": bf.src_net_mask,
        "dst_net_mask": bf.dst_net_mask,
        "columns": columns,
    })
}

fn schema(all: bool) -> Component {
    let c = Component::default_schema();
    if all { c.enable_all_columns() } else { c }
}

fn timestamp_source(s: &str) -> TimestampSource {
    s.parse().unwrap()
}

fn decap(s: &str) -> DecapsulationProtocol {
    s.parse().unwrap()
}

/// Columns are compared as maps, the JSON numbers as numbers.
fn normalize(mut v: Value) -> Value {
    if let Some(cols) = v.get_mut("columns") {
        let sorted: BTreeMap<String, Value> =
            cols.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        *cols = serde_json::to_value(sorted).unwrap();
    }
    v
}

fn load() -> Value {
    let raw = std::fs::read_to_string(root().join("tests/golden/testdata/decoders.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn decoders_match_go() {
    let golden = load();
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in golden["decoders"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let file = root().join(case["file"].as_str().unwrap());
        let sch = schema(case["all_columns"].as_bool().unwrap());
        let r = Reporter::new_mock();
        let decoder: Box<dyn Decoder> = match case["decoder"].as_str().unwrap() {
            "netflow" => Box::new(NetflowDecoder::new(&r)),
            _ => Box::new(SflowDecoder::new(&r)),
        };
        let options = Options {
            timestamp_source: timestamp_source(case["timestamp_source"].as_str().unwrap()),
            decapsulation_protocol: decap(case["decapsulation"].as_str().unwrap()),
        };
        let mut bf = sch.new_flow_message();
        bf.debug = true;
        let expected_packets = case["packets"].as_array().unwrap();
        let frames = pcap_frames(&file);
        assert_eq!(frames.len(), expected_packets.len(), "{name}: packet count");
        for (i, (frame, expected)) in frames.iter().zip(expected_packets).enumerate() {
            let payload = l4_payload(frame);
            let input =
                Input { time_received: 1_700_000_000, payload: &payload, source: "::ffff:127.0.0.1".parse().unwrap() };
            let mut flows = Vec::new();
            let result = decoder.decode(&input, &options, &mut bf, &mut |bf: &mut FlowMessage| {
                flows.push(normalize(capture(bf)));
                bf.clear();
            });
            let got = json!({
                "error": result.is_err(),
                "count": result.as_ref().copied().unwrap_or(0),
                "flows": flows,
            });
            let mut expected = expected.clone();
            for f in expected["flows"].as_array_mut().unwrap() {
                *f = normalize(f.clone());
            }
            checked += 1;
            if got != expected {
                failures.push(format!(
                    "{name} packet {i}:\n  got:      {}\n  expected: {}",
                    serde_json::to_string(&got).unwrap(),
                    serde_json::to_string(&expected).unwrap()
                ));
            }
        }
    }
    assert!(checked >= 1040, "only {checked} packets checked");
    if !failures.is_empty() {
        let n = failures.len();
        failures.truncate(5);
        panic!("{n} of {checked} packets differ from Go, first ones:\n{}", failures.join("\n"));
    }
}

#[test]
fn ethernet_matches_go() {
    let golden = load();
    let mut failures = Vec::new();
    for case in golden["ethernet"].as_array().unwrap() {
        let file = root().join(case["file"].as_str().unwrap());
        let sch = schema(case["all_columns"].as_bool().unwrap());
        let mut bf = sch.new_flow_message();
        bf.debug = true;
        let frames = pcap_frames(&file);
        let l3 =
            helpers::parse_ethernet(sch.schema(), &mut bf, decap(case["decapsulation"].as_str().unwrap()), &frames[0]);
        let got = json!({"l3_length": l3, "flow": normalize(capture(&bf))});
        let expected = json!({"l3_length": case["l3_length"], "flow": normalize(case["flow"].clone())});
        if got != expected {
            failures.push(format!(
                "{} all={} decap={}:\n  got:      {got}\n  expected: {expected}",
                case["file"], case["all_columns"], case["decapsulation"]
            ));
        }
    }
    if !failures.is_empty() {
        let n = failures.len();
        failures.truncate(5);
        panic!("{n} cases differ from Go, first ones:\n{}", failures.join("\n"));
    }
}

/// Replace null with an empty list, like Go encodes empty slices.
fn null_to_empty(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if x.is_null() && matches!(k.as_str(), "fields" | "scopes" | "options") {
                    *x = json!([]);
                }
                null_to_empty(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(null_to_empty),
        _ => {}
    }
}

#[test]
fn netflow_state_matches_go() {
    let golden = load();
    let states = golden["netflow_states"].as_object().unwrap();
    assert!(states.len() > 10);
    for (file, expected) in states {
        let r = Reporter::new_mock();
        let decoder = NetflowDecoder::new(&r);
        let sch = schema(true);
        let mut bf = sch.new_flow_message();
        for frame in pcap_frames(&root().join(file)) {
            let payload = l4_payload(&frame);
            let input =
                Input { time_received: 1_700_000_000, payload: &payload, source: "::ffff:127.0.0.1".parse().unwrap() };
            let _ = decoder.decode(&input, &Options::default(), &mut bf, &mut |bf: &mut FlowMessage| bf.clear());
        }
        let mut expected = expected.clone();
        null_to_empty(&mut expected);
        assert_eq!(decoder.state(), expected, "{file}");

        // The Go state can be restored, and saved back identically.
        let restored = NetflowDecoder::new(&r);
        restored.restore(&golden["netflow_states"][file]).unwrap();
        assert_eq!(restored.state(), expected, "{file} after restore");
    }
}
