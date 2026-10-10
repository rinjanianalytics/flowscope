// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Built-in functions of the expr language.

use crate::eval::Value;

fn arg<'a>(args: &'a [Value], i: usize, name: &str) -> Result<&'a Value, String> {
    args.get(i).ok_or_else(|| format!("not enough arguments to call {name}"))
}

fn string_arg<'a>(args: &'a [Value], i: usize, name: &str) -> Result<&'a str, String> {
    match arg(args, i, name)? {
        Value::String(s) => Ok(s),
        v => Err(format!("cannot use {} as argument (type string) to call {name}", v.type_name())),
    }
}

fn numbers(args: &[Value], name: &str) -> Result<Vec<Value>, String> {
    let items = if args.len() == 1 {
        match &args[0] {
            Value::Array(a) => a.clone(),
            v => vec![v.clone()],
        }
    } else {
        args.to_vec()
    };
    if items.iter().any(|v| !matches!(v, Value::Int(_) | Value::Float(_))) {
        return Err(format!("invalid argument for {name}"));
    }
    Ok(items)
}

fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        _ => 0.0,
    }
}

pub(crate) fn call(name: &str, args: Vec<Value>) -> Result<Value, String> {
    Ok(match name {
        "len" => Value::Int(match arg(&args, 0, name)? {
            Value::String(s) => s.chars().count() as i64,
            Value::Array(a) => a.len() as i64,
            Value::Map(m) => m.len() as i64,
            v => return Err(format!("invalid argument for len (type {})", v.type_name())),
        }),
        "abs" => match arg(&args, 0, name)? {
            Value::Int(i) => Value::Int(i.abs()),
            Value::Float(f) => Value::Float(f.abs()),
            v => return Err(format!("invalid argument for abs (type {})", v.type_name())),
        },
        "int" => Value::Int(match arg(&args, 0, name)? {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i64,
            Value::String(s) => s.trim().parse().map_err(|_| format!("invalid int {s:?}"))?,
            v => return Err(format!("invalid argument for int (type {})", v.type_name())),
        }),
        "float" => Value::Float(match arg(&args, 0, name)? {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            Value::String(s) => s.trim().parse().map_err(|_| format!("invalid float {s:?}"))?,
            v => return Err(format!("invalid argument for float (type {})", v.type_name())),
        }),
        "round" => Value::Float(as_f64(arg(&args, 0, name)?).round()),
        "ceil" => Value::Float(as_f64(arg(&args, 0, name)?).ceil()),
        "floor" => Value::Float(as_f64(arg(&args, 0, name)?).floor()),
        "string" => Value::String(arg(&args, 0, name)?.to_string()),
        "type" => Value::String(arg(&args, 0, name)?.type_name().to_string()),
        "upper" => Value::String(string_arg(&args, 0, name)?.to_uppercase()),
        "lower" => Value::String(string_arg(&args, 0, name)?.to_lowercase()),
        "trim" => {
            let s = string_arg(&args, 0, name)?;
            match args.get(1) {
                Some(Value::String(chars)) => Value::String(s.trim_matches(|c| chars.contains(c)).to_string()),
                _ => Value::String(s.trim().to_string()),
            }
        }
        "trimPrefix" => {
            let s = string_arg(&args, 0, name)?;
            let p = string_arg(&args, 1, name).unwrap_or(" ");
            Value::String(s.strip_prefix(p).unwrap_or(s).to_string())
        }
        "trimSuffix" => {
            let s = string_arg(&args, 0, name)?;
            let p = string_arg(&args, 1, name).unwrap_or(" ");
            Value::String(s.strip_suffix(p).unwrap_or(s).to_string())
        }
        "split" | "splitAfter" => {
            let s = string_arg(&args, 0, name)?;
            let sep = string_arg(&args, 1, name)?;
            let n = match args.get(2) {
                Some(Value::Int(n)) => *n,
                _ => -1,
            };
            let parts: Vec<String> = if name == "split" {
                if n < 0 {
                    s.split(sep).map(String::from).collect()
                } else {
                    s.splitn(n as usize, sep).map(String::from).collect()
                }
            } else {
                s.split_inclusive(sep).map(String::from).collect()
            };
            Value::Array(parts.into_iter().map(Value::String).collect())
        }
        "replace" => {
            let s = string_arg(&args, 0, name)?;
            Value::String(s.replace(string_arg(&args, 1, name)?, string_arg(&args, 2, name)?))
        }
        "repeat" => {
            let s = string_arg(&args, 0, name)?;
            match arg(&args, 1, name)? {
                Value::Int(n) if *n >= 0 && *n < 100_000 => Value::String(s.repeat(*n as usize)),
                _ => return Err("invalid argument for repeat".into()),
            }
        }
        "indexOf" | "lastIndexOf" => {
            let s = string_arg(&args, 0, name)?;
            let sub = string_arg(&args, 1, name)?;
            let pos = if name == "indexOf" { s.find(sub) } else { s.rfind(sub) };
            Value::Int(pos.map(|p| s[..p].chars().count() as i64).unwrap_or(-1))
        }
        "hasPrefix" => Value::Bool(string_arg(&args, 0, name)?.starts_with(string_arg(&args, 1, name)?)),
        "hasSuffix" => Value::Bool(string_arg(&args, 0, name)?.ends_with(string_arg(&args, 1, name)?)),
        "max" | "min" => {
            let items = numbers(&args, name)?;
            let mut best: Option<Value> = None;
            for v in items {
                best = Some(match best {
                    None => v,
                    Some(b) => {
                        let better = if name == "max" { as_f64(&v) > as_f64(&b) } else { as_f64(&v) < as_f64(&b) };
                        if better { v } else { b }
                    }
                });
            }
            best.unwrap_or(Value::Nil)
        }
        "sum" => {
            let items = numbers(&args, name)?;
            if items.iter().all(|v| matches!(v, Value::Int(_))) {
                Value::Int(items.iter().map(|v| as_f64(v) as i64).sum())
            } else {
                Value::Float(items.iter().map(as_f64).sum())
            }
        }
        "mean" => {
            let items = numbers(&args, name)?;
            if items.is_empty() {
                Value::Float(0.0)
            } else {
                Value::Float(items.iter().map(as_f64).sum::<f64>() / items.len() as f64)
            }
        }
        "join" => match arg(&args, 0, name)? {
            Value::Array(a) => {
                let sep = string_arg(&args, 1, name).unwrap_or("");
                Value::String(a.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(sep))
            }
            v => return Err(format!("invalid argument for join (type {})", v.type_name())),
        },
        "keys" | "values" => match arg(&args, 0, name)? {
            Value::Map(m) => {
                if name == "keys" {
                    Value::Array(m.keys().map(|k| Value::String(k.clone())).collect())
                } else {
                    Value::Array(m.values().cloned().collect())
                }
            }
            v => return Err(format!("invalid argument for {name} (type {})", v.type_name())),
        },
        "first" | "last" => match arg(&args, 0, name)? {
            Value::Array(a) => (if name == "first" { a.first() } else { a.last() }).cloned().unwrap_or(Value::Nil),
            v => return Err(format!("invalid argument for {name} (type {})", v.type_name())),
        },
        "reverse" => match arg(&args, 0, name)? {
            Value::Array(a) => Value::Array(a.iter().rev().cloned().collect()),
            v => return Err(format!("invalid argument for reverse (type {})", v.type_name())),
        },
        other => return Err(format!("unknown func {other}")),
    })
}

/// Format like Go's fmt.Sprintf, for the verbs used in practice (%s, %d,
/// %v, %q, %x, %X, %f, %%, with width and precision).
pub fn sprintf(format: &str, args: &[Value]) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    let mut next_arg = 0;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // Flags, width, precision.
        let mut spec = String::new();
        while let Some(&d) = chars.peek() {
            if "-+ 0#".contains(d) || d.is_ascii_digit() || d == '.' {
                spec.push(d);
                chars.next();
            } else {
                break;
            }
        }
        let Some(verb) = chars.next() else {
            out.push_str("%!(NOVERB)");
            break;
        };
        if verb == '%' {
            out.push('%');
            continue;
        }
        let Some(a) = args.get(next_arg) else {
            out.push_str(&format!("%!{verb}(MISSING)"));
            continue;
        };
        next_arg += 1;
        let left = spec.contains('-');
        let zero = spec.starts_with('0') && !left;
        let (width, precision) = {
            let s = spec.trim_start_matches(['-', '+', ' ', '0', '#']);
            let mut parts = s.splitn(2, '.');
            let w = parts.next().and_then(|w| w.parse::<usize>().ok());
            let p = parts.next().map(|p| p.parse::<usize>().unwrap_or(0));
            (w, p)
        };
        let s = match (verb, a) {
            ('d', Value::Int(i)) => i.to_string(),
            ('d', Value::Float(f)) => format!("%!d(float64={f})"),
            ('s' | 'v', Value::String(s)) => match precision {
                Some(p) => s.chars().take(p).collect(),
                None => s.clone(),
            },
            ('q', Value::String(s)) => format!("{s:?}"),
            ('x', Value::Int(i)) => format!("{i:x}"),
            ('X', Value::Int(i)) => format!("{i:X}"),
            ('x', Value::String(s)) => s.bytes().map(|b| format!("{b:02x}")).collect(),
            ('f' | 'F', v @ (Value::Float(_) | Value::Int(_))) => format!("{:.*}", precision.unwrap_or(6), as_f64(v)),
            ('t' | 'v', Value::Bool(b)) => b.to_string(),
            ('v', v) => v.to_string(),
            (verb, v) => format!("%!{verb}({}={v})", v.type_name()),
        };
        let len = s.chars().count();
        match width {
            Some(w) if w > len => {
                let pad = w - len;
                if left {
                    out.push_str(&s);
                    out.extend(std::iter::repeat_n(' ', pad));
                } else if zero {
                    out.extend(std::iter::repeat_n('0', pad));
                    out.push_str(&s);
                } else {
                    out.extend(std::iter::repeat_n(' ', pad));
                    out.push_str(&s);
                }
            }
            _ => out.push_str(&s),
        }
    }
    if next_arg < args.len() {
        out.push_str("%!(EXTRA ");
        let extra: Vec<String> = args[next_arg..].iter().map(|v| format!("{}={v}", v.type_name())).collect();
        out.push_str(&extra.join(", "));
        out.push(')');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format() {
        assert_eq!(sprintf("tenant-%s", &["foo".into()]), "tenant-foo");
        assert_eq!(sprintf("%d-%05d|%-4s|", &[Value::Int(1), Value::Int(42), "ab".into()]), "1-00042|ab  |");
        assert_eq!(sprintf("100%%", &[]), "100%");
        assert_eq!(sprintf("%s", &[]), "%!s(MISSING)");
        assert_eq!(sprintf("%.2f", &[Value::Float(1.234)]), "1.23");
    }
}
