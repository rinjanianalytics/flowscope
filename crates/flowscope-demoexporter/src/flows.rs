// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Generate flows and send them as NetFlow v9. Port of `demoexporter/flows`.

use crate::config::{FlowConfiguration, FlowsConfiguration};
use crate::random::Random;
use flowscope_common::reporter::{CounterVec, Reporter};
use ipnet::IpNet;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const PROTO_ICMPV4: u8 = 1;
const PROTO_TCP: u8 = 6;
const PROTO_UDP: u8 = 17;
const PROTO_ICMPV6: u8 = 58;

/// A generated flow.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedFlow {
    pub src_addr: IpAddr,
    pub dst_addr: IpAddr,
    pub packets: u32,
    pub octets: u32,
    pub input_int: u32,
    pub output_int: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub src_as: u32,
    pub dst_as: u32,
    pub proto: u8,
    pub forward_status: u8,
    pub flow_direction: u8,
    pub src_mask: u8,
    pub dst_mask: u8,
}

/// The number of items to produce during a second, for a rate per second.
/// Rates below one are spread over several seconds.
fn rate_to_count(rate: f64, unix: i64) -> u32 {
    // Count from a recent origin to keep the precision of the products.
    let seconds = unix.rem_euclid(360 * 86400) as f64;
    (((seconds + 1.0) * rate).trunc() - (seconds * rate).trunc()) as u32
}

/// A random address inside a prefix.
fn random_ip(prefix: &IpNet, r: &mut Random) -> IpAddr {
    let random = ((r.next_u64() as u128) << 64) | r.next_u64() as u128;
    match prefix {
        IpNet::V4(net) => {
            let host = random as u32 & u32::from(net.hostmask());
            IpAddr::V4((u32::from(net.network()) | host).into())
        }
        IpNet::V6(net) => {
            let host = random & u128::from(net.hostmask());
            IpAddr::V6((u128::from(net.network()) | host).into())
        }
    }
}

/// How close the time is to the peak hour: 1 at the peak, 0 twelve hours
/// away. Both are in hours since midnight.
fn peak_hour_distance(now: f64, peak: f64) -> f64 {
    let mut delta = (now - peak).abs() % 24.0;
    if 24.0 - delta < delta {
        delta = 24.0 - delta;
    }
    (12.0 - delta) / 12.0
}

/// The flows of one second. The result only depends on the configuration,
/// the seed and the time.
pub fn generate_flows(configs: &[FlowConfiguration], seed: i64, unix: i64) -> Vec<GeneratedFlow> {
    let mut flows = vec![];
    let mut r = Random::new(unix, seed);
    let hours = unix.rem_euclid(86400) as f64 / 3600.0;
    for config in configs {
        let (Some(src_net), Some(dst_net)) = (&config.src_net, &config.dst_net) else { continue };
        let peak = config.peak_hour.as_std().as_secs_f64() / 3600.0;
        let distance = peak_hour_distance(hours, peak);
        let square = distance * distance;
        let multiplier = 1.0 + (config.multiplier - 1.0) * square / (2.0 * (square - distance) + 1.0);
        let count = rate_to_count(config.per_second * multiplier * (0.9 + r.float() / 5.0), unix);
        for _ in 0..count {
            let octets = if config.size == 0 {
                r.below(1200) + 300
            } else {
                let size = (config.size as f64 * (r.normal() * 0.3 + 1.0)).max(0.0) as u32;
                if size > 9000 {
                    9000
                } else if size > 1500 && config.size <= 1500 {
                    1500
                } else {
                    size
                }
            };
            let src_addr = random_ip(src_net, &mut r);
            let dst_addr = random_ip(dst_net, &mut r);
            let proto = match (r.choose(&config.protocol_ids()), src_addr.is_ipv4()) {
                (Protocol::Tcp, _) => PROTO_TCP,
                (Protocol::Udp, _) => PROTO_UDP,
                (Protocol::Icmp, true) => PROTO_ICMPV4,
                (Protocol::Icmp, false) => PROTO_ICMPV6,
            };
            let mut port = |choices: &[u16]| match r.choose(choices) {
                0 => (r.below(2000) + 33000) as u16,
                port => port,
            };
            let (src_port, dst_port) = if proto == PROTO_TCP || proto == PROTO_UDP {
                let src = port(&config.src_port);
                (src, port(&config.dst_port))
            } else {
                (0, 0)
            };
            let flow = GeneratedFlow {
                src_addr,
                dst_addr,
                packets: 1,
                octets,
                input_int: r.choose(&config.in_if_index),
                output_int: r.choose(&config.out_if_index),
                src_port,
                dst_port,
                src_as: r.choose(&config.src_as),
                dst_as: r.choose(&config.dst_as),
                proto,
                forward_status: 64,
                flow_direction: 0,
                src_mask: src_net.prefix_len(),
                dst_mask: dst_net.prefix_len(),
            };
            let reverse = (config.reverse_direction_ratio > 0.0).then(|| GeneratedFlow {
                src_addr: flow.dst_addr,
                dst_addr: flow.src_addr,
                octets: (flow.octets as f32 * config.reverse_direction_ratio) as u32,
                input_int: flow.output_int,
                output_int: flow.input_int,
                src_port: flow.dst_port,
                dst_port: flow.src_port,
                src_as: flow.dst_as,
                dst_as: flow.src_as,
                src_mask: flow.dst_mask,
                dst_mask: flow.src_mask,
                ..flow.clone()
            });
            flows.push(flow);
            flows.extend(reverse);
        }
    }
    flows
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Protocol {
    #[default]
    Tcp,
    Udp,
    Icmp,
}

impl FlowConfiguration {
    fn protocol_ids(&self) -> Vec<Protocol> {
        self.protocol
            .iter()
            .map(|p| match p.as_str() {
                "udp" => Protocol::Udp,
                "icmp" => Protocol::Icmp,
                _ => Protocol::Tcp,
            })
            .collect()
    }
}

// NetFlow v9

const TEMPLATE_IPV4: u16 = 260;
const TEMPLATE_IPV6: u16 = 261;
const TEMPLATE_OPTIONS: u16 = 262;
const SAMPLER_ID: u16 = 1;

/// The fields common to IPv4 and IPv6 flows: type and length.
const IP_TEMPLATE: [(u16, u16); 16] = [
    (2, 4),  // IN_PKTS
    (1, 4),  // IN_BYTES
    (10, 4), // INPUT_SNMP
    (14, 4), // OUTPUT_SNMP
    (22, 4), // FIRST_SWITCHED
    (21, 4), // LAST_SWITCHED
    (7, 2),  // L4_SRC_PORT
    (11, 2), // L4_DST_PORT
    (16, 4), // SRC_AS
    (17, 4), // DST_AS
    (4, 1),  // PROTOCOL
    (89, 1), // FORWARDING_STATUS
    (61, 1), // flowDirection
    (48, 2), // FLOW_SAMPLER_ID
    (9, 1),  // SRC_MASK
    (13, 1), // DST_MASK
];
const IP_FLOW_LENGTH: usize = 43;
const IPV4_FLOW_LENGTH: usize = 8 + IP_FLOW_LENGTH;
const IPV6_FLOW_LENGTH: usize = 32 + IP_FLOW_LENGTH;
/// Payloads stay below 1400 bytes, to fit in a packet sent over IPv6.
const MAX_PAYLOAD: usize = 1400;

fn put_header(buf: &mut Vec<u8>, count: u16, uptime: u32, unix: u32, sequence: u32) {
    buf.extend_from_slice(&9u16.to_be_bytes());
    buf.extend_from_slice(&count.to_be_bytes());
    buf.extend_from_slice(&uptime.to_be_bytes());
    buf.extend_from_slice(&unix.to_be_bytes());
    buf.extend_from_slice(&sequence.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes()); // source ID
}

fn put_fields(buf: &mut Vec<u8>, fields: &[(u16, u16)]) {
    for (kind, length) in fields {
        buf.extend_from_slice(&kind.to_be_bytes());
        buf.extend_from_slice(&length.to_be_bytes());
    }
}

/// The packet declaring the templates, with the sampling rate.
pub fn netflow_templates(sequence: u32, sampling: u32, uptime: u32, unix: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    put_header(&mut buf, 4, uptime, unix, sequence);
    for (id, addresses) in [(TEMPLATE_IPV4, [(8, 4), (12, 4)]), (TEMPLATE_IPV6, [(27, 16), (28, 16)])] {
        let count = (addresses.len() + IP_TEMPLATE.len()) as u16;
        buf.extend_from_slice(&0u16.to_be_bytes()); // template flowset
        buf.extend_from_slice(&(count * 4 + 8).to_be_bytes());
        buf.extend_from_slice(&id.to_be_bytes());
        buf.extend_from_slice(&count.to_be_bytes());
        put_fields(&mut buf, &addresses);
        put_fields(&mut buf, &IP_TEMPLATE);
    }
    // Options template
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&26u16.to_be_bytes());
    buf.extend_from_slice(&TEMPLATE_OPTIONS.to_be_bytes());
    buf.extend_from_slice(&4u16.to_be_bytes()); // scope length
    buf.extend_from_slice(&12u16.to_be_bytes()); // option length
    put_fields(
        &mut buf,
        &[
            (1, 4),  // system scope
            (48, 2), // FLOW_SAMPLER_ID
            (50, 4), // FLOW_SAMPLER_RANDOM_INTERVAL
            (49, 1), // FLOW_SAMPLER_MODE
        ],
    );
    // And its data
    buf.extend_from_slice(&TEMPLATE_OPTIONS.to_be_bytes());
    buf.extend_from_slice(&15u16.to_be_bytes());
    buf.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]); // system scope
    buf.extend_from_slice(&SAMPLER_ID.to_be_bytes());
    buf.extend_from_slice(&sampling.to_be_bytes());
    buf.push(2); // mode: random
    buf
}

/// The packets carrying flows. IPv4 and IPv6 flows use different templates
/// and go to different packets. The sequence number grows with each packet.
pub fn netflow_data(flows: &[GeneratedFlow], sequence: u32, uptime: u32, unix: u32) -> Vec<Vec<u8>> {
    let mut packets = vec![];
    let mut sequence = sequence;
    for (v4, template, length) in [(true, TEMPLATE_IPV4, IPV4_FLOW_LENGTH), (false, TEMPLATE_IPV6, IPV6_FLOW_LENGTH)] {
        let family: Vec<&GeneratedFlow> = flows.iter().filter(|f| f.src_addr.is_ipv4() == v4).collect();
        for chunk in family.chunks(MAX_PAYLOAD / length) {
            let mut buf = Vec::with_capacity(24 + chunk.len() * length);
            put_header(&mut buf, chunk.len() as u16, uptime, unix, sequence);
            buf.extend_from_slice(&template.to_be_bytes());
            buf.extend_from_slice(&((chunk.len() * length + 4) as u16).to_be_bytes());
            for flow in chunk {
                for addr in [flow.src_addr, flow.dst_addr] {
                    match addr {
                        IpAddr::V4(ip) => buf.extend_from_slice(&ip.octets()),
                        IpAddr::V6(ip) => buf.extend_from_slice(&ip.octets()),
                    }
                }
                buf.extend_from_slice(&flow.packets.to_be_bytes());
                buf.extend_from_slice(&flow.octets.to_be_bytes());
                buf.extend_from_slice(&flow.input_int.to_be_bytes());
                buf.extend_from_slice(&flow.output_int.to_be_bytes());
                buf.extend_from_slice(&uptime.to_be_bytes()); // start
                buf.extend_from_slice(&uptime.to_be_bytes()); // end
                buf.extend_from_slice(&flow.src_port.to_be_bytes());
                buf.extend_from_slice(&flow.dst_port.to_be_bytes());
                buf.extend_from_slice(&flow.src_as.to_be_bytes());
                buf.extend_from_slice(&flow.dst_as.to_be_bytes());
                buf.push(flow.proto);
                buf.push(flow.forward_status);
                buf.push(flow.flow_direction);
                buf.extend_from_slice(&SAMPLER_ID.to_be_bytes());
                buf.push(flow.src_mask);
                buf.push(flow.dst_mask);
            }
            packets.push(buf);
            sequence = sequence.wrapping_add(1);
        }
    }
    packets
}

/// The flow generator.
pub struct Flows {
    config: FlowsConfiguration,
    sent: CounterVec,
    errors: CounterVec,
}

impl Flows {
    pub fn new(r: &Reporter, config: FlowsConfiguration) -> Flows {
        let m = r.metrics("demoexporter/flows");
        Flows {
            config,
            sent: m.counter_vec("sent_packets_total", "Number of packets sent.", &["type"]),
            errors: m.counter_vec("errors_total", "Number of transmission errors.", &["error"]),
        }
    }

    /// A socket connected to the target, whatever its address family.
    async fn dial(target: &str) -> Result<UdpSocket, String> {
        let addr = tokio::net::lookup_host(target)
            .await
            .map_err(|e| e.to_string())?
            .next()
            .ok_or_else(|| "no address found".to_string())?;
        let local = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let socket = UdpSocket::bind(local).await.map_err(|e| e.to_string())?;
        socket.connect(addr).await.map_err(|e| e.to_string())?;
        Ok(socket)
    }

    /// Send the flows of each second until the token is cancelled. The
    /// templates are sent again every 30 seconds.
    pub async fn run(self, token: CancellationToken) -> Result<(), String> {
        let target = self.config.target.clone();
        let mut socket = Self::dial(&target).await.map_err(|e| format!("cannot create socket to {target:?}: {e}"))?;
        tracing::info!("starting flows component");
        let mut sequence: u32 = 1;
        let mut elapsed: u32 = 0;
        let mut logged_errors = 0u32;
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = ticker.tick() => {}
            }
            let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
            let mut packets = vec![];
            if elapsed.is_multiple_of(30) {
                // The address of the target may change, in a container
                // setup for example.
                match Self::dial(&target).await {
                    Ok(new) if new.peer_addr().ok() != socket.peer_addr().ok() => {
                        tracing::info!(target = %target, address = ?new.peer_addr().ok(), "target resolved to new address");
                        socket = new;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.errors.with_label_values(&["redial"]).inc();
                        tracing::error!(error = %e, "cannot redial {target:?}");
                    }
                }
                packets
                    .push(("template", netflow_templates(sequence, self.config.sampling_rate, elapsed, unix as u32)));
            }
            let flows = generate_flows(&self.config.flows, self.config.seed, unix as i64);
            let first = sequence.wrapping_add(packets.len() as u32);
            packets.extend(netflow_data(&flows, first, elapsed, unix as u32).into_iter().map(|p| ("data", p)));
            elapsed = elapsed.wrapping_add(1);
            for (kind, payload) in packets {
                sequence = sequence.wrapping_add(1);
                match socket.send(&payload).await {
                    Ok(_) => self.sent.with_label_values(&[kind]).inc(),
                    Err(e) => {
                        self.errors.with_label_values(&["cannot write"]).inc();
                        // The inlet may not be there yet: do not flood.
                        if logged_errors < 10 {
                            logged_errors += 1;
                            tracing::error!(error = %e, "unable to send UDP payload");
                        }
                    }
                }
            }
            if elapsed.is_multiple_of(60) {
                logged_errors = 0;
            }
        }
        tracing::info!("flows component stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowscope_common::config::duration::Duration as ConfigDuration;

    fn config() -> FlowConfiguration {
        FlowConfiguration {
            per_second: 10.0,
            in_if_index: vec![10, 11],
            out_if_index: vec![20],
            peak_hour: ConfigDuration::from_secs(16 * 3600),
            multiplier: 3.0,
            src_net: Some("192.0.2.0/24".parse().unwrap()),
            dst_net: Some("203.0.113.0/28".parse().unwrap()),
            src_as: vec![65201],
            dst_as: vec![65202, 65203],
            src_port: vec![443],
            dst_port: vec![],
            protocol: vec!["tcp".into()],
            size: 1400,
            reverse_direction_ratio: 0.0,
        }
    }

    #[test]
    fn rate() {
        // Whole rates give the same count each second, the other ones
        // spread the remainder.
        for unix in 1_700_000_000..1_700_000_010 {
            assert_eq!(rate_to_count(5.0, unix), 5);
        }
        let total: u32 = (1_700_000_000..1_700_000_100).map(|t| rate_to_count(0.25, t)).sum();
        assert_eq!(total, 25);
        let total: u32 = (1_700_000_000..1_700_000_100).map(|t| rate_to_count(2.5, t)).sum();
        assert_eq!(total, 250);
    }

    #[test]
    fn peak() {
        assert_eq!(peak_hour_distance(16.0, 16.0), 1.0);
        assert_eq!(peak_hour_distance(4.0, 16.0), 0.0);
        assert_eq!(peak_hour_distance(22.0, 16.0), 0.5);
        assert_eq!(peak_hour_distance(1.0, 23.0), (12.0 - 2.0) / 12.0);
    }

    #[test]
    fn addresses() {
        let mut r = Random::new(1, 1);
        let v4: IpNet = "192.0.2.128/25".parse().unwrap();
        let v6: IpNet = "2001:db8:1:2::/64".parse().unwrap();
        let host: IpNet = "192.0.2.1/32".parse().unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            let ip = random_ip(&v4, &mut r);
            assert!(v4.contains(&ip), "{ip}");
            seen.insert(ip);
            assert!(v6.contains(&random_ip(&v6, &mut r)));
            assert_eq!(random_ip(&host, &mut r), host.addr());
        }
        assert!(seen.len() > 100, "addresses are spread over the prefix");
    }

    #[test]
    fn generated() {
        // 16:00 UTC is the peak hour: three times the base rate, more or
        // less 10%.
        let peak = 1_700_000_000 - 1_700_000_000 % 86400 + 16 * 3600;
        let flows = generate_flows(&[config()], 0, peak);
        assert!((27..=33).contains(&flows.len()), "{} flows at the peak", flows.len());
        let quiet = generate_flows(&[config()], 0, peak + 12 * 3600);
        assert!((9..=11).contains(&quiet.len()), "{} flows away from the peak", quiet.len());
        for flow in &flows {
            assert!("192.0.2.0/24".parse::<IpNet>().unwrap().contains(&flow.src_addr));
            assert!("203.0.113.0/28".parse::<IpNet>().unwrap().contains(&flow.dst_addr));
            assert_eq!((flow.src_mask, flow.dst_mask), (24, 28));
            assert_eq!(flow.proto, PROTO_TCP);
            assert_eq!(flow.src_port, 443);
            assert!((33000..35000).contains(&flow.dst_port));
            assert!([10, 11].contains(&flow.input_int));
            assert_eq!(flow.output_int, 20);
            assert_eq!(flow.src_as, 65201);
            assert!([65202, 65203].contains(&flow.dst_as));
            assert!(flow.octets <= 1500, "a size below 1500 is not exceeded");
        }
        // Same time and seed, same flows. Another seed, other flows.
        assert_eq!(flows, generate_flows(&[config()], 0, peak));
        assert_ne!(flows, generate_flows(&[config()], 100, peak));
    }

    #[test]
    fn reverse() {
        let config = FlowConfiguration { reverse_direction_ratio: 0.1, protocol: vec!["icmp".into()], ..config() };
        let flows = generate_flows(&[config], 0, 1_700_000_000);
        assert!(!flows.is_empty() && flows.len().is_multiple_of(2));
        for pair in flows.chunks(2) {
            let (flow, back) = (&pair[0], &pair[1]);
            assert_eq!((back.src_addr, back.dst_addr), (flow.dst_addr, flow.src_addr));
            assert_eq!((back.src_as, back.dst_as), (flow.dst_as, flow.src_as));
            assert_eq!((back.input_int, back.output_int), (flow.output_int, flow.input_int));
            assert_eq!((back.src_mask, back.dst_mask), (flow.dst_mask, flow.src_mask));
            assert_eq!(back.octets, (flow.octets as f32 * 0.1) as u32);
            assert_eq!((flow.proto, flow.src_port, flow.dst_port), (PROTO_ICMPV4, 0, 0));
        }
    }

    #[test]
    fn packets() {
        let v6 = FlowConfiguration {
            src_net: Some("2001:db8:1::/48".parse().unwrap()),
            dst_net: Some("2001:db8:2::/48".parse().unwrap()),
            per_second: 30.0,
            ..config()
        };
        let flows = generate_flows(&[FlowConfiguration { per_second: 40.0, ..config() }, v6], 0, 1_700_000_000);
        let packets = netflow_data(&flows, 7, 100, 1_700_000_000);
        let mut counted = 0;
        for (index, packet) in packets.iter().enumerate() {
            assert!(packet.len() <= 24 + MAX_PAYLOAD, "{} bytes", packet.len());
            assert_eq!(packet[0..2], [0, 9]);
            let count = u16::from_be_bytes([packet[2], packet[3]]) as usize;
            assert_eq!(u32::from_be_bytes(packet[12..16].try_into().unwrap()), 7 + index as u32);
            let template = u16::from_be_bytes([packet[20], packet[21]]);
            let length = if template == TEMPLATE_IPV4 { IPV4_FLOW_LENGTH } else { IPV6_FLOW_LENGTH };
            assert_eq!(u16::from_be_bytes([packet[22], packet[23]]) as usize, count * length + 4);
            assert_eq!(packet.len(), 24 + count * length);
            counted += count;
        }
        assert_eq!(counted, flows.len());
        // The lengths of the templates match the lengths of the records.
        let fields: usize = IP_TEMPLATE.iter().map(|(_, l)| *l as usize).sum();
        assert_eq!(fields, IP_FLOW_LENGTH);
        let templates = netflow_templates(1, 1000, 0, 1_700_000_000);
        assert_eq!(templates.len(), 20 + 2 * (8 + 18 * 4) + 26 + 15);
    }
}
