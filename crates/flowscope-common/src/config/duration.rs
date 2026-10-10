// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Durations in the format of Go's `time.Duration`: "1h30m", "500ms",
//! "1.5s". They serialize the way Go prints them ("1h30m0s") so a
//! configuration dumped by Rust is read back the same by Go.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration as StdDuration;

/// A duration with Go formatting. Negative durations are not supported.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration(pub StdDuration);

impl Duration {
    pub const ZERO: Duration = Duration(StdDuration::ZERO);

    pub const fn from_secs(s: u64) -> Self {
        Duration(StdDuration::from_secs(s))
    }

    pub const fn from_millis(ms: u64) -> Self {
        Duration(StdDuration::from_millis(ms))
    }

    pub const fn from_nanos(ns: u64) -> Self {
        Duration(StdDuration::from_nanos(ns))
    }

    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    pub fn as_std(&self) -> StdDuration {
        self.0
    }
}

impl From<StdDuration> for Duration {
    fn from(d: StdDuration) -> Self {
        Duration(d)
    }
}

impl From<Duration> for StdDuration {
    fn from(d: Duration) -> Self {
        d.0
    }
}

impl std::ops::Deref for Duration {
    type Target = StdDuration;
    fn deref(&self) -> &StdDuration {
        &self.0
    }
}

/// Parse a duration like Go's `time.ParseDuration`.
pub fn parse(s: &str) -> Result<StdDuration, String> {
    let orig = s;
    let mut s = s;
    if let Some(r) = s.strip_prefix('+') {
        s = r;
    }
    if s.starts_with('-') {
        return Err(format!("negative duration {orig:?} is not supported"));
    }
    if s == "0" {
        return Ok(StdDuration::ZERO);
    }
    if s.is_empty() {
        return Err(format!("invalid duration {orig:?}"));
    }
    let mut total: f64 = 0.0;
    while !s.is_empty() {
        // Number, possibly with a fractional part.
        let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
        if end == 0 {
            return Err(format!("invalid duration {orig:?}"));
        }
        let value: f64 = s[..end].parse().map_err(|_| format!("invalid duration {orig:?}"))?;
        s = &s[end..];
        let uend = s.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(s.len());
        let unit = &s[..uend];
        s = &s[uend..];
        let mult = match unit {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            "" => return Err(format!("missing unit in duration {orig:?}")),
            _ => return Err(format!("unknown unit {unit:?} in duration {orig:?}")),
        };
        total += value * mult;
    }
    Ok(StdDuration::from_nanos(total.round() as u64))
}

/// Format a duration like Go's `time.Duration.String()`.
pub fn format(d: StdDuration) -> String {
    let ns = d.as_nanos() as u64;
    if ns == 0 {
        return "0s".into();
    }
    if ns < 1_000_000_000 {
        // Use the smallest unit for sub-second durations.
        let (unit, div) = if ns < 1_000 {
            ("ns", 1)
        } else if ns < 1_000_000 {
            ("µs", 1_000)
        } else {
            ("ms", 1_000_000)
        };
        return format!("{}{unit}", fmt_frac(ns, div));
    }
    let mut out = String::new();
    let hours = ns / 3_600_000_000_000;
    let rem = ns % 3_600_000_000_000;
    let minutes = rem / 60_000_000_000;
    let rem = rem % 60_000_000_000;
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if hours > 0 || minutes > 0 {
        out.push_str(&format!("{minutes}m"));
    }
    out.push_str(&format!("{}s", fmt_frac(rem, 1_000_000_000)));
    out
}

/// Format `v / div` with the fractional part trimmed of trailing zeros.
fn fmt_frac(v: u64, div: u64) -> String {
    let int = v / div;
    let frac = v % div;
    if frac == 0 {
        return int.to_string();
    }
    let width = (div as f64).log10() as usize;
    let f = format!("{frac:0width$}");
    format!("{int}.{}", f.trim_end_matches('0'))
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format(self.0))
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format(self.0))
    }
}

impl std::str::FromStr for Duration {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        parse(s).map(Duration)
    }
}

impl Serialize for Duration {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(self.0))
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Duration;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a duration")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Duration, E> {
                parse(s).map(Duration).map_err(E::custom)
            }
            // Integers are nanoseconds, like a Go time.Duration.
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Duration, E> {
                Ok(Duration::from_nanos(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Duration, E> {
                u64::try_from(v)
                    .map(Duration::from_nanos)
                    .map_err(|_| E::custom("negative durations are not supported"))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Duration, E> {
                Ok(Duration::ZERO)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let cases = [
            ("0", 0, "0s"),
            ("1s", 1_000_000_000, "1s"),
            ("1.5s", 1_500_000_000, "1.5s"),
            ("500ms", 500_000_000, "500ms"),
            ("1m", 60_000_000_000, "1m0s"),
            ("1h", 3_600_000_000_000, "1h0m0s"),
            ("1h30m", 5_400_000_000_000, "1h30m0s"),
            ("2h45m30.5s", 9_930_500_000_000, "2h45m30.5s"),
            ("100us", 100_000, "100µs"),
            ("1500ns", 1500, "1.5µs"),
            ("24h", 86_400_000_000_000, "24h0m0s"),
            ("1m0s", 60_000_000_000, "1m0s"),
        ];
        for (input, ns, output) in cases {
            let d = parse(input).unwrap();
            assert_eq!(d.as_nanos() as u64, ns, "{input}");
            assert_eq!(format(d), output, "{input}");
        }
        for bad in ["", "1", "abc", "1x", "-1s"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
