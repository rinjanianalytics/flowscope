// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A PEG interpreter behaving like the parsers pigeon generates: same
//! backtracking, same farthest failure tracking for the "expected" list,
//! same error recovery. The grammar is written as data (`grammar.rs`), so
//! the filter language gives the same results and the same errors as the Go
//! version, which the completion relies on.

use std::collections::HashMap;
use std::rc::Rc;

/// Value of a parsed expression.
#[derive(Clone, Debug, Default)]
pub enum Value<T> {
    #[default]
    Nil,
    Text(String),
    List(Vec<Value<T>>),
    /// A value built by an action.
    Custom(T),
}

impl<T> Value<T> {
    pub fn text(&self) -> &str {
        match self {
            Value::Text(t) => t,
            _ => "",
        }
    }

    pub fn list(&self) -> &[Value<T>] {
        match self {
            Value::List(l) => l,
            _ => &[],
        }
    }
}

/// Context given to the actions.
pub struct Current<'a, T, S> {
    /// Matched text.
    pub text: &'a str,
    /// Offset of the match.
    pub offset: usize,
    /// Labeled values.
    pub labels: &'a HashMap<&'static str, Value<T>>,
    /// Global state.
    pub state: &'a mut S,
}

impl<T, S> Current<'_, T, S> {
    pub fn label(&self, name: &str) -> Option<&Value<T>> {
        self.labels.get(name)
    }
}

pub type ActionFn<T, S> = Rc<dyn Fn(&mut Current<T, S>) -> Result<Value<T>, String>>;
pub type PredicateFn<T, S> = Rc<dyn Fn(&mut Current<T, S>) -> Result<bool, String>>;

/// A parsing expression.
pub enum Pe<T, S> {
    /// A literal, compared case-insensitively when asked (it is then
    /// lowercase).
    Lit {
        val: Vec<char>,
        ignore_case: bool,
        want: String,
    },
    /// A character class.
    Class {
        chars: Vec<char>,
        ranges: Vec<(char, char)>,
        display: String,
    },
    Any,
    Seq(Vec<Pe<T, S>>),
    Choice(Vec<Pe<T, S>>),
    ZeroOrMore(Box<Pe<T, S>>),
    OneOrMore(Box<Pe<T, S>>),
    Optional(Box<Pe<T, S>>),
    Not(Box<Pe<T, S>>),
    Rule(&'static str),
    Labeled(&'static str, Box<Pe<T, S>>),
    Action(Box<Pe<T, S>>, ActionFn<T, S>),
    AndCode(PredicateFn<T, S>),
    /// Code run for its side effects (an error), always matching.
    StateCode(ActionFn<T, S>),
    /// Try an expression, with a recovery expression for the labels thrown.
    Recovery {
        expr: Box<Pe<T, S>>,
        labels: Vec<&'static str>,
        recover: Box<Pe<T, S>>,
    },
    Throw(&'static str),
}

/// A literal: `"="` or `"IN"i`.
pub fn lit<T, S>(s: &str) -> Pe<T, S> {
    Pe::Lit { val: s.chars().collect(), ignore_case: false, want: go_quote(s) }
}

pub fn ilit<T, S>(s: &str) -> Pe<T, S> {
    Pe::Lit { val: s.to_lowercase().chars().collect(), ignore_case: true, want: format!("{}i", go_quote(s)) }
}

/// Quote like Go's strconv.Quote, for the ASCII literals of the grammar.
fn go_quote(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A character class, given as in the grammar: `[A-Za-z0-9]`.
pub fn class<T, S>(display: &str) -> Pe<T, S> {
    let inner: Vec<char> = display[1..display.len() - 1].chars().collect();
    let mut chars = vec![];
    let mut ranges = vec![];
    let mut i = 0;
    let unescape = |c: char| match c {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        c => c,
    };
    while i < inner.len() {
        let mut c = inner[i];
        if c == '\\' && i + 1 < inner.len() {
            i += 1;
            c = unescape(inner[i]);
        }
        if i + 2 < inner.len() && inner[i + 1] == '-' {
            ranges.push((c, inner[i + 2]));
            i += 3;
        } else {
            chars.push(c);
            i += 1;
        }
    }
    Pe::Class { chars, ranges, display: display.into() }
}

pub fn seq<T, S>(items: Vec<Pe<T, S>>) -> Pe<T, S> {
    Pe::Seq(items)
}

pub fn choice<T, S>(items: Vec<Pe<T, S>>) -> Pe<T, S> {
    Pe::Choice(items)
}

pub fn rule<T, S>(name: &'static str) -> Pe<T, S> {
    Pe::Rule(name)
}

pub fn label<T, S>(name: &'static str, e: Pe<T, S>) -> Pe<T, S> {
    Pe::Labeled(name, Box::new(e))
}

pub fn star<T, S>(e: Pe<T, S>) -> Pe<T, S> {
    Pe::ZeroOrMore(Box::new(e))
}

pub fn plus<T, S>(e: Pe<T, S>) -> Pe<T, S> {
    Pe::OneOrMore(Box::new(e))
}

pub fn opt<T, S>(e: Pe<T, S>) -> Pe<T, S> {
    Pe::Optional(Box::new(e))
}

pub fn not<T, S>(e: Pe<T, S>) -> Pe<T, S> {
    Pe::Not(Box::new(e))
}

pub fn action<T, S>(e: Pe<T, S>, f: impl Fn(&mut Current<T, S>) -> Result<Value<T>, String> + 'static) -> Pe<T, S> {
    Pe::Action(Box::new(e), Rc::new(f))
}

pub fn pred<T, S>(f: impl Fn(&mut Current<T, S>) -> Result<bool, String> + 'static) -> Pe<T, S> {
    Pe::AndCode(Rc::new(f))
}

/// A named rule, with an optional display name.
pub struct Rule<T, S> {
    pub name: &'static str,
    pub expr: Pe<T, S>,
}

/// A parse error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub offset: usize,
    pub expected: Vec<String>,
}

/// The grammar.
pub struct Grammar<T, S> {
    rules: HashMap<&'static str, Pe<T, S>>,
}

impl<T: Clone, S> Grammar<T, S> {
    pub fn new(rules: Vec<Rule<T, S>>) -> Self {
        Grammar { rules: rules.into_iter().map(|r| (r.name, r.expr)).collect() }
    }

    /// Parse an input from a rule. Returns the value and the errors (an
    /// empty list on success).
    pub fn parse(&self, entry: &str, input: &str, state: &mut S) -> (Option<Value<T>>, Vec<ParseError>) {
        let mut p = Parser {
            g: self,
            input,
            chars: input.char_indices().collect(),
            pos: 0,
            max_fail: 0,
            max_fail_expected: vec![],
            invert: false,
            errors: vec![],
            recovery: vec![],
            labels: vec![HashMap::new()],
            rules: vec![],
            error_rules: vec![],
            too_deep: false,
            state,
        };
        let Some(start) = self.rules.get(entry) else {
            return (None, vec![p.error_at("invalid entrypoint".into(), 0, vec![])]);
        };
        let result = p.expr(start);
        if result.is_none() && p.errors.is_empty() {
            let mut expected: Vec<String> = p.max_fail_expected.clone();
            expected.sort();
            expected.dedup();
            let eof = expected.iter().any(|e| e == "!.");
            expected.retain(|e| e != "!.");
            if eof {
                expected.push("EOF".into());
            }
            let message = format!("no match found, expected: {}", list_join(&expected));
            let offset = p.offset_of(p.max_fail);
            let err = p.error_at(message, offset, expected);
            p.push_error(err);
        }
        // Like pigeon, drop the duplicate errors (same rule, position and
        // message).
        let mut seen = std::collections::HashSet::new();
        let errors = p
            .errors
            .into_iter()
            .zip(p.error_rules)
            .filter(|(e, r)| seen.insert((*r, e.offset, e.message.clone())))
            .map(|(e, _)| e)
            .collect();
        (result, errors)
    }
}

fn list_join(list: &[String]) -> String {
    match list.len() {
        0 => String::new(),
        1 => list[0].clone(),
        n => format!("{} or {}", list[..n - 1].join(", "), list[n - 1]),
    }
}

/// Rules nested at most. It allows about 45 levels of parentheses in a
/// filter.
const MAX_RULE_DEPTH: usize = 250;

struct Parser<'g, 'i, 's, T, S> {
    g: &'g Grammar<T, S>,
    input: &'i str,
    /// Characters with their byte offset.
    chars: Vec<(usize, char)>,
    /// Position, as an index in `chars`.
    pos: usize,
    max_fail: usize,
    max_fail_expected: Vec<String>,
    invert: bool,
    errors: Vec<ParseError>,
    recovery: Vec<(&'g [&'static str], &'g Pe<T, S>)>,
    labels: Vec<HashMap<&'static str, Value<T>>>,
    /// Rules being parsed, the innermost last.
    rules: Vec<&'static str>,
    /// Rule where each error happened, to dedupe them like pigeon.
    error_rules: Vec<Option<&'static str>>,
    /// Set when the input nests rules deeper than `MAX_RULE_DEPTH`.
    too_deep: bool,
    state: &'s mut S,
}

impl<'g, T: Clone, S> Parser<'g, '_, '_, T, S> {
    fn offset_of(&self, pos: usize) -> usize {
        self.chars.get(pos).map(|(o, _)| *o).unwrap_or(self.input.len())
    }

    fn current(&self) -> Option<char> {
        self.chars.get(self.pos).map(|(_, c)| *c)
    }

    /// Line and column like pigeon: a newline belongs to the next line, at
    /// column 0.
    fn error_at(&self, message: String, offset: usize, expected: Vec<String>) -> ParseError {
        let (mut line, mut col) = (1, 0);
        for (o, c) in &self.chars {
            if *o > offset {
                break;
            }
            col += 1;
            if *c == '\n' {
                line += 1;
                col = 0;
            }
        }
        if offset >= self.input.len() {
            // Reading at the end still counts a column.
            col += 1;
        }
        ParseError { message, line, column: col, offset, expected }
    }

    fn push_error(&mut self, err: ParseError) {
        self.errors.push(err);
        self.error_rules.push(self.rules.last().copied());
    }

    fn fail_at(&mut self, fail: bool, pos: usize, want: &str) {
        if fail != self.invert {
            return;
        }
        if pos < self.max_fail {
            return;
        }
        if pos > self.max_fail {
            self.max_fail = pos;
            self.max_fail_expected.clear();
        }
        let want = if self.invert { format!("!{want}") } else { want.to_string() };
        self.max_fail_expected.push(want);
    }

    fn text(&self, start: usize) -> &str {
        &self.input[self.offset_of(start)..self.offset_of(self.pos)]
    }

    fn expr(&mut self, e: &'g Pe<T, S>) -> Option<Value<T>> {
        match e {
            Pe::Lit { val, ignore_case, want } => {
                let start = self.pos;
                for want_c in val {
                    let cur =
                        self.current().map(|c| if *ignore_case { c.to_lowercase().next().unwrap_or(c) } else { c });
                    if cur != Some(*want_c) {
                        self.fail_at(false, start, want);
                        self.pos = start;
                        return None;
                    }
                    self.pos += 1;
                }
                self.fail_at(true, start, want);
                Some(Value::Text(self.text(start).to_string()))
            }
            Pe::Class { chars, ranges, display } => {
                let start = self.pos;
                let Some(c) = self.current() else {
                    self.fail_at(false, start, display);
                    return None;
                };
                if chars.contains(&c) || ranges.iter().any(|(a, b)| *a <= c && c <= *b) {
                    self.pos += 1;
                    self.fail_at(true, start, display);
                    Some(Value::Text(self.text(start).to_string()))
                } else {
                    self.fail_at(false, start, display);
                    None
                }
            }
            Pe::Any => {
                let start = self.pos;
                if self.current().is_none() {
                    self.fail_at(false, start, ".");
                    return None;
                }
                self.pos += 1;
                self.fail_at(true, start, ".");
                Some(Value::Text(self.text(start).to_string()))
            }
            Pe::Seq(items) => {
                let start = self.pos;
                let mut vals = Vec::with_capacity(items.len());
                for item in items {
                    match self.expr(item) {
                        Some(v) => vals.push(v),
                        None => {
                            self.pos = start;
                            return None;
                        }
                    }
                }
                Some(Value::List(vals))
            }
            Pe::Choice(alts) => {
                for alt in alts {
                    self.labels.push(HashMap::new());
                    let v = self.expr(alt);
                    self.labels.pop();
                    if v.is_some() {
                        return v;
                    }
                }
                None
            }
            Pe::ZeroOrMore(inner) => {
                let mut vals = vec![];
                loop {
                    self.labels.push(HashMap::new());
                    let v = self.expr(inner);
                    self.labels.pop();
                    match v {
                        Some(v) => vals.push(v),
                        None => return Some(Value::List(vals)),
                    }
                }
            }
            Pe::OneOrMore(inner) => {
                let mut vals = vec![];
                loop {
                    self.labels.push(HashMap::new());
                    let v = self.expr(inner);
                    self.labels.pop();
                    match v {
                        Some(v) => vals.push(v),
                        None if vals.is_empty() => return None,
                        None => return Some(Value::List(vals)),
                    }
                }
            }
            Pe::Optional(inner) => {
                self.labels.push(HashMap::new());
                let v = self.expr(inner);
                self.labels.pop();
                Some(v.unwrap_or(Value::Nil))
            }
            Pe::Not(inner) => {
                let start = self.pos;
                self.labels.push(HashMap::new());
                self.invert = !self.invert;
                let v = self.expr(inner);
                self.invert = !self.invert;
                self.labels.pop();
                self.pos = start;
                if v.is_some() { None } else { Some(Value::Nil) }
            }
            Pe::Rule(name) => {
                let rule = self.g.rules.get(name).unwrap_or_else(|| panic!("undefined rule {name}"));
                // The parser recurses for each rule. Without a limit, an
                // input like a long run of "(" overflows the stack, which
                // aborts the process.
                if self.too_deep {
                    return None;
                }
                if self.rules.len() >= MAX_RULE_DEPTH {
                    self.too_deep = true;
                    let offset = self.offset_of(self.pos);
                    let err = self.error_at("expression nested too deeply".into(), offset, vec![]);
                    self.push_error(err);
                    return None;
                }
                self.labels.push(HashMap::new());
                self.rules.push(name);
                // The frames are large, more so in a debug build: get more
                // stack when the current one is nearly used.
                let v = stacker::maybe_grow(256 * 1024, 4 * 1024 * 1024, || self.expr(rule));
                self.rules.pop();
                self.labels.pop();
                v
            }
            Pe::Labeled(name, inner) => {
                self.labels.push(HashMap::new());
                let v = self.expr(inner);
                self.labels.pop();
                if let Some(v) = &v {
                    self.labels.last_mut().unwrap().insert(name, v.clone());
                }
                v
            }
            Pe::Action(inner, f) => {
                let start = self.pos;
                self.labels.push(HashMap::new());
                let v = self.expr(inner);
                let labels = self.labels.pop().unwrap();
                v.as_ref()?;
                let offset = self.offset_of(start);
                let text = self.text(start).to_string();
                let mut cur = Current { text: &text, offset, labels: &labels, state: self.state };
                match f(&mut cur) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        let err = self.error_at(e, offset, vec![]);
                        self.push_error(err);
                        Some(Value::Nil)
                    }
                }
            }
            Pe::AndCode(f) => {
                let offset = self.offset_of(self.pos);
                let labels = self.labels.last().cloned().unwrap_or_default();
                let mut cur = Current { text: "", offset, labels: &labels, state: self.state };
                match f(&mut cur) {
                    Ok(true) => Some(Value::Nil),
                    Ok(false) => None,
                    Err(e) => {
                        let err = self.error_at(e, offset, vec![]);
                        self.push_error(err);
                        None
                    }
                }
            }
            Pe::StateCode(f) => {
                let offset = self.offset_of(self.pos);
                let labels = HashMap::new();
                let mut cur = Current { text: "", offset, labels: &labels, state: self.state };
                if let Err(e) = f(&mut cur) {
                    let err = self.error_at(e, offset, vec![]);
                    self.push_error(err);
                }
                Some(Value::Nil)
            }
            Pe::Recovery { expr, labels, recover } => {
                self.recovery.push((labels.as_slice(), recover));
                let v = self.expr(expr);
                self.recovery.pop();
                v
            }
            Pe::Throw(label) => {
                let candidates: Vec<&'g Pe<T, S>> =
                    self.recovery.iter().rev().filter(|(labels, _)| labels.contains(label)).map(|(_, r)| *r).collect();
                for r in candidates {
                    if let Some(v) = self.expr(r) {
                        return Some(v);
                    }
                }
                None
            }
        }
    }
}
