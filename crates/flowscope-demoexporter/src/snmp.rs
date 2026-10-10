// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A small SNMP agent describing the exporter and its interfaces. Port of
//! `demoexporter/snmp`.
//!
//! It answers SNMP v1 and v2c requests for the community "public".

use crate::config::SnmpConfiguration;
use flowscope_common::reporter::{CounterVec, Reporter};
use flowscope_snmp::ber;
use flowscope_snmp::message::{self, Message, Pdu, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const COMMUNITY: &[u8] = b"public";
const NO_SUCH_NAME: i64 = 2;
/// Speed of all interfaces, in Mbps.
const SPEED: u32 = 10_000;

/// The SNMP agent.
pub struct Snmp {
    socket: UdpSocket,
    /// The values, sorted by OID for "get next".
    oids: BTreeMap<Vec<u32>, Value>,
    requests: CounterVec,
}

impl Snmp {
    /// Bind the agent.
    pub async fn new(r: &Reporter, config: &SnmpConfiguration) -> Result<Snmp, String> {
        let text = |s: &str| Value::OctetString(s.as_bytes().to_vec());
        let mut oids = BTreeMap::new();
        oids.insert(vec![1, 3, 6, 1, 2, 1, 1, 5, 0], text(&config.name)); // sysName
        for (index, description) in &config.interfaces {
            let name = format!("Gi0/0/0/{index}");
            let with = |prefix: &[u32]| [prefix, &[*index]].concat();
            oids.insert(with(&[1, 3, 6, 1, 2, 1, 2, 2, 1, 2]), text(&name)); // ifDescr
            oids.insert(with(&[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 1]), text(&name)); // ifName
            oids.insert(with(&[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 15]), Value::Gauge32(SPEED)); // ifHighSpeed
            oids.insert(with(&[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 18]), text(description)); // ifAlias
        }
        let listen =
            if config.listen.starts_with(':') { format!("[::]{}", config.listen) } else { config.listen.clone() };
        let socket = UdpSocket::bind(&listen).await.map_err(|e| format!("unable to bind SNMP server: {e}"))?;
        let m = r.metrics("demoexporter/snmp");
        Ok(Snmp {
            socket,
            oids,
            requests: m.counter_vec("requests_total", "Number of SNMP requests handled.", &["oid"]),
        })
    }

    /// The address the agent listens to.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.socket.local_addr().ok()
    }

    fn found(&self, oid: &[u32], value: &Value) -> (Vec<u32>, Value) {
        self.requests.with_label_values(&[&ber::format_oid(oid)]).inc();
        (oid.to_vec(), value.clone())
    }

    /// The value after an OID, in the order of the tree.
    fn next(&self, oid: &[u32]) -> Option<(Vec<u32>, Value)> {
        use std::ops::Bound::{Excluded, Unbounded};
        self.oids.range::<[u32], _>((Excluded(oid), Unbounded)).next().map(|(o, v)| self.found(o, v))
    }

    /// The answer to a request. `None` when the request is not answered.
    fn answer(&self, data: &[u8]) -> Option<Vec<u8>> {
        let Ok(Message::Community { version, community, pdu }) = message::decode(data) else { return None };
        if community != COMMUNITY || version > 1 {
            return None;
        }
        let v1 = version == 0;
        let mut response =
            Pdu { kind: ber::RESPONSE, request_id: pdu.request_id, error_status: 0, error_index: 0, varbinds: vec![] };
        // SNMP v1 has no value to say something is missing: the whole
        // request fails and the variables are sent back.
        let fail = |response: &mut Pdu, index: usize| {
            response.error_status = NO_SUCH_NAME;
            response.error_index = index as i64 + 1;
            response.varbinds = pdu.varbinds.clone();
        };
        match pdu.kind {
            ber::GET_REQUEST => {
                for (index, (oid, _)) in pdu.varbinds.iter().enumerate() {
                    match self.oids.get(oid) {
                        Some(value) => response.varbinds.push(self.found(oid, value)),
                        None if v1 => {
                            fail(&mut response, index);
                            break;
                        }
                        None => response.varbinds.push((oid.clone(), Value::NoSuchObject)),
                    }
                }
            }
            ber::GET_NEXT_REQUEST => {
                for (index, (oid, _)) in pdu.varbinds.iter().enumerate() {
                    match self.next(oid) {
                        Some(found) => response.varbinds.push(found),
                        None if v1 => {
                            fail(&mut response, index);
                            break;
                        }
                        None => response.varbinds.push((oid.clone(), Value::EndOfMibView)),
                    }
                }
            }
            ber::GET_BULK_REQUEST if !v1 => {
                // For a bulk request, the error fields hold the number of
                // variables asked once and the repetitions for the others.
                let once = (pdu.error_status.max(0) as usize).min(pdu.varbinds.len());
                let repetitions = pdu.error_index.clamp(0, 50) as usize;
                for (oid, _) in &pdu.varbinds[..once] {
                    response.varbinds.push(self.next(oid).unwrap_or((oid.clone(), Value::EndOfMibView)));
                }
                let mut cursors: Vec<Vec<u32>> = pdu.varbinds[once..].iter().map(|(oid, _)| oid.clone()).collect();
                for _ in 0..repetitions {
                    for cursor in &mut cursors {
                        let found = self.next(cursor).unwrap_or((cursor.clone(), Value::EndOfMibView));
                        *cursor = found.0.clone();
                        response.varbinds.push(found);
                    }
                }
            }
            _ => return None,
        }
        Some(message::encode_community(version, &community, &response))
    }

    /// Answer the requests until the token is cancelled.
    pub async fn run(self, token: CancellationToken) -> Result<(), String> {
        tracing::info!(address = ?self.local_addr(), "starting SNMP component");
        let mut buf = vec![0u8; 65535];
        loop {
            let (size, from) = tokio::select! {
                _ = token.cancelled() => break,
                received = self.socket.recv_from(&mut buf) => match received {
                    Ok(received) => received,
                    Err(e) => {
                        tracing::debug!(error = %e, "cannot receive SNMP request");
                        continue;
                    }
                },
            };
            if let Some(answer) = self.answer(&buf[..size])
                && let Err(e) = self.socket.send_to(&answer, from).await
            {
                tracing::debug!(error = %e, "cannot send SNMP answer");
            }
        }
        tracing::info!("SNMP component stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn agent() -> Snmp {
        let config = SnmpConfiguration {
            name: "exporter1.example.com".into(),
            interfaces: [(1, "Transit: Cogent".to_string()), (2, "Core".to_string())].into(),
            listen: "127.0.0.1:0".into(),
        };
        Snmp::new(&Reporter::new(&Default::default()), &config).await.unwrap()
    }

    fn ask(agent: &Snmp, version: i64, community: &[u8], kind: u8, oids: &[&str], bulk: (i64, i64)) -> Option<Pdu> {
        let pdu = Pdu {
            kind,
            request_id: 42,
            error_status: bulk.0,
            error_index: bulk.1,
            varbinds: oids.iter().map(|o| (ber::parse_oid(o).unwrap(), Value::Null)).collect(),
        };
        let answer = agent.answer(&message::encode_community(version, community, &pdu))?;
        match message::decode(&answer).unwrap() {
            Message::Community { pdu, .. } => Some(pdu),
            _ => panic!("unexpected message"),
        }
    }

    fn text(s: &str) -> Value {
        Value::OctetString(s.as_bytes().to_vec())
    }

    #[tokio::test]
    async fn get() {
        let agent = agent().await;
        let pdu = ask(
            &agent,
            1,
            b"public",
            ber::GET_REQUEST,
            &[
                "1.3.6.1.2.1.1.5.0",
                "1.3.6.1.2.1.2.2.1.2.2",
                "1.3.6.1.2.1.31.1.1.1.1.1",
                "1.3.6.1.2.1.31.1.1.1.15.1",
                "1.3.6.1.2.1.31.1.1.1.18.1",
                "1.3.6.1.2.1.31.1.1.1.18.3",
            ],
            (0, 0),
        )
        .unwrap();
        assert_eq!((pdu.kind, pdu.request_id, pdu.error_status), (ber::RESPONSE, 42, 0));
        let values: Vec<Value> = pdu.varbinds.into_iter().map(|(_, v)| v).collect();
        assert_eq!(
            values,
            [
                text("exporter1.example.com"),
                text("Gi0/0/0/2"),
                text("Gi0/0/0/1"),
                Value::Gauge32(10000),
                text("Transit: Cogent"),
                Value::NoSuchObject,
            ]
        );
    }

    #[tokio::test]
    async fn refused() {
        let agent = agent().await;
        assert!(ask(&agent, 1, b"private", ber::GET_REQUEST, &["1.3.6.1.2.1.1.5.0"], (0, 0)).is_none());
        assert!(agent.answer(b"not snmp").is_none());
        // SNMP v1 fails the whole request for a missing object.
        let pdu =
            ask(&agent, 0, b"public", ber::GET_REQUEST, &["1.3.6.1.2.1.1.5.0", "1.3.6.1.2.1.1.6.0"], (0, 0)).unwrap();
        assert_eq!((pdu.error_status, pdu.error_index), (NO_SUCH_NAME, 2));
    }

    #[tokio::test]
    async fn walk() {
        let agent = agent().await;
        // Walk the whole tree with "get next".
        let mut oid = "1.3".to_string();
        let mut seen = vec![];
        loop {
            let pdu = ask(&agent, 1, b"public", ber::GET_NEXT_REQUEST, &[&oid], (0, 0)).unwrap();
            let (next, value) = pdu.varbinds[0].clone();
            if value == Value::EndOfMibView {
                break;
            }
            oid = ber::format_oid(&next);
            seen.push(oid.clone());
        }
        assert_eq!(seen.len(), 9);
        assert_eq!(seen[0], "1.3.6.1.2.1.1.5.0");
        assert_eq!(seen[8], "1.3.6.1.2.1.31.1.1.1.18.2");
        // The same with one bulk request.
        let pdu = ask(&agent, 1, b"public", ber::GET_BULK_REQUEST, &["1.3"], (0, 20)).unwrap();
        assert_eq!(pdu.varbinds.len(), 20);
        let bulk: Vec<String> = pdu.varbinds.iter().take(9).map(|(o, _)| ber::format_oid(o)).collect();
        assert_eq!(bulk, seen);
        assert_eq!(pdu.varbinds[9].1, Value::EndOfMibView);
    }

    #[tokio::test]
    async fn over_udp() {
        let agent = agent().await;
        let addr = agent.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(agent.run(token.clone()));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let pdu = Pdu {
            kind: ber::GET_REQUEST,
            request_id: 7,
            error_status: 0,
            error_index: 0,
            varbinds: vec![(ber::parse_oid("1.3.6.1.2.1.1.5.0").unwrap(), Value::Null)],
        };
        client.send_to(&message::encode_community(1, b"public", &pdu), addr).await.unwrap();
        let mut buf = [0u8; 1500];
        let (size, _) =
            tokio::time::timeout(std::time::Duration::from_secs(5), client.recv_from(&mut buf)).await.unwrap().unwrap();
        let Message::Community { pdu, .. } = message::decode(&buf[..size]).unwrap() else { panic!() };
        assert_eq!(pdu.varbinds[0].1, text("exporter1.example.com"));
        token.cancel();
        task.await.unwrap().unwrap();
    }
}
