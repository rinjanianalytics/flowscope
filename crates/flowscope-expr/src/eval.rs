// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Type checking and evaluation of expressions.

use crate::parse::{BinaryOp, Expr, UnaryOp};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex};

/// A runtime value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Map(_) => "map",
        }
    }

    fn truthy(&self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(*b),
            v => Err(format!("expected bool, got {}", v.type_name())),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => f.write_str("<nil>"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::String(s) => f.write_str(s),
            Value::Array(a) => {
                write!(f, "[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Map(m) => {
                write!(f, "map[")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{k}:{v}")?;
                }
                write!(f, "]")
            }
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.into())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::Int(v as i64)
    }
}
impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Value::Int(v as i64)
    }
}

/// Static type, for compile-time checks.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Any,
    Nil,
    Bool,
    Int,
    Float,
    String,
    Array,
    Map,
    /// A structure with known fields.
    Struct(BTreeMap<String, Type>),
}

impl Type {
    fn name(&self) -> &'static str {
        match self {
            Type::Any => "interface {}",
            Type::Nil => "nil",
            Type::Bool => "bool",
            Type::Int => "int",
            Type::Float => "float64",
            Type::String => "string",
            Type::Array => "array",
            Type::Map => "map",
            Type::Struct(_) => "struct",
        }
    }

    fn is_numeric(&self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }

    fn assignable_to(&self, target: &Type) -> bool {
        match (self, target) {
            (_, Type::Any) | (Type::Any, _) => true,
            (Type::Int, Type::Float) => true,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        }
    }
}

/// Signature of a host function.
#[derive(Clone, Debug)]
pub struct Signature {
    pub params: Vec<Type>,
    /// Type of extra arguments, if variadic.
    pub variadic: Option<Type>,
    pub returns: Type,
}

/// Compile-time environment: variables and host functions.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub variables: BTreeMap<String, Type>,
    pub functions: BTreeMap<String, Signature>,
}

/// Host side of the evaluation: host functions.
pub trait Host {
    fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String>;
}

/// Cache of compiled regular expressions.
#[derive(Clone, Default)]
pub(crate) struct RegexCache(Arc<Mutex<HashMap<String, Regex>>>);

impl RegexCache {
    pub(crate) fn get(&self, pattern: &str) -> Result<Regex, String> {
        let mut cache = self.0.lock().unwrap();
        if let Some(r) = cache.get(pattern) {
            return Ok(r.clone());
        }
        let r = Regex::new(pattern).map_err(|e| format!("error parsing regexp: {e}"))?;
        cache.insert(pattern.to_string(), r.clone());
        Ok(r)
    }
}

const BUILTINS: &[&str] = &[
    "len",
    "abs",
    "int",
    "float",
    "string",
    "upper",
    "lower",
    "trim",
    "trimPrefix",
    "trimSuffix",
    "split",
    "splitAfter",
    "replace",
    "repeat",
    "indexOf",
    "lastIndexOf",
    "hasPrefix",
    "hasSuffix",
    "max",
    "min",
    "sum",
    "mean",
    "join",
    "keys",
    "values",
    "first",
    "last",
    "all",
    "any",
    "none",
    "one",
    "filter",
    "map",
    "count",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "reverse",
    "type",
    "round",
    "ceil",
    "floor",
];

const PREDICATES: &[&str] =
    &["all", "any", "none", "one", "filter", "map", "count", "find", "findIndex", "findLast", "findLastIndex"];

/// Check an expression and infer its type.
pub(crate) fn check(e: &Expr, env: &Env, locals: &mut Vec<String>, regexes: &RegexCache) -> Result<Type, String> {
    Ok(match e {
        Expr::Nil => Type::Nil,
        Expr::Bool(_) => Type::Bool,
        Expr::Int(_) => Type::Int,
        Expr::Float(_) => Type::Float,
        Expr::String(_) => Type::String,
        Expr::Array(items) => {
            for i in items {
                check(i, env, locals, regexes)?;
            }
            Type::Array
        }
        Expr::Map(items) => {
            for (k, v) in items {
                check(k, env, locals, regexes)?;
                check(v, env, locals, regexes)?;
            }
            Type::Map
        }
        Expr::Ident(name) => {
            if name == "#" || locals.contains(name) {
                Type::Any
            } else {
                env.variables.get(name).cloned().ok_or_else(|| format!("unknown name {name}"))?
            }
        }
        Expr::Member(obj, key, _) => {
            let t = check(obj, env, locals, regexes)?;
            check(key, env, locals, regexes)?;
            match (&t, key.as_ref()) {
                (Type::Struct(fields), Expr::String(k)) => {
                    fields.get(k).cloned().ok_or_else(|| format!("type struct has no field {k}"))?
                }
                (Type::Int | Type::Float | Type::Bool, _) => {
                    return Err(format!("type {} has no fields", t.name()));
                }
                _ => Type::Any,
            }
        }
        Expr::Slice(obj, a, b) => {
            check(obj, env, locals, regexes)?;
            if let Some(a) = a {
                check(a, env, locals, regexes)?;
            }
            if let Some(b) = b {
                check(b, env, locals, regexes)?;
            }
            Type::Any
        }
        Expr::Closure(body) => {
            check(body, env, locals, regexes)?;
            Type::Any
        }
        Expr::Call(name, args) => {
            if let Some(sig) = env.functions.get(name) {
                let n = args.len();
                if n < sig.params.len() || (sig.variadic.is_none() && n > sig.params.len()) {
                    return Err(format!(
                        "{} arguments for {name} (expected {}, got {n})",
                        if n < sig.params.len() { "not enough" } else { "too many" },
                        sig.params.len()
                    ));
                }
                for (i, a) in args.iter().enumerate() {
                    let t = check(a, env, locals, regexes)?;
                    let expected = sig.params.get(i).or(sig.variadic.as_ref()).unwrap();
                    if !t.assignable_to(expected) {
                        return Err(format!(
                            "cannot use {} as argument (type {}) to call {name}",
                            t.name(),
                            expected.name()
                        ));
                    }
                }
                sig.returns.clone()
            } else if BUILTINS.contains(&name.as_str()) {
                for a in args {
                    check(a, env, locals, regexes)?;
                }
                match name.as_str() {
                    "len" | "int" | "indexOf" | "lastIndexOf" | "count" | "findIndex" | "findLastIndex" => Type::Int,
                    "float" | "mean" | "round" | "ceil" | "floor" => Type::Float,
                    "string" | "upper" | "lower" | "trim" | "trimPrefix" | "trimSuffix" | "replace" | "repeat"
                    | "join" | "type" => Type::String,
                    "hasPrefix" | "hasSuffix" | "all" | "any" | "none" | "one" => Type::Bool,
                    "split" | "splitAfter" | "keys" | "values" | "filter" | "map" | "reverse" => Type::Array,
                    _ => Type::Any,
                }
            } else {
                return Err(format!("unknown func {name}"));
            }
        }
        Expr::Unary(op, inner) => {
            let t = check(inner, env, locals, regexes)?;
            match op {
                UnaryOp::Not => {
                    if !matches!(t, Type::Bool | Type::Any) {
                        return Err(format!("invalid operation: ! (mismatched type {})", t.name()));
                    }
                    Type::Bool
                }
                UnaryOp::Neg | UnaryOp::Plus => {
                    if !(t.is_numeric() || t == Type::Any) {
                        return Err(format!("invalid operation: - (mismatched type {})", t.name()));
                    }
                    t
                }
            }
        }
        Expr::Binary(op, l, r) => {
            let lt = check(l, env, locals, regexes)?;
            let rt = check(r, env, locals, regexes)?;
            let mismatch = || format!("invalid operation: {op} (mismatched types {} and {})", lt.name(), rt.name());
            let any = lt == Type::Any || rt == Type::Any;
            match op {
                BinaryOp::Or | BinaryOp::And => {
                    if !(matches!(lt, Type::Bool | Type::Any) && matches!(rt, Type::Bool | Type::Any)) {
                        return Err(mismatch());
                    }
                    Type::Bool
                }
                BinaryOp::Eq | BinaryOp::Ne => Type::Bool,
                BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => {
                    let ok = any || (lt.is_numeric() && rt.is_numeric()) || (lt == Type::String && rt == Type::String);
                    if !ok {
                        return Err(mismatch());
                    }
                    Type::Bool
                }
                BinaryOp::Matches | BinaryOp::Contains | BinaryOp::StartsWith | BinaryOp::EndsWith => {
                    if !(matches!(lt, Type::String | Type::Any) && matches!(rt, Type::String | Type::Any)) {
                        return Err(mismatch());
                    }
                    if *op == BinaryOp::Matches
                        && let Expr::String(pattern) = r.as_ref()
                    {
                        regexes.get(pattern)?;
                    }
                    Type::Bool
                }
                BinaryOp::In | BinaryOp::NotIn => {
                    if !matches!(rt, Type::Array | Type::Map | Type::Struct(_) | Type::Any) {
                        return Err(mismatch());
                    }
                    Type::Bool
                }
                BinaryOp::Range => {
                    if !(matches!(lt, Type::Int | Type::Any) && matches!(rt, Type::Int | Type::Any)) {
                        return Err(mismatch());
                    }
                    Type::Array
                }
                BinaryOp::Add => {
                    if lt == Type::String && rt == Type::String {
                        Type::String
                    } else if lt.is_numeric() && rt.is_numeric() {
                        if lt == Type::Int && rt == Type::Int { Type::Int } else { Type::Float }
                    } else if any {
                        Type::Any
                    } else if lt == Type::Array && rt == Type::Array {
                        Type::Array
                    } else {
                        return Err(mismatch());
                    }
                }
                BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Mod => {
                    if lt.is_numeric() && rt.is_numeric() {
                        if lt == Type::Int && rt == Type::Int { Type::Int } else { Type::Float }
                    } else if any {
                        Type::Any
                    } else {
                        return Err(mismatch());
                    }
                }
                BinaryOp::Div | BinaryOp::Pow => {
                    if !(any || (lt.is_numeric() && rt.is_numeric())) {
                        return Err(mismatch());
                    }
                    Type::Float
                }
                BinaryOp::Coalesce => Type::Any,
            }
        }
        Expr::Ternary(c, a, b) => {
            let ct = check(c, env, locals, regexes)?;
            if !matches!(ct, Type::Bool | Type::Any) {
                return Err(format!("non-bool expression (type {}) used as condition", ct.name()));
            }
            let at = check(a, env, locals, regexes)?;
            let bt = check(b, env, locals, regexes)?;
            if at == bt { at } else { Type::Any }
        }
        Expr::Let(name, value, body) => {
            check(value, env, locals, regexes)?;
            locals.push(name.clone());
            let t = check(body, env, locals, regexes);
            locals.pop();
            t?
        }
    })
}

/// Evaluation context.
pub(crate) struct Context<'a> {
    pub vars: &'a BTreeMap<String, Value>,
    pub locals: Vec<(String, Value)>,
    pub current: Vec<Value>,
    pub host: &'a mut dyn Host,
    pub regexes: &'a RegexCache,
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => num(a) == num(b),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equal(a, b)),
        (a, b) => a == b,
    }
}

fn compare(op: BinaryOp, a: &Value, b: &Value) -> Result<bool, String> {
    let ord = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).ok_or("cannot compare NaN")?,
            _ => {
                return Err(format!(
                    "invalid operation: {op} (mismatched types {} and {})",
                    a.type_name(),
                    b.type_name()
                ));
            }
        },
    };
    Ok(match op {
        BinaryOp::Lt => ord.is_lt(),
        BinaryOp::Gt => ord.is_gt(),
        BinaryOp::Le => ord.is_le(),
        _ => ord.is_ge(),
    })
}

fn arith(op: BinaryOp, a: Value, b: Value) -> Result<Value, String> {
    let mismatch = || format!("invalid operation: {op} (mismatched types {} and {})", a.type_name(), b.type_name());
    match (op, &a, &b) {
        (BinaryOp::Add, Value::String(x), Value::String(y)) => return Ok(Value::String(format!("{x}{y}"))),
        (BinaryOp::Add, Value::Array(x), Value::Array(y)) => {
            return Ok(Value::Array(x.iter().chain(y).cloned().collect()));
        }
        (_, Value::Int(x), Value::Int(y)) => {
            let (x, y) = (*x, *y);
            return match op {
                BinaryOp::Add => Ok(Value::Int(x.wrapping_add(y))),
                BinaryOp::Sub => Ok(Value::Int(x.wrapping_sub(y))),
                BinaryOp::Mul => Ok(Value::Int(x.wrapping_mul(y))),
                BinaryOp::Mod => {
                    if y == 0 {
                        Err("integer divide by zero".into())
                    } else {
                        Ok(Value::Int(x.wrapping_rem(y)))
                    }
                }
                BinaryOp::Div => Ok(Value::Float(x as f64 / y as f64)),
                _ => Ok(Value::Float((x as f64).powf(y as f64))),
            };
        }
        _ => {}
    }
    let (x, y) = (num(&a).ok_or_else(mismatch)?, num(&b).ok_or_else(mismatch)?);
    Ok(Value::Float(match op {
        BinaryOp::Add => x + y,
        BinaryOp::Sub => x - y,
        BinaryOp::Mul => x * y,
        BinaryOp::Div => x / y,
        BinaryOp::Mod => x % y,
        _ => x.powf(y),
    }))
}

impl Context<'_> {
    pub(crate) fn eval(&mut self, e: &Expr) -> Result<Value, String> {
        Ok(match e {
            Expr::Nil => Value::Nil,
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Int(v) => Value::Int(*v),
            Expr::Float(v) => Value::Float(*v),
            Expr::String(s) => Value::String(s.clone()),
            Expr::Array(items) => Value::Array(items.iter().map(|i| self.eval(i)).collect::<Result<_, _>>()?),
            Expr::Map(items) => {
                let mut m = BTreeMap::new();
                for (k, v) in items {
                    let k = match self.eval(k)? {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    m.insert(k, self.eval(v)?);
                }
                Value::Map(m)
            }
            Expr::Ident(name) => {
                if name == "#" {
                    return self.current.last().cloned().ok_or_else(|| "no current element".to_string());
                }
                if let Some((_, v)) = self.locals.iter().rev().find(|(n, _)| n == name) {
                    return Ok(v.clone());
                }
                self.vars.get(name).cloned().unwrap_or(Value::Nil)
            }
            Expr::Member(obj, key, optional) => {
                let o = self.eval(obj)?;
                let k = self.eval(key)?;
                match (&o, &k) {
                    (Value::Nil, _) if *optional => Value::Nil,
                    (Value::Nil, _) => return Err("cannot fetch from nil".into()),
                    (Value::Map(m), Value::String(k)) => m.get(k).cloned().unwrap_or(Value::Nil),
                    (Value::Array(a), Value::Int(i)) => {
                        let len = a.len() as i64;
                        let idx = if *i < 0 { len + i } else { *i };
                        if idx < 0 || idx >= len {
                            return Err(format!("index out of range: {i} (array length is {len})"));
                        }
                        a[idx as usize].clone()
                    }
                    (Value::String(s), Value::Int(i)) => {
                        let chars: Vec<char> = s.chars().collect();
                        let len = chars.len() as i64;
                        let idx = if *i < 0 { len + i } else { *i };
                        if idx < 0 || idx >= len {
                            return Err(format!("index out of range: {i} (string length is {len})"));
                        }
                        Value::String(chars[idx as usize].to_string())
                    }
                    _ => return Err(format!("cannot fetch {k} from {}", o.type_name())),
                }
            }
            Expr::Slice(obj, a, b) => {
                let o = self.eval(obj)?;
                let len = match &o {
                    Value::Array(v) => v.len(),
                    Value::String(s) => s.chars().count(),
                    _ => return Err(format!("cannot slice {}", o.type_name())),
                } as i64;
                let bound = |ctx: &mut Self, x: &Option<Box<Expr>>, default: i64| -> Result<i64, String> {
                    match x {
                        None => Ok(default),
                        Some(e) => match ctx.eval(e)? {
                            Value::Int(i) => Ok((if i < 0 { len + i } else { i }).clamp(0, len)),
                            v => Err(format!("invalid slice index type {}", v.type_name())),
                        },
                    }
                };
                let from = bound(self, a, 0)? as usize;
                let to = (bound(self, b, len)? as usize).max(from);
                match o {
                    Value::Array(v) => Value::Array(v[from..to].to_vec()),
                    Value::String(s) => Value::String(s.chars().skip(from).take(to - from).collect()),
                    _ => unreachable!(),
                }
            }
            Expr::Closure(_) => return Err("unexpected predicate".into()),
            Expr::Call(name, args) => self.call(name, args)?,
            Expr::Unary(op, inner) => {
                let v = self.eval(inner)?;
                match (op, v) {
                    (UnaryOp::Not, v) => Value::Bool(!v.truthy()?),
                    (UnaryOp::Neg, Value::Int(i)) => Value::Int(-i),
                    (UnaryOp::Neg, Value::Float(f)) => Value::Float(-f),
                    (UnaryOp::Plus, v @ (Value::Int(_) | Value::Float(_))) => v,
                    (_, v) => return Err(format!("invalid operation: - (mismatched type {})", v.type_name())),
                }
            }
            Expr::Binary(op, l, r) => self.binary(*op, l, r)?,
            Expr::Ternary(c, a, b) => {
                if self.eval(c)?.truthy()? {
                    self.eval(a)?
                } else {
                    self.eval(b)?
                }
            }
            Expr::Let(name, value, body) => {
                let v = self.eval(value)?;
                self.locals.push((name.clone(), v));
                let r = self.eval(body);
                self.locals.pop();
                r?
            }
        })
    }

    fn binary(&mut self, op: BinaryOp, l: &Expr, r: &Expr) -> Result<Value, String> {
        match op {
            BinaryOp::And => {
                if !self.eval(l)?.truthy()? {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(self.eval(r)?.truthy()?));
            }
            BinaryOp::Or => {
                if self.eval(l)?.truthy()? {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(self.eval(r)?.truthy()?));
            }
            BinaryOp::Coalesce => {
                let v = self.eval(l)?;
                return if v == Value::Nil { self.eval(r) } else { Ok(v) };
            }
            _ => {}
        }
        let a = self.eval(l)?;
        let b = self.eval(r)?;
        let str_op = |f: fn(&str, &str) -> bool| -> Result<Value, String> {
            match (&a, &b) {
                (Value::String(x), Value::String(y)) => Ok(Value::Bool(f(x, y))),
                _ => Err(format!("invalid operation: {op} (mismatched types {} and {})", a.type_name(), b.type_name())),
            }
        };
        Ok(match op {
            BinaryOp::Eq => Value::Bool(equal(&a, &b)),
            BinaryOp::Ne => Value::Bool(!equal(&a, &b)),
            BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => Value::Bool(compare(op, &a, &b)?),
            BinaryOp::Contains => str_op(|x, y| x.contains(y))?,
            BinaryOp::StartsWith => str_op(|x, y| x.starts_with(y))?,
            BinaryOp::EndsWith => str_op(|x, y| x.ends_with(y))?,
            BinaryOp::Matches => match (&a, &b) {
                (Value::String(x), Value::String(pattern)) => Value::Bool(self.regexes.get(pattern)?.is_match(x)),
                _ => {
                    return Err(format!(
                        "invalid operation: matches (mismatched types {} and {})",
                        a.type_name(),
                        b.type_name()
                    ));
                }
            },
            BinaryOp::In | BinaryOp::NotIn => {
                let found = match &b {
                    Value::Array(items) => items.iter().any(|x| equal(&a, x)),
                    Value::Map(m) => match &a {
                        Value::String(k) => m.contains_key(k),
                        other => m.contains_key(&other.to_string()),
                    },
                    Value::Nil => false,
                    other => return Err(format!("operator \"in\" not defined on {}", other.type_name())),
                };
                Value::Bool(if op == BinaryOp::In { found } else { !found })
            }
            BinaryOp::Range => match (&a, &b) {
                (Value::Int(x), Value::Int(y)) => {
                    if y - x > 1_000_000 {
                        return Err("range too large".into());
                    }
                    Value::Array((*x..=*y).map(Value::Int).collect())
                }
                _ => return Err("invalid operation: .. (expected ints)".into()),
            },
            _ => arith(op, a, b)?,
        })
    }

    fn predicate(
        &mut self,
        items: &[Value],
        body: &Expr,
        mut f: impl FnMut(usize, &Value, &Value) -> Option<Value>,
    ) -> Result<Option<Value>, String> {
        for (i, item) in items.iter().enumerate() {
            self.current.push(item.clone());
            let r = self.eval(body);
            self.current.pop();
            if let Some(v) = f(i, item, &r?) {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<Value, String> {
        if PREDICATES.contains(&name) {
            if args.len() != 2 {
                return Err(format!("{name} expects 2 arguments"));
            }
            let items = match self.eval(&args[0])? {
                Value::Array(a) => a,
                Value::Nil => vec![],
                v => return Err(format!("{name} expects an array, got {}", v.type_name())),
            };
            let body = match &args[1] {
                Expr::Closure(b) => b.as_ref(),
                other => other,
            };
            let truthy = |v: &Value| v.truthy().unwrap_or(false);
            return Ok(match name {
                "all" => {
                    Value::Bool(self.predicate(&items, body, |_, _, r| (!truthy(r)).then_some(Value::Nil))?.is_none())
                }
                "any" => {
                    Value::Bool(self.predicate(&items, body, |_, _, r| truthy(r).then_some(Value::Nil))?.is_some())
                }
                "none" => {
                    Value::Bool(self.predicate(&items, body, |_, _, r| truthy(r).then_some(Value::Nil))?.is_none())
                }
                "one" | "count" => {
                    let mut n = 0;
                    self.predicate(&items, body, |_, _, r| {
                        if truthy(r) {
                            n += 1;
                        }
                        None
                    })?;
                    if name == "one" { Value::Bool(n == 1) } else { Value::Int(n) }
                }
                "filter" => {
                    let mut out = vec![];
                    self.predicate(&items, body, |_, item, r| {
                        if truthy(r) {
                            out.push(item.clone());
                        }
                        None
                    })?;
                    Value::Array(out)
                }
                "map" => {
                    let mut out = vec![];
                    self.predicate(&items, body, |_, _, r| {
                        out.push(r.clone());
                        None
                    })?;
                    Value::Array(out)
                }
                "find" => {
                    self.predicate(&items, body, |_, item, r| truthy(r).then(|| item.clone()))?.unwrap_or(Value::Nil)
                }
                "findIndex" => self
                    .predicate(&items, body, |i, _, r| truthy(r).then_some(Value::Int(i as i64)))?
                    .unwrap_or(Value::Int(-1)),
                "findLast" | "findLastIndex" => {
                    let mut found = None;
                    self.predicate(&items, body, |i, item, r| {
                        if truthy(r) {
                            found = Some(if name == "findLast" { item.clone() } else { Value::Int(i as i64) });
                        }
                        None
                    })?;
                    found.unwrap_or(if name == "findLast" { Value::Nil } else { Value::Int(-1) })
                }
                _ => unreachable!(),
            });
        }
        let values: Vec<Value> = args.iter().map(|a| self.eval(a)).collect::<Result<_, _>>()?;
        if !BUILTINS.contains(&name) {
            return self.host.call(name, values);
        }
        crate::builtins::call(name, values)
    }
}
