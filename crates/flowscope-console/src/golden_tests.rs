// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Compare the SQL with the one of the Go version
//! (`tests/golden/testdata/console-sql.json`, from
//! `console/golden_dump_test.go`).

use crate::clickhouse::{self as ch, FlowsTable, InputContext, Resolution, SECOND, Time};
use crate::graph::{CommonInput, LineInput, SankeyInput, union_all};
use crate::query;
use chrono::{DateTime, Utc};
use flowscope_schema::{Component, Schema};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Deserialize)]
struct GoldenResolution {
    table: String,
    interval: u64,
    table_interval: u64,
}

#[derive(Deserialize)]
struct GoldenGraph {
    kind: String,
    resolution: GoldenResolution,
    start: Time,
    end: Time,
    dimensions: Vec<String>,
    filter: String,
    limit: i64,
    limit_type: String,
    truncate_v4: i64,
    truncate_v6: i64,
    units: String,
    points: u64,
    bidirectional: bool,
    previous_period: bool,
    main_table_required: bool,
    axes: Vec<String>,
    union: String,
}

#[derive(Deserialize)]
struct GoldenTable {
    name: String,
    resolution: u64,
    oldest: DateTime<Utc>,
}

#[derive(Deserialize)]
struct GoldenTables {
    tables: Vec<GoldenTable>,
    start: Time,
    end: Time,
    points: u64,
    main_table_required: bool,
    table: String,
    interval: u64,
    table_interval: u64,
    r_start: Time,
    r_end: Time,
    offset: u64,
}

#[derive(Deserialize)]
pub(crate) struct Golden {
    selects: BTreeMap<String, String>,
    graphs: Vec<GoldenGraph>,
    tables: Vec<GoldenTables>,
    pub(crate) widgets: BTreeMap<String, String>,
}

pub(crate) fn golden() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/console-sql.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn all_schema() -> Arc<Schema> {
    Component::default_schema().enable_all_columns().schema().clone()
}

fn report(failures: Vec<String>, total: usize) {
    for f in failures.iter().take(10) {
        eprintln!("{f}\n");
    }
    assert!(failures.is_empty(), "{} failures out of {total}", failures.len());
}

#[test]
fn selects() {
    let golden = golden();
    let schema = all_schema();
    let mut failures = vec![];
    let mut count = 0;
    for column in schema.columns() {
        let mut qc = query::Column::new(&column.name);
        if qc.validate(&schema).is_err() {
            assert!(!golden.selects.contains_key(&column.name), "{} should validate", column.name);
            continue;
        }
        count += 1;
        let got = qc.to_sql_select(&schema, "default").to_string();
        match golden.selects.get(&column.name) {
            Some(want) if *want == got => {}
            want => failures.push(format!("{}:\n got  {got}\n want {want:?}", column.name)),
        }
    }
    assert_eq!(count, golden.selects.len());
    report(failures, count);
}

#[test]
fn graphs() {
    let golden = golden();
    let schema = all_schema();
    let mut failures = vec![];
    for g in &golden.graphs {
        let mut dimensions: Vec<query::Column> = g.dimensions.iter().map(|d| query::Column::new(d)).collect();
        query::validate_columns(&mut dimensions, &schema).unwrap();
        let mut filter = query::Filter::new(&g.filter);
        filter.validate(&schema, "default").unwrap();
        let common = CommonInput {
            schema: schema.clone(),
            database: "default".into(),
            start: g.start,
            end: g.end,
            dimensions,
            limit: g.limit,
            limit_type: g.limit_type.clone(),
            filter,
            truncate_v4: g.truncate_v4,
            truncate_v6: g.truncate_v6,
            units: g.units.clone(),
        };
        let res = Resolution {
            table: g.resolution.table.clone(),
            interval: g.resolution.interval,
            table_interval: g.resolution.table_interval as i64 * SECOND,
        };
        let (queries, main) = match g.kind.as_str() {
            "line" => {
                let input = LineInput {
                    common,
                    points: g.points,
                    bidirectional: g.bidirectional,
                    previous_period: g.previous_period,
                };
                (input.to_sql(&res), input.resolve_context().main_table_required)
            }
            _ => {
                let input = SankeyInput { common, bidirectional: g.bidirectional };
                (input.to_sql(&res), input.resolve_context().main_table_required)
            }
        };
        let ctx = format!(
            "{} {:?} filter={:?} {}/{} limit={} trunc={}/{} {} bidi={} prev={} {}..{}",
            g.kind,
            g.dimensions,
            g.filter,
            g.resolution.table,
            g.limit_type,
            g.limit,
            g.truncate_v4,
            g.truncate_v6,
            g.units,
            g.bidirectional,
            g.previous_period,
            g.start,
            g.end
        );
        if main != g.main_table_required {
            failures.push(format!("{ctx}: main table {main}"));
        }
        let axes: Vec<String> = queries.iter().map(|q| q.to_string()).collect();
        if axes.len() != g.axes.len() {
            failures.push(format!("{ctx}: {} axes, want {}", axes.len(), g.axes.len()));
            continue;
        }
        for (i, (got, want)) in axes.iter().zip(&g.axes).enumerate() {
            if got != want {
                failures.push(format!("{ctx} axis {}:\n got  {got}\n want {want}", i + 1));
            }
        }
        let union = union_all(queries).to_string();
        if union != g.union {
            failures.push(format!("{ctx} union:\n got  {union}\n want {}", g.union));
        }
    }
    report(failures, golden.graphs.len());
}

#[test]
fn tables() {
    let golden = golden();
    let mut failures = vec![];
    for g in &golden.tables {
        let tables: Vec<FlowsTable> = g
            .tables
            .iter()
            .map(|t| FlowsTable { name: t.name.clone(), resolution: t.resolution as i64 * SECOND, oldest: t.oldest })
            .collect();
        let input =
            InputContext { start: g.start, end: g.end, main_table_required: g.main_table_required, points: g.points };
        let res = ch::resolve(&tables, &input);
        let r = res.for_range(&g.start, &g.end);
        let got = (res.table.clone(), res.interval, res.table_interval / SECOND, r.start, r.end, r.offset);
        let want = (g.table.clone(), g.interval, g.table_interval as i64, g.r_start, g.r_end, g.offset);
        if got != want {
            failures.push(format!(
                "{:?} {}..{} points={} main={}:\n got  {got:?}\n want {want:?}",
                g.tables.iter().map(|t| &t.name).collect::<Vec<_>>(),
                g.start,
                g.end,
                g.points,
                g.main_table_required
            ));
        }
    }
    report(failures, golden.tables.len());
}

#[test]
fn widgets() {
    use crate::completion;
    use crate::widgets::{TopWidget, flow_last_sql, graph_sql, top_sql};
    let golden = golden();
    let schema = Component::default_schema().schema().clone();
    let now: Time = chrono::DateTime::parse_from_rfc3339("2022-04-11T15:45:10Z").unwrap();
    let tables = vec![FlowsTable {
        name: "flows".into(),
        resolution: 0,
        oldest: chrono::DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z").unwrap().to_utc(),
    }];
    let configs = [
        ("default", "InIfBoundary = external", "InIfBoundary = external", 24),
        ("no-filter", "", "", 1),
        ("main-filter", "SrcPort = 443 AND InIfBoundary = external", "DstPort = 80", 7 * 24),
    ];
    let mut got = BTreeMap::new();
    for (name, top_filter, graph_filter, hours) in configs {
        let mut top_filter = query::Filter::new(top_filter);
        top_filter.validate(&schema, "default").unwrap();
        let mut graph_filter = query::Filter::new(graph_filter);
        graph_filter.validate(&schema, "default").unwrap();
        for widget in TopWidget::ALL {
            let (q, _) = top_sql(widget, &top_filter, &tables, &now, "default");
            got.insert(format!("{name}/top/{}", widget.as_str()), q.to_string());
        }
        let (q, _) = graph_sql(&graph_filter, hours * ch::HOUR, &tables, &now);
        got.insert(format!("{name}/graph"), q.to_string());
        got.insert(format!("{name}/flow-last"), flow_last_sql(&schema).to_string());
    }
    for key in golden.widgets.keys().filter(|k| k.starts_with("default/complete/")) {
        let parts: Vec<&str> = key.splitn(5, '/').collect();
        let (column, prefix, limit) = (parts[2], parts[3], parts[4].parse::<i64>().unwrap());
        let limit = if limit == 0 { 20 } else { limit };
        if let Some((q, _)) = completion::value_query(&schema, column, prefix, limit, "default") {
            got.insert(key.clone(), q.to_string());
        }
    }
    let mut failures = vec![];
    for (key, want) in &golden.widgets {
        match got.get(key) {
            Some(g) if g == want => {}
            // Go sends broken SQL for a disabled column, Rust sends nothing.
            None if want.contains("``") => {}
            g => failures.push(format!("{key}:\n got  {g:?}\n want {want}")),
        }
    }
    for key in got.keys().filter(|k| !golden.widgets.contains_key(*k)) {
        failures.push(format!("{key}: not in golden"));
    }
    report(failures, golden.widgets.len());
}

#[derive(Deserialize)]
struct GoldenOutput {
    kind: String,
    input: serde_json::Value,
    rows: serde_json::Value,
    output: serde_json::Value,
}

#[test]
fn outputs() {
    use crate::output::{line_output, sankey_output};
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/console-output.json");
    let golden: Vec<GoldenOutput> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let schema = Component::default_schema().schema().clone();
    let mut failures = vec![];
    for (i, g) in golden.iter().enumerate() {
        let got = match g.kind.as_str() {
            "line" => {
                let mut input: LineInput = serde_json::from_value(g.input.clone()).unwrap();
                input.common.schema = schema.clone();
                query::validate_columns(&mut input.common.dimensions, &schema).unwrap();
                input.common.filter.validate(&schema, "default").unwrap();
                let rows = serde_json::from_value(g.rows.clone()).unwrap();
                serde_json::to_value(line_output(&input, rows)).unwrap()
            }
            _ => {
                let mut input: SankeyInput = serde_json::from_value(g.input.clone()).unwrap();
                input.common.schema = schema.clone();
                query::validate_columns(&mut input.common.dimensions, &schema).unwrap();
                input.common.filter.validate(&schema, "default").unwrap();
                let rows = serde_json::from_value(g.rows.clone()).unwrap();
                serde_json::to_value(sankey_output(&input, rows)).unwrap()
            }
        };
        if got != g.output {
            failures.push(format!("{} case {i}:\n got  {got}\n want {}", g.kind, g.output));
        }
    }
    report(failures, golden.len());
}
