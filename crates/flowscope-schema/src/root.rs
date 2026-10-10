// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Schema component. Port of `common/schema/root.go`.

use crate::config::{Configuration, CustomDict, SkipIndexType, default_indexes};
use crate::definition::{Column, Schema, flows, register_dynamic};
use crate::key::ColumnKey;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::Arc;

/// The schema component: the schema with the configuration applied.
#[derive(Clone, Debug)]
pub struct Component {
    config: Configuration,
    schema: Arc<Schema>,
}

impl Deref for Component {
    type Target = Schema;
    fn deref(&self) -> &Schema {
        &self.schema
    }
}

/// Go's `cases.Title(language.Und)` on a single alphanumeric word.
fn title(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(|c| c.to_lowercase())).collect(),
        None => String::new(),
    }
}

impl Component {
    /// Build the schema from the provided configuration.
    pub fn new(mut config: Configuration) -> Result<Component, String> {
        let mut schema = flows();

        // Effective indexes: defaults, then user overrides, minus NoIndexes.
        let mut effective = default_indexes();
        effective.extend(std::mem::take(&mut config.indexes));
        for k in &config.no_indexes {
            effective.remove(k);
        }
        config.indexes = effective;

        for &k in &config.materialize {
            if let Some(column) = schema.lookup_column_by_key_mut(k) {
                if column.clickhouse_alias.is_empty() {
                    return Err(format!("no alias configured for {k} that can be converted to generate"));
                }
                column.clickhouse_generate_from = std::mem::take(&mut column.clickhouse_alias);
                column.clickhouse_materialized = true;
                if !column.clickhouse_materialized_type.is_empty() {
                    column.clickhouse_type = column.clickhouse_materialized_type.clone();
                }
            }
        }
        for &k in &config.enabled {
            if let Some(column) = schema.lookup_column_by_key_mut(k) {
                column.disabled = false;
            }
        }
        let primary_keys = schema.clickhouse_primary_keys.clone();
        for &k in &config.disabled {
            if let Some(column) = schema.lookup_column_by_key_mut(k) {
                if column.no_disable {
                    return Err(format!("column {:?} cannot be disabled", k.name()));
                }
                if primary_keys.contains(&k) {
                    return Err(format!("column {:?} cannot be disabled (primary key)", k.name()));
                }
                column.disabled = true;
            }
        }
        for &k in &config.disabled {
            if let Some(column) = schema.lookup_column_by_key(k) {
                for &depend in &column.depends {
                    let other_disabled = schema.lookup_column_by_key(depend).is_some_and(|c| c.disabled);
                    if !other_disabled {
                        return Err(format!(
                            "column {:?} cannot be disabled without disabling {:?}",
                            k.name(),
                            depend.name()
                        ));
                    }
                }
            }
        }
        for &k in &config.not_main_table_only {
            if let Some(column) = schema.lookup_column_by_key_mut(k) {
                column.clickhouse_main_only = false;
            }
        }
        for &k in &config.main_table_only {
            if let Some(column) = schema.lookup_column_by_key_mut(k) {
                if column.no_disable {
                    return Err(format!("column {:?} cannot be present on main table only", k.name()));
                }
                if primary_keys.contains(&k) {
                    return Err(format!("column {:?} cannot be present on main table only (primary key)", k.name()));
                }
                column.clickhouse_main_only = true;
            }
        }

        for &col in config.indexes.keys() {
            if col.0 as usize >= schema.column_index.len() {
                return Err(format!("schema indexes: unknown column key {}", col.0));
            }
            if schema.lookup_column_by_key(col).is_none() {
                return Err(format!("schema indexes: unknown column {:?}", col.name()));
            }
        }

        // Columns from custom dictionaries come after the static ones.
        let mut custom_columns = Vec::new();
        for (dname, v) in &config.custom_dictionaries {
            for d in &v.dimensions {
                if v.keys.is_empty() {
                    return Err(format!("custom dictionary {dname} has no keys, this is not supported"));
                }
                if v.layout.as_str() == "ip_trie" && (v.keys.len() > 1 || v.keys[0].r#type != "String") {
                    return Err(format!(
                        "custom dictionary {dname} uses the ip_trie layout, which needs a single key of type String"
                    ));
                }
                if v.keys.len() > 1 {
                    for kv in &v.keys {
                        if kv.match_dimension.is_empty() && kv.match_dimension_suffix.is_empty() {
                            return Err(format!(
                                "custom dictionary {dname} has more than one key, but key {} has neither MatchDimension nor MatchDimensionSuffix set",
                                kv.name
                            ));
                        }
                    }
                }
                let mut matching = Vec::new();
                for kv in &v.keys {
                    if !kv.match_dimension.is_empty() {
                        matching.push(kv.match_dimension.clone());
                        continue;
                    }
                    if !kv.match_dimension_suffix.is_empty() {
                        matching.push(format!("{d}{}", kv.match_dimension_suffix));
                    }
                }
                let matching = if matching.is_empty() { d.clone() } else { format!("({})", matching.join(",")) };

                for a in &v.attributes {
                    let label = if a.label.is_empty() { title(&a.name) } else { a.label.clone() };
                    let name = format!("{d}{label}");
                    let key = ColumnKey(ColumnKey::Last.0 + schema.dynamic_columns);
                    let parser_type = match a.r#type.as_str() {
                        "IPv6" => "ip",
                        "String" => "string",
                        "UInt8" | "UInt16" | "UInt32" | "UInt64" => "uint",
                        _ => "",
                    };
                    custom_columns.push(Column {
                        key,
                        name: name.clone(),
                        parser_type,
                        clickhouse_type: format!("LowCardinality({})", a.r#type),
                        clickhouse_generate_from: format!("dictGet('custom_dict_{dname}', '{}', {matching})", a.name),
                        ..Default::default()
                    });
                    register_dynamic(key, &name);
                    schema.dynamic_columns += 1;
                }
            }
        }
        // Go panics in finalize() when a Src/InIf column has no Dst/OutIf
        // counterpart. Report an error instead.
        for c in &custom_columns {
            for (source, target) in [("Src", "Dst"), ("InIf", "OutIf")] {
                if let Some(rest) = c.name.strip_prefix(source) {
                    let other = format!("{target}{rest}");
                    if !custom_columns.iter().any(|o| o.name == other) {
                        return Err(format!("missing name mapping for {other:?}"));
                    }
                }
            }
        }
        schema.columns.extend(custom_columns);

        Ok(Component { config, schema: Arc::new(schema.finalize()) })
    }

    /// Schema with the default configuration.
    pub fn default_schema() -> Component {
        Component::new(Configuration::default()).expect("default schema is valid")
    }

    /// Enable every column. Only meant for tests.
    pub fn enable_all_columns(mut self) -> Component {
        self.schema = Arc::new((*self.schema).clone().enable_all_columns());
        self
    }

    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }

    /// Create a new flow message with an empty batch.
    pub fn new_flow_message(&self) -> crate::FlowMessage {
        self.schema.new_flow_message()
    }

    pub fn custom_dict_config(&self) -> &BTreeMap<String, CustomDict> {
        &self.config.custom_dictionaries
    }

    pub fn skip_indexes(&self) -> &BTreeMap<ColumnKey, SkipIndexType> {
        &self.config.indexes
    }
}
