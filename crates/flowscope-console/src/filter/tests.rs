// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use flowscope_schema::{Component, Configuration};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct GoldenError {
    message: String,
    line: usize,
    column: usize,
    offset: usize,
}

#[derive(Deserialize)]
struct GoldenParse {
    input: String,
    schema: String,
    reverse: bool,
    #[serde(default)]
    output: String,
    main_table_required: bool,
    #[serde(default)]
    errors: Option<Vec<GoldenError>>,
    #[serde(default)]
    human: String,
    #[serde(default)]
    expected: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GoldenReverse {
    input: String,
    output: String,
    #[serde(default)]
    error: String,
}

#[derive(Deserialize)]
struct GoldenComplete {
    column: String,
    expected: Option<Vec<String>>,
    #[serde(default)]
    errors: Option<Vec<GoldenError>>,
}

#[derive(Deserialize)]
struct Golden {
    parse: Vec<GoldenParse>,
    reverse: Vec<GoldenReverse>,
    complete: HashMap<String, Vec<GoldenComplete>>,
}

fn schemas() -> HashMap<&'static str, Arc<Schema>> {
    let custom: Configuration = serde_json::from_value(serde_json::json!({
        "custom-dictionaries": {
            "test": {
                "keys": [{"name": "SrcAddr", "type": "string"}],
                "attributes": [
                    {"name": "csv_col_name", "type": "string", "label": "DimensionAttribute"},
                    {"name": "role", "type": "string"},
                ],
                "source": "test.csv",
                "dimensions": ["SrcAddr", "DstAddr"],
            }
        }
    }))
    .unwrap();
    HashMap::from([
        ("default", Component::default_schema().schema().clone()),
        ("all", Component::default_schema().enable_all_columns().schema().clone()),
        ("custom", Component::new(custom).unwrap().schema().clone()),
    ])
}

fn same_errors(got: &Errors, want: &[GoldenError]) -> bool {
    got.0.len() == want.len()
        && got
            .0
            .iter()
            .zip(want)
            .all(|(g, w)| g.message == w.message && g.line == w.line && g.column == w.column && g.offset == w.offset)
}

fn show(errors: &Errors) -> String {
    errors
        .0
        .iter()
        .map(|e| format!("{}:{}({}) {} {:?}", e.line, e.column, e.offset, e.message, e.expected))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[test]
fn golden() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/testdata/filter.json");
    let golden: Golden = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let schemas = schemas();
    let mut failures = vec![];

    for g in &golden.parse {
        let meta = Meta {
            schema: schemas[g.schema.as_str()].clone(),
            database: "default".into(),
            reverse_direction: g.reverse,
        };
        let (result, required) = parse_full(&g.input, &meta);
        let ctx = format!("parse {:?} schema={} reverse={}", g.input, g.schema, g.reverse);
        if required != g.main_table_required {
            failures.push(format!("{ctx}: main table required {required}, want {}", g.main_table_required));
        }
        match (&result, &g.errors) {
            (Ok(expr), None) => {
                let got = expr.as_ref().map(|e| e.to_string()).unwrap_or_default();
                if got != g.output {
                    failures.push(format!("{ctx}:\n   got  {got}\n   want {}", g.output));
                }
            }
            (Err(errors), Some(want)) => {
                if !same_errors(errors, want) || errors.human() != g.human {
                    failures.push(format!("{ctx}:\n   got  {}\n   want {}", show(errors), g.human));
                }
                if errors.expected() != g.expected.as_deref().unwrap_or_default() {
                    failures.push(format!("{ctx}: expected {:?}, want {:?}", errors.expected(), g.expected));
                }
            }
            (Ok(expr), Some(_)) => failures.push(format!("{ctx}: got {expr:?}, want error {}", g.human)),
            (Err(errors), None) => failures.push(format!("{ctx}: got error {}, want {}", show(errors), g.output)),
        }
    }

    for g in &golden.reverse {
        match reverse(&g.input, &schemas["all"]) {
            Ok(got) if g.error.is_empty() && got == g.output => {}
            Err(errors) if !g.error.is_empty() && errors.0.iter().all(|e| g.error.contains(&e.message)) => {}
            got => failures.push(format!("reverse {:?}: got {got:?}, want {:?} / {:?}", g.input, g.output, g.error)),
        }
    }

    for (name, completes) in &golden.complete {
        let meta =
            Meta { schema: schemas[name.as_str()].clone(), database: "default".into(), reverse_direction: false };
        for g in completes {
            let errors = complete_after_column(&g.column, &meta);
            let ok = errors.expected() == g.expected.as_deref().unwrap_or_default()
                && same_errors(&errors, g.errors.as_deref().unwrap_or_default());
            if !ok {
                failures.push(format!("complete {name} {:?}: got {}, want {:?}", g.column, show(&errors), g.expected));
            }
        }
    }

    let total = golden.parse.len() + golden.reverse.len() + golden.complete.values().map(Vec::len).sum::<usize>();
    for f in failures.iter().take(40) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} failures out of {total} cases", failures.len());
}

/// A filter nested too deeply is refused. It used to overflow the stack,
/// which aborts the process.
#[test]
fn nesting_limit() {
    let schemas = schemas();
    let meta = Meta { schema: schemas["default"].clone(), database: "default".into(), reverse_direction: false };
    let nested = |depth: usize| format!("{}SrcAS = 1{}", "(".repeat(depth), ")".repeat(depth));
    assert!(parse(&nested(40), &meta).is_ok());
    for input in [nested(100_000), "(".repeat(100_000), "NOT ".repeat(100_000)] {
        let errors = parse(&input, &meta).unwrap_err();
        assert_eq!(errors.0[0].message, "expression nested too deeply");
    }
}
