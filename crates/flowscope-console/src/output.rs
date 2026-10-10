// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Turn the rows returned by ClickHouse into the answers of the line and
//! sankey graphs.

use crate::clickhouse as ch;
use crate::graph::{LineInput, SankeyInput, nearest_period};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};

/// Format a time like Go's JSON encoding (RFC 3339, trailing zeros of the
/// fraction removed).
pub fn go_time(t: &DateTime<Utc>) -> String {
    let s = t.to_rfc3339_opts(SecondsFormat::Nanos, true);
    match s.split_once('.') {
        Some((head, frac)) => {
            let digits = frac.trim_end_matches('Z').trim_end_matches('0');
            if digits.is_empty() { format!("{head}Z") } else { format!("{head}.{digits}Z") }
        }
        None => s,
    }
}

fn serialize_times<S: Serializer>(times: &[DateTime<Utc>], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(times.iter().map(go_time))
}

/// A row of the line graph query.
#[derive(Clone, Debug, Deserialize)]
pub struct LineRow {
    #[serde(rename = "Axis")]
    pub axis: u8,
    #[serde(rename = "Time")]
    pub time: DateTime<Utc>,
    #[serde(rename = "Xps")]
    pub xps: f64,
    #[serde(rename = "Dimensions", default, deserialize_with = "null_as_empty")]
    pub dimensions: Vec<String>,
}

fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(d)?.unwrap_or_default())
}

/// Answer of /graph/line. A row is a set of dimension values. Axis 1 is the
/// direct direction, axis 2 the reverse one, 3 and 4 the previous period.
/// Rows are sorted by axis, then by traffic.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LineOutput {
    #[serde(rename = "t", serialize_with = "serialize_times")]
    pub time: Vec<DateTime<Utc>>,
    pub rows: Vec<Vec<String>>,
    /// row → points
    pub points: Vec<Vec<i64>>,
    pub axis: Vec<i64>,
    #[serde(rename = "axis-names")]
    pub axis_names: BTreeMap<i64, String>,
    pub average: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<Vec<i64>>,
    pub min: Vec<i64>,
    pub max: Vec<i64>,
    pub last: Vec<i64>,
    #[serde(rename = "95th")]
    pub ninety_five_percentile: Vec<i64>,
}

/// Go's float to int conversion (truncation).
fn to_int(v: f64) -> i64 {
    v as i64
}

fn to_uint(v: f64) -> u64 {
    v as u64
}

#[derive(Default)]
struct RowStats {
    dimensions: Vec<String>,
    points: Vec<i64>,
    sum: u64,
    max: u64,
    last: i64,
}

/// Build the answer of the line graph.
pub fn line_output(input: &LineInput, mut rows: Vec<LineRow>) -> LineOutput {
    let common = &input.common;
    // The previous period comes without dimensions. Put them back.
    if !common.dimensions.is_empty() {
        let others = vec!["Other".to_string(); common.dimensions.len()];
        for row in rows.iter_mut().filter(|r| r.dimensions.is_empty()) {
            row.dimensions = others.clone();
        }
    }

    // Time axis. The first axis is assumed to have the complete view.
    let mut output = LineOutput::default();
    let mut last_time: Option<DateTime<Utc>> = None;
    for row in rows.iter().filter(|r| r.axis == 1) {
        if last_time != Some(row.time) {
            output.time.push(row.time);
            last_time = Some(row.time);
        }
    }

    // Collect, for each axis, the rows in the order they appear.
    let mut axes: Vec<u8> = vec![];
    let mut per_axis: HashMap<u8, (Vec<String>, HashMap<String, RowStats>)> = HashMap::new();
    let mut last_time_for_axis: HashMap<u8, Option<DateTime<Utc>>> = HashMap::new();
    let mut time_index_for_axis: HashMap<u8, i64> = HashMap::new();
    let n_times = output.time.len();
    for row in &rows {
        let axis = row.axis;
        if let std::collections::hash_map::Entry::Vacant(e) = last_time_for_axis.entry(axis) {
            e.insert(None);
            axes.push(axis);
            time_index_for_axis.insert(axis, -1);
            per_axis.insert(axis, (vec![], HashMap::new()));
        }
        if last_time_for_axis[&axis] != Some(row.time) {
            *time_index_for_axis.get_mut(&axis).unwrap() += 1;
            last_time_for_axis.insert(axis, Some(row.time));
        }
        let key = format!("{axis}-[{}]", row.dimensions.join(" "));
        let (order, stats) = per_axis.get_mut(&axis).unwrap();
        let stat = stats.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            RowStats { dimensions: row.dimensions.clone(), points: vec![0; n_times], ..Default::default() }
        });
        let index = time_index_for_axis[&axis];
        // Go panics when an axis has more points than the first one.
        if let Some(point) = usize::try_from(index).ok().and_then(|i| stat.points.get_mut(i)) {
            *point = to_int(row.xps);
        }
        stat.sum = stat.sum.wrapping_add(to_uint(row.xps));
        stat.max = stat.max.max(to_uint(row.xps));
        stat.last = to_int(row.xps);
    }
    axes.sort();

    // Sort the rows of each axis, "Other" last.
    for axis in &axes {
        let (order, stats) = per_axis.get_mut(axis).unwrap();
        let is_other = |k: &String| stats[k].dimensions.first().is_some_and(|d| d == "Other");
        order.sort_by(|a, b| {
            use std::cmp::Ordering;
            match (is_other(a), is_other(b)) {
                (true, true) => return Ordering::Equal,
                (true, false) => return Ordering::Greater,
                (false, true) => return Ordering::Less,
                _ => {}
            }
            let (sa, sb) = (&stats[a], &stats[b]);
            match common.limit_type.as_str() {
                "max" => sb.max.cmp(&sa.max),
                "last" => sb.last.cmp(&sa.last),
                _ => sb.sum.cmp(&sa.sum),
            }
        });
    }

    // Total, except for percentages.
    let compute_total = !common.units.ends_with('%');
    let interval_seconds =
        if compute_total && n_times >= 2 { ((output.time[1] - output.time[0]).num_seconds()) as u64 } else { 0 };
    let mut totals = vec![];

    for axis in &axes {
        let (order, stats) = &per_axis[axis];
        for key in order {
            let stat = &stats[key];
            output.rows.push(stat.dimensions.clone());
            output.axis.push(*axis as i64);
            output.points.push(stat.points.clone());
            output.average.push(if n_times > 0 { (stat.sum / n_times as u64) as i64 } else { 0 });
            if compute_total {
                totals.push(stat.sum.wrapping_mul(interval_seconds) as i64);
            }
            // The second to last value: the last one is not shown.
            let mut last = if n_times >= 2 { stat.points[n_times - 2] } else { 0 };
            let (mut min, mut max, mut p95) = (0, 0, 0);
            let n = stat.points.len();
            if n == 1 {
                let v = stat.points[0];
                (min, max, last, p95) = (v, v, v, v);
            } else if n > 1 {
                let mut sorted = stat.points.clone();
                sorted.sort();
                // Min, but not 0.
                for &v in &sorted {
                    min = v;
                    if v > 0 {
                        break;
                    }
                }
                max = sorted[n - 1];
                // 95th percentile with linear interpolation.
                let index = 0.95 * (n - 1) as f64;
                let j = index as usize;
                let fraction = index - j as f64;
                p95 = sorted[n - 1];
                if j < n - 1 {
                    p95 = (sorted[j] as f64 * (1.0 - fraction) + sorted[j + 1] as f64 * fraction).round() as i64;
                }
            }
            output.min.push(min);
            output.max.push(max);
            output.last.push(last);
            output.ninety_five_percentile.push(p95);
        }
    }
    if compute_total {
        output.total = Some(totals);
    }

    for axis in &output.axis {
        let name = match axis {
            1 => "Direct".to_string(),
            2 => "Reverse".to_string(),
            _ => {
                let (_, name) = nearest_period(ch::sub(&common.end, &common.start));
                format!("Previous {name}")
            }
        };
        output.axis_names.insert(*axis, name);
    }
    output
}

/// A row of the sankey query.
#[derive(Clone, Debug, Deserialize)]
pub struct SankeyRow {
    #[serde(rename = "Axis")]
    pub axis: u8,
    #[serde(rename = "Xps")]
    pub xps: f64,
    #[serde(rename = "Dimensions", default, deserialize_with = "null_as_empty")]
    pub dimensions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SankeyNode {
    pub name: String,
    pub axis: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SankeyLink {
    pub source: String,
    pub target: String,
    pub xps: i64,
    pub axis: i64,
}

/// Answer of /graph/sankey.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SankeyOutput {
    pub rows: Vec<Vec<String>>,
    pub xps: Vec<i64>,
    pub axis: Vec<i64>,
    #[serde(rename = "axis-names")]
    pub axis_names: BTreeMap<i64, String>,
    pub nodes: Vec<SankeyNode>,
    pub links: Vec<SankeyLink>,
}

/// Build the answer of the sankey graph.
pub fn sankey_output(input: &SankeyInput, rows: Vec<SankeyRow>) -> SankeyOutput {
    let common = &input.common;
    let mut output = SankeyOutput::default();

    // Labels used as node prefixes, for each axis.
    let mut labels: HashMap<u8, Vec<String>> = HashMap::new();
    labels.insert(1, common.dimensions.iter().map(|c| c.name().to_string()).collect());
    if input.bidirectional {
        labels.insert(
            2,
            common.dimensions.iter().map(|c| common.schema.reverse_column_direction(c.key()).name()).collect(),
        );
    }

    let mut added = std::collections::HashSet::new();
    for row in rows {
        let axis = row.axis as i64;
        output.xps.push(to_int(row.xps));
        output.axis.push(axis);
        let labels = labels.get(&row.axis).cloned().unwrap_or_default();
        let label = |i: usize| labels.get(i).cloned().unwrap_or_default();
        for i in 0..row.dimensions.len().saturating_sub(1) {
            let d1 = format!("{}: {}", label(i), row.dimensions[i]);
            let d2 = format!("{}: {}", label(i + 1), row.dimensions[i + 1]);
            for d in [&d1, &d2] {
                if added.insert((d.clone(), axis)) {
                    output.nodes.push(SankeyNode { name: d.clone(), axis });
                }
            }
            match output.links.iter_mut().find(|l| l.axis == axis && l.source == d1 && l.target == d2) {
                Some(link) => link.xps += to_int(row.xps),
                None => output.links.push(SankeyLink { source: d1, target: d2, xps: to_int(row.xps), axis }),
            }
        }
        output.rows.push(row.dimensions);
    }
    output.links.sort_by(|a, b| {
        a.axis.cmp(&b.axis).then_with(|| if a.xps == b.xps { a.source.cmp(&b.source) } else { b.xps.cmp(&a.xps) })
    });
    for axis in &output.axis {
        match axis {
            1 => output.axis_names.insert(1, "Direct".into()),
            2 => output.axis_names.insert(2, "Reverse".into()),
            _ => None,
        };
    }
    output
}
