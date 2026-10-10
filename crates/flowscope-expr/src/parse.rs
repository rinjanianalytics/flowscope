// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Lexer and parser for the expr language (https://expr-lang.org), as used
//! by the classifiers.

use std::fmt;

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    /// An identifier, `#` (current element in a predicate) is "#".
    Ident(String),
    /// Member access. The bool is true for `?.`.
    Member(Box<Expr>, Box<Expr>, bool),
    /// Slice `a[from:to]`.
    Slice(Box<Expr>, Option<Box<Expr>>, Option<Box<Expr>>),
    Call(String, Vec<Expr>),
    /// A predicate (closure) argument: `{ # > 1 }`.
    Closure(Box<Expr>),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `let x = value; body`.
    Let(String, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
    Plus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    In,
    NotIn,
    Matches,
    Contains,
    StartsWith,
    EndsWith,
    Range,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Coalesce,
}

impl BinaryOp {
    fn precedence(self) -> u16 {
        match self {
            BinaryOp::Or => 10,
            BinaryOp::And => 15,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge
            | BinaryOp::In
            | BinaryOp::NotIn
            | BinaryOp::Matches
            | BinaryOp::Contains
            | BinaryOp::StartsWith
            | BinaryOp::EndsWith => 20,
            BinaryOp::Range => 25,
            BinaryOp::Add | BinaryOp::Sub => 30,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 60,
            BinaryOp::Pow => 100,
            BinaryOp::Coalesce => 500,
        }
    }

    fn right_associative(self) -> bool {
        self == BinaryOp::Pow
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BinaryOp::Or => "||",
            BinaryOp::And => "&&",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Gt => ">",
            BinaryOp::Le => "<=",
            BinaryOp::Ge => ">=",
            BinaryOp::In => "in",
            BinaryOp::NotIn => "not in",
            BinaryOp::Matches => "matches",
            BinaryOp::Contains => "contains",
            BinaryOp::StartsWith => "startsWith",
            BinaryOp::EndsWith => "endsWith",
            BinaryOp::Range => "..",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "**",
            BinaryOp::Coalesce => "??",
        };
        f.write_str(s)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
    Eof,
}

/// Parse error with the position (in characters).
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at position {})", self.message, self.position)
    }
}

const OPERATORS: &[&str] = &[
    "...", "?.", "??", "**", "==", "!=", "<=", ">=", "&&", "||", "..", "+", "-", "*", "/", "%", "^", "<", ">", "!",
    "(", ")", "[", "]", "{", "}", ",", ":", "?", ".", "#", "|", ";", "=",
];

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, ParseError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let err = |message: String, position: usize| ParseError { message, position };
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        let start = i;
        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let mut s = String::new();
            let mut is_float = false;
            if c == '0' && i + 1 < chars.len() && matches!(chars[i + 1], 'x' | 'X' | 'o' | 'O' | 'b' | 'B') {
                let radix = match chars[i + 1] {
                    'x' | 'X' => 16,
                    'o' | 'O' => 8,
                    _ => 2,
                };
                i += 2;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    if chars[i] != '_' {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                let v = i64::from_str_radix(&s, radix).map_err(|_| err(format!("invalid number {s:?}"), start))?;
                out.push((Tok::Int(v), start));
                continue;
            }
            while i < chars.len() {
                let d = chars[i];
                if d.is_ascii_digit() || d == '_' {
                    if d != '_' {
                        s.push(d);
                    }
                    i += 1;
                } else if d == '.' && i + 1 < chars.len() && chars[i + 1] == '.' {
                    break; // range operator
                } else if d == '.' && !is_float {
                    is_float = true;
                    s.push(d);
                    i += 1;
                } else if (d == 'e' || d == 'E') && i + 1 < chars.len() {
                    is_float = true;
                    s.push(d);
                    i += 1;
                    if chars[i] == '+' || chars[i] == '-' {
                        s.push(chars[i]);
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if is_float {
                out.push((Tok::Float(s.parse().map_err(|_| err(format!("invalid number {s:?}"), start))?), start));
            } else {
                out.push((Tok::Int(s.parse().map_err(|_| err(format!("invalid number {s:?}"), start))?), start));
            }
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            loop {
                if i >= chars.len() {
                    return Err(err("literal not terminated".into(), start));
                }
                let d = chars[i];
                i += 1;
                if d == quote {
                    break;
                }
                if d == '\\' {
                    if i >= chars.len() {
                        return Err(err("literal not terminated".into(), start));
                    }
                    let e = chars[i];
                    i += 1;
                    match e {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        'a' => s.push('\x07'),
                        'b' => s.push('\x08'),
                        'f' => s.push('\x0c'),
                        'v' => s.push('\x0b'),
                        '0'..='7' => {
                            let mut v = e.to_digit(8).unwrap();
                            for _ in 0..2 {
                                if i < chars.len() && chars[i].is_digit(8) {
                                    v = v * 8 + chars[i].to_digit(8).unwrap();
                                    i += 1;
                                }
                            }
                            s.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                        }
                        'x' | 'u' | 'U' => {
                            let n = match e {
                                'x' => 2,
                                'u' => 4,
                                _ => 8,
                            };
                            let hex: String = chars[i..(i + n).min(chars.len())].iter().collect();
                            i += n;
                            let v = u32::from_str_radix(&hex, 16).map_err(|_| err("invalid escape".into(), i))?;
                            s.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                        }
                        other => {
                            if other != '\\' && other != '"' && other != '\'' {
                                return Err(err(format!("invalid char escape \\{other}"), i - 1));
                            }
                            s.push(other)
                        }
                    }
                } else {
                    s.push(d);
                }
            }
            out.push((Tok::Str(s), start));
            continue;
        }
        if c == '`' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '`' {
                s.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() {
                return Err(err("literal not terminated".into(), start));
            }
            i += 1;
            out.push((Tok::Str(s), start));
            continue;
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            let mut s = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                s.push(chars[i]);
                i += 1;
            }
            out.push((Tok::Ident(s), start));
            continue;
        }
        let rest: String = chars[i..(i + 3).min(chars.len())].iter().collect();
        match OPERATORS.iter().find(|op| rest.starts_with(**op)) {
            Some(op) => {
                i += op.chars().count();
                out.push((Tok::Op(op), start));
            }
            None => return Err(err(format!("unrecognized character: {c:?}"), start)),
        }
    }
    out.push((Tok::Eof, chars.len()));
    Ok(out)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    /// Depth of predicates, to accept `#`.
    closure_depth: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn position(&self) -> usize {
        self.toks[self.pos].1
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError { message: message.into(), position: self.position() })
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn is_ident(&self, name: &str) -> bool {
        matches!(self.peek(), Tok::Ident(i) if i == name)
    }

    fn expect_op(&mut self, op: &str) -> Result<(), ParseError> {
        if self.is_op(op) {
            self.next();
            Ok(())
        } else {
            let found = match self.peek() {
                Tok::Eof => "EOF".to_string(),
                t => format!("{t:?}"),
            };
            self.error(format!("unexpected token {found}, expected {op:?}"))
        }
    }

    fn binary_op(&self) -> Option<(BinaryOp, usize)> {
        let op = match self.peek() {
            Tok::Op("||") => BinaryOp::Or,
            Tok::Op("&&") => BinaryOp::And,
            Tok::Op("==") => BinaryOp::Eq,
            Tok::Op("!=") => BinaryOp::Ne,
            Tok::Op("<") => BinaryOp::Lt,
            Tok::Op(">") => BinaryOp::Gt,
            Tok::Op("<=") => BinaryOp::Le,
            Tok::Op(">=") => BinaryOp::Ge,
            Tok::Op("..") => BinaryOp::Range,
            Tok::Op("+") => BinaryOp::Add,
            Tok::Op("-") => BinaryOp::Sub,
            Tok::Op("*") => BinaryOp::Mul,
            Tok::Op("/") => BinaryOp::Div,
            Tok::Op("%") => BinaryOp::Mod,
            Tok::Op("**") | Tok::Op("^") => BinaryOp::Pow,
            Tok::Op("??") => BinaryOp::Coalesce,
            Tok::Ident(i) => match i.as_str() {
                "or" => BinaryOp::Or,
                "and" => BinaryOp::And,
                "in" => BinaryOp::In,
                "matches" => BinaryOp::Matches,
                "contains" => BinaryOp::Contains,
                "startsWith" => BinaryOp::StartsWith,
                "endsWith" => BinaryOp::EndsWith,
                "not" => {
                    // "not in"
                    if matches!(self.toks.get(self.pos + 1), Some((Tok::Ident(n), _)) if n == "in") {
                        return Some((BinaryOp::NotIn, 2));
                    }
                    return None;
                }
                _ => return None,
            },
            _ => return None,
        };
        Some((op, 1))
    }

    /// Top-level: let bindings, ternary, pipes.
    fn expression(&mut self) -> Result<Expr, ParseError> {
        if self.is_ident("let") {
            self.next();
            let name = match self.next() {
                Tok::Ident(n) => n,
                _ => return self.error("expected a variable name"),
            };
            self.expect_op("=")?;
            let value = self.expression()?;
            self.expect_op(";")?;
            let body = self.expression()?;
            return Ok(Expr::Let(name, Box::new(value), Box::new(body)));
        }
        let mut e = self.ternary()?;
        // Pipe: a | f(b) is f(a, b).
        while self.is_op("|") {
            self.next();
            let name = match self.next() {
                Tok::Ident(n) => n,
                _ => return self.error("expected a function after |"),
            };
            let mut args = vec![e];
            if self.is_op("(") {
                args.extend(self.call_arguments(&name)?);
            }
            e = Expr::Call(name, args);
        }
        Ok(e)
    }

    fn ternary(&mut self) -> Result<Expr, ParseError> {
        let cond = self.binary(0)?;
        if self.is_op("?") {
            self.next();
            let a = self.ternary()?;
            self.expect_op(":")?;
            let b = self.ternary()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(a), Box::new(b)));
        }
        Ok(cond)
    }

    fn binary(&mut self, min_prec: u16) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        while let Some((op, ntoks)) = self.binary_op() {
            let prec = op.precedence();
            if prec < min_prec {
                break;
            }
            for _ in 0..ntoks {
                self.next();
            }
            let next_min = if op.right_associative() { prec } else { prec + 1 };
            let right = self.binary(next_min)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.is_op("!") || self.is_ident("not") {
            self.next();
            // "not" has precedence 50: it binds tighter than comparisons.
            let e = self.binary(50)?;
            return Ok(Expr::Unary(UnaryOp::Not, Box::new(e)));
        }
        if self.is_op("-") {
            self.next();
            let e = self.binary(90)?;
            return Ok(Expr::Unary(UnaryOp::Neg, Box::new(e)));
        }
        if self.is_op("+") {
            self.next();
            let e = self.binary(90)?;
            return Ok(Expr::Unary(UnaryOp::Plus, Box::new(e)));
        }
        self.postfix()
    }

    fn call_arguments(&mut self, _name: &str) -> Result<Vec<Expr>, ParseError> {
        self.expect_op("(")?;
        let mut args = Vec::new();
        if self.is_op(")") {
            self.next();
            return Ok(args);
        }
        loop {
            if self.is_op("{") {
                self.next();
                self.closure_depth += 1;
                let body = self.expression()?;
                self.closure_depth -= 1;
                self.expect_op("}")?;
                args.push(Expr::Closure(Box::new(body)));
            } else {
                args.push(self.expression()?);
            }
            if self.is_op(")") {
                self.next();
                return Ok(args);
            }
            self.expect_op(",")?;
        }
    }

    fn postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.primary()?;
        loop {
            if self.is_op(".") || self.is_op("?.") {
                let optional = self.is_op("?.");
                self.next();
                let name = match self.next() {
                    Tok::Ident(n) => n,
                    _ => return self.error("expected a name after ."),
                };
                e = Expr::Member(Box::new(e), Box::new(Expr::String(name)), optional);
            } else if self.is_op("[") {
                self.next();
                if self.is_op(":") {
                    self.next();
                    let to = if self.is_op("]") { None } else { Some(Box::new(self.expression()?)) };
                    self.expect_op("]")?;
                    e = Expr::Slice(Box::new(e), None, to);
                    continue;
                }
                let index = self.expression()?;
                if self.is_op(":") {
                    self.next();
                    let to = if self.is_op("]") { None } else { Some(Box::new(self.expression()?)) };
                    self.expect_op("]")?;
                    e = Expr::Slice(Box::new(e), Some(Box::new(index)), to);
                    continue;
                }
                self.expect_op("]")?;
                e = Expr::Member(Box::new(e), Box::new(index), false);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        match self.next() {
            Tok::Int(v) => Ok(Expr::Int(v)),
            Tok::Float(v) => Ok(Expr::Float(v)),
            Tok::Str(s) => Ok(Expr::String(s)),
            Tok::Op("(") => {
                let e = self.expression()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Op("[") => {
                let mut items = Vec::new();
                while !self.is_op("]") {
                    items.push(self.expression()?);
                    if !self.is_op("]") {
                        self.expect_op(",")?;
                    }
                }
                self.next();
                Ok(Expr::Array(items))
            }
            Tok::Op("{") => {
                let mut items = Vec::new();
                while !self.is_op("}") {
                    let key = match self.next() {
                        Tok::Ident(n) => Expr::String(n),
                        Tok::Str(s) => Expr::String(s),
                        Tok::Int(v) => Expr::Int(v),
                        Tok::Op("(") => {
                            let k = self.expression()?;
                            self.expect_op(")")?;
                            k
                        }
                        _ => return self.error("invalid map key"),
                    };
                    self.expect_op(":")?;
                    items.push((key, self.expression()?));
                    if !self.is_op("}") {
                        self.expect_op(",")?;
                    }
                }
                self.next();
                Ok(Expr::Map(items))
            }
            Tok::Op("#") => {
                if self.closure_depth == 0 {
                    return self.error("cannot use # outside of a predicate");
                }
                // `#.Field` or `#`.
                Ok(Expr::Ident("#".into()))
            }
            Tok::Op(".") if self.closure_depth > 0 => {
                // `.Field` is `#.Field` inside a predicate.
                match self.next() {
                    Tok::Ident(n) => {
                        Ok(Expr::Member(Box::new(Expr::Ident("#".into())), Box::new(Expr::String(n)), false))
                    }
                    _ => self.error("expected a name after ."),
                }
            }
            Tok::Ident(name) => match name.as_str() {
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "nil" => Ok(Expr::Nil),
                _ if self.is_op("(") => {
                    let args = self.call_arguments(&name)?;
                    Ok(Expr::Call(name, args))
                }
                _ => Ok(Expr::Ident(name)),
            },
            Tok::Eof => {
                self.pos = self.toks.len() - 1;
                self.error("unexpected end of expression")
            }
            t => {
                self.pos -= 1;
                self.error(format!("unexpected token {t:?}"))
            }
        }
    }
}

/// Parse an expression.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0, closure_depth: 0 };
    let e = p.expression()?;
    if !matches!(p.peek(), Tok::Eof) {
        return p.error(format!("unexpected token {:?}", p.peek()));
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        Expr::Binary(op, Box::new(l), Box::new(r))
    }

    #[test]
    fn precedence() {
        assert_eq!(
            parse("1 + 2 * 3").unwrap(),
            b(BinaryOp::Add, Expr::Int(1), b(BinaryOp::Mul, Expr::Int(2), Expr::Int(3)))
        );
        assert_eq!(
            parse("a || b && c").unwrap(),
            b(
                BinaryOp::Or,
                Expr::Ident("a".into()),
                b(BinaryOp::And, Expr::Ident("b".into()), Expr::Ident("c".into()))
            )
        );
        assert_eq!(
            parse("2 ** 3 ** 2").unwrap(),
            b(BinaryOp::Pow, Expr::Int(2), b(BinaryOp::Pow, Expr::Int(3), Expr::Int(2)))
        );
        assert!(matches!(parse("x not in [1, 2]").unwrap(), Expr::Binary(BinaryOp::NotIn, ..)));
        assert!(matches!(parse("not a == b").unwrap(), Expr::Binary(BinaryOp::Eq, ..)));
    }

    #[test]
    fn classifier() {
        let e = parse(r#"Exporter.Name startsWith "expo" && Classify("europe")"#).unwrap();
        assert!(matches!(e, Expr::Binary(BinaryOp::And, ..)));
        let e = parse(r#"ClassifyRegex(Exporter.Name, "^(\\w+).r", "europe-$1")"#).unwrap();
        let Expr::Call(_, args) = e else { panic!() };
        assert_eq!(args[1], Expr::String(r"^(\w+).r".into()));
        assert!(parse(r#"Classify("europe""#).is_err());
        assert!(parse("filter([1, 2], {# > 1})").is_ok());
        assert!(parse("a ? b : c").is_ok());
        assert!(parse("let x = 1; x + 1").is_ok());
        assert!(parse("[1,2,3][1:]").is_ok());
        assert!(parse("1..5").is_ok());
    }
}
