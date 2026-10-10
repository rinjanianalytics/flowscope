// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Optional protobuf encoding of enriched flows, used by the outlet Kafka
//! output. Port of `common/schema/protobuf.go`.

use crate::clickhouse::{Fnv128, base32_nopad};
use crate::definition::Schema;

/// Append a varint.
pub(crate) fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// Append a field tag. Wire type 0 is varint, 2 is length-delimited.
pub(crate) fn put_tag(buf: &mut Vec<u8>, field: u32, wire_type: u8) {
    put_varint(buf, ((field as u64) << 3) | wire_type as u64);
}

/// Append a length-delimited field.
pub(crate) fn put_bytes(buf: &mut Vec<u8>, field: u32, data: &[u8]) {
    put_tag(buf, field, 2);
    put_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

impl Schema {
    /// The .proto definition matching the protobuf encoding of the schema.
    ///
    /// IP columns are 16-byte values (IPv4 mapped into IPv6), Enum8 columns
    /// are encoded as their numeric value, Array(UInt128) elements are 16-byte
    /// values (high then low, big-endian).
    pub fn protobuf_definition(&self) -> String {
        self.protobuf_hash_and_definition().1
    }

    /// Hash identifying the protobuf message layout.
    pub fn protobuf_message_hash(&self) -> String {
        self.protobuf_hash_and_definition().0
    }

    fn protobuf_hash_and_definition(&self) -> (String, String) {
        let mut lines = Vec::new();
        let mut hash = Fnv128::new();
        for column in self.columns() {
            if column.protobuf_index <= 0 {
                continue;
            }
            let mut t = column.protobuf_type.type_name().to_string();
            if t.is_empty() {
                continue;
            }
            if column.protobuf_repeated {
                t = format!("repeated {t}");
            }
            let line = format!("{t} {} = {};", column.name, column.protobuf_index);
            hash.write(line.as_bytes());
            lines.push(line);
        }
        let h = base32_nopad(&hash.sum());
        let definition = format!("\nsyntax = \"proto3\";\n\nmessage FlowMessagev{h} {{\n {}\n}}\n", lines.join("\n "));
        (h, definition)
    }
}
