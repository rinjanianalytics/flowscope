// SPDX-FileCopyrightText: 2024 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Turn gNMI notifications into flat events and events into interface
//! information. Port of `subscribe.go`, `collector.go` (state update) and
//! `utils.go`.

use super::pb::gnmi::{self, typed_value::Value as TypedValue};
use crate::metadata::Interface;
use crate::metadata::gnmi_config::{IfSpeedPathUnit, Model};
use std::collections::HashMap;

/// A value received in a subscription. Deletions are not handled: the
/// state is rebuilt on each poll.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// Path without keys.
    pub path: String,
    /// Comma-separated keys.
    pub keys: String,
    pub value: String,
}

/// Events of a subscribe response.
pub fn subscribe_response_to_events(response: &gnmi::SubscribeResponse) -> Vec<Event> {
    let mut events = vec![];
    let Some(gnmi::subscribe_response::Response::Update(n)) = &response.response else { return events };
    let prefix = path_to_event(n.prefix.as_ref(), &Event::default());
    for u in &n.update {
        let mut ev = path_to_event(u.path.as_ref(), &prefix);
        let Some(value) = u.val.as_ref().and_then(|v| v.value.as_ref()) else { continue };
        let json = match value {
            TypedValue::AsciiVal(s) | TypedValue::StringVal(s) => {
                ev.value = s.clone();
                events.push(ev);
                continue;
            }
            TypedValue::IntVal(i) => {
                ev.value = i.to_string();
                events.push(ev);
                continue;
            }
            TypedValue::UintVal(u) => {
                ev.value = u.to_string();
                events.push(ev);
                continue;
            }
            TypedValue::JsonIetfVal(j) | TypedValue::JsonVal(j) => j,
            _ => continue,
        };
        if json.is_empty() {
            events.push(ev);
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else { continue };
        json_append_to_events(&mut events, ev, &value);
    }
    events
}

/// Events of several subscribe responses.
pub fn subscribe_responses_to_events(responses: &[gnmi::SubscribeResponse]) -> Vec<Event> {
    responses.iter().flat_map(subscribe_response_to_events).collect()
}

/// Format a JSON number like Go does after decoding it as a float64 and
/// converting it to an int64.
fn json_number(n: &serde_json::Number) -> String {
    match (n.as_i64(), n.as_u64(), n.as_f64()) {
        (Some(i), _, _) => i.to_string(),
        (_, Some(u), _) => (u as f64 as i64).to_string(),
        (_, _, Some(f)) => (f as i64).to_string(),
        _ => n.to_string(),
    }
}

/// Append the events for a JSON value below an event.
fn json_append_to_events(events: &mut Vec<Event>, ev: Event, value: &serde_json::Value) {
    use serde_json::Value;
    match value {
        // Lists: each element is a map. Its scalar values at the top level
        // are the keys (OpenConfig puts list keys there).
        Value::Array(items) => {
            for item in items {
                let Value::Object(map) = item else { continue };
                let mut keys: Vec<String> = map
                    .iter()
                    .filter_map(|(k, v)| match v {
                        Value::String(s) => Some(format!("{k}={s}")),
                        Value::Number(n) => Some(format!("{k}={}", json_number(n))),
                        _ => None,
                    })
                    .collect();
                keys.sort();
                let key_str = keys.join(",");
                let mut current = ev.clone();
                if !key_str.is_empty() {
                    current.keys =
                        if current.keys.is_empty() { key_str } else { format!("{},{key_str}", current.keys) };
                }
                json_append_to_events(events, current, item);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let mut current = ev.clone();
                current.path = join_path(&current.path, k);
                json_append_to_events(events, current, v);
            }
        }
        Value::Number(n) => events.push(Event { value: json_number(n), ..ev }),
        Value::String(s) => events.push(Event { value: s.clone(), ..ev }),
        _ => {}
    }
}

/// Join path elements like Go's path.Join.
fn join_path(base: &str, elem: &str) -> String {
    let joined = if base.is_empty() { elem.to_string() } else { format!("{base}/{elem}") };
    // Clean: no empty or "." elements.
    let absolute = joined.starts_with('/');
    let parts: Vec<&str> = joined.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let cleaned = parts.join("/");
    if absolute { format!("/{cleaned}") } else { cleaned }
}

/// Turn a gNMI path into an event without value, below a prefix.
fn path_to_event(path: Option<&gnmi::Path>, prefix: &Event) -> Event {
    let Some(path) = path else { return prefix.clone() };
    let mut p = prefix.path.clone();
    let mut keys = prefix.keys.clone();
    for elem in &path.elem {
        p.push('/');
        // Remove the namespace.
        p.push_str(elem.name.split_once(':').map(|(_, after)| after).unwrap_or(&elem.name));
        if !elem.key.is_empty() {
            let mut k: Vec<String> = elem.key.iter().map(|(k, v)| format!("{k}={v}")).collect();
            k.sort();
            keys.push(',');
            keys.push_str(&k.join(","));
        }
    }
    Event { path: p, keys: keys.trim_start_matches(',').to_string(), value: String::new() }
}

/// State of an exporter, built from events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExporterState {
    pub name: String,
    pub interfaces: HashMap<u32, Interface>,
}

impl ExporterState {
    /// Rebuild the interfaces from the events of a poll.
    pub fn update(&mut self, events: &[Event], model: &Model) {
        // First pass: system name, indexes and speeds by keys. A key is
        // assumed to point to the same interface everywhere.
        let mut indexes: HashMap<&str, u32> = HashMap::new();
        let mut speeds: HashMap<&str, u64> = HashMap::new();
        let mut rest = vec![];
        'outer: for ev in events {
            if model.system_name_paths.contains(&ev.path) {
                self.name = ev.value.clone();
                continue;
            }
            if model.if_index_paths.contains(&ev.path) {
                match ev.value.parse::<u32>() {
                    Ok(index) => {
                        indexes.insert(&ev.keys, index);
                    }
                    Err(_) => continue 'outer,
                }
            }
            for sp in &model.if_speed_paths {
                if ev.path == sp.path {
                    match convert_speed(&ev.value, sp.unit) {
                        Ok(speed) => {
                            speeds.insert(&ev.keys, speed);
                        }
                        Err(_) => continue 'outer,
                    }
                }
            }
            rest.push(ev);
        }

        // Second pass: names and descriptions.
        self.interfaces = HashMap::new();
        for ev in rest {
            if model.if_name_paths.contains(&ev.path) {
                if let Some(&index) = indexes.get(ev.keys.as_str()) {
                    self.interfaces.entry(index).or_default().name = ev.value.clone();
                }
                continue;
            }
            if model.if_description_paths.contains(&ev.path)
                && let Some(&index) = indexes.get(ev.keys.as_str())
            {
                self.interfaces.entry(index).or_default().description = ev.value.clone();
            }
        }

        // Third pass: unnamed interfaces and speeds.
        for (keys, index) in indexes {
            let mut iface = self.interfaces.get(&index).cloned().unwrap_or_default();
            if iface.name.is_empty() && !model.if_name_keys.is_empty() {
                'inner: for key in keys.split(',') {
                    for name in &model.if_name_keys {
                        if let Some(v) = key.strip_prefix(&format!("{name}=")) {
                            iface.name = v.to_string();
                            break 'inner;
                        }
                    }
                }
            }
            if iface.name.is_empty() {
                self.interfaces.remove(&index);
                continue;
            }
            // Speed, from the most specific keys to the least specific.
            let mut k = keys;
            while iface.speed == 0 && !k.is_empty() {
                iface.speed = speeds.get(k).copied().unwrap_or(0);
                k = &k[..k.rfind(',').unwrap_or(0)];
            }
            self.interfaces.insert(index, iface);
        }
    }
}

/// Convert a speed to Mbps.
pub fn convert_speed(value: &str, unit: IfSpeedPathUnit) -> Result<u64, String> {
    let parse = |s: &str| s.parse::<u32>().map(u64::from).map_err(|e| e.to_string());
    match unit {
        // The Go version parsed a 32-bit value: speeds above 4.29 Gbps in
        // bits per second (100G LAGs) were rejected.
        IfSpeedPathUnit::Bps => Ok(value.parse::<u64>().map_err(|e| e.to_string())? / 1_000_000),
        IfSpeedPathUnit::Mbps => parse(value),
        IfSpeedPathUnit::Human => convert_human_speed(value),
        IfSpeedPathUnit::Ethernet => {
            if value == "SPEED_UNKNOWN" {
                return Ok(0);
            }
            match value.strip_prefix("SPEED_").and_then(|v| v.strip_suffix('B')) {
                Some(v) => convert_human_speed(v),
                None => Err(format!("unknown speed {value}")),
            }
        }
    }
}

/// Convert a speed like 10G or 100M to Mbps. The Go version panicked on an
/// empty value ("SPEED_B").
fn convert_human_speed(value: &str) -> Result<u64, String> {
    let Some(multiplier) = value.chars().last() else { return Err("empty speed".into()) };
    let number: u64 = value[..value.len() - multiplier.len_utf8()].parse::<u32>().map_err(|e| e.to_string())?.into();
    match multiplier {
        'K' => Ok(number / 1000),
        'M' => Ok(number),
        'G' => Ok(number * 1000),
        'T' => Ok(number * 1_000_000),
        m => Err(format!("unknown multiplier {m}")),
    }
}

/// Parse a path like gnmic: "origin:/a/b[k=v]/c".
pub fn parse_path(s: &str) -> Result<gnmi::Path, String> {
    let mut path = gnmi::Path::default();
    let mut rest = s;
    // An origin is before the first ":" when it comes before any "/".
    if let Some(colon) = rest.find(':')
        && rest.find('/').is_none_or(|slash| colon < slash)
        && rest.find('[').is_none_or(|bracket| colon < bracket)
    {
        path.origin = rest[..colon].to_string();
        rest = &rest[colon + 1..];
    }
    let mut elems = vec![];
    let mut current = String::new();
    let mut depth = 0;
    for c in rest.chars() {
        match c {
            '[' => {
                depth += 1;
                current.push(c);
            }
            ']' => {
                depth -= 1;
                current.push(c);
            }
            '/' if depth == 0 => {
                if !current.is_empty() {
                    elems.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if depth != 0 {
        return Err(format!("unbalanced brackets in {s:?}"));
    }
    if !current.is_empty() {
        elems.push(current);
    }
    for e in elems {
        let (name, keys) = match e.find('[') {
            Some(i) => (&e[..i], &e[i..]),
            None => (e.as_str(), ""),
        };
        let mut elem = gnmi::PathElem { name: name.to_string(), key: HashMap::new() };
        for kv in keys.split(']').filter(|k| !k.is_empty()) {
            let kv = kv.strip_prefix('[').ok_or_else(|| format!("invalid key in {s:?}"))?;
            let (k, v) = kv.split_once('=').ok_or_else(|| format!("invalid key in {s:?}"))?;
            elem.key.insert(k.to_string(), v.to_string());
        }
        path.elem.push(elem);
    }
    Ok(path)
}
