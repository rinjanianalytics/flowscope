// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Compare what is extracted from the test GeoIP databases with the Go
//! version (`go run ./tests/golden/geoip . > tests/golden/testdata/geoip.json`).

use flowscope_common::daemon::Daemon;
use flowscope_common::reporter::Reporter;
use flowscope_outlet::geoip::{Configuration, GeoIp};
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn matches_go() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("tests/golden/testdata/geoip.json")).unwrap()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for (file, expected) in golden.as_object().unwrap() {
        let path = root.join(file).to_string_lossy().to_string();
        for which in ["asn", "geo"] {
            let config = if which == "asn" {
                Configuration { asn_database: vec![path.clone()], ..Default::default() }
            } else {
                Configuration { geo_database: vec![path.clone()], ..Default::default() }
            };
            let g = GeoIp::new(&Reporter::new_mock(), config);
            g.start(&Daemon::new()).unwrap();
            let mut got = Vec::new();
            if which == "asn" {
                g.iter_asn(&mut |p, asn| got.push(json!({"prefix": p, "asn": asn})));
            } else {
                g.iter_geo(&mut |p, info| {
                    let mut e = serde_json::Map::new();
                    e.insert("prefix".into(), json!(p));
                    for (k, v) in [("country", info.country), ("state", info.state), ("city", info.city)] {
                        if !v.is_empty() {
                            e.insert(k.into(), json!(v));
                        }
                    }
                    got.push(Value::Object(e));
                });
            }
            let expected = expected[which].as_array().unwrap();
            assert_eq!(got.len(), expected.len(), "{file} {which}: number of entries");
            for (g, e) in got.iter().zip(expected) {
                assert_eq!(g, e, "{file} {which}");
            }
        }
    }
}
