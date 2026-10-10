// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! BMP routing provider: a BMP collector feeding a RIB. Port of
//! `outlet/routing/provider/bmp`.

pub mod parse;
pub mod rib;

use super::rd::{Rd, Rt};
use super::{LargeCommunity as RoutingLargeCommunity, LookupResult, Provider};
use flowscope_common::config::{Duration, migrate};
use flowscope_common::daemon::Daemon;
use flowscope_common::helpers::validate_listen;
use flowscope_common::migrated;
use flowscope_common::reporter::{CounterVec, GaugeVec, Reporter, SummaryVec};
use futures::future::BoxFuture;
use ipnet::{IpNet, Ipv6Net};
use parse::{
    Family, HEADER_SIZE, MSG_INITIATION, MSG_PEER_DOWN_NOTIFICATION, MSG_PEER_UP_NOTIFICATION, MSG_ROUTE_MIRRORING,
    MSG_ROUTE_MONITORING, MSG_STATISTICS_REPORT, MSG_TERMINATION, Nlri as ParsedNlri, Open, PEER_HEADER_SIZE,
    PEER_TYPE_L3VPN, PeerHeader, RouteMonitoring, Update, UpdateOptions, add_path_families, parse_header,
    parse_info_tlvs, parse_peer_header, parse_peer_up, parse_route_monitoring,
};
use rib::{Nlri, RawRoute, Rib, RouteAttributes, addr_to6};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// BMP provider configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "kebab-case", default, deny_unknown_fields)]
pub struct BmpConfiguration {
    /// Address to listen to for BMP connections.
    pub listen: String,
    /// Route distinguishers to accept (all when empty).
    pub rds: Vec<Rd>,
    /// Route targets to accept (all when empty).
    pub rts: Vec<Rt>,
    #[serde(rename = "collect-asns")]
    pub collect_asns: bool,
    #[serde(rename = "collect-as-paths")]
    pub collect_as_paths: bool,
    pub collect_communities: bool,
    /// How long to keep routes from a disconnected peer.
    pub keep: Duration,
    /// Size of the socket receive buffer.
    pub receive_buffer: u64,
    /// Number of buffered messages per connection.
    pub message_buffer: u64,
    /// Number of shards of the RIB.
    #[serde(rename = "rib-shards")]
    pub rib_shards: u32,
}

impl Default for BmpConfiguration {
    fn default() -> Self {
        BmpConfiguration {
            listen: ":10179".into(),
            rds: vec![],
            rts: vec![],
            collect_asns: true,
            collect_as_paths: true,
            collect_communities: true,
            keep: Duration::from_secs(300),
            receive_buffer: 0,
            message_buffer: 10000,
            rib_shards: 16,
        }
    }
}

fn migrate_bmp(map: &mut Map<String, Value>) -> Result<(), String> {
    for k in
        ["RIBPeerRemovalMaxTime", "RIBPeerRemovalSleepInterval", "RIBPeerRemovalMaxQueue", "RIBPeerRemovalBatchRoutes"]
    {
        migrate::remove(map, k);
    }
    Ok(())
}

migrated!(BmpConfiguration, migrate_bmp);

impl BmpConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        validate_listen(&self.listen).map_err(|e| format!("routing.provider.listen: {e}"))?;
        if self.keep.as_secs() < 1 {
            return Err("routing.provider.keep should be at least 1s".into());
        }
        if self.message_buffer < 1 {
            return Err("routing.provider.message-buffer should be at least 1".into());
        }
        if ![1, 2, 4, 8, 16, 32, 64, 128, 256].contains(&self.rib_shards) {
            return Err("routing.provider.rib-shards should be a power of 2 up to 256".into());
        }
        Ok(())
    }
}

/// Key identifying a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PeerKey {
    /// Exporter address and source port.
    pub exporter: SocketAddr,
    /// Peer address (None for a Loc-RIB peer).
    pub ip: Option<Ipv6Addr>,
    pub peer_type: u8,
    pub distinguisher: u64,
    pub asn: u32,
    pub bgp_id: u32,
}

impl PeerKey {
    fn from_header(exporter: SocketAddr, h: &PeerHeader) -> PeerKey {
        PeerKey {
            exporter,
            ip: h.address,
            peer_type: h.peer_type,
            distinguisher: h.distinguisher,
            asn: h.asn,
            bgp_id: h.bgp_id,
        }
    }
}

/// Information attached to a peer.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// Reference used in the RIB.
    pub reference: u32,
    /// When to remove the peer once its connection is down.
    pub stale_until: Option<Instant>,
    /// Decoding options (ADD-PATH).
    pub options: UpdateOptions,
}

struct Metrics {
    opened_connections: CounterVec,
    closed_connections: CounterVec,
    peers: GaugeVec,
    routes: GaugeVec,
    buffer_size: GaugeVec,
    ignored_nlri: CounterVec,
    messages: CounterVec,
    errors: CounterVec,
    ignored: CounterVec,
    panics: CounterVec,
    locked: SummaryVec,
    peer_removal_done: CounterVec,
    message_queue_full: CounterVec,
    message_queue_not_full: CounterVec,
    prefixes_added: CounterVec,
    prefixes_removed: CounterVec,
    prefixes_updated: CounterVec,
}

#[derive(Default)]
pub struct Peers {
    pub peers: HashMap<PeerKey, PeerInfo>,
    last_reference: u32,
}

/// State shared by the tasks of the provider.
pub struct Inner {
    config: BmpConfiguration,
    accepted_rds: HashSet<u64>,
    accepted_rts: HashSet<u64>,
    active: AtomicBool,
    pub rib: Rib,
    pub peers: RwLock<Peers>,
    metrics: Metrics,
    local_addr: Mutex<Option<SocketAddr>>,
}

/// The BMP provider.
pub struct BmpProvider {
    pub inner: Arc<Inner>,
    daemon: Daemon,
}

fn unmap(ip: &Ipv6Addr) -> String {
    flowscope_common::helpers::unmap_ip(ip)
}

impl BmpProvider {
    pub fn new(r: &Reporter, config: BmpConfiguration, daemon: &Daemon) -> Result<Self, String> {
        let m = r.metrics("outlet/routing/provider/bmp");
        let e = &["exporter"];
        let metrics = Metrics {
            opened_connections: m.counter_vec("opened_connections_total", "Number of opened connections.", e),
            closed_connections: m.counter_vec("closed_connections_total", "Number of closed connections.", e),
            peers: m.gauge_vec("peers", "Number of peers up.", e),
            routes: m.gauge_vec("routes", "Number of routes up.", e),
            buffer_size: m.gauge_vec("buffer_size_bytes", "Size of the in-kernel buffer for this connection.", e),
            ignored_nlri: m.counter_vec(
                "ignored_nlri_total",
                "Number ignored MP NLRI received.",
                &["exporter", "type"],
            ),
            messages: m.counter_vec(
                "received_messages_total",
                "Number of BMP messages received.",
                &["exporter", "type"],
            ),
            errors: m.counter_vec(
                "errors_total",
                "Number of fatal errors while processing BMP messages.",
                &["exporter", "error"],
            ),
            ignored: m.counter_vec("ignored_updates_total", "Number of ignored BGP updates.", &["exporter", "error"]),
            panics: m.counter_vec("panics_total", "Number of fatal errors while processing BMP messages.", e),
            locked: m.summary_vec(
                "locked_duration_seconds",
                "Duration during which the RIB is locked.",
                std::time::Duration::from_secs(600),
                &["reason"],
            ),
            peer_removal_done: m.counter_vec("removed_peers_total", "Number of peers removed from the RIB.", e),
            message_queue_full: m.counter_vec(
                "message_queue_full_total",
                "Number of BMP messages hitting the message queue limit.",
                e,
            ),
            message_queue_not_full: m.counter_vec(
                "message_queue_notfull_total",
                "Number of BMP messages not hitting the message queue limit.",
                e,
            ),
            prefixes_added: m.counter_vec("prefixes_added_total", "Number of prefixes added to the RIB.", e),
            prefixes_removed: m.counter_vec("prefixes_removed_total", "Number of prefixes removed from the RIB.", e),
            prefixes_updated: m.counter_vec("prefixes_updated_total", "Number of prefixes updated in the RIB.", e),
        };
        Ok(BmpProvider {
            inner: Arc::new(Inner {
                accepted_rds: config.rds.iter().map(|r| r.0).collect(),
                accepted_rts: config.rts.iter().map(|r| r.0).collect(),
                config,
                active: AtomicBool::new(false),
                rib: Rib::new(),
                peers: RwLock::new(Peers::default()),
                metrics,
                local_addr: Mutex::new(None),
            }),
            daemon: daemon.clone(),
        })
    }

    /// Address the provider listens to, once started.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        *self.inner.local_addr.lock().unwrap()
    }
}

impl Provider for BmpProvider {
    fn start(&self) -> Result<(), String> {
        tracing::info!("starting BMP provider");
        let listen = &self.inner.config.listen;
        let addr = if listen.starts_with(':') { format!("[::]{listen}") } else { listen.clone() };
        let std_listener =
            std::net::TcpListener::bind(&addr).map_err(|e| format!("unable to listen to {listen}: {e}"))?;
        std_listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let listener = TcpListener::from_std(std_listener).map_err(|e| e.to_string())?;
        *self.inner.local_addr.lock().unwrap() = listener.local_addr().ok();
        let inner = self.inner.clone();
        let token = self.daemon.token();
        let daemon = self.daemon.clone();
        self.daemon.spawn("outlet/bmp", async move {
            loop {
                let accepted = tokio::select! {
                    _ = token.cancelled() => return Ok::<(), String>(()),
                    a = listener.accept() => a,
                };
                let (stream, remote) = accepted.map_err(|e| format!("cannot accept new connection: {e}"))?;
                let exporter = SocketAddr::new(IpAddr::V6(addr_to6(remote.ip())), remote.port());
                inner.setup_socket(&stream, &exporter);
                inner.active.store(true, Ordering::Relaxed);
                let inner = inner.clone();
                let token = token.clone();
                daemon.spawn_background(async move { inner.serve_connection(stream, exporter, token).await });
            }
        });
        // Removal of stale peers.
        let inner = self.inner.clone();
        let token = self.daemon.token();
        self.daemon.spawn_background(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    _ = ticker.tick() => inner.remove_stale_peers(Instant::now()),
                }
            }
        });
        Ok(())
    }

    fn lookup(
        &self,
        ip: Ipv6Addr,
        nh: Option<Ipv6Addr>,
        _agent: Ipv6Addr,
    ) -> BoxFuture<'_, Result<LookupResult, String>> {
        let result = self.inner.lookup(ip, nh);
        Box::pin(async move { result })
    }
}

/// A BMP message passed from the reader to the processor.
struct BmpMessage {
    msg_type: u8,
    body: Vec<u8>,
}

impl Inner {
    fn lookup(&self, ip: Ipv6Addr, nh: Option<Ipv6Addr>) -> Result<LookupResult, String> {
        let c = &self.config;
        if !c.collect_asns && !c.collect_as_paths && !c.collect_communities {
            return Ok(LookupResult::default());
        }
        if !self.active.load(Ordering::Relaxed) {
            return Ok(LookupResult::default());
        }
        let (attrs, next_hop, mut plen) = self.rib.lookup(ip, nh).ok_or("no route found")?;
        if ip.to_ipv4_mapped().is_some() {
            plen = plen.saturating_sub(96);
        }
        Ok(LookupResult {
            asn: attrs.asn,
            as_path: attrs.as_path,
            communities: attrs.communities,
            large_communities: attrs
                .large_communities
                .iter()
                .map(|l| RoutingLargeCommunity { asn: l.asn, local_data1: l.local_data1, local_data2: l.local_data2 })
                .collect(),
            net_mask: plen,
            next_hop,
        })
    }

    /// Apply the requested receive buffer size and report the actual one.
    fn setup_socket(&self, stream: &TcpStream, exporter: &SocketAddr) {
        use std::os::fd::AsRawFd;
        let exporter_str = unmap(&addr_to6(exporter.ip()));
        let fd = stream.as_raw_fd();
        let size = self.config.receive_buffer as libc::c_int;
        if size > 0 {
            // SAFETY: valid socket and a pointer to an int of the right size.
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    &size as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            if rc != 0 {
                tracing::warn!(listen = %self.config.listen, error = %std::io::Error::last_os_error(),
                    "unable to set requested TCP receive buffer size ({size} bytes)");
            }
        }
        let mut actual: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: valid socket and pointers to an int and its size.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                &mut actual as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc == 0 {
            self.metrics.buffer_size.with_label_values(&[&exporter_str]).set(actual as f64);
            if size > 0 && actual < size {
                tracing::warn!(listen = %self.config.listen, requested = size, actual,
                    "TCP receive buffer size was capped by system limits (check net.core.rmem_max)");
            }
        }
    }

    /// Read BMP messages from a connection and hand them to a processing
    /// task through a bounded queue.
    async fn serve_connection(self: Arc<Self>, stream: TcpStream, exporter: SocketAddr, token: CancellationToken) {
        let exporter_str = unmap(&addr_to6(exporter.ip()));
        let m = &self.metrics;
        m.opened_connections.with_label_values(&[&exporter_str]).inc();
        m.closed_connections.with_label_values(&[&exporter_str]);
        let sock = socket2::SockRef::from(&stream);
        let _ = sock.set_linger(Some(std::time::Duration::ZERO));
        let _ = sock.set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(std::time::Duration::from_secs(60)));
        self.handle_connection_up(&exporter_str);

        let (mut reader, writer) = stream.into_split();
        let (tx, rx) = tokio::sync::mpsc::channel::<BmpMessage>(self.config.message_buffer.max(1) as usize);
        // Cancelled when the processor stops, to unblock the reader.
        let conn_token = token.child_token();
        let processor = {
            let this = self.clone();
            let conn_token = conn_token.clone();
            let exporter_str = exporter_str.clone();
            tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(this.process_messages(exporter, &exporter_str, rx));
                if futures::FutureExt::catch_unwind(result).await.is_err() {
                    tracing::error!(exporter = %exporter_str, "fatal error while processing BMP messages");
                    this.metrics.panics.with_label_values(&[&exporter_str]).inc();
                }
                conn_token.cancel();
            })
        };

        let label = |t: &str| m.messages.with_label_values(&[&exporter_str, t]);
        for t in [
            "route-monitoring",
            "statistics-report",
            "peer-down-notification",
            "peer-up-notification",
            "initiation",
            "termination",
            "route-mirroring",
            "unknown",
        ] {
            label(t);
        }
        let error = |e: &str| m.errors.with_label_values(&[&exporter_str, e]).inc();
        let mut init = false;
        let mut header = [0u8; HEADER_SIZE];
        loop {
            let read = tokio::select! {
                _ = conn_token.cancelled() => break,
                r = reader.read_exact(&mut header) => r,
            };
            if let Err(e) = read {
                if !token.is_cancelled() && e.kind() != std::io::ErrorKind::UnexpectedEof {
                    tracing::error!(exporter = %exporter_str, error = %e, "cannot read BMP header");
                    error("cannot read BMP header");
                }
                break;
            }
            let h = parse_header(&header);
            if h.version != 3 {
                tracing::error!(exporter = %exporter_str, "cannot decode BMP header");
                error("cannot decode BMP header");
                break;
            }
            if (h.length as usize) < HEADER_SIZE || h.length as usize > MAX_BMP_MESSAGE_SIZE {
                tracing::error!(exporter = %exporter_str, length = h.length, "invalid BMP message length");
                error("invalid BMP message length");
                break;
            }
            let keep = match h.msg_type {
                MSG_ROUTE_MONITORING => {
                    label("route-monitoring").inc();
                    true
                }
                MSG_STATISTICS_REPORT => {
                    label("statistics-report").inc();
                    false
                }
                MSG_PEER_DOWN_NOTIFICATION => {
                    label("peer-down-notification").inc();
                    true
                }
                MSG_PEER_UP_NOTIFICATION => {
                    label("peer-up-notification").inc();
                    true
                }
                MSG_INITIATION => {
                    label("initiation").inc();
                    init = true;
                    true
                }
                MSG_TERMINATION => {
                    label("termination").inc();
                    true
                }
                MSG_ROUTE_MIRRORING => {
                    // Not parsed: it may contain malformed BGP messages.
                    label("route-mirroring").inc();
                    false
                }
                t => {
                    tracing::info!(exporter = %exporter_str, "unknown BMP message type {t}");
                    label("unknown").inc();
                    false
                }
            };
            if !init {
                tracing::error!(exporter = %exporter_str, "first message is not `initiation'");
                error("first message not initiation");
                break;
            }
            let mut body = vec![0u8; h.length as usize - HEADER_SIZE];
            let read = tokio::select! {
                _ = conn_token.cancelled() => break,
                r = reader.read_exact(&mut body) => r,
            };
            if let Err(e) = read {
                if !token.is_cancelled() {
                    tracing::error!(exporter = %exporter_str, error = %e, "cannot read BMP body");
                    error("cannot read BMP body");
                }
                break;
            }
            if !keep {
                continue;
            }
            let msg = BmpMessage { msg_type: h.msg_type, body };
            match tx.try_send(msg) {
                Ok(()) => m.message_queue_not_full.with_label_values(&[&exporter_str]).inc(),
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                Err(tokio::sync::mpsc::error::TrySendError::Full(msg)) => {
                    m.message_queue_full.with_label_values(&[&exporter_str]).inc();
                    let sent = tokio::select! {
                        _ = conn_token.cancelled() => false,
                        r = tx.send(msg) => r.is_ok(),
                    };
                    if !sent {
                        break;
                    }
                }
            }
        }
        drop(tx);
        drop(reader);
        drop(writer);
        let _ = processor.await;
        if !token.is_cancelled() {
            tracing::info!(exporter = %exporter_str, "connection down for {exporter_str}");
            self.mark_exporter_as_stale(exporter, Instant::now() + *self.config.keep);
        }
        m.closed_connections.with_label_values(&[&exporter_str]).inc();
    }

    async fn process_messages(
        &self,
        exporter: SocketAddr,
        exporter_str: &str,
        mut rx: tokio::sync::mpsc::Receiver<BmpMessage>,
    ) {
        while let Some(m) = rx.recv().await {
            let mut body = &m.body[..];
            let mut pkey = None;
            let mut options = UpdateOptions::default();
            if m.msg_type != MSG_INITIATION && m.msg_type != MSG_TERMINATION {
                let header = match parse_peer_header(body) {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::error!(exporter = %exporter_str, error = %e, "cannot parse BMP peer header");
                        self.metrics.errors.with_label_values(&[exporter_str, "cannot parse BMP peer header"]).inc();
                        return;
                    }
                };
                body = &body[PEER_HEADER_SIZE..];
                let key = PeerKey::from_header(exporter, &header);
                if let Some(info) = self.peers.read().unwrap().peers.get(&key) {
                    options = info.options.clone();
                }
                // RFC 7854: the A flag tells AS_PATH uses 2-byte AS numbers.
                // gobgp ignores it.
                options.two_byte_as = header.flags & 0x20 != 0;
                pkey = Some(key);
            }
            match m.msg_type {
                MSG_INITIATION => {
                    let name = parse_info_tlvs(body).into_iter().find(|(t, _)| *t == 2);
                    match name {
                        Some((_, v)) => {
                            tracing::info!(exporter = %exporter_str, sysname = %String::from_utf8_lossy(&v), "new connection")
                        }
                        None => tracing::info!(exporter = %exporter_str, "new connection"),
                    }
                }
                MSG_TERMINATION => {
                    let reason = parse_info_tlvs(body).into_iter().find(|(t, _)| *t == 0);
                    match reason {
                        Some((_, v)) => tracing::info!(exporter = %exporter_str,
                            reason = %String::from_utf8_lossy(&v), "termination message received"),
                        None => tracing::info!(exporter = %exporter_str, "termination message received"),
                    }
                    return;
                }
                MSG_PEER_UP_NOTIFICATION => match parse_peer_up(body) {
                    Ok((sent, received)) => self.handle_peer_up(pkey.unwrap(), &sent, &received),
                    Err(e) => {
                        tracing::error!(exporter = %exporter_str, error = %e, "cannot parse BMP body");
                        self.metrics.errors.with_label_values(&[exporter_str, "cannot parse BMP body"]).inc();
                        return;
                    }
                },
                MSG_PEER_DOWN_NOTIFICATION => self.handle_peer_down(pkey.unwrap()),
                MSG_ROUTE_MONITORING => match parse_route_monitoring(body, &options) {
                    Ok(RouteMonitoring::Update(update)) => self.handle_route_monitoring(pkey.unwrap(), &update),
                    Ok(RouteMonitoring::Other) => {}
                    Err(e) => {
                        self.metrics.ignored.with_label_values(&[exporter_str, e.handling.label()]).inc();
                    }
                },
                _ => {}
            }
        }
    }

    fn handle_connection_up(&self, exporter_str: &str) {
        let m = &self.metrics;
        // Only create the series: the label may cover several exporters.
        m.peers.with_label_values(&[exporter_str]).add(0.0);
        m.routes.with_label_values(&[exporter_str]).add(0.0);
        m.prefixes_added.with_label_values(&[exporter_str]).inc_by(0.0);
        m.prefixes_removed.with_label_values(&[exporter_str]).inc_by(0.0);
        m.prefixes_updated.with_label_values(&[exporter_str]).inc_by(0.0);
    }

    fn add_peer(peers: &mut Peers, key: PeerKey) -> &mut PeerInfo {
        peers.last_reference = peers.last_reference.wrapping_add(1);
        if peers.last_reference == 0 {
            // Very unlikely: four billion peer ups. Better restart.
            panic!("too many peer up events");
        }
        let reference = peers.last_reference;
        peers.peers.entry(key).or_insert(PeerInfo { reference, stale_until: None, options: UpdateOptions::default() })
    }

    /// Remove a peer, with the lock held.
    fn remove_peer(&self, peers: &mut Peers, key: &PeerKey, reason: &str) {
        let exporter_str = unmap(&addr_to6(key.exporter.ip()));
        let peer_str = key.ip.map(|ip| unmap(&ip)).unwrap_or_else(|| "invalid IP".into());
        tracing::info!("remove peer {peer_str} for exporter {exporter_str} (reason: {reason})");
        let start = Instant::now();
        let Some(info) = peers.peers.remove(key) else { return };
        let (routes, prefixes) = self.rib.flush_peer(info.reference);
        let m = &self.metrics;
        m.routes.with_label_values(&[&exporter_str]).sub(routes as f64);
        m.prefixes_removed.with_label_values(&[&exporter_str]).inc_by(prefixes as f64);
        m.peers.with_label_values(&[&exporter_str]).dec();
        m.peer_removal_done.with_label_values(&[&exporter_str]).inc();
        m.locked.with_label_values(&["peer-removal"]).observe(start.elapsed().as_secs_f64());
    }

    /// Remove peers stale at the given time.
    pub fn remove_stale_peers(&self, now: Instant) {
        if !self.peers.read().unwrap().peers.values().any(|p| p.stale_until.is_some_and(|s| s <= now)) {
            return;
        }
        let mut peers = self.peers.write().unwrap();
        let stale: Vec<PeerKey> =
            peers.peers.iter().filter(|(_, p)| p.stale_until.is_some_and(|s| s <= now)).map(|(k, _)| *k).collect();
        for key in stale {
            self.remove_peer(&mut peers, &key, "stale");
        }
    }

    fn mark_exporter_as_stale(&self, exporter: SocketAddr, until: Instant) {
        let mut peers = self.peers.write().unwrap();
        for (key, info) in peers.peers.iter_mut() {
            if key.exporter == exporter {
                info.stale_until = Some(until);
            }
        }
    }

    fn handle_peer_down(&self, key: PeerKey) {
        let mut peers = self.peers.write().unwrap();
        if !peers.peers.contains_key(&key) {
            tracing::info!(
                "received peer down from exporter {} for peer {}, but no peer up",
                unmap(&addr_to6(key.exporter.ip())),
                key.ip.map(|ip| unmap(&ip)).unwrap_or_default()
            );
            return;
        }
        self.remove_peer(&mut peers, &key, "down");
    }

    fn handle_peer_up(&self, key: PeerKey, sent: &Open, received: &Open) {
        let exporter_str = unmap(&addr_to6(key.exporter.ip()));
        let mut peers = self.peers.write().unwrap();
        if peers.peers.contains_key(&key) {
            tracing::info!(
                "received extra peer up from exporter {exporter_str} for peer {}",
                key.ip.map(|ip| unmap(&ip)).unwrap_or_default()
            );
        } else {
            self.metrics.peers.with_label_values(&[&exporter_str]).inc();
        }
        let info = Self::add_peer_if_missing(&mut peers, key);
        info.options.add_path = add_path_families(sent, received);
        tracing::debug!(addpath = ?info.options.add_path, "new peer from exporter {exporter_str}");
    }

    fn add_peer_if_missing(peers: &mut Peers, key: PeerKey) -> &mut PeerInfo {
        if peers.peers.contains_key(&key) {
            return peers.peers.get_mut(&key).unwrap();
        }
        Self::add_peer(peers, key)
    }

    fn is_accepted_rd(&self, rd: u64) -> bool {
        self.accepted_rds.is_empty() || self.accepted_rds.contains(&rd)
    }

    fn handle_route_monitoring(&self, key: PeerKey, update: &Update) {
        // An L3VPN peer is ignored when its RD is not accepted.
        if key.peer_type == PEER_TYPE_L3VPN && !self.is_accepted_rd(key.distinguisher) {
            return;
        }
        let c = &self.config;
        let exporter_str = unmap(&addr_to6(key.exporter.ip()));
        let mut nh = update.next_hop;
        let mut rta = RouteAttributes::default();
        if c.collect_asns || c.collect_as_paths {
            rta.as_path = update.as_path.clone().unwrap_or_default();
        }
        if c.collect_communities {
            rta.communities = update.communities.clone().unwrap_or_default();
            rta.large_communities = update.large_communities.clone().unwrap_or_default();
        }
        if !self.accepted_rts.is_empty() {
            let rts: Vec<u64> =
                update.extended_communities.iter().filter_map(Rt::from_extended_community).map(|rt| rt.0).collect();
            let accepted = rts.iter().any(|rt| self.accepted_rts.contains(rt));
            // Without RT, accept if 0 is in the accepted list.
            if !accepted && (!rts.is_empty() || !self.accepted_rts.contains(&0)) {
                return;
            }
        }
        // Without AS path, the peer AS is the origin AS, otherwise the last AS.
        if c.collect_asns {
            rta.asn = rta.as_path.last().copied().unwrap_or(key.asn);
        }
        if !c.collect_as_paths {
            rta.as_path = vec![];
        }

        let peer_ref = {
            let mut peers = self.peers.write().unwrap();
            if !peers.peers.contains_key(&key) {
                // We may have missed the peer up notification.
                tracing::info!(
                    "received route monitoring from exporter {exporter_str} for peer {}, but no peer up",
                    key.ip.map(|ip| unmap(&ip)).unwrap_or_default()
                );
                self.metrics.peers.with_label_values(&[&exporter_str]).inc();
            }
            Self::add_peer_if_missing(&mut peers, key).reference
        };

        let (mut routes_added, mut routes_removed) = (0i64, 0i64);
        let (mut prefixes_added, mut prefixes_removed, mut prefixes_updated) = (0u64, 0u64, 0u64);
        let mut add = |prefix: IpNet, family: Family, rd: u64, path: u32, nh: Option<Ipv6Addr>| {
            let prefix = map_prefix(prefix);
            let (added, is_new) = self.rib.add_route(
                prefix,
                RawRoute {
                    peer: peer_ref,
                    nlri: Nlri { family, path, rd },
                    next_hop: nh,
                    attributes: rta.clone(),
                    prefix_len: prefix.prefix_len(),
                },
            );
            routes_added += added as i64;
            if is_new {
                prefixes_added += 1;
            } else {
                prefixes_updated += 1;
            }
        };
        // Regular NLRI.
        if key.peer_type == PEER_TYPE_L3VPN || self.is_accepted_rd(0) {
            for p in &update.nlri {
                if let ParsedNlri::Prefix(prefix) = p.nlri {
                    add(prefix, Family::IPV4_UNICAST, key.distinguisher, p.path_id, nh);
                }
            }
        }
        let mut remove = |prefix: IpNet, family: Family, rd: u64, path: u32| {
            let (removed, prefix_removed) =
                self.rib.remove_route(map_prefix(prefix), peer_ref, Nlri { family, path, rd });
            routes_removed += removed as i64;
            if prefix_removed {
                prefixes_removed += 1;
            }
        };
        // Withdrawn routes.
        if key.peer_type == PEER_TYPE_L3VPN || self.is_accepted_rd(0) {
            for p in &update.withdrawn {
                if let ParsedNlri::Prefix(prefix) = p.nlri {
                    remove(prefix, Family::IPV4_UNICAST, key.distinguisher, p.path_id);
                }
            }
        }
        // MP reach and unreach NLRI, in order.
        for mp in &update.mp {
            if mp.reach {
                nh = mp.next_hop;
            }
            for p in &mp.paths {
                let (prefix, rd) = match p.nlri {
                    ParsedNlri::Prefix(prefix) => (prefix, key.distinguisher),
                    ParsedNlri::WithRd(rd, prefix) => (prefix, rd),
                    ParsedNlri::Unhandled => {
                        self.metrics.ignored_nlri.with_label_values(&[&exporter_str, &mp.family.to_string()]).inc();
                        continue;
                    }
                };
                if key.peer_type != PEER_TYPE_L3VPN && !self.is_accepted_rd(rd) {
                    continue;
                }
                if mp.reach {
                    add(prefix, mp.family, rd, p.path_id, nh);
                } else {
                    remove(prefix, mp.family, rd, p.path_id);
                }
            }
        }

        let m = &self.metrics;
        m.routes.with_label_values(&[&exporter_str]).add((routes_added - routes_removed) as f64);
        m.prefixes_added.with_label_values(&[&exporter_str]).inc_by(prefixes_added as f64);
        m.prefixes_removed.with_label_values(&[&exporter_str]).inc_by(prefixes_removed as f64);
        m.prefixes_updated.with_label_values(&[&exporter_str]).inc_by(prefixes_updated as f64);
    }
}

/// Map an IPv4 prefix to an IPv6 one, like the Go version (the RIB unmaps
/// it again, but the prefix length kept with the route is the IPv6 one).
fn map_prefix(prefix: IpNet) -> IpNet {
    match prefix {
        IpNet::V4(p) => IpNet::V6(Ipv6Net::new(p.addr().to_ipv6_mapped(), p.prefix_len() + 96).unwrap()),
        p => p,
    }
}

/// Largest accepted BMP message: two BGP messages of 65535 bytes at most.
const MAX_BMP_MESSAGE_SIZE: usize = 1 << 20;

#[cfg(test)]
mod tests;
