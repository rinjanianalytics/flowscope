// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Binary primitives of the native protocol: unsigned LEB128 varints,
//! length-prefixed strings and little-endian integers.

use std::io::{self, Read};

pub fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    put_uvarint(out, s.len() as u64);
    out.extend_from_slice(s);
}

pub fn read_uvarint(r: &mut dyn Read) -> io::Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let mut b = [0u8];
        r.read_exact(&mut b)?;
        v |= ((b[0] & 0x7f) as u64) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
        if shift > 63 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "varint overflow"));
        }
    }
}

/// Maximum size of a string we accept, to avoid allocating garbage.
const MAX_STRING: u64 = 1 << 30;

pub fn read_bytes(r: &mut dyn Read) -> io::Result<Vec<u8>> {
    let n = read_uvarint(r)?;
    if n > MAX_STRING {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "string too large"));
    }
    let mut buf = vec![0u8; n as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn read_string(r: &mut dyn Read) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&read_bytes(r)?).into_owned())
}

pub fn read_fixed<const N: usize>(r: &mut dyn Read) -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

pub fn read_u8(r: &mut dyn Read) -> io::Result<u8> {
    Ok(read_fixed::<1>(r)?[0])
}

pub fn read_i32(r: &mut dyn Read) -> io::Result<i32> {
    Ok(i32::from_le_bytes(read_fixed(r)?))
}

pub fn read_u64(r: &mut dyn Read) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_fixed(r)?))
}

pub fn read_vec(r: &mut dyn Read, n: usize) -> io::Result<Vec<u8>> {
    if n as u64 > MAX_STRING * 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "column too large"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}
