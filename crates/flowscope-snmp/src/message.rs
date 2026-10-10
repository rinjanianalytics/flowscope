// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SNMP messages (v1/v2c and v3) and PDUs.

use crate::ber::{self, Error, Reader};
use crate::usm::{AuthProtocol, PrivProtocol};

/// A value in a variable binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Integer(i64),
    OctetString(Vec<u8>),
    Null,
    Oid(Vec<u32>),
    IpAddress([u8; 4]),
    Counter32(u32),
    Gauge32(u32),
    TimeTicks(u32),
    Opaque(Vec<u8>),
    Counter64(u64),
    NoSuchObject,
    NoSuchInstance,
    EndOfMibView,
}

impl Value {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Value::Integer(v) => ber::put_int(out, *v),
            Value::OctetString(v) => ber::put_octets(out, v),
            Value::Null => ber::put_tlv(out, ber::NULL, &[]),
            Value::Oid(v) => ber::put_tlv(out, ber::OID, &ber::oid_bytes(v)),
            Value::IpAddress(v) => ber::put_tlv(out, ber::IP_ADDRESS, v),
            Value::Counter32(v) => ber::put_tlv(out, ber::COUNTER32, &ber::uint_bytes(*v as u64)),
            Value::Gauge32(v) => ber::put_tlv(out, ber::GAUGE32, &ber::uint_bytes(*v as u64)),
            Value::TimeTicks(v) => ber::put_tlv(out, ber::TIMETICKS, &ber::uint_bytes(*v as u64)),
            Value::Opaque(v) => ber::put_tlv(out, ber::OPAQUE, v),
            Value::Counter64(v) => ber::put_tlv(out, ber::COUNTER64, &ber::uint_bytes(*v)),
            Value::NoSuchObject => ber::put_tlv(out, ber::NO_SUCH_OBJECT, &[]),
            Value::NoSuchInstance => ber::put_tlv(out, ber::NO_SUCH_INSTANCE, &[]),
            Value::EndOfMibView => ber::put_tlv(out, ber::END_OF_MIB_VIEW, &[]),
        }
    }

    fn decode(tag: u8, v: &[u8]) -> Result<Value, Error> {
        Ok(match tag {
            ber::INTEGER => Value::Integer(ber::decode_int(v)?),
            ber::OCTET_STRING => Value::OctetString(v.to_vec()),
            ber::NULL => Value::Null,
            ber::OID => Value::Oid(ber::decode_oid(v)?),
            ber::IP_ADDRESS => Value::IpAddress(v.try_into().map_err(|_| Error("invalid IP address".into()))?),
            ber::COUNTER32 => Value::Counter32(ber::decode_uint(v)? as u32),
            ber::GAUGE32 => Value::Gauge32(ber::decode_uint(v)? as u32),
            ber::TIMETICKS => Value::TimeTicks(ber::decode_uint(v)? as u32),
            ber::OPAQUE => Value::Opaque(v.to_vec()),
            ber::COUNTER64 => Value::Counter64(ber::decode_uint(v)?),
            ber::NO_SUCH_OBJECT => Value::NoSuchObject,
            ber::NO_SUCH_INSTANCE => Value::NoSuchInstance,
            ber::END_OF_MIB_VIEW => Value::EndOfMibView,
            t => return Err(Error(format!("unknown value type {t:#x}"))),
        })
    }
}

/// A PDU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pdu {
    pub kind: u8,
    pub request_id: i32,
    pub error_status: i64,
    pub error_index: i64,
    pub varbinds: Vec<(Vec<u32>, Value)>,
}

impl Pdu {
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut body = Vec::new();
        ber::put_int(&mut body, self.request_id as i64);
        ber::put_int(&mut body, self.error_status);
        ber::put_int(&mut body, self.error_index);
        let mut vbs = Vec::new();
        for (oid, value) in &self.varbinds {
            let mut vb = Vec::new();
            ber::put_tlv(&mut vb, ber::OID, &ber::oid_bytes(oid));
            value.encode(&mut vb);
            ber::put_tlv(&mut vbs, ber::SEQUENCE, &vb);
        }
        ber::put_tlv(&mut body, ber::SEQUENCE, &vbs);
        ber::put_tlv(out, self.kind, &body);
    }

    pub fn decode(r: &mut Reader) -> Result<Pdu, Error> {
        let (kind, body) = r.tlv()?;
        let mut b = Reader::new(body);
        let request_id = b.int()? as i32;
        let error_status = b.int()?;
        let error_index = b.int()?;
        let mut vbs = b.sequence()?;
        let mut varbinds = Vec::new();
        while !vbs.is_empty() {
            let mut vb = vbs.sequence()?;
            let oid = ber::decode_oid(vb.expect(ber::OID)?)?;
            let (tag, v) = vb.tlv()?;
            varbinds.push((oid, Value::decode(tag, v)?));
        }
        Ok(Pdu { kind, request_id, error_status, error_index, varbinds })
    }
}

/// Encode a v1/v2c message.
pub fn encode_community(version: i64, community: &[u8], pdu: &Pdu) -> Vec<u8> {
    let mut body = Vec::new();
    ber::put_int(&mut body, version);
    ber::put_octets(&mut body, community);
    pdu.encode(&mut body);
    let mut out = Vec::new();
    ber::put_tlv(&mut out, ber::SEQUENCE, &body);
    out
}

/// A decoded message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Community { version: i64, community: Vec<u8>, pdu: Pdu },
    V3(V3Message),
}

/// USM security parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsmParams {
    pub engine_id: Vec<u8>,
    pub engine_boots: u32,
    pub engine_time: u32,
    pub user_name: Vec<u8>,
    pub auth_params: Vec<u8>,
    pub priv_params: Vec<u8>,
}

/// A v3 message, as decoded (the scoped PDU may still be encrypted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V3Message {
    pub msg_id: i64,
    pub max_size: i64,
    pub flags: u8,
    pub usm: UsmParams,
    /// Decoded scoped PDU, when not encrypted.
    pub scoped: Option<ScopedPdu>,
    /// Encrypted scoped PDU.
    pub encrypted: Option<Vec<u8>>,
}

/// A scoped PDU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedPdu {
    pub context_engine_id: Vec<u8>,
    pub context_name: Vec<u8>,
    pub pdu: Pdu,
}

impl ScopedPdu {
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        ber::put_octets(&mut body, &self.context_engine_id);
        ber::put_octets(&mut body, &self.context_name);
        self.pdu.encode(&mut body);
        let mut out = Vec::new();
        ber::put_tlv(&mut out, ber::SEQUENCE, &body);
        out
    }

    pub fn decode(data: &[u8]) -> Result<ScopedPdu, Error> {
        let mut r = Reader::new(data);
        let mut s = r.sequence()?;
        Ok(ScopedPdu {
            context_engine_id: s.octets()?.to_vec(),
            context_name: s.octets()?.to_vec(),
            pdu: Pdu::decode(&mut s)?,
        })
    }
}

pub const FLAG_AUTH: u8 = 1;
pub const FLAG_PRIV: u8 = 2;
pub const FLAG_REPORTABLE: u8 = 4;

/// Security settings for a v3 message.
#[derive(Clone, Debug)]
pub struct Security<'a> {
    pub auth: AuthProtocol,
    pub auth_key: &'a [u8],
    pub privacy: PrivProtocol,
    pub priv_key: &'a [u8],
    /// Salt for privacy.
    pub salt: u64,
}

/// Encode a v3 message, authenticating and encrypting as requested.
pub fn encode_v3(msg_id: i64, flags: u8, usm: &UsmParams, scoped: &ScopedPdu, sec: &Security) -> Vec<u8> {
    let mut usm = usm.clone();
    let plain = scoped.encode();
    let data = if flags & FLAG_PRIV != 0 {
        let (enc, params) = sec.privacy.encrypt(sec.priv_key, usm.engine_boots, usm.engine_time, sec.salt, &plain);
        usm.priv_params = params;
        let mut out = Vec::new();
        ber::put_octets(&mut out, &enc);
        out
    } else {
        plain
    };
    let auth = flags & FLAG_AUTH != 0;
    usm.auth_params = if auth { vec![0; sec.auth.mac_len()] } else { vec![] };
    // Encode the USM parameters, remembering where the MAC goes.
    let mut sp = Vec::new();
    ber::put_octets(&mut sp, &usm.engine_id);
    ber::put_int(&mut sp, usm.engine_boots as i64);
    ber::put_int(&mut sp, usm.engine_time as i64);
    ber::put_octets(&mut sp, &usm.user_name);
    let mut auth_tlv = Vec::new();
    ber::put_octets(&mut auth_tlv, &usm.auth_params);
    let auth_in_sp = sp.len() + (auth_tlv.len() - usm.auth_params.len());
    sp.extend_from_slice(&auth_tlv);
    ber::put_octets(&mut sp, &usm.priv_params);
    let mut sp_seq = Vec::new();
    ber::put_tlv(&mut sp_seq, ber::SEQUENCE, &sp);
    let auth_in_sp_seq = auth_in_sp + (sp_seq.len() - sp.len());
    let mut global = Vec::new();
    ber::put_int(&mut global, msg_id);
    ber::put_int(&mut global, 65507);
    ber::put_octets(&mut global, &[flags]);
    ber::put_int(&mut global, 3); // USM
    let mut body = Vec::new();
    ber::put_int(&mut body, 3);
    ber::put_tlv(&mut body, ber::SEQUENCE, &global);
    let before_sp = body.len();
    ber::put_octets(&mut body, &sp_seq);
    let auth_in_body = before_sp + (body.len() - before_sp - sp_seq.len()) + auth_in_sp_seq;
    body.extend_from_slice(&data);
    let mut out = Vec::new();
    ber::put_tlv(&mut out, ber::SEQUENCE, &body);
    let auth_offset = auth_in_body + (out.len() - body.len());
    if auth {
        // The MAC is computed with zeroed parameters, then put in place.
        let mac = sec.auth.mac(sec.auth_key, &out);
        out[auth_offset..auth_offset + mac.len()].copy_from_slice(&mac);
    }
    out
}

/// Decode a message.
pub fn decode(data: &[u8]) -> Result<Message, Error> {
    let mut r = Reader::new(data);
    let mut m = r.sequence()?;
    let version = m.int()?;
    if version != 3 {
        let community = m.octets()?.to_vec();
        let pdu = Pdu::decode(&mut m)?;
        return Ok(Message::Community { version, community, pdu });
    }
    let mut global = m.sequence()?;
    let msg_id = global.int()?;
    let max_size = global.int()?;
    let flags = global.octets()?.first().copied().unwrap_or(0);
    let _model = global.int()?;
    let sp = m.octets()?;
    let mut s = Reader::new(sp).sequence()?;
    let usm = UsmParams {
        engine_id: s.octets()?.to_vec(),
        engine_boots: s.int()? as u32,
        engine_time: s.int()? as u32,
        user_name: s.octets()?.to_vec(),
        auth_params: s.octets()?.to_vec(),
        priv_params: s.octets()?.to_vec(),
    };
    let (scoped, encrypted) = if flags & FLAG_PRIV != 0 {
        (None, Some(m.octets()?.to_vec()))
    } else {
        let (tag, body) = m.tlv()?;
        if tag != ber::SEQUENCE {
            return Err(Error("invalid scoped PDU".into()));
        }
        let mut raw = Vec::new();
        ber::put_tlv(&mut raw, ber::SEQUENCE, body);
        (Some(ScopedPdu::decode(&raw)?), None)
    };
    Ok(Message::V3(V3Message { msg_id, max_size, flags, usm, scoped, encrypted }))
}

/// Verify the authentication of a received v3 message.
pub fn verify_v3(data: &[u8], msg: &V3Message, auth: AuthProtocol, key: &[u8]) -> bool {
    if msg.usm.auth_params.len() != auth.mac_len() {
        return false;
    }
    let mut copy = data.to_vec();
    let mut needle = vec![ber::OCTET_STRING, msg.usm.auth_params.len() as u8];
    needle.extend_from_slice(&msg.usm.auth_params);
    let Some(pos) = copy.windows(needle.len()).position(|w| w == needle.as_slice()) else {
        return false;
    };
    let start = pos + 2;
    copy[start..start + msg.usm.auth_params.len()].fill(0);
    auth.mac(key, &copy) == msg.usm.auth_params
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdu() -> Pdu {
        Pdu {
            kind: ber::GET_REQUEST,
            request_id: 12345,
            error_status: 0,
            error_index: 0,
            varbinds: vec![(ber::parse_oid("1.3.6.1.2.1.1.5.0").unwrap(), Value::Null)],
        }
    }

    #[test]
    fn community() {
        let enc = encode_community(1, b"public", &pdu());
        assert_eq!(decode(&enc).unwrap(), Message::Community { version: 1, community: b"public".to_vec(), pdu: pdu() });
    }

    #[test]
    fn v3_roundtrip() {
        let engine = b"\x80\x00\x1f\x88\x04test".to_vec();
        let auth_key = AuthProtocol::Sha256.localized_key(b"authpass", &engine);
        let priv_key = PrivProtocol::Aes.localized_key(AuthProtocol::Sha256, b"privpass", &engine);
        let usm = UsmParams {
            engine_id: engine.clone(),
            engine_boots: 2,
            engine_time: 300,
            user_name: b"user".to_vec(),
            ..Default::default()
        };
        let scoped = ScopedPdu { context_engine_id: engine.clone(), context_name: vec![], pdu: pdu() };
        let sec = Security {
            auth: AuthProtocol::Sha256,
            auth_key: &auth_key,
            privacy: PrivProtocol::Aes,
            priv_key: &priv_key,
            salt: 7,
        };
        let enc = encode_v3(1, FLAG_AUTH | FLAG_PRIV | FLAG_REPORTABLE, &usm, &scoped, &sec);
        let Message::V3(m) = decode(&enc).unwrap() else { panic!() };
        assert!(verify_v3(&enc, &m, AuthProtocol::Sha256, &auth_key));
        let plain = PrivProtocol::Aes
            .decrypt(
                &priv_key,
                m.usm.engine_boots,
                m.usm.engine_time,
                &m.usm.priv_params,
                m.encrypted.as_ref().unwrap(),
            )
            .unwrap();
        assert_eq!(ScopedPdu::decode(&plain).unwrap(), scoped);
        // Tampering breaks authentication.
        let mut bad = enc.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        let Message::V3(m) = decode(&bad).unwrap() else { panic!() };
        assert!(!verify_v3(&bad, &m, AuthProtocol::Sha256, &auth_key));
    }
}
