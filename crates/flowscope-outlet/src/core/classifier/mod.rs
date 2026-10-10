// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Exporter and interface classifiers, written in the expr language. Port
//! of `outlet/core/classifier.go`.

use flowscope_expr::{Env, Expr, Host, Program, Signature, Type, Value, sprintf};
use flowscope_schema::InterfaceBoundary;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};

/// Information about an exporter given to classifiers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExporterInfo {
    pub ip: String,
    pub name: String,
}

/// Result of an exporter classification.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExporterClassification {
    pub group: String,
    pub role: String,
    pub site: String,
    pub region: String,
    pub tenant: String,
    pub reject: bool,
    pub name: String,
}

/// Information about an interface given to classifiers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InterfaceInfo {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub speed: u32,
    pub vlan: u16,
}

/// Result of an interface classification.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InterfaceClassification {
    pub connectivity: String,
    pub provider: String,
    pub boundary: InterfaceBoundary,
    pub reject: bool,
    pub name: String,
    pub description: String,
}

static REGEX_CACHE: LazyLock<RwLock<HashMap<String, Regex>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

fn regex(pattern: &str) -> Result<Regex, String> {
    if let Some(r) = REGEX_CACHE.read().unwrap().get(pattern) {
        return Ok(r.clone());
    }
    let r = Regex::new(pattern).map_err(|e| format!("cannot compile regex {pattern:?}: {e}"))?;
    REGEX_CACHE.write().unwrap().insert(pattern.to_string(), r.clone());
    Ok(r)
}

/// Lowercase and keep only `[a-z0-9.+-]`.
pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '+' | '-'))
        .collect()
}

fn classify_string(input: &str, output: &mut String) -> bool {
    if output.is_empty() {
        *output = normalize(input);
    }
    true
}

fn classify_string_with_regex(input: &str, pattern: &str, template: &str, output: &mut String) -> Result<bool, String> {
    if output.is_empty() {
        let re = regex(pattern)?;
        let Some(caps) = re.captures(input) else { return Ok(false) };
        let mut result = String::new();
        caps.expand(template, &mut result);
        *output = normalize(&result);
    }
    Ok(true)
}

fn sig(params: &[Type], variadic: Option<Type>) -> Signature {
    Signature { params: params.to_vec(), variadic, returns: Type::Bool }
}

fn common_functions(e: &mut Env) {
    e.functions.insert(
        "Format".into(),
        Signature { params: vec![Type::String], variadic: Some(Type::Any), returns: Type::String },
    );
    e.functions.insert("Reject".into(), sig(&[], None));
    e.functions.insert("SetName".into(), sig(&[Type::String], None));
}

fn add_classify(e: &mut Env, name: &str) {
    e.functions.insert(name.into(), sig(&[Type::String], None));
    e.functions.insert(format!("{name}Regex"), sig(&[Type::String, Type::String, Type::String], None));
}

fn exporter_type() -> Type {
    Type::Struct(BTreeMap::from([("IP".into(), Type::String), ("Name".into(), Type::String)]))
}

fn exporter_env() -> Env {
    let mut e = Env::default();
    e.variables.insert("Exporter".into(), exporter_type());
    common_functions(&mut e);
    for name in ["Classify", "ClassifyGroup", "ClassifyRole", "ClassifySite", "ClassifyRegion", "ClassifyTenant"] {
        add_classify(&mut e, name);
    }
    e
}

fn interface_env() -> Env {
    let mut e = Env::default();
    e.variables.insert("Exporter".into(), exporter_type());
    e.variables.insert(
        "Interface".into(),
        Type::Struct(BTreeMap::from([
            ("Index".into(), Type::Int),
            ("Name".into(), Type::String),
            ("Description".into(), Type::String),
            ("Speed".into(), Type::Int),
            ("VLAN".into(), Type::Int),
        ])),
    );
    common_functions(&mut e);
    e.functions.insert("ClassifyExternal".into(), sig(&[], None));
    e.functions.insert("ClassifyInternal".into(), sig(&[], None));
    e.functions.insert("SetDescription".into(), sig(&[Type::String], None));
    for name in ["ClassifyProvider", "ClassifyConnectivity"] {
        add_classify(&mut e, name);
    }
    e
}

/// Check the regex given as a literal to `*Regex()` functions.
fn check_regexes(e: &Expr) -> Result<(), String> {
    let mut stack = vec![e];
    while let Some(e) = stack.pop() {
        match e {
            Expr::Call(name, args) => {
                if name.ends_with("Regex")
                    && args.len() == 3
                    && let Expr::String(pattern) = &args[1]
                    && Regex::new(pattern).is_err()
                {
                    return Err(format!("invalid regular expression {pattern:?}"));
                }
                stack.extend(args.iter());
            }
            Expr::Array(items) => stack.extend(items.iter()),
            Expr::Map(items) => items.iter().for_each(|(k, v)| {
                stack.push(k);
                stack.push(v);
            }),
            Expr::Member(a, b, _) => {
                stack.push(a);
                stack.push(b);
            }
            Expr::Slice(a, b, c) => {
                stack.push(a);
                stack.extend(b.iter().map(|x| x.as_ref()));
                stack.extend(c.iter().map(|x| x.as_ref()));
            }
            Expr::Closure(a) | Expr::Unary(_, a) => stack.push(a),
            Expr::Binary(_, a, b) | Expr::Let(_, a, b) => {
                stack.push(a);
                stack.push(b);
            }
            Expr::Ternary(a, b, c) => {
                stack.push(a);
                stack.push(b);
                stack.push(c);
            }
            _ => {}
        }
    }
    Ok(())
}

fn string_arg(args: &[Value], i: usize) -> Result<&str, String> {
    args.get(i).and_then(|v| v.as_str()).ok_or_else(|| format!("argument {i} should be a string"))
}

fn format_function(args: &[Value]) -> Result<Value, String> {
    let fmt = string_arg(args, 0)?;
    Ok(Value::String(sprintf(fmt, &args[1..])))
}

macro_rules! rule {
    ($name:ident, $env:ident, $what:literal) => {
        /// A classifier rule.
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name(Program);

        impl $name {
            pub fn new(source: &str) -> Result<Self, String> {
                let program = Program::compile(source, &$env(), true)
                    .map_err(|e| format!("cannot compile {} classifier rule {source:?}: {e}", $what))?;
                check_regexes(program.expr())?;
                Ok($name(program))
            }

            pub fn source(&self) -> &str {
                self.0.source()
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.0.source())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $name::new(&String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

rule!(ExporterClassifierRule, exporter_env, "exporter");
rule!(InterfaceClassifierRule, interface_env, "interface");

struct ExporterHost<'a>(&'a mut ExporterClassification);

fn exporter_field<'a>(ec: &'a mut ExporterClassification, name: &str) -> Option<&'a mut String> {
    Some(match name.trim_end_matches("Regex") {
        "Classify" | "ClassifyGroup" => &mut ec.group,
        "ClassifyRole" => &mut ec.role,
        "ClassifySite" => &mut ec.site,
        "ClassifyRegion" => &mut ec.region,
        "ClassifyTenant" => &mut ec.tenant,
        _ => return None,
    })
}

impl Host for ExporterHost<'_> {
    fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let ec = &mut *self.0;
        match name {
            "Format" => format_function(&args),
            "Reject" => {
                ec.reject = true;
                Ok(Value::Bool(false))
            }
            "SetName" => {
                if ec.name.is_empty() {
                    ec.name = string_arg(&args, 0)?.to_string();
                }
                Ok(Value::Bool(true))
            }
            _ => {
                let Some(out) = exporter_field(ec, name) else { return Err(format!("unknown func {name}")) };
                if name.ends_with("Regex") {
                    classify_string_with_regex(string_arg(&args, 0)?, string_arg(&args, 1)?, string_arg(&args, 2)?, out)
                        .map(Value::Bool)
                } else {
                    Ok(Value::Bool(classify_string(string_arg(&args, 0)?, out)))
                }
            }
        }
    }
}

struct InterfaceHost<'a>(&'a mut InterfaceClassification);

impl Host for InterfaceHost<'_> {
    fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let ic = &mut *self.0;
        match name {
            "Format" => format_function(&args),
            "Reject" => {
                ic.reject = true;
                Ok(Value::Bool(false))
            }
            "ClassifyExternal" | "ClassifyInternal" => {
                if ic.boundary == InterfaceBoundary::Undefined {
                    ic.boundary = if name == "ClassifyExternal" {
                        InterfaceBoundary::External
                    } else {
                        InterfaceBoundary::Internal
                    };
                }
                Ok(Value::Bool(true))
            }
            "SetName" => {
                if ic.name.is_empty() {
                    ic.name = string_arg(&args, 0)?.to_string();
                }
                Ok(Value::Bool(true))
            }
            "SetDescription" => {
                if ic.description.is_empty() {
                    ic.description = string_arg(&args, 0)?.to_string();
                }
                Ok(Value::Bool(true))
            }
            "ClassifyProvider" | "ClassifyConnectivity" | "ClassifyProviderRegex" | "ClassifyConnectivityRegex" => {
                let out = if name.starts_with("ClassifyProvider") { &mut ic.provider } else { &mut ic.connectivity };
                if name.ends_with("Regex") {
                    classify_string_with_regex(string_arg(&args, 0)?, string_arg(&args, 1)?, string_arg(&args, 2)?, out)
                        .map(Value::Bool)
                } else {
                    Ok(Value::Bool(classify_string(string_arg(&args, 0)?, out)))
                }
            }
            _ => Err(format!("unknown func {name}")),
        }
    }
}

fn exporter_value(e: &ExporterInfo) -> Value {
    Value::Map(BTreeMap::from([
        ("IP".into(), Value::from(e.ip.as_str())),
        ("Name".into(), Value::from(e.name.as_str())),
    ]))
}

impl ExporterClassifierRule {
    /// Run the rule on an exporter, updating the classification.
    pub fn exec(&self, e: &ExporterInfo, ec: &mut ExporterClassification) -> Result<(), String> {
        let vars = BTreeMap::from([("Exporter".to_string(), exporter_value(e))]);
        self.0
            .run_bool(&vars, &mut ExporterHost(ec))
            .map(|_| ())
            .map_err(|err| format!("unable to execute classifier {:?}: {err}", self.0.source()))
    }
}

impl InterfaceClassifierRule {
    /// Run the rule on an interface, updating the classification.
    pub fn exec(&self, e: &ExporterInfo, i: &InterfaceInfo, ic: &mut InterfaceClassification) -> Result<(), String> {
        let iface = Value::Map(BTreeMap::from([
            ("Index".into(), Value::from(i.index)),
            ("Name".into(), Value::from(i.name.as_str())),
            ("Description".into(), Value::from(i.description.as_str())),
            ("Speed".into(), Value::from(i.speed)),
            ("VLAN".into(), Value::from(i.vlan)),
        ]));
        let vars = BTreeMap::from([("Exporter".to_string(), exporter_value(e)), ("Interface".to_string(), iface)]);
        self.0
            .run_bool(&vars, &mut InterfaceHost(ic))
            .map(|_| ())
            .map_err(|err| format!("unable to execute classifier {:?}: {err}", self.0.source()))
    }
}

#[cfg(test)]
mod tests;
