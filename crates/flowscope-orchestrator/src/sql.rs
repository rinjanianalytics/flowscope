// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Just enough SQL for the migrations: a tokenizer to compare the statements
//! ClickHouse keeps about a table with the ones we would use to create it,
//! and a parser for the `engine_full` column.
//!
//! The Go version parses both statements with a ClickHouse SQL parser and
//! compares them once formatted. Comparing token streams gives the same
//! result: layout, identifier quoting and keyword case do not matter, and
//! the parentheses ClickHouse adds or removes around a function argument are
//! dropped.

use std::collections::BTreeMap;

/// A SQL token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// Identifier or keyword, uppercased when it is a known keyword.
    Word(String),
    Str(String),
    Num(String),
    Punct(&'static str),
}

const KEYWORDS: &[&str] = &[
    "ADD",
    "ALIAS",
    "ALTER",
    "AND",
    "ARRAY",
    "AS",
    "BY",
    "CASE",
    "CLUSTER",
    "CODEC",
    "CREATE",
    "CREDENTIALS",
    "DEFAULT",
    "DICTIONARY",
    "DROP",
    "ELSE",
    "END",
    "ENGINE",
    "EXISTS",
    "FORMAT",
    "FROM",
    "IF",
    "IN",
    "INDEX",
    "INJECTIVE",
    "INTERVAL",
    "IS",
    "JOIN",
    "KEY",
    "LAYOUT",
    "LIFETIME",
    "LIKE",
    "MATERIALIZED",
    "MAX",
    "MIN",
    "MODIFY",
    "NOT",
    "NULL",
    "ON",
    "OR",
    "ORDER",
    "PARTITION",
    "PASSWORD",
    "PRIMARY",
    "REPLACE",
    "SELECT",
    "SETTINGS",
    "SOURCE",
    "SYNC",
    "TABLE",
    "THEN",
    "TO",
    "TTL",
    "TYPE",
    "URL",
    "USER",
    "VIEW",
    "WHEN",
    "WITH",
];

/// Split SQL into tokens.
pub fn tokenize(sql: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let upper = word.to_ascii_uppercase();
            tokens.push(Token::Word(if KEYWORDS.contains(&upper.as_str()) { upper } else { word }));
            continue;
        }
        if c == '`' || c == '"' {
            let (s, next) = read_quoted(&chars, i, c)?;
            tokens.push(Token::Word(s));
            i = next;
            continue;
        }
        if c == '\'' {
            let (s, next) = read_quoted(&chars, i, '\'')?;
            tokens.push(Token::Str(s));
            i = next;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            if c == '0' && matches!(chars.get(i + 1), Some('x' | 'X')) {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex: String = chars[start + 2..i].iter().collect();
                let value = u128::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
                tokens.push(Token::Num(value.to_string()));
                continue;
            }
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || ((chars[i] == 'e' || chars[i] == 'E') && i + 1 < chars.len())
                    || ((chars[i] == '+' || chars[i] == '-') && matches!(chars[i - 1], 'e' | 'E')))
            {
                i += 1;
            }
            tokens.push(Token::Num(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let punct = match two.as_str() {
            "::" => Some("::"),
            "<=" => Some("<="),
            ">=" => Some(">="),
            "!=" | "<>" => Some("!="),
            "==" => Some("="),
            "||" => Some("||"),
            "->" => Some("->"),
            _ => None,
        };
        if let Some(p) = punct {
            tokens.push(Token::Punct(p));
            i += 2;
            continue;
        }
        let p = match c {
            '(' => "(",
            ')' => ")",
            '[' => "[",
            ']' => "]",
            ',' => ",",
            '.' => ".",
            ';' => ";",
            '+' => "+",
            '-' => "-",
            '*' => "*",
            '/' => "/",
            '%' => "%",
            '=' => "=",
            '<' => "<",
            '>' => ">",
            '?' => "?",
            ':' => ":",
            '{' => "{",
            '}' => "}",
            _ => return Err(format!("unexpected character {c:?} in SQL")),
        };
        tokens.push(Token::Punct(p));
        i += 1;
    }
    // A trailing semicolon does not matter.
    while tokens.last() == Some(&Token::Punct(";")) {
        tokens.pop();
    }
    Ok(tokens)
}

fn read_quoted(chars: &[char], start: usize, quote: char) -> Result<(String, usize), String> {
    let mut s = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            s.push(match n {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                other => other,
            });
            i += 2;
            continue;
        }
        if c == quote {
            if chars.get(i + 1) == Some(&quote) {
                s.push(quote);
                i += 2;
                continue;
            }
            return Ok((s, i + 1));
        }
        s.push(c);
        i += 1;
    }
    Err("unterminated quoted string".into())
}

/// Index of the matching closing parenthesis or bracket.
fn matching(tokens: &[Token], open: usize) -> Option<usize> {
    let (o, c) = match tokens[open] {
        Token::Punct("(") => ("(", ")"),
        Token::Punct("[") => ("[", "]"),
        _ => return None,
    };
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate().skip(open) {
        if *t == Token::Punct(o) {
            depth += 1;
        } else if *t == Token::Punct(c) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn has_top_level_comma(tokens: &[Token]) -> bool {
    let mut depth = 0;
    for t in tokens {
        match t {
            Token::Punct("(" | "[") => depth += 1,
            Token::Punct(")" | "]") => depth -= 1,
            Token::Punct(",") if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Remove the parentheses wrapping a whole function argument or a whole
/// select item: ClickHouse keeps or drops them depending on its version.
fn drop_extra_parens(mut tokens: Vec<Token>) -> Vec<Token> {
    loop {
        let mut changed = false;
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i] != Token::Punct("(") {
                i += 1;
                continue;
            }
            let Some(close) = matching(&tokens, i) else { break };
            let after = tokens.get(close + 1);
            let opens_argument = i > 0 && is_argument_start(&tokens, i);
            let ends_argument = matches!(after, Some(Token::Punct(")" | ",")))
                || matches!(after, Some(Token::Word(w)) if w == "AS" || w == "FROM")
                || after.is_none();
            if opens_argument && ends_argument && close > i + 1 && !has_top_level_comma(&tokens[i + 1..close]) {
                tokens.remove(close);
                tokens.remove(i);
                changed = true;
                continue;
            }
            i += 1;
        }
        if !changed {
            return tokens;
        }
    }
}

/// Whether the parenthesis at `i` starts an argument or a select item (and
/// is not the parenthesis of a function call).
fn is_argument_start(tokens: &[Token], i: usize) -> bool {
    match &tokens[i - 1] {
        Token::Punct("(") => {
            // The previous parenthesis must be a function call one.
            i >= 2 && matches!(tokens[i - 2], Token::Word(_))
        }
        Token::Punct(",") => true,
        Token::Word(w) => w == "SELECT",
        _ => false,
    }
}

/// Normalize tokens for comparison: identifiers are compared without case.
fn normalize(tokens: Vec<Token>) -> Vec<Token> {
    drop_extra_parens(tokens)
        .into_iter()
        .map(|t| match t {
            Token::Word(w) => Token::Word(w.to_ascii_lowercase()),
            Token::Num(n) => Token::Num(normalize_number(&n)),
            other => other,
        })
        .collect()
}

fn normalize_number(n: &str) -> String {
    if let Ok(i) = n.parse::<u128>() {
        return i.to_string();
    }
    match n.parse::<f64>() {
        Ok(f) => format!("{f}"),
        Err(_) => n.to_string(),
    }
}

/// Whether two SQL texts mean the same. Unparsable SQL never matches.
pub fn matches(a: &str, b: &str) -> bool {
    match (tokenize(a), tokenize(b)) {
        (Ok(a), Ok(b)) => normalize(a) == normalize(b),
        _ => false,
    }
}

/// Remove the SETTINGS clause of a CREATE TABLE statement, as ClickHouse adds
/// the default ones.
pub fn strip_table_settings(sql: &str) -> String {
    let Ok(tokens) = tokenize(sql) else { return sql.to_string() };
    let is_table = tokens.iter().take(4).any(|t| *t == Token::Word("TABLE".into()));
    if !is_table {
        return sql.to_string();
    }
    match top_level_position(&tokens, "SETTINGS") {
        Some(pos) => render(&tokens[..pos]),
        None => sql.to_string(),
    }
}

fn top_level_position(tokens: &[Token], keyword: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate() {
        match t {
            Token::Punct("(" | "[") => depth += 1,
            Token::Punct(")" | "]") => depth -= 1,
            Token::Word(w) if depth == 0 && w == keyword => return Some(i),
            _ => {}
        }
    }
    None
}

/// Render tokens back to SQL (one line).
pub fn render(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut prev: Option<&Token> = None;
    for t in tokens {
        let space = !matches!(
            (prev, t),
            (None, _)
                | (Some(Token::Punct("(" | "[" | "." | "::")), _)
                | (_, Token::Punct(")" | "]" | "," | "." | "::"))
                | (Some(Token::Word(_)), Token::Punct("(" | "["))
        );
        if space {
            out.push(' ');
        }
        match t {
            Token::Word(w) => out.push_str(&quote_identifier_if_needed(w)),
            Token::Str(s) => out.push_str(&quote_string(s)),
            Token::Num(n) => out.push_str(n),
            Token::Punct(p) => out.push_str(p),
        }
        prev = Some(t);
    }
    out
}

/// Quote an identifier when needed.
pub fn quote_identifier_if_needed(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain { name.to_string() } else { flowscope_clickhouse::quote_identifier(name) }
}

/// Quote a string literal.
pub fn quote_string(s: &str) -> String {
    flowscope_clickhouse::quote_string(s)
}

/// An engine, parsed from the `engine_full` column of `system.tables`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Engine {
    pub name: String,
    pub args: Vec<Vec<Token>>,
    pub ttl: Vec<Token>,
    pub settings: BTreeMap<String, Vec<Token>>,
}

/// Clauses which may follow the engine.
const ENGINE_CLAUSES: &[&str] = &["PARTITION", "PRIMARY", "ORDER", "SAMPLE", "TTL", "SETTINGS", "COMMENT"];

fn split_top_level_commas(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut out = vec![];
    let mut current = vec![];
    let mut depth = 0;
    for t in tokens {
        match t {
            Token::Punct("(" | "[") => depth += 1,
            Token::Punct(")" | "]") => depth -= 1,
            Token::Punct(",") if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(t.clone());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

impl Engine {
    pub fn parse(sql: &str) -> Result<Engine, String> {
        let tokens = tokenize(sql)?;
        let Some(Token::Word(name)) = tokens.first() else { return Err("no engine name".into()) };
        let mut engine = Engine { name: name.clone(), ..Default::default() };
        let mut i = 1;
        if tokens.get(1) == Some(&Token::Punct("(")) {
            let close = matching(&tokens, 1).ok_or("unbalanced engine arguments")?;
            engine.args = split_top_level_commas(&tokens[2..close]);
            i = close + 1;
        }
        // Find the clauses.
        let mut depth = 0;
        let mut clauses: Vec<(String, usize)> = vec![];
        for (j, t) in tokens.iter().enumerate().skip(i) {
            match t {
                Token::Punct("(" | "[") => depth += 1,
                Token::Punct(")" | "]") => depth -= 1,
                Token::Word(w) if depth == 0 && ENGINE_CLAUSES.contains(&w.as_str()) => {
                    // "PRIMARY KEY" and "ORDER BY": the keyword itself.
                    clauses.push((w.clone(), j));
                }
                _ => {}
            }
        }
        for (k, (name, start)) in clauses.iter().enumerate() {
            let end = clauses.get(k + 1).map(|c| c.1).unwrap_or(tokens.len());
            let body = &tokens[start + 1..end];
            match name.as_str() {
                "TTL" => engine.ttl = body.to_vec(),
                "SETTINGS" => {
                    for item in split_top_level_commas(body) {
                        if let (Some(Token::Word(n)), Some(Token::Punct("="))) = (item.first(), item.get(1)) {
                            engine.settings.insert(n.clone(), item[2..].to_vec());
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(engine)
    }

    /// The ZooKeeper path of a replicated engine.
    pub fn replicated_path(&self) -> Option<String> {
        if !self.name.starts_with("Replicated") {
            return None;
        }
        match self.args.first().map(|a| a.as_slice()) {
            Some([Token::Str(path)]) => Some(path.clone()),
            _ => None,
        }
    }

    /// Whether the TTL is the provided expression.
    pub fn ttl_matches(&self, expr: &str) -> bool {
        match tokenize(expr) {
            Ok(t) => normalize(t) == normalize(self.ttl.clone()),
            Err(_) => false,
        }
    }

    /// Whether the settings are exactly the provided ones.
    pub fn settings_match(&self, wanted: &[(String, String)]) -> bool {
        if self.settings.len() != wanted.len() {
            return false;
        }
        wanted.iter().all(|(name, value)| {
            self.settings.get(name).is_some_and(|v| tokenize(value).map(normalize).ok() == Some(normalize(v.clone())))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        assert!(matches("SELECT a, `b` FROM db.t", "select A,b from db.t"));
        assert!(matches("CAST((96 + Mask), 'UInt8')", "CAST(96 + Mask, 'UInt8')"));
        assert!(matches("SELECT (a + 1) AS x", "SELECT a + 1 AS x"));
        assert!(!matches("SELECT (a + 1) * 2", "SELECT a + 1 * 2"));
        assert!(matches("EType = 0x800", "EType = 2048"));
        assert!(matches("ORDER BY (a, b)", "ORDER BY (a, b)"));
        assert!(!matches("ORDER BY (a, b)", "ORDER BY (b, a)"));
        assert!(matches("x IN ('a''b')", "x IN ('a\\'b')"));
        assert!(!matches("SELECT 'x", "SELECT 'x'"));
    }

    #[test]
    fn strip_settings() {
        let stripped = strip_table_settings(
            "CREATE TABLE db.t (`a` UInt8) ENGINE = MergeTree ORDER BY a SETTINGS index_granularity = 8192",
        );
        assert!(matches(&stripped, "CREATE TABLE db.t (a UInt8) ENGINE = MergeTree ORDER BY a"), "{stripped}");
        let dict = "CREATE DICTIONARY d (a UInt8) PRIMARY KEY a SETTINGS(x = 0)";
        assert_eq!(strip_table_settings(dict), dict);
    }

    #[test]
    fn engine() {
        let e = Engine::parse(
            "ReplicatedSummingMergeTree('/clickhouse/tables/shard-{shard}/default/flows_1m0s', 'replica-{replica}', (Bytes, Packets)) \
             PARTITION BY toYYYYMMDDhhmmss(toStartOfInterval(TimeReceived, toIntervalSecond(12096))) \
             PRIMARY KEY (TimeReceived, ExporterAddress) ORDER BY (TimeReceived, ExporterAddress, SrcAS) \
             TTL TimeReceived + toIntervalSecond(604800) SETTINGS index_granularity = 8192, ttl_only_drop_parts = 1",
        )
        .unwrap();
        assert_eq!(e.name, "ReplicatedSummingMergeTree");
        assert_eq!(e.replicated_path().as_deref(), Some("/clickhouse/tables/shard-{shard}/default/flows_1m0s"));
        assert!(e.ttl_matches("TimeReceived + toIntervalSecond(604800)"));
        assert!(!e.ttl_matches("TimeReceived + toIntervalSecond(1)"));
        assert!(e.settings_match(&[
            ("index_granularity".into(), "8192".into()),
            ("ttl_only_drop_parts".into(), "1".into())
        ]));
        assert!(!e.settings_match(&[("index_granularity".into(), "8192".into())]));
        assert_eq!(Engine::parse("MergeTree ORDER BY a").unwrap().replicated_path(), None);
    }
}
