// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Table and interval selection, and the SQL bits shared by the queries.
//! Durations are in nanoseconds, like Go durations, so the arithmetic gives
//! the same results.

use crate::query;
use crate::sqlbuilder::{self as sb, Expr, PREC_ATOM, PREC_MULTIPLICATION};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Utc};

/// A Go-like duration, in nanoseconds.
pub type Nanos = i64;

pub const SECOND: Nanos = 1_000_000_000;
pub const HOUR: Nanos = 3600 * SECOND;

/// Seconds between the Go zero time (year 1) and the Unix epoch. Go
/// truncates times relative to the zero time.
const ZERO_TO_UNIX: i128 = 62_135_596_800;

/// A timestamp, with the offset it was given with.
pub type Time = DateTime<FixedOffset>;

/// Nanoseconds since the Unix epoch.
pub fn unix_nanos(t: &Time) -> i128 {
    t.timestamp() as i128 * 1_000_000_000 + t.timestamp_subsec_nanos() as i128
}

/// `t2 - t1`, saturated like Go.
pub fn sub(t2: &Time, t1: &Time) -> Nanos {
    (unix_nanos(t2) - unix_nanos(t1)).clamp(i64::MIN as i128, i64::MAX as i128) as Nanos
}

pub fn add(t: &Time, d: Nanos) -> Time {
    *t + chrono::Duration::nanoseconds(d)
}

/// Go's `Time.Truncate`: round down to a multiple of d since the zero time.
pub fn truncate_time(t: &Time, d: Nanos) -> Time {
    if d <= 0 {
        return *t;
    }
    let since_zero = unix_nanos(t) + ZERO_TO_UNIX * 1_000_000_000;
    let r = since_zero.rem_euclid(d as i128);
    *t - chrono::Duration::nanoseconds(r as i64)
}

/// Go's `Duration.Truncate`.
pub fn truncate_duration(d: Nanos, m: Nanos) -> Nanos {
    if m <= 0 { d } else { d - d % m }
}

/// Go's `Time.AddDate(-1, 0, 0)`: the same date one year before, a February
/// 29th becoming a March 1st.
pub fn one_year_before(t: &Time) -> Time {
    let local = t.naive_local();
    let date = NaiveDate::from_ymd_opt(local.year() - 1, local.month(), local.day())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(local.year() - 1, 3, 1).unwrap());
    let naive = date.and_time(local.time());
    t.offset().from_local_datetime(&naive).single().unwrap_or(*t)
}

/// A consolidated or unconsolidated flows table.
#[derive(Clone, Debug)]
pub struct FlowsTable {
    pub name: String,
    pub resolution: Nanos,
    pub oldest: DateTime<Utc>,
}

/// A time range, as requested by a handler. It is what the resolver needs
/// to pick a table.
#[derive(Clone, Debug, PartialEq)]
pub struct InputContext {
    pub start: Time,
    pub end: Time,
    pub main_table_required: bool,
    pub points: u64,
}

/// The table and the interval to use for a query.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub table: String,
    /// Seconds between two points.
    pub interval: u64,
    /// Resolution of the table.
    pub table_interval: Nanos,
}

/// A resolution applied to a time range.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub table: String,
    pub interval: u64,
    pub start: Time,
    pub end: Time,
    /// Shift of the buckets of toStartOfInterval, which would otherwise not
    /// start on `start`.
    pub offset: u64,
}

/// Pick the table and the table interval, and return the target interval.
pub fn compute_table_and_interval(tables: &[FlowsTable], input: &InputContext) -> (String, Nanos, Nanos) {
    let span = sub(&input.end, &input.start) as u64;
    let target = ((span / input.points.max(1)) as Nanos).max(SECOND);
    if input.main_table_required {
        return ("flows".into(), SECOND, target);
    }
    let (table, interval) = best_table(tables, &input.start, target);
    (table, interval, target)
}

/// The best table starting at the provided time.
fn best_table(tables: &[FlowsTable], start: &Time, target: Nanos) -> (String, Nanos) {
    let mut table = "flows".to_string();
    let mut interval = SECOND;
    if !tables.is_empty() {
        let after = |t: &FlowsTable| t.oldest + chrono::Duration::nanoseconds(t.resolution);
        // First, the tables matching the time criteria.
        let mut candidates: Vec<usize> =
            (0..tables.len()).filter(|&i| start.with_timezone(&Utc) > after(&tables[i])).collect();
        if candidates.is_empty() {
            // No candidate, fallback to the one with the oldest data.
            let mut best = 0;
            for (idx, t) in tables.iter().enumerate() {
                if tables[best].oldest > after(t) {
                    best = idx;
                }
            }
            candidates = vec![best];
            // Add the others that are not far off in term of oldest data.
            for (idx, t) in tables.iter().enumerate() {
                if idx != best && tables[best].oldest > t.oldest {
                    candidates.push(idx);
                }
            }
        }
        candidates.sort_by_key(|&i| tables[i].resolution);
        // If possible, use the first resolution before the target interval.
        while candidates.len() > 1 && tables[candidates[1]].resolution <= target {
            candidates.remove(0);
        }
        table = tables[candidates[0]].name.clone();
        interval = tables[candidates[0]].resolution;
    }
    (table, interval.max(SECOND))
}

/// Pick the best table for the time range and the interval between two
/// points.
pub fn resolve(tables: &[FlowsTable], input: &InputContext) -> Resolution {
    let (table, table_interval, target) = compute_table_and_interval(tables, input);
    let interval = if target > table_interval { truncate_duration(target, table_interval) } else { table_interval };
    Resolution { table, interval: (interval / SECOND) as u64, table_interval }
}

impl Resolution {
    /// Apply the resolution to a time range, aligning it on the interval.
    pub fn for_range(&self, start: &Time, end: &Time) -> Resolved {
        let interval = self.interval as Nanos * SECOND;
        let start = truncate_time(start, self.table_interval);
        let end = truncate_time(end, self.table_interval);
        let end = add(&start, truncate_duration(sub(&end, &start), interval));
        Resolved {
            table: self.table.clone(),
            interval: self.interval,
            offset: interval_offset(&start, self.interval),
            start,
            end,
        }
    }
}

/// How much the buckets of toStartOfInterval have to be shifted to start on
/// the beginning of the range.
pub fn interval_offset(start: &Time, interval: u64) -> u64 {
    let seconds = interval as i64;
    let unix = start.timestamp();
    let aligned = unix / seconds * seconds;
    let diff = (unix_nanos(start) - aligned as i128 * 1_000_000_000) / 1_000_000_000;
    interval.wrapping_sub(diff as u64)
}

impl Resolved {
    /// Move the range back in time, keeping its length.
    pub fn shift_back(&self, offset: Nanos) -> Resolved {
        let start = add(&self.start, -offset);
        Resolved {
            table: self.table.clone(),
            interval: self.interval,
            offset: interval_offset(&start, self.interval),
            end: add(&self.end, -offset),
            start,
        }
    }

    pub fn timefilter_start(&self) -> Expr {
        date_time(&self.start)
    }

    pub fn timefilter_end(&self) -> Expr {
        date_time(&self.end)
    }

    pub fn timefilter(&self) -> Expr {
        sb::between(sb::column("TimeReceived"), self.timefilter_start(), self.timefilter_end(), false)
    }

    /// Put TimeReceived into a bucket, shifted so the first one starts at the
    /// beginning of the range.
    pub fn to_start_of_interval(&self) -> Expr {
        sb::op(
            sb::function(
                "toStartOfInterval",
                vec![sb::op(sb::column("TimeReceived"), "+", seconds(self.offset)), seconds(self.interval)],
            ),
            "-",
            seconds(self.offset),
        )
    }

    /// The WHERE clause for a filter, restricted to the time range.
    pub fn r#where(&self, filter: &query::Filter) -> Expr {
        sb::and([Some(self.timefilter()), filter.direct()].into_iter().flatten()).unwrap()
    }
}

/// An interval of the provided number of seconds.
pub fn seconds(count: u64) -> Expr {
    sb::interval(sb::uint(count), "second")
}

/// A timestamp as a UTC date-time.
pub fn date_time(t: &Time) -> Expr {
    sb::function(
        "toDateTime",
        vec![sb::string(&t.with_timezone(&Utc).format("%Y-%m-%d %H:%M:%S").to_string()), sb::string("UTC")],
    )
}

/// Valid units.
pub const UNITS: [&str; 6] = ["fps", "pps", "l3bps", "l2bps", "inl2%", "outl2%"];

/// The aggregate expression for the units.
pub fn units_expr(units: &str) -> Expr {
    let sql = match units {
        "fps" => "COUNT(*)",
        "pps" => "SUM(Packets * SamplingRate)",
        "l3bps" => "SUM(Bytes * SamplingRate * 8)",
        // For each packet, we add the Ethernet header (14 bytes), the FCS (4
        // bytes), the preamble and start frame delimiter (8 bytes) and the
        // IPG (~ 12 bytes).
        "l2bps" => "SUM((Bytes + 38 * Packets) * SamplingRate * 8)",
        // Like l2bps, as a percentage of the interface speed.
        "inl2%" => {
            "ifNotFinite(SUM((Bytes + 38 * Packets) * SamplingRate * 8 * 100 / (InIfSpeed * 1000000)) / \
             COUNT(DISTINCT ExporterAddress, InIfName), 0)"
        }
        "outl2%" => {
            "ifNotFinite(SUM((Bytes + 38 * Packets) * SamplingRate * 8 * 100 / (OutIfSpeed * 1000000)) / \
             COUNT(DISTINCT ExporterAddress, OutIfName), 0)"
        }
        _ => panic!("unknown units {units}"),
    };
    sb::raw(sql, PREC_ATOM)
}

/// What one row is worth for the units. None for the percentages, as they
/// divide by a number of interfaces.
pub fn units_weight_expr(units: &str) -> Option<Expr> {
    Some(match units {
        "fps" => sb::raw("1", PREC_ATOM),
        "pps" => sb::raw("Packets * SamplingRate", PREC_MULTIPLICATION),
        "l3bps" => sb::raw("Bytes * SamplingRate * 8", PREC_MULTIPLICATION),
        "l2bps" => sb::raw("(Bytes + 38 * Packets) * SamplingRate * 8", PREC_MULTIPLICATION),
        _ => return None,
    })
}

/// The units for the opposite direction.
pub fn reverse_units(units: &str) -> &str {
    match units {
        "inl2%" => "outl2%",
        "outl2%" => "inl2%",
        u => u,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Time {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn go_time() {
        assert_eq!(one_year_before(&t("2020-02-29T10:00:00Z")), t("2019-03-01T10:00:00Z"));
        assert_eq!(one_year_before(&t("2020-07-25T17:34:00+02:00")), t("2019-07-25T17:34:00+02:00"));
        // A week is not aligned on the Unix epoch in Go.
        assert_eq!(truncate_time(&t("2022-04-14T10:00:00Z"), 7 * 24 * HOUR), t("2022-04-11T00:00:00Z"));
        assert_eq!(truncate_time(&t("2022-04-10T15:45:10.5Z"), 60 * SECOND), t("2022-04-10T15:45:00Z"));
        assert_eq!(interval_offset(&t("2022-04-10T15:45:10Z"), 60), 50);
        assert_eq!(interval_offset(&t("2022-04-10T15:45:00Z"), 60), 60);
    }
}
