// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! UDP input. Port of `inlet/flow/input/udp`.
//!
//! Each worker owns a socket bound with SO_REUSEPORT to the same address and
//! runs in its own thread. The kernel receive timestamp and the number of
//! packets dropped by the kernel come from socket control messages.

use crate::SendFn;
use crate::config::UdpConfiguration;
use flowscope_common::reporter::{CounterVec, Gauge, GaugeVec, Reporter, SummaryVec};
use flowscope_pb::RawFlow;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// SO_RXQ_OVFL and SO_TIMESTAMP_NEW, from asm-generic/socket.h.
#[cfg(target_os = "linux")]
const SO_RXQ_OVFL: libc::c_int = 40;
#[cfg(target_os = "linux")]
const SO_TIMESTAMP_NEW: libc::c_int = 63;

#[derive(Clone)]
struct Metrics {
    bytes: CounterVec,
    packets: CounterVec,
    packet_size: SummaryVec,
    buffer_size: GaugeVec,
    errors: CounterVec,
    in_drops: GaugeVec,
    ebpf: Gauge,
}

impl Metrics {
    fn new(r: &Reporter) -> Self {
        let m = r.metrics("inlet/flow/input/udp");
        let labels = &["listener", "worker", "exporter"];
        Metrics {
            bytes: m.counter_vec("bytes_total", "Bytes received by the application.", labels),
            packets: m.counter_vec("packets_total", "Packets received by the application.", labels),
            packet_size: m.summary_vec("size_bytes", "Summary of packet size.", Duration::ZERO, labels),
            buffer_size: m.gauge_vec(
                "buffer_size_bytes",
                "Size of the in-kernel buffer for this worker.",
                &["listener", "worker"],
            ),
            errors: m.counter_vec(
                "errors_total",
                "Errors while receiving packets by the application.",
                &["listener", "worker"],
            ),
            in_drops: m.gauge_vec(
                "in_dropped_packets_total",
                "Dropped packets due to listen queue full.",
                &["listener", "worker"],
            ),
            ebpf: m.gauge("ebpf_loaded", "Is eBPF load balancer enabled?"),
        }
    }
}

/// A running UDP input.
pub struct UdpInput {
    local_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    #[cfg(target_os = "linux")]
    _ebpf: Option<crate::ebpf::ReuseportProgram>,
}

/// Resolve the listen address. ":port" listens on all addresses (IPv6
/// socket accepting IPv4 too).
fn resolve(listen: &str) -> Result<SocketAddr, String> {
    if let Some(port) = listen.strip_prefix(':') {
        let port: u16 = port.parse().map_err(|_| format!("invalid port in {listen:?}"))?;
        return Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port));
    }
    listen
        .to_socket_addrs()
        .map_err(|e| format!("unable to resolve {listen}: {e}"))?
        .next()
        .ok_or_else(|| format!("unable to resolve {listen}"))
}

fn bind(addr: SocketAddr, receive_buffer: u64) -> Result<Socket, String> {
    let domain = if addr.is_ipv6() { Domain::IPV6 } else { Domain::IPV4 };
    let socket =
        Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| format!("cannot create socket: {e}"))?;
    if addr.is_ipv6() {
        let _ = socket.set_only_v6(false);
    }
    socket.set_reuse_address(true).map_err(|e| format!("cannot set option SO_REUSEADDR: {e}"))?;
    #[cfg(unix)]
    socket.set_reuse_port(true).map_err(|e| format!("cannot set option SO_REUSEPORT: {e}"))?;
    #[cfg(target_os = "linux")]
    for (name, opt) in [("SO_RXQ_OVFL", SO_RXQ_OVFL), ("SO_TIMESTAMP_NEW", SO_TIMESTAMP_NEW)] {
        let one: libc::c_int = 1;
        // SAFETY: valid socket, option value is a c_int.
        let rc = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                opt,
                &one as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            tracing::warn!(error = %std::io::Error::last_os_error(), "cannot set option {name}");
        }
    }
    if receive_buffer > 0
        && let Err(e) = socket.set_recv_buffer_size(receive_buffer as usize)
    {
        tracing::warn!(error = %e, "unable to set requested buffer size ({receive_buffer} bytes)");
    }
    // Wake up regularly to check if we should stop.
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| format!("cannot set receive timeout: {e}"))?;
    socket.bind(&addr.into()).map_err(|e| format!("unable to listen to {addr}: {e}"))?;
    Ok(socket)
}

/// Information extracted from control messages.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ControlMessage {
    pub drops: Option<u32>,
    pub received: Option<u64>,
}

/// Parse control messages (SO_RXQ_OVFL and SO_TIMESTAMP_NEW).
#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)] // cmsg_len is not usize on every libc
pub(crate) fn parse_control(buf: &[u8]) -> ControlMessage {
    let mut out = ControlMessage::default();
    let hdr_len = std::mem::size_of::<libc::cmsghdr>();
    let align = std::mem::size_of::<usize>();
    let mut offset = 0;
    while offset + hdr_len <= buf.len() {
        // SAFETY: we checked the header fits; read unaligned.
        let hdr: libc::cmsghdr = unsafe { std::ptr::read_unaligned(buf[offset..].as_ptr() as *const libc::cmsghdr) };
        let len = hdr.cmsg_len as usize;
        if len < hdr_len || offset + len > buf.len() {
            break;
        }
        let data = &buf[offset + hdr_len..offset + len];
        if hdr.cmsg_level == libc::SOL_SOCKET {
            if hdr.cmsg_type == SO_RXQ_OVFL && data.len() >= 4 {
                out.drops = Some(u32::from_ne_bytes(data[..4].try_into().unwrap()));
            } else if hdr.cmsg_type == SO_TIMESTAMP_NEW && data.len() >= 16 {
                let sec = i64::from_ne_bytes(data[..8].try_into().unwrap());
                if sec > 0 {
                    out.received = Some(sec as u64);
                }
            }
        }
        offset += len.div_ceil(align) * align;
    }
    out
}

/// Receive one packet. Returns the size, the source and the control
/// messages.
#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)] // msg_controllen is not usize on every libc
fn receive(
    socket: &Socket,
    payload: &mut [u8],
    control: &mut [u8],
) -> std::io::Result<(usize, SocketAddr, ControlMessage)> {
    // SAFETY: zeroed sockaddr_storage is a valid value.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut iov = libc::iovec { iov_base: payload.as_mut_ptr() as *mut libc::c_void, iov_len: payload.len() };
    // SAFETY: zeroed msghdr is a valid value, fields are set below.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = &mut storage as *mut _ as *mut libc::c_void;
    msg.msg_namelen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = control.len() as _;
    // SAFETY: msg points to valid buffers for the duration of the call.
    let n = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, 0) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let source = sockaddr_to_std(&storage)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "unknown address family"))?;
    let cmsg = parse_control(&control[..msg.msg_controllen as usize]);
    Ok((n as usize, source, cmsg))
}

#[cfg(target_os = "linux")]
fn sockaddr_to_std(storage: &libc::sockaddr_storage) -> Option<SocketAddr> {
    match storage.ss_family as libc::c_int {
        libc::AF_INET => {
            // SAFETY: family checked.
            let a: &libc::sockaddr_in = unsafe { &*(storage as *const _ as *const libc::sockaddr_in) };
            let ip = std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
            Some(SocketAddr::new(IpAddr::V4(ip), u16::from_be(a.sin_port)))
        }
        libc::AF_INET6 => {
            // SAFETY: family checked.
            let a: &libc::sockaddr_in6 = unsafe { &*(storage as *const _ as *const libc::sockaddr_in6) };
            let ip = Ipv6Addr::from(a.sin6_addr.s6_addr);
            Some(SocketAddr::new(IpAddr::V6(ip), u16::from_be(a.sin6_port)))
        }
        _ => None,
    }
}

/// Exporter address as a string, IPv4 addresses without the IPv6 prefix, and
/// its 16-byte form.
pub(crate) fn exporter(ip: IpAddr) -> (String, [u8; 16]) {
    match ip {
        IpAddr::V4(v4) => (v4.to_string(), v4.to_ipv6_mapped().octets()),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => (v4.to_string(), v6.octets()),
            None => (v6.to_string(), v6.octets()),
        },
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl UdpInput {
    /// Start the input.
    pub fn start(r: &Reporter, config: &UdpConfiguration, send: SendFn) -> Result<UdpInput, String> {
        let metrics = Metrics::new(r);
        metrics.ebpf.set(0.0);
        tracing::info!(listen = %config.listen, "starting UDP input");
        let mut addr = resolve(&config.listen)?;
        let mut sockets = Vec::with_capacity(config.workers);
        for i in 0..config.workers {
            let socket = bind(addr, config.receive_buffer)?;
            // With port 0, the next workers reuse the port of the first one.
            addr = socket
                .local_addr()
                .ok()
                .and_then(|a| a.as_socket())
                .ok_or_else(|| "cannot get local address".to_string())?;
            if i == 0 {
                tracing::info!(listen = %addr, "UDP input listening");
            }
            let actual = socket.recv_buffer_size().unwrap_or(0);
            metrics.buffer_size.with_label_values(&[&config.listen, &i.to_string()]).set(actual as f64);
            if config.receive_buffer > 0 && (actual as u64) < config.receive_buffer {
                tracing::warn!(listen = %config.listen, requested = config.receive_buffer, actual,
                    "UDP receive buffer size was capped by system limits (check net.core.rmem_max)");
            }
            metrics.errors.with_label_values(&[&config.listen, &i.to_string()]);
            metrics.in_drops.with_label_values(&[&config.listen, &i.to_string()]);
            sockets.push(socket);
        }

        #[cfg(target_os = "linux")]
        let ebpf = {
            let fds: Vec<i32> = sockets.iter().map(|s| s.as_raw_fd()).collect();
            match crate::ebpf::ReuseportProgram::attach(&fds) {
                Ok(p) => {
                    metrics.ebpf.set(1.0);
                    Some(p)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "cannot attach eBPF program for SO_REUSEPORT");
                    None
                }
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        for (i, socket) in sockets.into_iter().enumerate() {
            let stop = stop.clone();
            let metrics = metrics.clone();
            let send = send.clone();
            let listen = config.listen.clone();
            let thread = std::thread::Builder::new()
                .name(format!("udp-worker-{i}"))
                .spawn(move || worker(i, socket, listen, metrics, send, stop))
                .map_err(|e| format!("cannot start worker: {e}"))?;
            threads.push(thread);
        }
        Ok(UdpInput {
            local_addr: addr,
            stop,
            threads,
            #[cfg(target_os = "linux")]
            _ebpf: ebpf,
        })
    }

    /// Address the input listens to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stop the input and wait for the workers.
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
        tracing::info!("UDP listener stopped");
    }
}

fn worker(id: usize, socket: Socket, listen: String, metrics: Metrics, send: SendFn, stop: Arc<AtomicBool>) {
    let worker = id.to_string();
    let mut payload = vec![0u8; 9000];
    let mut control = vec![0u8; 128];
    let mut last_error_log: Option<Instant> = None;
    let mut flow = RawFlow::default();
    while !stop.load(Ordering::Relaxed) {
        #[cfg(target_os = "linux")]
        let result = receive(&socket, &mut payload, &mut control);
        #[cfg(not(target_os = "linux"))]
        let result = {
            let buf = unsafe { &mut *(payload.as_mut_slice() as *mut [u8] as *mut [std::mem::MaybeUninit<u8>]) };
            socket.recv_from(buf).map(|(n, a)| (n, a.as_socket().unwrap(), ControlMessage::default()))
        };
        let (n, source, cmsg) = match result {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                if last_error_log.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                    last_error_log = Some(Instant::now());
                    tracing::error!(error = %e, worker, listen, "unable to receive UDP packet");
                }
                metrics.errors.with_label_values(&[&listen, &worker]).inc();
                continue;
            }
        };
        if let Some(drops) = cmsg.drops {
            metrics.in_drops.with_label_values(&[&listen, &worker]).set(drops as f64);
        }
        let (exporter, address) = exporter(source.ip());
        metrics.bytes.with_label_values(&[&listen, &worker, &exporter]).inc_by(n as f64);
        metrics.packets.with_label_values(&[&listen, &worker, &exporter]).inc();
        metrics.packet_size.with_label_values(&[&listen, &worker, &exporter]).observe(n as f64);

        flow.time_received = cmsg.received.unwrap_or_else(now_secs);
        flow.payload.clear();
        flow.payload.extend_from_slice(&payload[..n]);
        flow.source_address.clear();
        flow.source_address.extend_from_slice(&address);
        send(&exporter, &mut flow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn exporters() {
        assert_eq!(exporter("192.0.2.1".parse().unwrap()).0, "192.0.2.1");
        assert_eq!(exporter("::ffff:192.0.2.1".parse().unwrap()).0, "192.0.2.1");
        assert_eq!(exporter("2001:db8::1".parse().unwrap()).0, "2001:db8::1");
        assert_eq!(exporter("192.0.2.1".parse().unwrap()).1[..12], [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff]);
    }

    #[test]
    fn receive_packets() {
        let r = Reporter::new_mock();
        let received: Arc<Mutex<Vec<(String, RawFlow)>>> = Arc::new(Mutex::new(vec![]));
        let rx = received.clone();
        let send: SendFn = Arc::new(move |exporter: &str, flow: &mut RawFlow| {
            rx.lock().unwrap().push((exporter.to_string(), flow.clone()));
        });
        let config = UdpConfiguration { listen: "127.0.0.1:0".into(), workers: 2, receive_buffer: 0 };
        let input = UdpInput::start(&r, &config, send).unwrap();
        let addr = input.local_addr();
        let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        for i in 0..10u8 {
            client.send_to(&[i, 1, 2, 3], addr).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while received.lock().unwrap().len() < 10 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        input.stop();
        let got = received.lock().unwrap();
        assert_eq!(got.len(), 10);
        let now = now_secs();
        for (exporter, flow) in got.iter() {
            assert_eq!(exporter, "127.0.0.1");
            assert_eq!(flow.payload.len(), 4);
            assert_eq!(&flow.payload[1..], &[1, 2, 3]);
            assert_eq!(
                flow.source_address,
                "127.0.0.1".parse::<std::net::Ipv4Addr>().unwrap().to_ipv6_mapped().octets()
            );
            assert!(flow.time_received.abs_diff(now) < 5, "{} vs {now}", flow.time_received);
        }
        let metrics = r.render_metrics();
        assert!(metrics.contains("akvorado_inlet_flow_input_udp_packets_total"), "{metrics}");
    }
}
