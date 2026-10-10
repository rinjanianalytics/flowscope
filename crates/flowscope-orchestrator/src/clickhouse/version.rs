// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Check the version of ClickHouse. Port of `orchestrator/clickhouse/version.go`.

use std::cmp::Ordering;

/// A version, as a list of numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Version(Vec<u64>);

impl Version {
    fn parse(s: &str) -> Option<Version> {
        let parts: Option<Vec<u64>> = s.split('.').map(|p| p.parse().ok()).collect();
        parts.filter(|p| !p.is_empty()).map(Version)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    /// Missing parts are zeros, like hashicorp/go-version.
    fn cmp(&self, other: &Self) -> Ordering {
        let n = self.0.len().max(other.0.len());
        for i in 0..n {
            let (a, b) = (self.0.get(i).copied().unwrap_or(0), other.0.get(i).copied().unwrap_or(0));
            match a.cmp(&b) {
                Ordering::Equal => continue,
                o => return o,
            }
        }
        Ordering::Equal
    }
}

fn check_ranges(current: &Version, version: &str, reason: &str, ranges: &[(&str, &str)]) -> Result<(), String> {
    for (min, max) in ranges {
        let (min, max) = (Version::parse(min).unwrap(), Version::parse(max).unwrap());
        if *current >= min && *current < max {
            let max: Vec<String> = max.0.iter().map(|p| p.to_string()).collect();
            return Err(format!("incompatible ClickHouse version {version} ({reason}): upgrade to {}", max.join(".")));
        }
    }
    Ok(())
}

/// Check the version of ClickHouse is supported.
pub fn validate_version(v: &str) -> Result<(), String> {
    let current = Version::parse(v).ok_or_else(|| format!("cannot parse version {v:?}"))?;
    // 22.4 added INTERPOLATE to ORDER BY ... WITH FILL.
    if current < Version::parse("22.4").unwrap() {
        return Err(format!("required minimal version of ClickHouse is 22.4 (got {v})"));
    }
    // IPv6 encoding problems: https://github.com/ClickHouse/ClickHouse/issues/49924
    check_ranges(
        &current,
        v,
        "IPv6 protobuf encoding bug",
        &[
            ("23", "23.2.7.32"),
            ("23.2", "23.2.7.23"),
            ("23.3", "23.3.3.52"),
            ("23.4", "23.4.3.48"),
            ("23.5", "23.5.1.3174"),
        ],
    )?;
    // Analyzer and INTERPOLATE: https://github.com/ClickHouse/ClickHouse/pull/64096
    check_ranges(
        &current,
        v,
        "experimental analyzer bug with INTERPOLATE",
        &[("24.3", "24.3.4.147"), ("24.4", "24.4.2.141"), ("24.5", "24.5.1.1763")],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        for (v, ok) in [
            ("22.3", false),
            ("22.4", true),
            ("22.8.5.29", true),
            ("23.1.2", false),
            ("23.2.7.32", true),
            ("23.3.3.51", false),
            ("23.3.3.52", true),
            ("23.5.1.3174", true),
            ("24.3.4.146", false),
            ("24.3.4.147", true),
            ("25.8.1.1", true),
            ("nope", false),
        ] {
            assert_eq!(validate_version(v).is_ok(), ok, "{v}");
        }
    }
}
