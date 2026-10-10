// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! SQL expressions and queries, written the way the Go version writes them
//! (its `common/sqlbuilder`), so the same inputs give the same SQL text. The
//! text is the single-line form of the Go formatter.

use std::fmt;

/// A SQL expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Ident(String),
    /// A string literal (unescaped value).
    Str(String),
    Num(String),
    Func(String, Vec<Expr>),
    /// A parametric function: `topKWeighted(10, 20)(x, y)`.
    ParamFunc(String, Vec<Expr>, Vec<Expr>),
    /// A binary operation. The operator is kept as given ("NOT IN").
    Binary(Box<Expr>, String, Box<Expr>),
    Not(Box<Expr>),
    /// A parenthesized list: a tuple or a single expression in parentheses.
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    Between {
        expr: Box<Expr>,
        not: bool,
        low: Box<Expr>,
        high: Box<Expr>,
    },
    Interval(Box<Expr>, String),
    Alias(Box<Expr>, String),
    Star,
    Subquery(Box<Query>),
    /// A type, written as is.
    RawType(String),
    /// Fixed SQL written in the code, with its precedence.
    Raw(String, u8),
}

const PREC_LAMBDA: u8 = 1;
const PREC_OR: u8 = 2;
const PREC_AND: u8 = 3;
const PREC_NOT: u8 = 4;
const PREC_COMPARISON: u8 = 5;
const PREC_CONCAT: u8 = 6;
const PREC_ADDITION: u8 = 7;
/// Precedence of `*`, `/` and `%`.
pub const PREC_MULTIPLICATION: u8 = 8;
const PREC_CAST: u8 = 9;
/// Precedence of an atom (function call, column, literal).
pub const PREC_ATOM: u8 = 10;

fn operator_name(op: &str) -> String {
    let upper = op.to_uppercase();
    upper.strip_prefix("NOT ").unwrap_or(&upper).to_string()
}

fn operator_precedence(op: &str) -> u8 {
    match operator_name(op).as_str() {
        "->" => PREC_LAMBDA,
        "OR" => PREC_OR,
        "AND" => PREC_AND,
        "=" | "==" | "!=" | "<>" | "<" | "<=" | ">" | ">=" | "IN" | "LIKE" | "ILIKE" | "REGEXP" | "IS" => {
            PREC_COMPARISON
        }
        "||" => PREC_CONCAT,
        "+" | "-" => PREC_ADDITION,
        "*" | "/" | "%" => PREC_MULTIPLICATION,
        "::" => PREC_CAST,
        _ => 0,
    }
}

impl Expr {
    fn precedence(&self) -> u8 {
        match self {
            Expr::Binary(_, op, _) => operator_precedence(op),
            Expr::Not(_) => PREC_NOT,
            Expr::Between { .. } => PREC_COMPARISON,
            Expr::Raw(_, prec) => *prec,
            _ => PREC_ATOM,
        }
    }
}

/// A column.
pub fn column(name: &str) -> Expr {
    Expr::Ident(name.into())
}

pub fn columns(names: &[&str]) -> Vec<Expr> {
    names.iter().map(|n| column(n)).collect()
}

pub fn string(value: &str) -> Expr {
    Expr::Str(value.into())
}

pub fn uint(value: u64) -> Expr {
    Expr::Num(value.to_string())
}

pub fn int(value: i64) -> Expr {
    Expr::Num(value.to_string())
}

/// A number written as is (`0xffff`).
pub fn number(literal: &str) -> Expr {
    Expr::Num(literal.into())
}

pub fn function(name: &str, args: Vec<Expr>) -> Expr {
    Expr::Func(name.into(), args)
}

pub fn parametric_function(name: &str, parameters: Vec<Expr>, args: Vec<Expr>) -> Expr {
    Expr::ParamFunc(name.into(), parameters, args)
}

pub fn lambda(parameter: &str, body: Expr) -> Expr {
    op(column(parameter), "->", body)
}

/// Fixed SQL, with the precedence of its top-level operator.
pub fn raw(sql: &str, precedence: u8) -> Expr {
    Expr::Raw(sql.into(), precedence)
}

/// A binary operation, with parentheses where the precedence needs them.
pub fn op(left: Expr, operator: &str, right: Expr) -> Expr {
    let name = operator_name(operator);
    let prec = operator_precedence(operator);
    let left = if left.precedence() < prec { parens(left) } else { left };
    let associative = name == "AND" || name == "OR";
    let rp = right.precedence();
    let right = if rp < prec || (rp == prec && !associative) { parens(right) } else { right };
    Expr::Binary(Box::new(left), operator.into(), Box::new(right))
}

pub fn cast(expr: Expr, type_name: &str) -> Expr {
    op(expr, "::", Expr::RawType(type_name.into()))
}

fn combine(operator: &str, exprs: impl IntoIterator<Item = Expr>) -> Option<Expr> {
    exprs.into_iter().reduce(|acc, e| op(acc, operator, e))
}

/// AND of the expressions. None when there is none.
pub fn and(exprs: impl IntoIterator<Item = Expr>) -> Option<Expr> {
    combine("AND", exprs)
}

/// OR of the expressions. None when there is none.
pub fn or(exprs: impl IntoIterator<Item = Expr>) -> Option<Expr> {
    combine("OR", exprs)
}

pub fn not(expr: Expr) -> Expr {
    let expr = match expr {
        e @ (Expr::Binary(..) | Expr::Between { .. } | Expr::Not(_)) => parens(e),
        e => e,
    };
    Expr::Not(Box::new(expr))
}

pub fn parens(expr: Expr) -> Expr {
    Expr::Tuple(vec![expr])
}

pub fn tuple(items: Vec<Expr>) -> Expr {
    Expr::Tuple(items)
}

pub fn array(items: Vec<Expr>) -> Expr {
    Expr::Array(items)
}

pub fn between(expr: Expr, low: Expr, high: Expr, not: bool) -> Expr {
    Expr::Between { expr: Box::new(expr), not, low: Box::new(low), high: Box::new(high) }
}

pub fn interval(value: Expr, unit: &str) -> Expr {
    Expr::Interval(Box::new(value), unit.into())
}

pub fn alias(expr: Expr, name: &str) -> Expr {
    Expr::Alias(Box::new(expr), name.into())
}

pub fn star() -> Expr {
    Expr::Star
}

/// Quote an identifier when it is not a bare one.
pub fn ident(name: &str) -> String {
    let bare = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if bare { name.to_string() } else { format!("`{}`", name.replace('`', "``")) }
}

/// A table name, optionally in a database.
pub fn table(name: &str, database: Option<&str>) -> String {
    match database {
        Some(db) if !db.is_empty() => format!("{}.{}", ident(db), ident(name)),
        _ => ident(name),
    }
}

/// Quote a string literal like the Go version: backslash escapes.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn join<T: fmt::Display>(items: &[T]) -> String {
    items.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ")
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Ident(name) => f.write_str(&ident(name)),
            Expr::Str(s) => f.write_str(&quote(s)),
            Expr::Num(n) | Expr::RawType(n) | Expr::Raw(n, _) => f.write_str(n),
            Expr::Func(name, args) => write!(f, "{}({})", ident(name), join(args)),
            Expr::ParamFunc(name, params, args) => write!(f, "{}({})({})", ident(name), join(params), join(args)),
            Expr::Binary(l, op, r) if op == "::" => write!(f, "{l}::{r}"),
            Expr::Binary(l, op, r) => write!(f, "{l} {op} {r}"),
            Expr::Not(e) => write!(f, "NOT {e}"),
            Expr::Tuple(items) => write!(f, "({})", join(items)),
            Expr::Array(items) => write!(f, "[{}]", join(items)),
            Expr::Between { expr, not, low, high } => {
                write!(f, "{expr} {}BETWEEN {low} AND {high}", if *not { "NOT " } else { "" })
            }
            Expr::Interval(value, unit) => write!(f, "INTERVAL {value} {unit}"),
            Expr::Alias(e, name) => write!(f, "{e} AS {}", ident(name)),
            Expr::Star => f.write_str("*"),
            Expr::Subquery(q) => write!(f, "({q})"),
        }
    }
}

/// A modifier of a `*` select item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modifier {
    Replace(Vec<Expr>),
    Except(Vec<String>),
}

impl fmt::Display for Modifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Modifier::Replace(items) => write!(f, "REPLACE({})", join(items)),
            Modifier::Except(names) => {
                write!(f, "EXCEPT({})", names.iter().map(|n| ident(n)).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// An ORDER BY item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderItem {
    expr: Expr,
    descending: bool,
    fill: Option<(Expr, Expr, Expr)>,
}

pub fn order(expr: Expr) -> OrderItem {
    OrderItem { expr, descending: false, fill: None }
}

impl OrderItem {
    pub fn desc(mut self) -> Self {
        self.descending = true;
        self
    }

    pub fn fill(mut self, from: Expr, to: Expr, step: Expr) -> Self {
        self.fill = Some((from, to, step));
        self
    }
}

impl fmt::Display for OrderItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.expr)?;
        if self.descending {
            f.write_str(" DESC")?;
        }
        if let Some((from, to, step)) = &self.fill {
            write!(f, " WITH FILL FROM {from} TO {to} STEP {step}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cte {
    /// `name AS (SELECT ...)`
    Named(String, Query),
    /// `(SELECT ...) AS name`
    Scalar(Query, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum From {
    Table(String),
    Select(Box<Query>),
}

/// A SELECT query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    with: Vec<Cte>,
    distinct: bool,
    items: Vec<(Expr, Option<Modifier>)>,
    from: Option<From>,
    r#where: Option<Expr>,
    group_by: Vec<Expr>,
    order_by: Vec<OrderItem>,
    interpolate: Vec<(String, Expr)>,
    limit: Option<i64>,
    settings: Vec<(String, Expr)>,
    /// Next query of a UNION, with true for UNION DISTINCT.
    union: Option<(bool, Box<Query>)>,
}

/// A SELECT query with the provided items.
pub fn select(items: Vec<Expr>) -> Query {
    Query { items: items.into_iter().map(|e| (e, None)).collect(), ..Query::default() }
}

impl Query {
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    pub fn item(mut self, expr: Expr, modifier: Option<Modifier>) -> Self {
        self.items.push((expr, modifier));
        self
    }

    /// FROM a table (already formatted with [`table`]).
    pub fn from(mut self, table: String) -> Self {
        self.from = Some(From::Table(table));
        self
    }

    pub fn from_select(mut self, sub: Query) -> Self {
        self.from = Some(From::Select(Box::new(sub)));
        self
    }

    pub fn r#where(mut self, expr: Option<Expr>) -> Self {
        self.r#where = expr;
        self
    }

    pub fn group_by(mut self, exprs: Vec<Expr>) -> Self {
        self.group_by = exprs;
        self
    }

    pub fn order_by(mut self, items: Vec<OrderItem>) -> Self {
        self.order_by = items;
        self
    }

    pub fn interpolate(mut self, column: &str, expr: Expr) -> Self {
        self.interpolate.push((column.into(), expr));
        self
    }

    pub fn limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn setting(mut self, name: &str, value: Expr) -> Self {
        self.settings.push((name.into(), value));
        self
    }

    pub fn with(mut self, name: &str, sub: Query) -> Self {
        self.with.push(Cte::Named(name.into(), sub));
        self
    }

    pub fn with_scalar(mut self, sub: Query, name: &str) -> Self {
        self.with.push(Cte::Scalar(sub, name.into()));
        self
    }

    fn union(mut self, distinct: bool, other: Query) -> Self {
        let mut last = &mut self;
        while last.union.is_some() {
            last = &mut last.union.as_mut().unwrap().1;
        }
        last.union = Some((distinct, Box::new(other)));
        self
    }

    pub fn union_all(self, other: Query) -> Self {
        self.union(false, other)
    }

    pub fn union_distinct(self, other: Query) -> Self {
        self.union(true, other)
    }

    pub fn subquery(self) -> Expr {
        Expr::Subquery(Box::new(self))
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.with.is_empty() {
            let ctes: Vec<String> = self
                .with
                .iter()
                .map(|cte| match cte {
                    Cte::Named(name, q) => format!("{} AS ({q})", ident(name)),
                    Cte::Scalar(q, name) => format!("({q}) AS {}", ident(name)),
                })
                .collect();
            write!(f, "WITH {} ", ctes.join(", "))?;
        }
        f.write_str("SELECT ")?;
        if self.distinct {
            f.write_str("DISTINCT ")?;
        }
        let items: Vec<String> = self
            .items
            .iter()
            .map(|(e, m)| match m {
                Some(m) => format!("{e} {m}"),
                None => e.to_string(),
            })
            .collect();
        f.write_str(&items.join(", "))?;
        match &self.from {
            Some(From::Table(t)) => write!(f, " FROM {t}")?,
            Some(From::Select(q)) => write!(f, " FROM ({q})")?,
            None => {}
        }
        if let Some(w) = &self.r#where {
            write!(f, " WHERE {w}")?;
        }
        if !self.group_by.is_empty() {
            write!(f, " GROUP BY {}", join(&self.group_by))?;
        }
        if !self.order_by.is_empty() {
            write!(f, " ORDER BY {}", join(&self.order_by))?;
            if !self.interpolate.is_empty() {
                let items: Vec<String> = self.interpolate.iter().map(|(c, e)| format!("{} AS {e}", ident(c))).collect();
                write!(f, " INTERPOLATE ({})", items.join(", "))?;
            }
        }
        if let Some(limit) = self.limit {
            write!(f, " LIMIT {limit}")?;
        }
        if !self.settings.is_empty() {
            let items: Vec<String> = self.settings.iter().map(|(n, e)| format!("{}={e}", ident(n))).collect();
            write!(f, " SETTINGS {}", items.join(", "))?;
        }
        if let Some((distinct, next)) = &self.union {
            write!(f, " UNION {} {next}", if *distinct { "DISTINCT" } else { "ALL" })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence() {
        let a = op(column("SrcAS"), "=", uint(1));
        let b = op(column("DstAS"), "=", uint(2));
        let c = op(column("SrcPort"), "=", uint(3));
        assert_eq!(
            and(vec![a.clone(), or(vec![b.clone(), c.clone()]).unwrap()]).unwrap().to_string(),
            "SrcAS = 1 AND (DstAS = 2 OR SrcPort = 3)"
        );
        assert_eq!(
            or(vec![a.clone(), and(vec![b, c]).unwrap()]).unwrap().to_string(),
            "SrcAS = 1 OR DstAS = 2 AND SrcPort = 3"
        );
        assert_eq!(not(a).to_string(), "NOT (SrcAS = 1)");
        assert_eq!(cast(uint(3), "UInt128").to_string(), "3::UInt128");
        assert_eq!(quote("it's \\"), r"'it\'s \\'");
        assert_eq!(ident("a b"), "`a b`");
    }

    #[test]
    fn query() {
        let q = select(vec![alias(int(1), "axis"), star()])
            .from_select(
                select(vec![column("a")])
                    .from(table("source", None))
                    .group_by(vec![column("a")])
                    .order_by(vec![order(column("a")).fill(uint(1), uint(2), uint(3))])
                    .interpolate("a", array(vec![string("Other")])),
            )
            .with("source", select(vec![star()]).from(table("flows", None)).setting("x", uint(1)));
        assert_eq!(
            q.to_string(),
            "WITH source AS (SELECT * FROM flows SETTINGS x=1) SELECT 1 AS axis, * FROM (SELECT a FROM source \
             GROUP BY a ORDER BY a WITH FILL FROM 1 TO 2 STEP 3 INTERPOLATE (a AS ['Other']))"
        );
    }
}
