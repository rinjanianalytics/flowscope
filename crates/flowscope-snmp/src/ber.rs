// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Minimal BER encoding and decoding for SNMP.

use std::fmt;

pub const INTEGER: u8 = 0x02;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const SEQUENCE: u8 = 0x30;
pub const IP_ADDRESS: u8 = 0x40;
pub const COUNTER32: u8 = 0x41;
pub const GAUGE32: u8 = 0x42;
pub const TIMETICKS: u8 = 0x43;
pub const OPAQUE: u8 = 0x44;
pub const COUNTER64: u8 = 0x46;
pub const NO_SUCH_OBJECT: u8 = 0x80;
pub const NO_SUCH_INSTANCE: u8 = 0x81;
pub const END_OF_MIB_VIEW: u8 = 0x82;

pub const GET_REQUEST: u8 = 0xa0;
pub const GET_NEXT_REQUEST: u8 = 0xa1;
pub const RESPONSE: u8 = 0xa2;
pub const GET_BULK_REQUEST: u8 = 0xa5;
pub const REPORT: u8 = 0xa8;

/// Decoding error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err<T>(msg: &str) -> Result<T, Error> {
    Err(Error(msg.to_string()))
}

/// Append a length.
pub fn put_len(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
}

/// Append a TLV.
pub fn put_tlv(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    put_len(out, value.len());
    out.extend_from_slice(value);
}

/// Encode a signed integer (minimal two's complement).
pub fn int_bytes(v: i64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let mut start = 0;
    while start < 7
        && ((bytes[start] == 0 && bytes[start + 1] & 0x80 == 0)
            || (bytes[start] == 0xff && bytes[start + 1] & 0x80 != 0))
    {
        start += 1;
    }
    bytes[start..].to_vec()
}

/// Encode an unsigned integer (with a leading zero when needed).
pub fn uint_bytes(v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    let mut out = bytes[skip..].to_vec();
    if out[0] & 0x80 != 0 {
        out.insert(0, 0);
    }
    out
}

pub fn put_int(out: &mut Vec<u8>, v: i64) {
    put_tlv(out, INTEGER, &int_bytes(v));
}

pub fn put_octets(out: &mut Vec<u8>, v: &[u8]) {
    put_tlv(out, OCTET_STRING, v);
}

/// Parse a dotted OID.
pub fn parse_oid(s: &str) -> Result<Vec<u32>, Error> {
    s.trim_start_matches('.')
        .split('.')
        .map(|p| p.parse::<u32>().map_err(|_| Error(format!("invalid OID {s:?}"))))
        .collect()
}

/// Encode an OID.
pub fn oid_bytes(oid: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    if oid.len() < 2 {
        out.push(0);
        return out;
    }
    let mut push = |mut v: u32| {
        let mut tmp = vec![(v & 0x7f) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        tmp.reverse();
        out.extend(tmp);
    };
    push(oid[0] * 40 + oid[1]);
    for &v in &oid[2..] {
        push(v);
    }
    out
}

/// Format an OID.
pub fn format_oid(oid: &[u32]) -> String {
    oid.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(".")
}

/// A BER reader.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Read a TLV: tag and value.
    pub fn tlv(&mut self) -> Result<(u8, &'a [u8]), Error> {
        if self.data.len() < 2 {
            return err("truncated TLV");
        }
        let tag = self.data[0];
        let first = self.data[1];
        let (len, header) = if first & 0x80 == 0 {
            (first as usize, 2)
        } else {
            let n = (first & 0x7f) as usize;
            if n == 0 || n > 4 || self.data.len() < 2 + n {
                return err("invalid length");
            }
            let mut len = 0usize;
            for b in &self.data[2..2 + n] {
                len = (len << 8) | *b as usize;
            }
            (len, 2 + n)
        };
        if self.data.len() < header + len {
            return err("truncated value");
        }
        let value = &self.data[header..header + len];
        self.data = &self.data[header + len..];
        Ok((tag, value))
    }

    /// Read a TLV with the expected tag.
    pub fn expect(&mut self, tag: u8) -> Result<&'a [u8], Error> {
        let (t, v) = self.tlv()?;
        if t != tag {
            return Err(Error(format!("expected tag {tag:#x}, got {t:#x}")));
        }
        Ok(v)
    }

    pub fn sequence(&mut self) -> Result<Reader<'a>, Error> {
        Ok(Reader::new(self.expect(SEQUENCE)?))
    }

    pub fn int(&mut self) -> Result<i64, Error> {
        decode_int(self.expect(INTEGER)?)
    }

    pub fn octets(&mut self) -> Result<&'a [u8], Error> {
        self.expect(OCTET_STRING)
    }
}

pub fn decode_int(v: &[u8]) -> Result<i64, Error> {
    if v.is_empty() || v.len() > 8 {
        return err("invalid integer");
    }
    let mut r: i64 = if v[0] & 0x80 != 0 { -1 } else { 0 };
    for b in v {
        r = (r << 8) | *b as i64;
    }
    Ok(r)
}

pub fn decode_uint(v: &[u8]) -> Result<u64, Error> {
    if v.is_empty() || v.len() > 9 {
        return err("invalid unsigned integer");
    }
    Ok(v.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64))
}

pub fn decode_oid(v: &[u8]) -> Result<Vec<u32>, Error> {
    if v.is_empty() {
        return err("empty OID");
    }
    let mut out = Vec::new();
    let mut cur: u64 = 0;
    for (i, b) in v.iter().enumerate() {
        cur = (cur << 7) | (b & 0x7f) as u64;
        if cur > u32::MAX as u64 {
            return err("OID component too large");
        }
        if b & 0x80 == 0 {
            if out.is_empty() {
                let first = cur as u32;
                let (a, b) = if first < 40 {
                    (0, first)
                } else if first < 80 {
                    (1, first - 40)
                } else {
                    (2, first - 80)
                };
                out.push(a);
                out.push(b);
            } else {
                out.push(cur as u32);
            }
            cur = 0;
        } else if i == v.len() - 1 {
            return err("truncated OID");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints() {
        for v in [0i64, 1, 127, 128, 255, 256, -1, -128, -129, i32::MAX as i64, i64::MIN] {
            assert_eq!(decode_int(&int_bytes(v)).unwrap(), v, "{v}");
        }
        assert_eq!(int_bytes(128), vec![0, 128]);
        assert_eq!(uint_bytes(0xffffffff), vec![0, 0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn oids() {
        for s in ["1.3.6.1.2.1.1.5.0", "1.3.6.1.2.1.31.1.1.1.18.4294967295", "2.999.1"] {
            let oid = parse_oid(s).unwrap();
            assert_eq!(format_oid(&decode_oid(&oid_bytes(&oid)).unwrap()), s);
        }
        assert_eq!(oid_bytes(&parse_oid("1.3.6.1").unwrap()), vec![0x2b, 6, 1]);
    }

    #[test]
    fn lengths() {
        let mut out = Vec::new();
        put_tlv(&mut out, OCTET_STRING, &vec![7u8; 300]);
        assert_eq!(&out[..4], &[0x04, 0x82, 0x01, 0x2c]);
        let (tag, v) = Reader::new(&out).tlv().unwrap();
        assert_eq!((tag, v.len()), (OCTET_STRING, 300));
    }
}
