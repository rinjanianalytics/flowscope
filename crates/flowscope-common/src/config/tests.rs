// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Port of `cmd/akvorado/config_test.go` and `common/helpers/yaml` tests.

use super::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
#[derive(Default)]
struct Dummy {
    module1: Module1,
    module2: Module2,
}

impl Validate for Dummy {
    fn validate(&self) -> Result<(), String> {
        if self.module1.topic.len() < 3 {
            return Err("Module1.Topic must be at least 3 characters".into());
        }
        if self.module1.workers < 1 {
            return Err("Module1.Workers must be at least 1".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct Module1 {
    listen: String,
    topic: String,
    workers: i64,
}

impl Default for Module1 {
    fn default() -> Self {
        Module1 { listen: "127.0.0.1:8080".into(), topic: "nothingness".into(), workers: 100 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct Module2 {
    details: Details,
    elements: Vec<Element>,
    stuff: String,
}

impl Default for Module2 {
    fn default() -> Self {
        Module2 {
            details: Details::default(),
            elements: vec![Element { name: "el1".into(), gauge: 10 }, Element { name: "el2".into(), gauge: 11 }],
            stuff: "hello".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct Details {
    workers: i64,
    interval_value: Duration,
}

impl Default for Details {
    fn default() -> Self {
        Details { workers: 1, interval_value: Duration::from_secs(60) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct Element {
    name: String,
    gauge: i64,
}

fn env(vars: &[(&str, &str)]) -> Vec<(String, String)> {
    vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn defaults_only() {
    let got: Dummy = decode(Value::Null, "dummy", vec![]).unwrap();
    assert_eq!(got, Dummy::default());
}

#[test]
fn env_override() {
    let tree = parse_str(
        "---
module1:
 topic: flows
module2:
 details:
  workers: 5
  interval-value: 20m
",
    )
    .unwrap();
    let vars = env(&[
        ("FLOWSCOPE_CFG_DUMMY_MODULE1_LISTEN", "127.0.0.1:9000"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE1_TOPIC", "something"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_DETAILS_INTERVALVALUE", "10m"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_STUFF", "bye"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_ELEMENTS_0_NAME", "something"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_ELEMENTS_0_GAUGE", "18"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_ELEMENTS_1_NAME", "something else"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_ELEMENTS_1_GAUGE", "7"),
        ("FLOWSCOPE_CFG_OTHER_MODULE1_TOPIC", "ignored"),
    ]);
    let got: Dummy = decode(tree, "dummy", vars).unwrap();
    let expected = Dummy {
        module1: Module1 { listen: "127.0.0.1:9000".into(), topic: "something".into(), workers: 100 },
        module2: Module2 {
            details: Details { workers: 5, interval_value: Duration::from_secs(600) },
            elements: vec![
                Element { name: "something".into(), gauge: 18 },
                Element { name: "something else".into(), gauge: 7 },
            ],
            stuff: "bye".into(),
        },
    };
    assert_eq!(got, expected);
}

/// A variable of Akvorado stops the service, whatever the component it is
/// for: it would be ignored otherwise.
#[test]
fn env_override_old_prefix_refused() {
    let vars = env(&[
        ("AKVORADO_CFG_DUMMY_MODULE1_TOPIC", "old"),
        ("FLOWSCOPE_CFG_DUMMY_MODULE2_STUFF", "new"),
        ("AKVORADO_CFG_OTHER_MODULE1_TOPIC", "old"),
    ]);
    let error = decode::<Dummy>(Value::Null, "dummy", vars).unwrap_err().to_string();
    assert_eq!(
        error,
        "these variables of Akvorado are not read: rename AKVORADO_CFG_DUMMY_MODULE1_TOPIC, \
         AKVORADO_CFG_OTHER_MODULE1_TOPIC to start with FLOWSCOPE_CFG_"
    );
    let vars = env(&[("AKVORADO_CFG_DUMMY_MODULE1_TOPIC", "old")]);
    let error = decode::<Dummy>(Value::Null, "dummy", vars).unwrap_err().to_string();
    assert_eq!(
        error,
        "this variable of Akvorado is not read: rename AKVORADO_CFG_DUMMY_MODULE1_TOPIC to start with FLOWSCOPE_CFG_"
    );
}

#[test]
fn env_override_extends_list() {
    let vars = env(&[("FLOWSCOPE_CFG_DUMMY_MODULE2_ELEMENTS_3_NAME", "fourth")]);
    let got: Dummy = decode(Value::Null, "dummy", vars).unwrap();
    assert_eq!(
        got.module2.elements,
        vec![
            Element { name: "el1".into(), gauge: 10 },
            Element { name: "el2".into(), gauge: 11 },
            Element::default(),
            Element { name: "fourth".into(), gauge: 0 },
        ]
    );
}

#[test]
fn list_replaces_default() {
    let tree = parse_str("module2:\n  elements:\n    - name: only\n").unwrap();
    let got: Dummy = decode(tree, "dummy", vec![]).unwrap();
    assert_eq!(got.module2.elements, vec![Element { name: "only".into(), gauge: 0 }]);
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default, deny_unknown_fields)]
struct Modules {
    modules: Vec<Dummy>,
}

impl Validate for Modules {}

#[test]
fn default_in_slice() {
    let tree = parse_str("modules:\n- module1:\n    topic: flows1\n- module1:\n    topic: flows2\n").unwrap();
    let got: Modules = decode(tree, "dummy", vec![]).unwrap();
    let mut first = Dummy::default();
    first.module1.topic = "flows1".into();
    let mut second = Dummy::default();
    second.module1.topic = "flows2".into();
    assert_eq!(got, Modules { modules: vec![first, second] });
}

#[test]
fn validation() {
    let tree = parse_str("module1:\n  topic: fl\n").unwrap();
    let err = decode::<Dummy>(tree, "dummy", vec![]).unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err}");
}

#[test]
fn unknown_key() {
    let tree = parse_str("module1:\n  topics: flows\n").unwrap();
    let err = decode::<Dummy>(tree, "dummy", vec![]).unwrap_err();
    assert_eq!(err.to_string(), "unable to parse configuration: module1.topics: invalid key \"topics\"");
}

#[test]
fn zero_overrides_default() {
    let tree = parse_str("module1:\n  workers: 0\n").unwrap();
    let got = decode::<Dummy>(tree, "dummy", vec![]).unwrap_err();
    assert!(matches!(got, Error::Invalid(_)));
    let tree = parse_str("module2:\n  details:\n    workers: 0\n").unwrap();
    let got: Dummy = decode(tree, "dummy", vec![]).unwrap();
    assert_eq!(got.module2.details.workers, 0);
}

#[test]
fn dump_roundtrip() {
    let config = Dummy::default();
    let out = dump(&config).unwrap();
    assert!(out.contains("interval-value: 1m0s"), "{out}");
    let back: Dummy = decode(parse_str(&out).unwrap(), "dummy", vec![]).unwrap();
    assert_eq!(back, config);
}

#[test]
fn go_style_keys() {
    // The Go orchestrator dumps keys in lowercase without dashes.
    let tree = parse_str("module2:\n  details:\n    intervalvalue: 5s\n").unwrap();
    let got: Dummy = decode(tree, "dummy", vec![]).unwrap();
    assert_eq!(got.module2.details.interval_value, Duration::from_secs(5));
}

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/yaml").join(name)
}

#[test]
fn include() {
    let (got, paths) = load_file(&testdata("base.yaml")).unwrap();
    assert_eq!(
        got,
        json!({
            "file1": {"name": "1.yaml"},
            "file2": {"name": "2.yaml"},
            "nested": {"file1": {"name": "1.yaml"}},
            "list1": ["el1", "el2", "el3"],
            "list2": [{"protocol": "tcp", "size": 1300}, "el2", "el3"],
        })
    );
    let mut names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
    names.sort();
    assert_eq!(names, ["1.yaml", "2.yaml", "base.yaml", "list1.yaml", "list2.yaml", "nested.yaml"]);
}

#[test]
fn include_merge() {
    let (got, _) = load_file(&testdata("merge.yaml")).unwrap();
    assert_eq!(got, json!({"name": "1.yaml", "other": "value"}));
}

/// An anchor using a merge key is merged in another mapping: the demo
/// exporter configuration does that.
#[test]
fn nested_merge_keys() {
    let got = parse_str(
        "base: &base\n  a: 1\nmiddle: &middle\n  b: 2\n  <<: *base\nflows:\n  - c: 3\n    <<: [*middle]\n  - <<: *middle\n    a: 9\n",
    )
    .unwrap();
    assert_eq!(got["flows"], json!([{"a": 1, "b": 2, "c": 3}, {"a": 9, "b": 2}]));
}

#[test]
fn type_change_replaces() {
    let mut base = json!({"inputs": {"type": "udp", "listen": ":2055", "workers": 4}});
    merge(&mut base, json!({"inputs": {"type": "file", "paths": ["a"]}}), MergeMode::Defaults);
    assert_eq!(base, json!({"inputs": {"type": "file", "paths": ["a"]}}));
    let mut base = json!({"inputs": {"type": "udp", "listen": ":2055", "workers": 4}});
    merge(&mut base, json!({"inputs": {"Listen": ":2056"}}), MergeMode::Defaults);
    assert_eq!(base, json!({"inputs": {"type": "udp", "listen": ":2056", "workers": 4}}));
}
