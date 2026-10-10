// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Templates and sampling rates, per exporter. Port of
//! `outlet/flow/decoder/netflow/templates.go` and `persist.go`. The JSON
//! form is the same as the Go version, so a state file can be moved from one
//! to the other.

use super::packet::{IpfixOptionsTemplateRecord, Nfv9OptionsTemplateRecord, Template, TemplateRecord, TemplateStore};
use flowscope_common::reporter::CounterVec;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

type TemplateKey = (u16, u32, u16);
type SamplingRateKey = (u16, u32, u64);

/// Templates and sampling rates of one exporter.
pub struct Exporter {
    pub(crate) key: String,
    templates: RwLock<HashMap<TemplateKey, Template>>,
    sampling_rates: RwLock<HashMap<SamplingRateKey, u32>>,
    metrics: CounterVec,
}

impl Exporter {
    fn new(key: &str, metrics: CounterVec) -> Self {
        Exporter {
            key: key.to_string(),
            templates: RwLock::new(HashMap::new()),
            sampling_rates: RwLock::new(HashMap::new()),
            metrics,
        }
    }

    pub fn sampling_rate(&self, version: u16, obs_domain_id: u32, sampler_id: u64) -> u32 {
        self.sampling_rates.read().unwrap().get(&(version, obs_domain_id, sampler_id)).copied().unwrap_or(0)
    }

    pub fn set_sampling_rate(&self, version: u16, obs_domain_id: u32, sampler_id: u64, rate: u32) {
        self.sampling_rates.write().unwrap().insert((version, obs_domain_id, sampler_id), rate);
    }
}

impl TemplateStore for Exporter {
    fn get_template(&self, version: u16, obs_domain_id: u32, template_id: u16) -> Option<Template> {
        self.templates.read().unwrap().get(&(version, obs_domain_id, template_id)).cloned()
    }

    fn add_template(&self, version: u16, obs_domain_id: u32, template_id: u16, template: Template) {
        self.metrics
            .with_label_values(&[
                &self.key,
                &version.to_string(),
                &obs_domain_id.to_string(),
                &template_id.to_string(),
                template.kind(),
            ])
            .inc();
        self.templates.write().unwrap().insert((version, obs_domain_id, template_id), template);
    }
}

/// All exporters.
pub struct Collection {
    exporters: Mutex<HashMap<String, Arc<Exporter>>>,
    metrics: CounterVec,
}

impl Collection {
    pub fn new(metrics: CounterVec) -> Self {
        Collection { exporters: Mutex::new(HashMap::new()), metrics }
    }

    /// Get (or create) the state of an exporter.
    pub fn get(&self, key: &str) -> Arc<Exporter> {
        let mut exporters = self.exporters.lock().unwrap();
        if let Some(e) = exporters.get(key) {
            return e.clone();
        }
        let e = Arc::new(Exporter::new(key, self.metrics.clone()));
        exporters.insert(key.to_string(), e.clone());
        e
    }

    /// Serialize the state like the Go version.
    pub fn to_json(&self) -> Value {
        let exporters = self.exporters.lock().unwrap();
        let mut out = serde_json::Map::new();
        for (key, e) in exporters.iter() {
            let mut templates = BTreeMap::new();
            for ((v, o, t), template) in e.templates.read().unwrap().iter() {
                let (kind, value) = match template {
                    Template::Data(t) => ("data", serde_json::to_value(t).unwrap()),
                    Template::IpfixOptions(t) => ("ipfix-option", serde_json::to_value(t).unwrap()),
                    Template::Nfv9Options(t) => ("nfv9-option", serde_json::to_value(t).unwrap()),
                };
                templates.insert(format!("{v}-{o}-{t}"), serde_json::json!({"Type": kind, "Template": value}));
            }
            let rates: BTreeMap<String, u32> =
                e.sampling_rates.read().unwrap().iter().map(|((v, o, s), r)| (format!("{v}-{o}-{s}"), *r)).collect();
            out.insert(key.clone(), serde_json::json!({"Key": key, "Templates": templates, "SamplingRates": rates}));
        }
        Value::Object(out)
    }

    /// Restore the state saved by `to_json()` (or by the Go version).
    pub fn restore_json(&self, value: &Value) -> Result<(), String> {
        #[derive(Deserialize)]
        struct TypedTemplate {
            #[serde(rename = "Type")]
            kind: String,
            #[serde(rename = "Template")]
            template: Value,
        }
        #[derive(Deserialize)]
        struct State {
            #[serde(rename = "Templates", default)]
            templates: HashMap<String, TypedTemplate>,
            #[serde(rename = "SamplingRates", default)]
            sampling_rates: HashMap<String, u32>,
        }
        fn parse_key<T: std::str::FromStr>(k: &str) -> Result<(u16, u32, T), String> {
            let parts: Vec<&str> = k.split('-').collect();
            match parts.as_slice() {
                [a, b, c] => Ok((
                    a.parse().map_err(|_| format!("invalid key {k:?}"))?,
                    b.parse().map_err(|_| format!("invalid key {k:?}"))?,
                    c.parse().map_err(|_| format!("invalid key {k:?}"))?,
                )),
                _ => Err(format!("invalid key {k:?}")),
            }
        }
        let states: HashMap<String, State> = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let mut exporters = HashMap::new();
        for (key, state) in states {
            let e = Exporter::new(&key, self.metrics.clone());
            {
                let mut templates = e.templates.write().unwrap();
                for (k, t) in state.templates {
                    let template = match t.kind.as_str() {
                        "data" => Template::Data(
                            serde_json::from_value::<TemplateRecord>(t.template).map_err(|e| e.to_string())?,
                        ),
                        "ipfix-option" => Template::IpfixOptions(
                            serde_json::from_value::<IpfixOptionsTemplateRecord>(t.template)
                                .map_err(|e| e.to_string())?,
                        ),
                        "nfv9-option" => Template::Nfv9Options(
                            serde_json::from_value::<Nfv9OptionsTemplateRecord>(t.template)
                                .map_err(|e| e.to_string())?,
                        ),
                        other => return Err(format!("unknown type {other:?}")),
                    };
                    templates.insert(parse_key::<u16>(&k)?, template);
                }
                let mut rates = e.sampling_rates.write().unwrap();
                for (k, r) in state.sampling_rates {
                    rates.insert(parse_key::<u64>(&k)?, r);
                }
            }
            exporters.insert(key, Arc::new(e));
        }
        *self.exporters.lock().unwrap() = exporters;
        Ok(())
    }
}
