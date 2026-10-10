// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Compare the schema against the output of the Go implementation, produced by
//! `go run ./tests/golden/schema tests/golden/testdata/schema-cases.yaml`.

use flowscope_schema::{Component, Configuration, TableOption};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../tests/golden/testdata/schema-cases.yaml");
const GOLDEN: &str = include_str!("../../../tests/golden/testdata/schema.json");

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    config: serde_norway::Value,
}

fn options() -> Vec<(&'static str, Vec<TableOption>)> {
    use TableOption::*;
    vec![
        ("none", vec![]),
        ("skip_main_only", vec![SkipMainOnlyColumns]),
        ("skip_generated_aliased", vec![SkipGeneratedColumns, SkipAliasedColumns]),
        ("skip_time_main_alias", vec![SkipTimeReceived, SkipMainOnlyColumns, SkipAliasedColumns]),
        ("substitute_generates", vec![SubstituteGenerates, SkipAliasedColumns]),
    ]
}

fn build(config: serde_norway::Value) -> Result<Component, String> {
    let config: Configuration = serde_norway::from_value(config).map_err(|e| e.to_string())?;
    config.validate()?;
    Component::new(config)
}

fn dump(name: &str, c: &Component) -> Value {
    let mut create = BTreeMap::new();
    let mut select = BTreeMap::new();
    for (n, opts) in options() {
        create.insert(n, c.clickhouse_create_table(&opts));
        select.insert(n, c.clickhouse_select_columns(&opts));
    }
    let skip: BTreeMap<String, String> =
        c.skip_indexes().iter().map(|(k, v)| (k.name(), v.clickhouse_type().unwrap())).collect();
    let mut columns: Vec<_> = c.all_columns().iter().collect();
    columns.sort_by_key(|c| c.key);
    let columns: Vec<Value> = columns
        .iter()
        .map(|c| {
            json!({
                "key": c.key.0,
                "name": c.name,
                "disabled": c.disabled,
                "no_disable": c.no_disable,
                "group": c.group.0,
                "depends": c.depends.iter().map(|d| d.0).collect::<Vec<_>>(),
                "parser_type": c.parser_type,
                "clickhouse_type": c.clickhouse_type,
                "clickhouse_codec": c.clickhouse_codec,
                "clickhouse_alias": c.clickhouse_alias,
                "clickhouse_not_sorting_key": c.clickhouse_not_sorting_key,
                "clickhouse_generate_from": c.clickhouse_generate_from,
                "clickhouse_main_only": c.clickhouse_main_only,
                "clickhouse_materialized": c.clickhouse_materialized,
                "console_not_dimension": c.console_not_dimension,
                "console_truncate_ip": c.console_truncate_ip,
                "protobuf_index": c.protobuf_index,
                "protobuf_type": c.protobuf_type.type_name(),
                "protobuf_repeated": c.protobuf_repeated,
            })
        })
        .collect();
    json!({
        "name": name,
        "error": false,
        "create_table": create,
        "select_columns": select,
        "hash": c.clickhouse_hash(),
        "sorting_keys": c.clickhouse_sorting_keys(),
        "primary_keys": c.clickhouse_primary_keys(),
        "protobuf_definition": c.protobuf_definition(),
        "protobuf_hash": c.protobuf_message_hash(),
        "columns": columns,
        "skip_indexes": skip,
    })
}

#[test]
fn matches_go() {
    let cases: Vec<Case> = serde_norway::from_str(CASES).unwrap();
    let golden: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(cases.len(), golden.len(), "regenerate the golden file");
    for (case, expected) in cases.into_iter().zip(golden) {
        assert_eq!(expected["name"], case.name);
        let got = match build(case.config) {
            Ok(c) => dump(&case.name, &c),
            Err(e) => {
                assert!(expected["error"].as_bool().unwrap(), "{}: unexpected error: {e}", case.name);
                continue;
            }
        };
        assert!(!expected["error"].as_bool().unwrap(), "{}: expected an error", case.name);
        assert_eq!(got, expected, "{}", case.name);
    }
}
