// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! An evaluator for the expr language (https://expr-lang.org), used by the
//! Go version of Akvorado for classifier rules. It covers the parts of the
//! language useful for classifiers: literals, member access, operators
//! (with the same precedence), string operators (`contains`, `startsWith`,
//! `endsWith`, `matches`), `in`, ranges, ternary, `??`, `let`, predicates
//! (`all`, `filter`, ... with `#`) and the common built-in functions.
//! Names and argument types are checked when compiling, like expr does.

mod builtins;
mod eval;
mod parse;

pub use builtins::sprintf;
pub use eval::{Env, Host, Signature, Type, Value};
pub use parse::{BinaryOp, Expr, ParseError, UnaryOp, parse};

use std::collections::BTreeMap;

/// A compiled expression.
#[derive(Clone)]
pub struct Program {
    expr: Expr,
    source: String,
    regexes: eval::RegexCache,
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}

impl PartialEq for Program {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Program {
    /// Compile an expression. When `as_bool` is set, the expression must
    /// return a boolean.
    pub fn compile(source: &str, env: &Env, as_bool: bool) -> Result<Program, String> {
        let expr = parse(source).map_err(|e| e.to_string())?;
        let regexes = eval::RegexCache::default();
        let t = eval::check(&expr, env, &mut Vec::new(), &regexes)?;
        if as_bool && !matches!(t, Type::Bool | Type::Any) {
            return Err(format!(
                "expected bool, but got {}",
                match t {
                    Type::Int => "int",
                    Type::Float => "float64",
                    Type::String => "string",
                    Type::Nil => "nil",
                    Type::Array => "array",
                    Type::Map => "map",
                    _ => "unknown",
                }
            ));
        }
        Ok(Program { expr, source: source.to_string(), regexes })
    }

    /// The parsed expression.
    pub fn expr(&self) -> &Expr {
        &self.expr
    }

    /// The source of the expression.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Run the expression.
    pub fn run(&self, vars: &BTreeMap<String, Value>, host: &mut dyn Host) -> Result<Value, String> {
        let mut ctx = eval::Context { vars, locals: Vec::new(), current: Vec::new(), host, regexes: &self.regexes };
        ctx.eval(&self.expr)
    }

    /// Run the expression and expect a boolean.
    pub fn run_bool(&self, vars: &BTreeMap<String, Value>, host: &mut dyn Host) -> Result<bool, String> {
        match self.run(vars, host)? {
            Value::Bool(b) => Ok(b),
            v => Err(format!("expected bool, but got {}", v.type_name())),
        }
    }
}

/// A host without functions.
pub struct NoHost;

impl Host for NoHost {
    fn call(&mut self, name: &str, _args: Vec<Value>) -> Result<Value, String> {
        Err(format!("unknown func {name}"))
    }
}

#[cfg(test)]
mod tests;
