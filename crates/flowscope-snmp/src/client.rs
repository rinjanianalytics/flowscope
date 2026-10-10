// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! An SNMP client for GET requests (v2c and v3).

use crate::ber;
use crate::message::{
    self, FLAG_AUTH, FLAG_PRIV, FLAG_REPORTABLE, Message, Pdu, ScopedPdu, Security, UsmParams, Value,
};
use crate::usm::{AuthProtocol, PrivProtocol};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// Credentials for a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credentials {
    Community(String),
    User {
        user_name: String,
        auth: AuthProtocol,
        auth_passphrase: String,
        privacy: PrivProtocol,
        priv_passphrase: String,
        context_name: String,
    },
}

/// State learned from an agent (SNMPv3), to avoid a discovery each time.
#[derive(Clone, Debug)]
pub struct EngineState {
    pub engine_id: Vec<u8>,
    pub engine_boots: u32,
    engine_time: u32,
    learned_at: Instant,
    pub auth_key: Vec<u8>,
    pub priv_key: Vec<u8>,
}

impl EngineState {
    fn current_time(&self) -> u32 {
        self.engine_time.saturating_add(self.learned_at.elapsed().as_secs() as u32)
    }
}

/// Result of a GET.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub error_status: i64,
    pub error_index: i64,
    pub varbinds: Vec<(Vec<u32>, Value)>,
}

/// Errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("request timeout")]
    Timeout,
    #[error("I/O error: {0}")]
    Io(String),
    #[error("invalid response: {0}")]
    Invalid(String),
    #[error("authentication failure")]
    Authentication,
    #[error("SNMP report {0}")]
    Report(String),
}

static REQUEST_ID: AtomicI32 = AtomicI32::new(0);
static SALT: AtomicU64 = AtomicU64::new(0);

fn next_id() -> i32 {
    let mut id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        id = rand_seed() as i32 & 0x7fffffff;
        REQUEST_ID.store(id + 1, Ordering::Relaxed);
    }
    id & 0x7fffffff
}

fn next_salt() -> u64 {
    let s = SALT.fetch_add(1, Ordering::Relaxed);
    if s == 0 {
        let seed = rand_seed();
        SALT.store(seed + 1, Ordering::Relaxed);
        return seed;
    }
    s
}

fn rand_seed() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    t.as_nanos() as u64 ^ (std::process::id() as u64) << 32
}

// usmStats OIDs in reports.
const USM_STATS_NOT_IN_TIME_WINDOWS: &str = "1.3.6.1.6.3.15.1.1.2.0";
const USM_STATS_UNKNOWN_ENGINE_IDS: &str = "1.3.6.1.6.3.15.1.1.4.0";

/// Options of a request.
#[derive(Clone, Debug)]
pub struct Options {
    pub timeout: Duration,
    pub retries: u32,
}

/// Callback deciding if a datagram answers the request.
type Accept<'a> = &'a mut (dyn FnMut(&[u8]) -> Option<Result<Vec<u8>, Error>> + Send);

/// Send a datagram and wait for an answer matching `accept`, with retries.
async fn exchange(
    target: SocketAddr,
    request: &[u8],
    options: &Options,
    on_retry: &mut (dyn FnMut() + Send),
    accept: Accept<'_>,
) -> Result<Vec<u8>, Error> {
    let bind: SocketAddr = if target.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
    let socket = UdpSocket::bind(bind).await.map_err(|e| Error::Io(e.to_string()))?;
    let mut buf = vec![0u8; 65536];
    for attempt in 0..=options.retries {
        if attempt > 0 {
            on_retry();
        }
        socket.send_to(request, target).await.map_err(|e| Error::Io(e.to_string()))?;
        let deadline = tokio::time::Instant::now() + options.timeout;
        loop {
            match tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await {
                Err(_) => break,
                Ok(Err(e)) => return Err(Error::Io(e.to_string())),
                Ok(Ok((n, from))) => {
                    if from.ip() != target.ip() {
                        continue;
                    }
                    if let Some(r) = accept(&buf[..n]) {
                        return r;
                    }
                }
            }
        }
    }
    Err(Error::Timeout)
}

fn get_pdu(oids: &[Vec<u32>]) -> Pdu {
    Pdu {
        kind: ber::GET_REQUEST,
        request_id: next_id(),
        error_status: 0,
        error_index: 0,
        varbinds: oids.iter().map(|o| (o.clone(), Value::Null)).collect(),
    }
}

/// GET some OIDs with a community.
pub async fn get_v2c(
    target: SocketAddr,
    community: &str,
    oids: &[Vec<u32>],
    options: &Options,
    on_retry: &mut (dyn FnMut() + Send),
) -> Result<Response, Error> {
    let pdu = get_pdu(oids);
    let request = message::encode_community(1, community.as_bytes(), &pdu);
    let raw = exchange(target, &request, options, on_retry, &mut |data| match message::decode(data) {
        Ok(Message::Community { pdu: p, .. }) if p.request_id == pdu.request_id => Some(Ok(data.to_vec())),
        _ => None,
    })
    .await?;
    let Ok(Message::Community { pdu: p, .. }) = message::decode(&raw) else {
        return Err(Error::Invalid("not a community message".into()));
    };
    Ok(Response { error_status: p.error_status, error_index: p.error_index, varbinds: p.varbinds })
}

/// Discover the engine of an agent.
async fn discover(
    target: SocketAddr,
    options: &Options,
    on_retry: &mut (dyn FnMut() + Send),
) -> Result<UsmParams, Error> {
    let msg_id = next_id() as i64;
    let scoped = ScopedPdu { context_engine_id: vec![], context_name: vec![], pdu: get_pdu(&[]) };
    let sec = Security { auth: AuthProtocol::None, auth_key: &[], privacy: PrivProtocol::None, priv_key: &[], salt: 0 };
    let request = message::encode_v3(msg_id, FLAG_REPORTABLE, &UsmParams::default(), &scoped, &sec);
    let raw = exchange(target, &request, options, on_retry, &mut |data| match message::decode(data) {
        Ok(Message::V3(m)) if m.msg_id == msg_id => Some(Ok(data.to_vec())),
        _ => None,
    })
    .await?;
    let Ok(Message::V3(m)) = message::decode(&raw) else { return Err(Error::Invalid("not a v3 message".into())) };
    if m.usm.engine_id.is_empty() {
        return Err(Error::Invalid("no engine ID in discovery answer".into()));
    }
    Ok(m.usm)
}

/// GET some OIDs with SNMPv3. `state` is the cached engine state, updated
/// on success.
#[allow(clippy::too_many_arguments)]
pub async fn get_v3(
    target: SocketAddr,
    creds: &Credentials,
    oids: &[Vec<u32>],
    options: &Options,
    state: &mut Option<EngineState>,
    on_retry: &mut (dyn FnMut() + Send),
) -> Result<Response, Error> {
    let Credentials::User { user_name, auth, auth_passphrase, privacy, priv_passphrase, context_name } = creds else {
        return Err(Error::Invalid("v3 needs a user".into()));
    };
    for attempt in 0..3 {
        if state.is_none() {
            let usm = discover(target, options, on_retry).await?;
            let auth_key = auth.localized_key(auth_passphrase.as_bytes(), &usm.engine_id);
            let priv_key = privacy.localized_key(*auth, priv_passphrase.as_bytes(), &usm.engine_id);
            *state = Some(EngineState {
                engine_id: usm.engine_id,
                engine_boots: usm.engine_boots,
                engine_time: usm.engine_time,
                learned_at: Instant::now(),
                auth_key,
                priv_key,
            });
        }
        let st = state.as_ref().unwrap().clone();
        let mut flags = FLAG_REPORTABLE;
        if *auth != AuthProtocol::None {
            flags |= FLAG_AUTH;
            if *privacy != PrivProtocol::None {
                flags |= FLAG_PRIV;
            }
        }
        let msg_id = next_id() as i64;
        let usm = UsmParams {
            engine_id: st.engine_id.clone(),
            engine_boots: st.engine_boots,
            engine_time: st.current_time(),
            user_name: user_name.as_bytes().to_vec(),
            ..Default::default()
        };
        let pdu = get_pdu(oids);
        let scoped =
            ScopedPdu { context_engine_id: st.engine_id.clone(), context_name: context_name.as_bytes().to_vec(), pdu };
        let sec = Security {
            auth: *auth,
            auth_key: &st.auth_key,
            privacy: *privacy,
            priv_key: &st.priv_key,
            salt: next_salt(),
        };
        let request = message::encode_v3(msg_id, flags, &usm, &scoped, &sec);
        let raw = exchange(target, &request, options, on_retry, &mut |data| match message::decode(data) {
            Ok(Message::V3(m)) if m.msg_id == msg_id => Some(Ok(data.to_vec())),
            _ => None,
        })
        .await?;
        let Ok(Message::V3(m)) = message::decode(&raw) else { return Err(Error::Invalid("not a v3 message".into())) };
        if m.flags & FLAG_AUTH != 0 && !message::verify_v3(&raw, &m, *auth, &st.auth_key) {
            return Err(Error::Authentication);
        }
        let scoped = match (&m.scoped, &m.encrypted) {
            (Some(s), _) => s.clone(),
            (None, Some(enc)) => {
                let plain = privacy
                    .decrypt(&st.priv_key, m.usm.engine_boots, m.usm.engine_time, &m.usm.priv_params, enc)
                    .map_err(Error::Invalid)?;
                ScopedPdu::decode(&plain).map_err(|e| Error::Invalid(e.to_string()))?
            }
            _ => return Err(Error::Invalid("no PDU".into())),
        };
        if scoped.pdu.kind == ber::REPORT {
            let oid = scoped.pdu.varbinds.first().map(|(o, _)| ber::format_oid(o)).unwrap_or_default();
            if (oid == USM_STATS_NOT_IN_TIME_WINDOWS || oid == USM_STATS_UNKNOWN_ENGINE_IDS) && attempt < 2 {
                // Resynchronize and try again.
                if let Some(s) = state.as_mut() {
                    s.engine_boots = m.usm.engine_boots;
                    s.engine_time = m.usm.engine_time;
                    s.learned_at = Instant::now();
                    if oid == USM_STATS_UNKNOWN_ENGINE_IDS {
                        *state = None;
                    }
                }
                continue;
            }
            return Err(Error::Report(oid));
        }
        // Keep the clock of the agent in sync.
        if let Some(s) = state.as_mut()
            && m.usm.engine_boots >= s.engine_boots
        {
            s.engine_boots = m.usm.engine_boots;
            s.engine_time = m.usm.engine_time;
            s.learned_at = Instant::now();
        }
        return Ok(Response {
            error_status: scoped.pdu.error_status,
            error_index: scoped.pdu.error_index,
            varbinds: scoped.pdu.varbinds,
        });
    }
    Err(Error::Invalid("cannot synchronize with the agent".into()))
}
