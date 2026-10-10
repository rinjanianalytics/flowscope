// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! RawFlow protobuf message. Port of `common/pb`. The definition is in
//! `rawflow.proto`, next to the manifest of this crate; it is written by hand here to avoid a build
//! dependency on protoc. Field numbers must not change.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Version of the message. The Kafka topic is "<topic>-v<VERSION>".
pub const VERSION: u32 = 5;

/// A raw flow, as received by the inlet.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RawFlow {
    /// When the flow was received (seconds since epoch).
    #[prost(uint64, tag = "1")]
    pub time_received: u64,
    /// Payload of the flow.
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
    /// Source IPv6 address.
    #[prost(bytes = "vec", tag = "3")]
    pub source_address: Vec<u8>,
    /// Use the source address as the exporter address.
    #[prost(bool, tag = "4")]
    pub use_source_address: bool,
    /// Use this specific decoder.
    #[prost(enumeration = "Decoder", tag = "5")]
    pub decoder: i32,
    /// Source of the timestamp.
    #[prost(enumeration = "TimestampSource", tag = "6")]
    pub timestamp_source: i32,
    /// Protocol to decapsulate.
    #[prost(enumeration = "DecapsulationProtocol", tag = "7")]
    pub decapsulation_protocol: i32,
    /// Per-exporter rate limit (flows/s), 0 means no limit.
    #[prost(uint64, tag = "8")]
    pub rate_limit: u64,
}

macro_rules! text_enum {
    ($t:ident { $($variant:ident = $value:literal => $name:literal),+ $(,)? } $(aliases { $($alias:literal => $avariant:ident),* })?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
        #[repr(i32)]
        pub enum $t {
            $($variant = $value,)+
        }

        impl $t {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($t::$variant => $name,)+
                }
            }
        }

        impl std::str::FromStr for $t {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s {
                    $($name => Ok($t::$variant),)+
                    $($($alias => Ok($t::$avariant),)*)?
                    _ => Err(format!("unknown {} {s:?}", stringify!($t))),
                }
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

text_enum!(Decoder {
    Unspecified = 0 => "",
    Netflow = 1 => "netflow",
    Sflow = 2 => "sflow",
    Gob = 3 => "gob",
});

text_enum!(TimestampSource {
    Input = 0 => "input",
    NetflowPacket = 1 => "netflow-packet",
    NetflowFirstSwitched = 2 => "netflow-first-switched",
} aliases { "" => Input, "udp" => Input });

text_enum!(DecapsulationProtocol {
    None = 0 => "none",
    Ipip = 1 => "ipip",
    Gre = 2 => "gre",
    Vxlan = 3 => "vxlan",
    Srv6 = 4 => "srv6",
});

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn encode() {
        let flow = RawFlow {
            time_received: 1_700_000_000,
            payload: vec![1, 2, 3],
            source_address: vec![0; 16],
            use_source_address: true,
            decoder: Decoder::Sflow as i32,
            timestamp_source: TimestampSource::NetflowPacket as i32,
            decapsulation_protocol: DecapsulationProtocol::Vxlan as i32,
            rate_limit: 1000,
        };
        let buf = flow.encode_to_vec();
        // Field 1 varint, field 2 bytes...
        assert_eq!(&buf[..6], &[0x08, 0x80, 0xe2, 0xcf, 0xaa, 0x06]);
        assert_eq!(RawFlow::decode(buf.as_slice()).unwrap(), flow);
    }

    #[test]
    fn text() {
        assert_eq!("udp".parse::<TimestampSource>(), Ok(TimestampSource::Input));
        assert_eq!("netflow".parse::<Decoder>(), Ok(Decoder::Netflow));
        assert!("nope".parse::<DecapsulationProtocol>().is_err());
        assert_eq!(DecapsulationProtocol::Srv6.to_string(), "srv6");
    }
}
