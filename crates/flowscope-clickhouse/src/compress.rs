// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Compressed frames of the native protocol: a 16-byte CityHash128 (v1.0.2)
//! checksum, a method byte, the compressed size (header included), the
//! uncompressed size and the data.

use std::io::{self, Read};

const METHOD_NONE: u8 = 0x02;
const METHOD_LZ4: u8 = 0x82;
const HEADER: usize = 9;
/// Frames are cut at this uncompressed size, like ClickHouse.
const MAX_FRAME: usize = 1 << 20;

fn checksum(data: &[u8]) -> [u8; 16] {
    let h = cityhash_rs::cityhash_102_128(data);
    // cityhash-rs puts the first word of CityHash's pair in the high bits.
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&((h >> 64) as u64).to_le_bytes());
    out[8..].copy_from_slice(&(h as u64).to_le_bytes());
    out
}

/// Compress data into one or several LZ4 frames.
pub fn compress(data: &[u8], out: &mut Vec<u8>) {
    for chunk in data.chunks(MAX_FRAME) {
        let compressed = lz4_flex::block::compress(chunk);
        let mut frame = Vec::with_capacity(HEADER + compressed.len());
        frame.push(METHOD_LZ4);
        frame.extend_from_slice(&((HEADER + compressed.len()) as u32).to_le_bytes());
        frame.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        frame.extend_from_slice(&compressed);
        out.extend_from_slice(&checksum(&frame));
        out.extend_from_slice(&frame);
    }
}

/// A reader decompressing frames as needed.
pub struct Decompressor<'a> {
    inner: &'a mut dyn Read,
    buf: Vec<u8>,
    pos: usize,
}

impl<'a> Decompressor<'a> {
    pub fn new(inner: &'a mut dyn Read) -> Self {
        Decompressor { inner, buf: Vec::new(), pos: 0 }
    }

    fn next_frame(&mut self) -> io::Result<()> {
        let mut sum = [0u8; 16];
        self.inner.read_exact(&mut sum)?;
        let mut header = [0u8; HEADER];
        self.inner.read_exact(&mut header)?;
        let compressed_size = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
        if !(HEADER..=1 << 30).contains(&compressed_size) || size > 1 << 30 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid compressed frame"));
        }
        let mut frame = vec![0u8; compressed_size];
        frame[..HEADER].copy_from_slice(&header);
        self.inner.read_exact(&mut frame[HEADER..])?;
        if checksum(&frame) != sum {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "checksum mismatch in compressed frame"));
        }
        self.buf = match header[0] {
            METHOD_LZ4 => lz4_flex::block::decompress(&frame[HEADER..], size)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
            METHOD_NONE => frame[HEADER..].to_vec(),
            m => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsupported compression {m:#x}"))),
        };
        self.pos = 0;
        Ok(())
    }
}

impl Read for Decompressor<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.buf.len() {
            self.next_frame()?;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        compress(&data, &mut out);
        let mut reader: &[u8] = &out;
        let mut d = Decompressor::new(&mut reader);
        let mut back = vec![0u8; data.len()];
        d.read_exact(&mut back).unwrap();
        assert_eq!(back, data);
    }
}
