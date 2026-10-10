// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! A small syntax highlighter for the code blocks of the documentation,
//! with the Dracula colors. It knows the few languages the documentation
//! uses (shell sessions, YAML, SQL). Other code is not highlighted. The
//! HTML has the shape of the one of the Go version (chroma).

/// Escape like Go's html.EscapeString.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            c => out.push(c),
        }
    }
    out
}

const COMMENT: &str = "color:#6272a4";
const KEYWORD: &str = "color:#ff79c6";
const STRING: &str = "color:#f1fa8c";
const NUMBER: &str = "color:#bd93f9";
const ATTRIBUTE: &str = "color:#50fa7b";
const PROMPT: &str = "color:#50fa7b";
const OUTPUT: &str = "color:#a3b3e4";
const BUILTIN: &str = "color:#bd93f9;font-style:italic";
const CONSTANT: &str = "color:#bd93f9";

fn span(style: &str, text: &str) -> String {
    format!("<span style=\"{style}\">{}</span>", escape(text))
}

const SQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "GROUP",
    "BY",
    "ORDER",
    "LIMIT",
    "AS",
    "IN",
    "IS",
    "NULL",
    "ON",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "CREATE",
    "TABLE",
    "DROP",
    "ALTER",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "DELETE",
    "WITH",
    "UNION",
    "ALL",
    "DISTINCT",
    "HAVING",
    "BETWEEN",
    "LIKE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "DESC",
    "ASC",
    "SET",
    "SYSTEM",
    "SHOW",
    "DATABASE",
    "IF",
    "EXISTS",
    "ENGINE",
    "PARTITION",
    "INTERVAL",
    "TTL",
    "MODIFY",
    "COLUMN",
    "ADD",
    "SETTINGS",
    "FORMAT",
    "OPTIMIZE",
    "FINAL",
    "KILL",
    "QUERY",
];

/// Highlight one line of SQL.
fn sql_line(line: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            out.push_str(&span(COMMENT, &chars[i..].iter().collect::<String>()));
            break;
        }
        if c == '\'' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            i = (i + 1).min(chars.len());
            out.push_str(&span(STRING, &chars[start..i].iter().collect::<String>()));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push_str(&span(NUMBER, &chars[start..i].iter().collect::<String>()));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if SQL_KEYWORDS.contains(&word.to_uppercase().as_str()) {
                out.push_str(&span(KEYWORD, &word));
            } else if chars.get(i) == Some(&'(') {
                out.push_str(&span(ATTRIBUTE, &word));
            } else {
                out.push_str(&escape(&word));
            }
            continue;
        }
        if "=<>!+-*/%|".contains(c) {
            out.push_str(&span(KEYWORD, &c.to_string()));
        } else {
            out.push_str(&escape(&c.to_string()));
        }
        i += 1;
    }
    out
}

/// Highlight one line of YAML.
fn yaml_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    if trimmed.starts_with('#') {
        return format!("{}{}", escape(indent), span(COMMENT, trimmed));
    }
    let (body, comment) = match trimmed.find(" #") {
        Some(p) => (&trimmed[..p], Some(&trimmed[p..])),
        None => (trimmed, None),
    };
    let mut out = escape(indent);
    let (dash, body) = match body.strip_prefix("- ") {
        Some(rest) => ("- ", rest),
        None => ("", body),
    };
    if !dash.is_empty() {
        out.push_str(&span(KEYWORD, "-"));
        out.push(' ');
    }
    let value = match body.find(':').filter(|&p| body[p + 1..].is_empty() || body[p + 1..].starts_with(' ')) {
        Some(p) if !body.starts_with('"') && !body.starts_with('\'') => {
            out.push_str(&span(KEYWORD, &body[..p]));
            out.push_str(&span(KEYWORD, ":"));
            &body[p + 1..]
        }
        _ => body,
    };
    let v = value.trim_start();
    out.push_str(&escape(&value[..value.len() - v.len()]));
    if !v.is_empty() {
        let style = if v.starts_with('"') || v.starts_with('\'') {
            STRING
        } else if v.parse::<f64>().is_ok() {
            NUMBER
        } else if ["true", "false", "null", "yes", "no"].contains(&v) {
            CONSTANT
        } else if v.starts_with('!') || v.starts_with('&') || v.starts_with('*') {
            ATTRIBUTE
        } else {
            STRING
        };
        out.push_str(&span(style, v));
    }
    if let Some(c) = comment {
        out.push_str(&span(COMMENT, c));
    }
    out
}

/// Highlight one line of shell.
fn shell_line(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return span(COMMENT, line);
    }
    let mut out = String::new();
    for (i, word) in line.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if word.starts_with('"') || word.starts_with('\'') {
            out.push_str(&span(STRING, word));
        } else if word.starts_with('-') && word.len() > 1 {
            out.push_str(&escape(word));
        } else if i == 0 && ["cd", "echo", "export", "sudo", "help", "set", "test"].contains(&word) {
            out.push_str(&span(BUILTIN, word));
        } else {
            out.push_str(&escape(word));
        }
    }
    out
}

/// Highlight one line of a shell session: prompts are highlighted, the
/// output is dimmed.
fn console_line(line: &str) -> String {
    for prompt in ["$ ", "# ", "% ", "> "] {
        if let Some(command) = line.strip_prefix(prompt) {
            return format!("{} {}", span(PROMPT, prompt.trim_end()), shell_line(command));
        }
    }
    if line.is_empty() { String::new() } else { span(OUTPUT, line) }
}

/// Render a code block like chroma: one flex span per line.
pub fn code_block(language: &str, code: &str) -> String {
    let highlight: fn(&str) -> String = match language {
        "console" | "shell-session" => console_line,
        "sh" | "bash" | "shell" => shell_line,
        "yaml" | "yml" => yaml_line,
        "sql" => sql_line,
        _ => escape,
    };
    let mut out = String::from("<pre style=\"color:#f8f8f2;-webkit-text-size-adjust:none;\"><code>");
    for line in code.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(c) => (c, "\n"),
            None => (line, ""),
        };
        out.push_str("<span style=\"display:flex;\"><span>");
        out.push_str(&highlight(content));
        out.push_str(newline);
        out.push_str("</span></span>");
    }
    out.push_str("</code></pre>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(
            code_block("console", "# docker pull x\n"),
            "<pre style=\"color:#f8f8f2;-webkit-text-size-adjust:none;\"><code><span style=\"display:flex;\"><span>\
             <span style=\"color:#50fa7b\">#</span> docker pull x\n</span></span></code></pre>"
        );
        assert!(code_block("yaml", "key: \"value\" # c\n").contains("color:#f1fa8c\">&#34;value&#34;"));
        assert!(code_block("sql", "SELECT 1 -- c\n").contains("color:#ff79c6\">SELECT"));
    }
}
