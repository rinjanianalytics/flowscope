// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Route distinguishers and route targets. Port of
//! `outlet/routing/provider/bmp/rd.go` and `rt.go`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::Ipv4Addr;

/// A route distinguisher, as its 64-bit encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rd(pub u64);

/// A route target, encoded like a route distinguisher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rt(pub u64);

impl std::str::FromStr for Rd {
    type Err = String;

    fn from_str(text: &str) -> Result<Rd, String> {
        let elems: Vec<&str> = text.split(':').collect();
        let (typ, elems): (i32, &[&str]) = match elems.len() {
            1 => {
                return text.parse::<u64>().map(Rd).map_err(|_| "cannot parse RD as a 64-bit number".into());
            }
            3 => {
                let t: i32 = elems[0].parse().map_err(|_| "cannot parse RD type".to_string())?;
                if !(0..=2).contains(&t) {
                    return Err("cannot parse RD type".into());
                }
                (t, &elems[1..])
            }
            2 => (-1, &elems[..]),
            _ => return Err("cannot parse RD".into()),
        };
        if typ == 1 || (typ == -1 && elems[0].contains('.')) {
            let ip: Ipv4Addr = elems[0].parse().map_err(|_| "cannot parse RD as IPv4 address + index".to_string())?;
            let index: u16 = elems[1].parse().map_err(|_| "cannot parse RD as IPv4 address + index".to_string())?;
            return Ok(Rd((1 << 48) + ((u32::from(ip) as u64) << 16) + index as u64));
        }
        let asn: u32 = elems[0].parse().map_err(|_| "cannot parse RD as ASN + index".to_string())?;
        let index: u32 = elems[1].parse().map_err(|_| "cannot parse RD as ASN + index".to_string())?;
        let (asn, index) = (asn as u64, index as u64);
        if typ == 0 && asn > 65535 {
            return Err("cannot parse RD as ASN2 + index".into());
        }
        if asn <= 65535 && typ != 2 {
            return Ok(Rd((asn << 32) + index));
        }
        if index > 65535 {
            return Err("cannot parse RD as ASN4 + index".into());
        }
        Ok(Rd((2 << 48) + (asn << 16) + index))
    }
}

impl fmt::Display for Rd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let typ = self.0 >> 48;
        let r = self.0 & 0xffff_ffff_ffff;
        match typ {
            0 => write!(f, "{}:{}", (r >> 32) & 0xffff, r & 0xffff_ffff),
            1 => write!(
                f,
                "{}.{}.{}.{}:{}",
                (r >> 40) & 0xff,
                (r >> 32) & 0xff,
                (r >> 24) & 0xff,
                (r >> 16) & 0xff,
                r & 0xffff
            ),
            2 => {
                let asn = (r >> 16) & 0xffff_ffff;
                if asn <= 65535 { write!(f, "2:{asn}:{}", r & 0xffff) } else { write!(f, "{asn}:{}", r & 0xffff) }
            }
            _ => Ok(()),
        }
    }
}

impl std::str::FromStr for Rt {
    type Err = String;
    fn from_str(s: &str) -> Result<Rt, String> {
        s.parse::<Rd>().map(|rd| Rt(rd.0))
    }
}

impl fmt::Display for Rt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Rd(self.0).fmt(f)
    }
}

impl Rt {
    /// Route target from an extended community (8 bytes), if it is one.
    pub fn from_extended_community(ec: &[u8; 8]) -> Option<Rt> {
        // Sub-type 0x02 is route target.
        if ec[1] != 0x02 {
            return None;
        }
        let mut encoded = [0u8; 8];
        encoded[1] = ec[0] & 0x03;
        encoded[2..].copy_from_slice(&ec[2..]);
        Some(Rt(u64::from_be_bytes(encoded)))
    }
}

macro_rules! text_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}
text_serde!(Rd);
text_serde!(Rt);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let cases: &[(&str, u64, &str)] = &[
            ("0", 0, "0:0"),
            ("100", 100, "0:100"),
            ("51324:65201", 220434901565105, ""),
            ("51324:65536", 220434901565440, ""),
            ("65535:0", 281470681743360, ""),
            ("0:65535:0", 281470681743360, "65535:0"),
            ("65536:0", 562954248388608, ""),
            ("65536:3", 562954248388611, ""),
            ("2:65535:0", 562954248323072, ""),
            ("1.1.1.1:0", 282578800148480, ""),
            ("1:1.1.1.1:0", 282578800148480, "1.1.1.1:0"),
        ];
        for (input, value, canonical) in cases {
            let rd: Rd = input.parse().unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(rd.0, *value, "{input}");
            let expected = if canonical.is_empty() { input } else { canonical };
            assert_eq!(rd.to_string(), *expected, "{input}");
        }
        for bad in [
            "gfjkgjkf",
            "18446744073709551616",
            "65536:65536",
            "0:65536:0",
            "2:65536:65536",
            "1:1897:0",
            "2:1897:65536",
            "2:1.1.1.1:0",
            "0:1.1.1.1:0",
        ] {
            assert!(bad.parse::<Rd>().is_err(), "{bad}");
        }
    }

    #[test]
    fn route_target() {
        // Two-octet AS route target 65000:100.
        let ec = [0x00, 0x02, 0xfd, 0xe8, 0, 0, 0, 100];
        assert_eq!(Rt::from_extended_community(&ec).unwrap().to_string(), "65000:100");
        assert!(Rt::from_extended_community(&[0x00, 0x03, 0, 0, 0, 0, 0, 0]).is_none());
    }
}
