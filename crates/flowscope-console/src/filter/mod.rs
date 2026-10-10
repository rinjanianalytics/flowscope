// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Filter language: parse a user filter into a SQL expression.

mod grammar;
mod peg;

#[cfg(test)]
mod tests;

use crate::sqlbuilder::Expr;
use flowscope_schema::{Column, Schema};
use grammar::Node;
use peg::{Grammar, Value};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use peg::ParseError;

/// Parser state.
pub struct State {
    schema: Arc<Schema>,
    database: String,
    reverse: bool,
    main_table_required: bool,
    /// Column names found in the input, by offset, when requested.
    columns: Option<BTreeMap<usize, (usize, Column)>>,
}

thread_local! {
    static GRAMMAR: Grammar<Node, State> = grammar::grammar();
}

/// Options to parse a filter.
#[derive(Clone)]
pub struct Meta {
    /// Data schema.
    pub schema: Arc<Schema>,
    /// ClickHouse database holding the dictionaries.
    pub database: String,
    /// Parse the filter for the reverse direction.
    pub reverse_direction: bool,
}

/// Result of a parse.
#[derive(Debug)]
pub struct Parsed {
    /// The expression. None for an empty filter.
    pub expr: Option<Expr>,
    /// The main table is needed to run the expression.
    pub main_table_required: bool,
}

/// Parse errors. Parsing may report several of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Errors(pub Vec<ParseError>);

/// A serializable error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OneError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

impl Errors {
    /// The first error, for humans.
    pub fn human(&self) -> String {
        self.0.first().map(|e| format!("at line {}, position {}: {}", e.line, e.column, e.message)).unwrap_or_default()
    }

    /// All errors.
    pub fn all(&self) -> Vec<OneError> {
        self.0
            .iter()
            .map(|e| OneError { message: e.message.clone(), line: e.line, column: e.column, offset: e.offset })
            .collect()
    }

    /// What was expected at the first error.
    pub fn expected(&self) -> &[String] {
        self.0.first().map(|e| e.expected.as_slice()).unwrap_or_default()
    }
}

impl std::fmt::Display for Errors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.human())
    }
}

impl std::error::Error for Errors {}

/// Full result of a parse, errors and side results included.
struct Outcome {
    value: Option<Value<Node>>,
    errors: Vec<ParseError>,
    state: State,
}

fn run(entry: &str, input: &str, meta: &Meta, columns: bool) -> Outcome {
    let mut state = State {
        schema: meta.schema.clone(),
        database: meta.database.clone(),
        reverse: meta.reverse_direction,
        main_table_required: false,
        columns: columns.then(BTreeMap::new),
    };
    let (value, errors) = GRAMMAR.with(|g| g.parse(entry, input, &mut state));
    Outcome { value, errors, state }
}

/// Parse a filter. On error, `main_table_required` is still computed from
/// what was parsed, like the Go version.
pub fn parse_full(input: &str, meta: &Meta) -> (Result<Option<Expr>, Errors>, bool) {
    let outcome = run("Input", input, meta, false);
    let required = outcome.state.main_table_required;
    if !outcome.errors.is_empty() {
        return (Err(Errors(outcome.errors)), required);
    }
    let expr = match outcome.value {
        Some(Value::Custom(Node::Expr(e))) => e,
        _ => None,
    };
    (Ok(expr), required)
}

/// Parse a filter.
pub fn parse(input: &str, meta: &Meta) -> Result<Parsed, Errors> {
    let (result, main_table_required) = parse_full(input, meta);
    result.map(|expr| Parsed { expr, main_table_required })
}

/// What may follow a column name, for the completion. The errors are
/// returned, the expected list is in the first one.
pub fn complete_after_column(column: &str, meta: &Meta) -> Errors {
    let outcome = run("ConditionExpr", &format!("{column} "), meta, false);
    Errors(outcome.errors)
}

/// Return the filter for the opposite direction. Column names are replaced
/// by their opposite. The rest of the text does not change.
pub fn reverse(input: &str, schema: &Arc<Schema>) -> Result<String, Errors> {
    if input.trim().is_empty() {
        return Ok(input.to_string());
    }
    let meta = Meta { schema: schema.clone(), database: String::new(), reverse_direction: false };
    let outcome = run("Input", input, &meta, true);
    if !outcome.errors.is_empty() {
        return Err(Errors(outcome.errors));
    }
    let mut out = String::new();
    let mut last = 0;
    for (offset, (length, column)) in outcome.state.columns.unwrap_or_default() {
        let end = offset + length;
        out.push_str(&input[last..offset]);
        let opposite = grammar::opposite_column(schema, &column);
        if opposite.name != column.name {
            out.push_str(&opposite.name);
        } else {
            out.push_str(&input[offset..end]);
        }
        last = end;
    }
    out.push_str(&input[last..]);
    Ok(out)
}
