// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Configuration loading. Port of `cmd/akvorado/config.go`,
//! `common/helpers/yaml` and the mapstructure hooks in `common/helpers`.
//!
//! A configuration is read from a YAML file (with `!include` support) or
//! fetched from the orchestrator. It is merged on top of the default
//! configuration, then `AKVORADO_CFG_<COMPONENT>_<PATH>` environment variables
//! are applied. The result is decoded with the weakly-typed deserializer from
//! `de`.

pub mod de;
pub mod duration;
#[macro_use]
pub mod parametrized;
#[macro_use]
pub mod migrate;

pub use de::{Error as DecodeError, from_value, normalize_key};
pub use duration::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Configuration-related errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("in {path}: {message}")]
    Yaml { path: PathBuf, message: String },
    #[error("unable to fetch configuration file: {0}")]
    Fetch(String),
    #[error("received configuration file is not YAML ({0})")]
    NotYaml(String),
    #[error("unable to parse configuration: {0}")]
    Decode(#[from] de::Error),
    #[error("unable to parse override {name:?}: {source}")]
    Override { name: String, source: de::Error },
    #[error("invalid configuration:\n{0}")]
    Invalid(String),
    #[error("unable to dump configuration: {0}")]
    Dump(String),
}

/// Validation of a decoded configuration, replacing Go's validator tags.
pub trait Validate {
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Rewrite the configuration as written by the user, before the
    /// defaults are merged in. This is where a setting can be told apart
    /// from a default value.
    fn prepare(_tree: &mut Value) -> Result<(), String> {
        Ok(())
    }
}

/// Convert a YAML value to the configuration tree. Map keys become strings.
fn yaml_to_tree(v: serde_norway::Value) -> Result<Value, String> {
    use serde_norway::Value as Y;
    Ok(match v {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_u64() {
                Value::from(i)
            } else if let Some(i) = n.as_i64() {
                Value::from(i)
            } else {
                de::number(n.as_f64().unwrap_or(0.0))
            }
        }
        Y::String(s) => Value::String(s),
        Y::Sequence(s) => Value::Array(s.into_iter().map(yaml_to_tree).collect::<Result<_, _>>()?),
        Y::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let key = match k {
                    Y::String(s) => s,
                    Y::Number(n) => n.to_string(),
                    Y::Bool(b) => b.to_string(),
                    Y::Null => String::new(),
                    other => return Err(format!("unsupported map key {other:?}")),
                };
                out.insert(key, yaml_to_tree(v)?);
            }
            Value::Object(out)
        }
        Y::Tagged(t) => return Err(format!("unsupported tag {}", t.tag)),
    })
}

/// Remove top-level keys starting with a dot and unwrap a map whose only key
/// is the empty string (used to include a list).
fn strip_top_level(v: &mut serde_norway::Value) {
    if let serde_norway::Value::Mapping(m) = v {
        m.retain(|k, _| !k.as_str().is_some_and(|s| s.starts_with('.')));
        if m.len() == 1 && m.keys().next().and_then(|k| k.as_str()) == Some("") {
            let inner = m.values().next().unwrap().clone();
            *v = inner;
        }
    }
}

/// Load a YAML file, resolving `!include` tags relative to `root`. Returns the
/// list of files read.
fn load_with_include(root: &Path, file: &str, depth: usize) -> Result<(serde_norway::Value, Vec<PathBuf>), Error> {
    let path = root.join(file);
    if depth > 32 {
        return Err(Error::Yaml { path, message: "too many nested includes".into() });
    }
    let raw = std::fs::read_to_string(&path).map_err(|source| Error::Read { path: path.clone(), source })?;
    let mut value: serde_norway::Value = if raw.trim().is_empty() {
        serde_norway::Value::Null
    } else {
        serde_norway::from_str(&raw).map_err(|e| Error::Yaml { path: path.clone(), message: e.to_string() })?
    };
    strip_top_level(&mut value);
    let mut paths = vec![PathBuf::from(file)];
    resolve_includes(root, &path, &mut value, &mut paths, depth)?;
    Ok((value, paths))
}

fn resolve_includes(
    root: &Path,
    current: &Path,
    v: &mut serde_norway::Value,
    paths: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<(), Error> {
    use serde_norway::Value as Y;
    match v {
        Y::Tagged(t) if t.tag == "include" => {
            let Y::String(file) = &t.value else {
                return Err(Error::Yaml { path: current.into(), message: "!include expects a file name".into() });
            };
            let (included, more) = load_with_include(root, file, depth + 1)?;
            paths.extend(more);
            *v = included;
        }
        Y::Tagged(t) => resolve_includes(root, current, &mut t.value, paths, depth)?,
        Y::Sequence(s) => {
            for x in s.iter_mut() {
                resolve_includes(root, current, x, paths, depth)?;
            }
        }
        Y::Mapping(m) => {
            for (_, x) in m.iter_mut() {
                resolve_includes(root, current, x, paths, depth)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Parse a YAML file with includes into a configuration tree.
pub fn load_file(path: &Path) -> Result<(Value, Vec<PathBuf>), Error> {
    let path = std::fs::canonicalize(path).map_err(|source| Error::Read { path: path.into(), source })?;
    let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let file = path.file_name().unwrap().to_string_lossy().to_string();
    let (mut value, paths) = load_with_include(&root, &file, 0)?;
    value.apply_merge().map_err(|e| Error::Yaml { path: path.clone(), message: e.to_string() })?;
    let tree = yaml_to_tree(value).map_err(|message| Error::Yaml { path: path.clone(), message })?;
    let mut paths: Vec<PathBuf> = paths.into_iter().map(|p| root.join(p)).collect();
    paths.sort();
    paths.dedup();
    Ok((tree, paths))
}

/// Parse a YAML string into a configuration tree (no include support).
pub fn parse_str(input: &str) -> Result<Value, Error> {
    let mut value: serde_norway::Value = if input.trim().is_empty() {
        serde_norway::Value::Null
    } else {
        serde_norway::from_str(input).map_err(|e| Error::Yaml { path: "<input>".into(), message: e.to_string() })?
    };
    strip_top_level(&mut value);
    value.apply_merge().map_err(|e| Error::Yaml { path: "<input>".into(), message: e.to_string() })?;
    yaml_to_tree(value).map_err(|message| Error::Yaml { path: "<input>".into(), message })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeMode {
    /// Merging a configuration file onto the defaults: lists are replaced.
    Defaults,
    /// Merging an environment override: lists are merged by index.
    Override,
}

fn find_key<'a>(map: &'a mut Map<String, Value>, key: &str) -> Option<&'a mut Value> {
    let n = normalize_key(key);
    let existing = map.keys().find(|k| normalize_key(k) == n)?.clone();
    map.get_mut(&existing)
}

fn type_of(map: &Map<String, Value>) -> Option<String> {
    map.iter().find(|(k, _)| k.eq_ignore_ascii_case("type")).and_then(|(_, v)| v.as_str()).map(|s| s.to_lowercase())
}

/// Whether all keys of a map look like IP addresses or subnets: such a map is
/// a subnet map, replaced as a whole like in the Go version.
fn is_subnet_map(m: &Map<String, Value>) -> bool {
    !m.is_empty()
        && m.keys().all(|k| {
            let addr = k.split('/').next().unwrap_or("");
            !addr.is_empty()
                && addr.contains([':', '.'])
                && addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        })
}

fn merge(base: &mut Value, overlay: Value, mode: MergeMode) {
    match (base, overlay) {
        (_, Value::Null) => {}
        (b @ Value::Object(_), Value::Object(o)) if mode == MergeMode::Defaults && is_subnet_map(&o) => {
            *b = Value::Object(o);
        }
        (Value::Object(b), Value::Object(o)) => {
            // A different type for a parametrized configuration replaces it
            // entirely: the fields of the old type do not apply.
            if let (Some(bt), Some(ot)) = (type_of(b), type_of(&o))
                && bt != ot
            {
                *b = o;
                return;
            }
            for (k, v) in o {
                match find_key(b, &k) {
                    Some(existing) => merge(existing, v, mode),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (Value::Array(b), Value::Array(o)) if mode == MergeMode::Override => {
            for (i, v) in o.into_iter().enumerate() {
                if b.len() <= i {
                    b.resize(i + 1, Value::Null);
                }
                merge(&mut b[i], v, mode);
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Build the override tree for an environment variable, if it applies to the
/// component. `AKVORADO_CFG_INLET_FLOW_INPUTS_0_LISTEN=:2055` gives
/// `{FLOW: {INPUTS: [{LISTEN: ":2055"}]}}`.
fn env_override(component: &str, name: &str, value: &str) -> Option<Value> {
    let parts: Vec<&str> = name.split('_').collect();
    let component = component.to_uppercase().replace('-', "");
    if parts.len() < 4 || parts[0] != "AKVORADO" || parts[1] != "CFG" || parts[2] != component {
        return None;
    }
    let mut tree = Value::String(value.to_string());
    for part in parts[3..].iter().rev() {
        tree = match part.parse::<usize>() {
            Ok(index) => {
                let mut list = vec![Value::Null; index + 1];
                list[index] = tree;
                Value::Array(list)
            }
            Err(_) => {
                let mut m = Map::new();
                m.insert(part.to_string(), tree);
                Value::Object(m)
            }
        };
    }
    Some(tree)
}

/// Where to read the configuration from.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// A file path, an HTTP URL, or empty for the defaults.
    pub path: String,
    /// Dump the resulting configuration on stdout.
    pub dump: bool,
}

/// Fetch a configuration tree from the orchestrator.
pub async fn fetch(url: &str, component: &str) -> Result<Value, Error> {
    let mut u = reqwest::Url::parse(url).map_err(|e| Error::Fetch(format!("cannot parse configuration URL: {e}")))?;
    if u.path().is_empty() || u.path() == "/" {
        u.set_path(&format!("/api/v0/orchestrator/configuration/{component}"));
    }
    if let Some(fragment) = u.fragment().map(|f| f.to_string()) {
        let p = format!("{}/{fragment}", u.path());
        u.set_path(&p);
        u.set_fragment(None);
    }
    let resp = reqwest::get(u).await.map_err(|e| Error::Fetch(e.to_string()))?;
    let content_type =
        resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let media_type = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    if media_type != "application/x-yaml" && media_type != "application/yaml" {
        return Err(Error::NotYaml(content_type));
    }
    let body = resp.text().await.map_err(|e| Error::Fetch(format!("unable to read configuration file: {e}")))?;
    parse_str(&body)
}

/// Read the configuration tree pointed by the options. Returns the tree and
/// the files read (to watch them).
pub async fn read_tree(options: &Options, component: &str) -> Result<(Value, Vec<PathBuf>), Error> {
    let path = options.path.as_str();
    if path.is_empty() {
        Ok((Value::Null, vec![]))
    } else if path.starts_with("http://") || path.starts_with("https://") {
        Ok((fetch(path, component).await?, vec![]))
    } else {
        load_file(Path::new(path))
    }
}

/// Decode a configuration tree on top of the defaults of `T`, apply the
/// environment overrides, and validate the result.
pub fn decode<T>(tree: Value, component: &str, env: impl IntoIterator<Item = (String, String)>) -> Result<T, Error>
where
    T: Default + Serialize + DeserializeOwned + Validate,
{
    let mut tree = tree;
    T::prepare(&mut tree).map_err(Error::Invalid)?;
    let mut base = serde_json::to_value(T::default()).map_err(|e| Error::Dump(e.to_string()))?;
    merge(&mut base, tree, MergeMode::Defaults);
    // Check the file alone decodes, to report errors before overrides.
    let mut config: T = from_value(base.clone())?;
    let mut overrides: Vec<(String, String)> =
        env.into_iter().filter(|(k, _)| k.starts_with("AKVORADO_CFG_")).collect();
    overrides.sort();
    for (name, value) in overrides {
        if let Some(tree) = env_override(component, &name, &value) {
            merge(&mut base, tree, MergeMode::Override);
            config = from_value(base.clone()).map_err(|source| Error::Override { name: name.clone(), source })?;
        }
    }
    config.validate().map_err(Error::Invalid)?;
    Ok(config)
}

/// Load, decode and validate the configuration for a component. Optionally
/// dump it on stdout.
pub async fn parse<T>(options: &Options, component: &str) -> Result<(T, Vec<PathBuf>), Error>
where
    T: Default + Serialize + DeserializeOwned + Validate,
{
    let (tree, paths) = read_tree(options, component).await?;
    let config: T = decode(tree, component, std::env::vars())?;
    if options.dump {
        println!("---\n{}", dump(&config)?);
    }
    Ok((config, paths))
}

/// Dump a configuration as YAML.
pub fn dump<T: Serialize>(config: &T) -> Result<String, Error> {
    serde_norway::to_string(config).map_err(|e| Error::Dump(e.to_string()))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod merge_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subnet_maps_are_replaced() {
        let mut base = json!({"ports": {"::/0": 161}, "other": {"a": 1}});
        merge(&mut base, json!({"ports": {"10.0.0.0/8": 1161}, "other": {"b": 2}}), MergeMode::Defaults);
        assert_eq!(base, json!({"ports": {"10.0.0.0/8": 1161}, "other": {"a": 1, "b": 2}}));
    }
}
