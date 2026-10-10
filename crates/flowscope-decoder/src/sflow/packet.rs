// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! sFlow v5 packet parsing. Only what the decoder uses is kept. It follows
//! goflow2 (used by the Go version) closely, including its limits and the
//! handling of truncated records.

use crate::netflow::packet::{Error, Reader};

fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}

/// A flow record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowRecord<'a> {
    /// Raw packet header (format 1).
    SampledHeader { protocol: u32, header: &'a [u8] },
    /// Ethernet frame data (format 2).
    SampledEthernet { length: u32, src_mac: &'a [u8], dst_mac: &'a [u8] },
    /// IPv4 data (format 3).
    SampledIpv4 {
        length: u32,
        protocol: u32,
        src_ip: &'a [u8],
        dst_ip: &'a [u8],
        src_port: u32,
        dst_port: u32,
        tos: u32,
    },
    /// IPv6 data (format 4).
    SampledIpv6 {
        length: u32,
        protocol: u32,
        src_ip: &'a [u8],
        dst_ip: &'a [u8],
        src_port: u32,
        dst_port: u32,
        priority: u32,
    },
    /// Extended switch data (format 1001).
    ExtendedSwitch { src_vlan: u32, dst_vlan: u32 },
    /// Extended router data (format 1002).
    ExtendedRouter { next_hop: Option<&'a [u8]>, src_mask_len: u32, dst_mask_len: u32 },
    /// Extended gateway data (format 1003).
    ExtendedGateway {
        next_hop: Option<&'a [u8]>,
        r#as: u32,
        src_as: u32,
        /// AS path when there is exactly one segment.
        as_path: Vec<u32>,
        communities: Vec<u32>,
    },
    /// Something we do not use.
    Other,
    /// A record announced but not present.
    Missing,
}

/// Type of sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    Flow,
    Counter,
    ExpandedFlow,
    Drop,
}

impl SampleKind {
    pub fn name(self) -> &'static str {
        match self {
            SampleKind::Flow => "FlowSample",
            SampleKind::Counter => "CounterSample",
            SampleKind::ExpandedFlow => "ExpandedFlowSample",
            SampleKind::Drop => "DropSample",
        }
    }
}

/// A sample. Counter samples only keep their number of records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample<'a> {
    pub kind: SampleKind,
    pub sampling_rate: u32,
    /// Flow samples: interface in the compact form (format in the 2 high
    /// bits). Expanded flow samples: format and value.
    pub input_format: u32,
    pub input_value: u32,
    pub output_format: u32,
    pub output_value: u32,
    /// Raw input and output for flow samples.
    pub input: u32,
    pub output: u32,
    /// Flow records (empty for counter samples).
    pub records: Vec<FlowRecord<'a>>,
    /// Number of records announced.
    pub records_count: usize,
}

/// A parsed sFlow packet. `samples` has one entry per announced sample:
/// `None` for samples not present in the packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub agent_ip: Option<&'a [u8]>,
    pub samples: Vec<Option<Sample<'a>>>,
}

fn decode_ip<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, Error> {
    let version = r.u32().map_err(|_| invalid("DecodeIP"))?;
    let len = match version {
        0 => return Ok(None),
        1 => 4,
        2 => 16,
        v => return Err(invalid(format!("DecodeIP: unknown IP version {v}"))),
    };
    if r.len() < len {
        return Err(invalid("DecodeIP: truncated data"));
    }
    Ok(Some(r.next(len)))
}

fn opaque<'a>(r: &mut Reader<'a>, length: u32) -> Result<&'a [u8], Error> {
    let length = length as usize;
    if length > r.len() {
        return Err(invalid("unexpected EOF"));
    }
    let data = r.next(length);
    let padding = (4 - length % 4) % 4;
    if padding != 0 {
        if r.len() < padding {
            return Err(invalid("unexpected EOF"));
        }
        r.next(padding);
    }
    Ok(data)
}

fn bytes<'a>(r: &mut Reader<'a>, n: usize) -> Result<&'a [u8], Error> {
    let b = r.next(n);
    if b.len() < n { Err(invalid("unexpected EOF")) } else { Ok(b) }
}

fn decode_flow_record<'a>(format: u32, mut r: Reader<'a>) -> Result<FlowRecord<'a>, Error> {
    let e = |err: Error| invalid(format!("[data-format:{format}] {err}"));
    Ok(match format {
        1 => {
            let protocol = r.u32().map_err(e)?;
            let _frame_length = r.u32().map_err(e)?;
            let _stripped = r.u32().map_err(e)?;
            let header_length = r.u32().map_err(e)?;
            let header = opaque(&mut r, header_length).map_err(e)?;
            FlowRecord::SampledHeader { protocol, header }
        }
        2 => {
            let length = r.u32().map_err(e)?;
            let src_mac = bytes(&mut r, 6).map_err(e)?;
            r.u16().map_err(e)?;
            let dst_mac = bytes(&mut r, 6).map_err(e)?;
            r.u16().map_err(e)?;
            let _eth_type = r.u32().map_err(e)?;
            FlowRecord::SampledEthernet { length, src_mac, dst_mac }
        }
        3 => FlowRecord::SampledIpv4 {
            length: r.u32().map_err(e)?,
            protocol: r.u32().map_err(e)?,
            src_ip: bytes(&mut r, 4).map_err(e)?,
            dst_ip: bytes(&mut r, 4).map_err(e)?,
            src_port: r.u32().map_err(e)?,
            dst_port: r.u32().map_err(e)?,
            tos: {
                let _tcp_flags = r.u32().map_err(e)?;
                r.u32().map_err(e)?
            },
        },
        4 => FlowRecord::SampledIpv6 {
            length: r.u32().map_err(e)?,
            protocol: r.u32().map_err(e)?,
            src_ip: bytes(&mut r, 16).map_err(e)?,
            dst_ip: bytes(&mut r, 16).map_err(e)?,
            src_port: r.u32().map_err(e)?,
            dst_port: r.u32().map_err(e)?,
            priority: {
                let _tcp_flags = r.u32().map_err(e)?;
                r.u32().map_err(e)?
            },
        },
        1001 => {
            let src_vlan = r.u32().map_err(e)?;
            let _src_priority = r.u32().map_err(e)?;
            let dst_vlan = r.u32().map_err(e)?;
            let _dst_priority = r.u32().map_err(e)?;
            FlowRecord::ExtendedSwitch { src_vlan, dst_vlan }
        }
        1002 => {
            let next_hop = decode_ip(&mut r).map_err(e)?;
            FlowRecord::ExtendedRouter {
                next_hop,
                src_mask_len: r.u32().map_err(e)?,
                dst_mask_len: r.u32().map_err(e)?,
            }
        }
        1003 => {
            let next_hop = decode_ip(&mut r).map_err(e)?;
            let r#as = r.u32().map_err(e)?;
            let src_as = r.u32().map_err(e)?;
            let _src_peer_as = r.u32().map_err(e)?;
            let segments = r.u32().map_err(e)?;
            if segments > 1000 {
                return Err(e(invalid(format!("as-path segments of {segments} seems quite large"))));
            }
            let mut first_path = Vec::new();
            for i in 0..segments {
                let _segment_type = r.u32().map_err(e)?;
                let length = r.u32().map_err(e)?;
                if length > 1000 {
                    return Err(e(invalid(format!("as-path length of {length} seems quite large"))));
                }
                if length as usize > r.len() / 4 {
                    return Err(e(invalid(format!("invalid AS path length: {length}"))));
                }
                let path: Vec<u32> = (0..length).map(|_| r.u32()).collect::<Result<_, _>>().map_err(e)?;
                if i == 0 {
                    first_path = path;
                }
            }
            // Like goflow2, the AS path is only set with exactly one segment.
            let as_path = if segments == 1 { first_path } else { Vec::new() };
            let communities_length = r.u32().map_err(e)?;
            if communities_length > 1000 {
                return Err(e(invalid(format!("communities length of {communities_length} seems quite large"))));
            }
            if communities_length as usize > r.len() / 4 {
                return Err(e(invalid(format!("invalid communities length: {communities_length}"))));
            }
            let communities: Vec<u32> =
                (0..communities_length).map(|_| r.u32()).collect::<Result<_, _>>().map_err(e)?;
            let _local_pref = r.u32().map_err(e)?;
            FlowRecord::ExtendedGateway { next_hop, r#as, src_as, as_path, communities }
        }
        1036 => {
            r.u32().map_err(e)?;
            FlowRecord::Other
        }
        1037 => {
            r.u32().map_err(e)?;
            let l = r.u32().map_err(e)?;
            opaque(&mut r, l).map_err(e)?;
            r.u32().map_err(e)?;
            FlowRecord::Other
        }
        1038 => {
            let l = r.u32().map_err(e)?;
            opaque(&mut r, l).map_err(e)?;
            FlowRecord::Other
        }
        _ => FlowRecord::Other,
    })
}

/// Counter records are decoded only to report errors like goflow2.
fn check_counter_record(format: u32, mut r: Reader) -> Result<(), Error> {
    let e = |_| invalid(format!("[data-format:{format}] unexpected EOF"));
    match format {
        // Generic interface counters: 5 x u32 + u64 + ... (88 bytes).
        1 => {
            for size in [4, 4, 8, 4, 4, 8, 4, 4, 4, 4, 4, 4, 8, 4, 4, 4, 4, 4, 4] {
                bytes(&mut r, size).map_err(e)?;
            }
        }
        // Ethernet counters: 13 x u32.
        2 => {
            for _ in 0..13 {
                r.u32().map_err(e)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn decode_sample<'a>(format: u32, mut r: Reader<'a>) -> Result<Sample<'a>, Error> {
    let seq = r.u32().map_err(|_| invalid("header seq"))?;
    let fe = |msg: String| invalid(format!("[format:{format} seq:{seq}] {msg}"));
    let kind = match format {
        1 => SampleKind::Flow,
        2 | 4 => SampleKind::Counter,
        3 => SampleKind::ExpandedFlow,
        5 => SampleKind::Drop,
        _ => return Err(fe(format!("unknown format {format}"))),
    };
    // Source ID.
    match format {
        1 | 2 => {
            r.u32().map_err(|_| fe("header source".into()))?;
        }
        _ => {
            r.u32().map_err(|_| fe("header source".into()))?;
            r.u32().map_err(|_| fe("header source".into()))?;
        }
    }
    let mut sample = Sample {
        kind,
        sampling_rate: 0,
        input_format: 0,
        input_value: 0,
        output_format: 0,
        output_value: 0,
        input: 0,
        output: 0,
        records: Vec::new(),
        records_count: 0,
    };
    let rec_err = |_| fe("raw".into());
    let count = match kind {
        SampleKind::Flow => {
            sample.sampling_rate = r.u32().map_err(rec_err)?;
            let _pool = r.u32().map_err(rec_err)?;
            let _drops = r.u32().map_err(rec_err)?;
            sample.input = r.u32().map_err(rec_err)?;
            sample.output = r.u32().map_err(rec_err)?;
            r.u32().map_err(rec_err)?
        }
        SampleKind::Counter => r.u32().map_err(|_| fe("eth".into()))?,
        SampleKind::ExpandedFlow => {
            sample.sampling_rate = r.u32().map_err(rec_err)?;
            let _pool = r.u32().map_err(rec_err)?;
            let _drops = r.u32().map_err(rec_err)?;
            sample.input_format = r.u32().map_err(rec_err)?;
            sample.input_value = r.u32().map_err(rec_err)?;
            sample.output_format = r.u32().map_err(rec_err)?;
            sample.output_value = r.u32().map_err(rec_err)?;
            r.u32().map_err(rec_err)?
        }
        SampleKind::Drop => {
            let _drops = r.u32().map_err(rec_err)?;
            let _input = r.u32().map_err(rec_err)?;
            let _output = r.u32().map_err(rec_err)?;
            let _reason = r.u32().map_err(rec_err)?;
            r.u32().map_err(rec_err)?
        }
    };
    if count > 1000 {
        return Err(fe(format!("too many flow records: {count}")));
    }
    sample.records_count = count as usize;
    if kind != SampleKind::Counter {
        sample.records = vec![FlowRecord::Missing; count as usize];
    }
    for i in 0..count as usize {
        if r.len() < 8 {
            break;
        }
        let data_format = r.u32().map_err(|_| fe("record header".into()))?;
        let length = r.u32().map_err(|_| fe("record header".into()))? as usize;
        if length > r.len() {
            break;
        }
        let body = Reader::new(r.next(length));
        if kind == SampleKind::Counter {
            check_counter_record(data_format, body).map_err(|err| fe(format!("counter [{err}]")))?;
        } else {
            sample.records[i] = decode_flow_record(data_format, body).map_err(|err| fe(format!("record [{err}]")))?;
        }
    }
    Ok(sample)
}

/// Parse an sFlow packet, including the version.
pub fn parse(payload: &[u8]) -> Result<Packet<'_>, Error> {
    let mut r = Reader::new(payload);
    let version = r.u32().map_err(|_| invalid("version"))?;
    if version != 5 {
        return Err(invalid(format!("unknown version {version}")));
    }
    let ip_version = r.u32().map_err(|_| invalid("IP version"))?;
    let agent_ip = match ip_version {
        0 => None,
        1 => Some(bytes(&mut r, 4).map_err(|_| invalid("IPv4"))?),
        2 => Some(bytes(&mut r, 16).map_err(|_| invalid("IPv6"))?),
        v => return Err(invalid(format!("unknown IP version {v}"))),
    };
    let header_err = |_| invalid("header");
    let _sub_agent_id = r.u32().map_err(header_err)?;
    let _sequence = r.u32().map_err(header_err)?;
    let _uptime = r.u32().map_err(header_err)?;
    let count = r.u32().map_err(header_err)?;
    if count > 1000 {
        return Err(invalid(format!("too many samples: {count}")));
    }
    let mut samples = vec![None; count as usize];
    for slot in samples.iter_mut() {
        if r.len() < 8 {
            break;
        }
        let format = r.u32().map_err(header_err)?;
        let length = r.u32().map_err(header_err)? as usize;
        if length > r.len() {
            break;
        }
        let body = Reader::new(r.next(length));
        *slot = Some(decode_sample(format, body).map_err(|e| invalid(format!("sample [{e}]")))?);
    }
    Ok(Packet { agent_ip, samples })
}
