// SPDX-FileCopyrightText: 2023 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Database migrations. Port of `orchestrator/clickhouse/migrations.go` and
//! `migrations_helpers.go`.
//!
//! Each step checks what exists and skips itself when nothing has to change.
//! The flow tables are updated in place (ALTER), the other tables and the
//! views are recreated when they differ.

use super::{ClickHouse, Configuration, ResolutionConfiguration, TableSetting};
use crate::sql::{self, Engine, quote_identifier_if_needed as ident, quote_string};
use flowscope_clickhouse::{Connection, Setting, Value};
use flowscope_common::clickhousedb::ClickHouseDb;
use flowscope_schema::{ColumnKey, TableOption};
use std::collections::BTreeMap;
use std::net::UdpSocket;

/// A step that did nothing.
const SKIP: &str = "migration: skip this step";

type StepResult = Result<(), String>;

fn skip() -> StepResult {
    Err(SKIP.into())
}

fn string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Value::UInt(u) => u.to_string(),
        Value::Int(i) => i.to_string(),
        other => format!("{other:?}"),
    }
}

/// Name of a table for a resolution.
fn flows_table(resolution: &ResolutionConfiguration) -> String {
    if resolution.interval.is_zero() { "flows".into() } else { format!("flows_{}", resolution.interval) }
}

/// Default settings of the flow tables.
fn default_table_settings() -> BTreeMap<String, TableSetting> {
    BTreeMap::from([
        ("index_granularity".to_string(), TableSetting::Int(8192)),
        ("ttl_only_drop_parts".to_string(), TableSetting::Int(1)),
    ])
}

/// Table settings: the defaults with the configured ones, sorted by name.
fn table_settings(extra: &BTreeMap<String, TableSetting>) -> Vec<(String, String)> {
    let mut merged = default_table_settings();
    merged.extend(extra.clone());
    merged.into_iter().map(|(k, v)| (k, v.expr())).collect()
}

/// Context of a migration run.
pub(super) struct Migration<'a> {
    db: &'a ClickHouseDb,
    config: Configuration,
    schema: &'a flowscope_schema::Schema,
    custom_dicts: &'a BTreeMap<String, flowscope_schema::CustomDict>,
    skip_indexes: &'a BTreeMap<ColumnKey, flowscope_schema::SkipIndexType>,
    database: String,
    cluster: String,
    shards: u64,
    applied: u64,
    not_applied: u64,
}

impl ClickHouse {
    pub(super) fn migrate_database(&self) -> Result<(), String> {
        let db = self.db.as_ref().ok_or("no ClickHouse")?;
        let mut config = self.config.lock().unwrap().clone();
        if config.orchestrator_url.is_empty() {
            config.orchestrator_url = guess_http_base_url("1.1.1.1", *self.http_port.lock().unwrap())?;
            self.config.lock().unwrap().orchestrator_url = config.orchestrator_url.clone();
        }
        let schema = self.schema.schema().clone();
        let mut m = Migration {
            db,
            config,
            schema: &schema,
            custom_dicts: self.schema.custom_dict_config(),
            skip_indexes: self.schema.skip_indexes(),
            database: db.database_name().to_string(),
            cluster: db.cluster_name().to_string(),
            shards: 0,
            applied: 0,
            not_applied: 0,
        };
        let result = m.run();
        self.metrics.migrations_applied.inc_by(m.applied as f64);
        self.metrics.migrations_not_applied.inc_by(m.not_applied as f64);
        result?;
        self.metrics.migrations_running.set(0.0);
        self.done.send_replace(true);
        tracing::info!("database migration done");
        m.reload_dictionaries();
        Ok(())
    }
}

/// Guess the URL ClickHouse can use to reach us: our address towards the
/// Internet, with the port of the HTTP server.
fn guess_http_base_url(ip: &str, http_port: u16) -> Result<String, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("cannot get our IP address: {e}"))?;
    socket.connect((ip, 80)).map_err(|e| format!("cannot get our IP address: {e}"))?;
    let local = socket.local_addr().map_err(|e| format!("cannot get our IP address: {e}"))?;
    let base = format!("http://{}", std::net::SocketAddr::new(local.ip(), http_port));
    tracing::debug!("detected base URL is {base}");
    Ok(base)
}

impl Migration<'_> {
    /// Settings for the statements.
    fn settings(&self, extra: &[(&str, &str)]) -> Vec<Setting> {
        let mut s: Vec<Setting> = extra.iter().map(|(k, v)| Setting::new(k, v)).collect();
        if !self.cluster.is_empty() {
            s.push(Setting::new("alter_sync", 2));
        }
        s
    }

    fn exec(&self, sql: &str, extra: &[(&str, &str)]) -> Result<(), String> {
        tracing::debug!("execute: {sql}");
        let settings = self.settings(extra);
        self.db.with_conn(|c| c.execute(sql, &settings)).map_err(|e| e.to_string())
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<Value>>, String> {
        let settings = self.settings(&[]);
        self.db
            .with_conn(|c: &mut Connection| c.query(sql, &settings))
            .map(|r| r.iter_rows().collect())
            .map_err(|e| e.to_string())
    }

    /// ON CLUSTER clause, when in a cluster.
    fn on_cluster(&self) -> String {
        if self.cluster.is_empty() { String::new() } else { format!(" ON CLUSTER {}", ident(&self.cluster)) }
    }

    /// A table of our database.
    fn table(&self, name: &str) -> String {
        format!("{}.{}", ident(&self.database), ident(name))
    }

    /// Table name of the local table (in a sharded cluster).
    fn local_table(&self, name: &str) -> String {
        if !self.cluster.is_empty() && self.shards > 1 { format!("{name}_local") } else { name.into() }
    }

    fn wrap(&mut self, result: StepResult) -> Result<(), String> {
        match result {
            Ok(()) => {
                self.applied += 1;
                Ok(())
            }
            Err(e) if e == SKIP => {
                self.not_applied += 1;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn run(&mut self) -> Result<(), String> {
        let rows = self.query("SELECT version()")?;
        let version = rows.first().and_then(|r| r.first()).map(string).ok_or("unable to parse database settings")?;
        super::version::validate_version(&version).map_err(|e| format!("incorrect ClickHouse version: {e}"))?;

        if !self.cluster.is_empty() {
            let rows = self.query(&format!(
                "SELECT countDistinct(shard_num) AS num FROM system.clusters WHERE cluster = {}",
                quote_string(&self.cluster)
            ))?;
            self.shards = rows.first().and_then(|r| r.first()).and_then(|v| v.as_u64()).unwrap_or(0);
            if self.shards == 0 {
                return Err("cannot get the number of shards for the cluster".into());
            }
        }

        // Dictionaries
        let attr = |name: &str, t: &str, injective: bool| Attribute {
            name: name.into(),
            r#type: t.into(),
            default: None,
            injective,
        };
        let builtin = [
            ("asns", "hashed", vec![attr("asn", "UInt32", true), attr("name", "String", false)], vec!["asn"]),
            (
                "protocols",
                "hashed",
                vec![attr("proto", "UInt8", true), attr("name", "String", false), attr("description", "String", false)],
                vec!["proto"],
            ),
            (
                "icmp",
                "complex_key_hashed",
                vec![
                    attr("proto", "UInt8", false),
                    attr("type", "UInt8", false),
                    attr("code", "UInt8", false),
                    attr("name", "String", false),
                ],
                vec!["proto", "type", "code"],
            ),
            ("tcp", "hashed", vec![attr("port", "UInt16", true), attr("name", "String", false)], vec!["port"]),
            ("udp", "hashed", vec![attr("port", "UInt16", true), attr("name", "String", false)], vec!["port"]),
        ];
        for (name, layout, attributes, keys) in builtin {
            let r = self.create_dictionary(name, layout, &attributes, &keys);
            self.wrap(r)?;
        }
        // Custom dictionaries
        let customs: Vec<(String, flowscope_schema::CustomDict)> =
            self.custom_dicts.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, dict) in customs {
            let mut attributes = vec![];
            let mut keys = vec![];
            for k in &dict.keys {
                attributes.push(Attribute {
                    name: k.name.clone(),
                    r#type: k.r#type.clone(),
                    default: None,
                    injective: false,
                });
                keys.push(k.name.as_str());
            }
            for a in &dict.attributes {
                let default = if a.default.is_empty() { "None".to_string() } else { a.default.clone() };
                attributes.push(Attribute {
                    name: a.name.clone(),
                    r#type: a.r#type.clone(),
                    default: Some(default),
                    injective: false,
                });
            }
            let r = self.create_dictionary(&format!("custom_dict_{name}"), dict.layout.as_str(), &attributes, &keys);
            self.wrap(r)?;
        }

        // Flow tables
        for resolution in self.config.resolutions.clone() {
            let r = self.create_or_update_flows_table(&resolution);
            self.wrap(r)?;
            let r = self.create_distributed_table(&flows_table(&resolution));
            self.wrap(r)?;
            let r = self.create_flows_consumer_view(&resolution);
            self.wrap(r)?;
        }

        // Remaining tables
        let r = self.create_exporters_table();
        self.wrap(r)?;
        let r = self.create_exporters_consumer_view();
        self.wrap(r)?;
        let r = self.create_raw_flows_table();
        self.wrap(r)?;
        let r = self.create_raw_flows_consumer_view();
        self.wrap(r)?;
        Ok(())
    }

    /// A column of system.tables for a table. Empty when the table does not
    /// exist.
    fn table_column(&self, table: &str, column: &str) -> Result<String, String> {
        let rows = self.query(&format!(
            "SELECT {column} FROM system.tables WHERE name = {} AND database = {}",
            quote_string(table),
            quote_string(&self.database)
        ))?;
        Ok(rows.first().and_then(|r| r.first()).map(string).unwrap_or_default())
    }

    /// Whether the table exists and matches the provided statement.
    /// `column` is "create_table_query" or "as_select".
    fn table_already_exists(&self, table: &str, column: &str, target: &str) -> Result<bool, String> {
        let mut existing = self.table_column(table, column)?;
        // ClickHouse adds the database in front of the dictionaries, we do not.
        for function in ["dictGetOrDefault", "dictGet"] {
            existing = existing.replace(&format!("{function}('{}.", self.database), &format!("{function}('"));
        }
        let existing = sql::strip_table_settings(&existing);
        if sql::matches(target, &existing) {
            return Ok(true);
        }
        tracing::debug!(target, existing, "table {table} state difference detected");
        Ok(false)
    }

    /// The ZooKeeper path of an existing replicated table, or the default one.
    fn existing_zk_path(&self, table: &str) -> String {
        match self.table_engine(table) {
            Ok(Some(engine)) if engine.replicated_path().is_some() => engine.replicated_path().unwrap(),
            Ok(_) => format!("/clickhouse/tables/shard-{{shard}}/{}/{table}", self.database),
            Err(e) => {
                tracing::warn!(error = %e, "cannot get the engine of table {table}");
                format!("/clickhouse/tables/shard-{{shard}}/{}/{table}", self.database)
            }
        }
    }

    /// A MergeTree engine, replicated in a cluster.
    fn merge_tree_engine(&self, table: &str, variant: &str, args: &[String]) -> String {
        let mut args = args.to_vec();
        let name = if self.cluster.is_empty() {
            format!("{variant}MergeTree")
        } else {
            let path = self.existing_zk_path(table);
            args.splice(0..0, [quote_string(&path), quote_string("replica-{replica}")]);
            format!("Replicated{variant}MergeTree")
        };
        if args.is_empty() { name } else { format!("{name}({})", args.join(", ")) }
    }

    fn table_engine(&self, table: &str) -> Result<Option<Engine>, String> {
        let full = self.table_column(table, "engine_full")?;
        if full.is_empty() {
            return Ok(None);
        }
        Engine::parse(&full).map(Some).map_err(|e| format!("cannot parse the engine of table {table}: {e}"))
    }

    fn create_dictionary(&self, name: &str, layout: &str, attributes: &[Attribute], keys: &[&str]) -> StepResult {
        let url = format!("{}/api/v0/orchestrator/clickhouse/{name}.csv", self.config.orchestrator_url);
        let mut source = format!("URL {} FORMAT {}", quote_string(&url), quote_string("CSVWithNames"));
        if let Some(auth) = &self.config.orchestrator_basic_auth {
            source.push_str(&format!(
                " CREDENTIALS(user {} password {})",
                quote_string(&auth.username),
                quote_string(&auth.password)
            ));
        }
        let attributes: Vec<String> = attributes
            .iter()
            .map(|a| {
                let mut s = format!("{} {}", ident(&a.name), a.r#type);
                if let Some(d) = &a.default {
                    s.push_str(&format!(" DEFAULT {}", quote_string(d)));
                }
                if a.injective {
                    s.push_str(" INJECTIVE");
                }
                s
            })
            .collect();
        let keys: Vec<String> = keys.iter().map(|k| ident(k)).collect();
        let body = format!(
            "({}) PRIMARY KEY {} SOURCE(HTTP({source})) LIFETIME(MIN 0 MAX 3600) LAYOUT({}()) SETTINGS(format_csv_allow_single_quotes = 0)",
            attributes.join(", "),
            keys.join(", "),
            layout.to_uppercase()
        );
        let target = format!("CREATE DICTIONARY {} {body}", self.table(name));
        if self.table_already_exists(name, "create_table_query", &target)? {
            tracing::info!("dictionary {name} already exists, skip migration");
            return skip();
        }
        tracing::info!("create dictionary {name}");
        self.exec(&format!("CREATE OR REPLACE DICTIONARY {}{} {body}", self.table(name), self.on_cluster()), &[])
            .map_err(|e| format!("cannot create dictionary {name}: {e}"))
    }

    fn create_exporters_table(&self) -> StepResult {
        let mut columns = vec![];
        for c in self.schema.columns() {
            if c.key == ColumnKey::TimeReceived || c.name.starts_with("Exporter") {
                columns.push(format!("{} {}", ident(&c.name), c.clickhouse_type));
            }
            if c.name.starts_with("InIf") {
                columns.push(format!("{} {}", ident(&c.name[2..]), c.clickhouse_type));
            }
        }
        let name = "exporters";
        let body = format!(
            "({}) ENGINE = {} ORDER BY (ExporterAddress, IfName) TTL TimeReceived + toIntervalDay(1)",
            columns.join(", "),
            self.merge_tree_engine(name, "Replacing", &["TimeReceived".into()])
        );
        let target = format!("CREATE TABLE {} {body}", self.table(name));
        if self.table_already_exists(name, "create_table_query", &target)? {
            tracing::info!("exporters table already exists, skip migration");
            return skip();
        }
        tracing::info!("create exporters table");
        self.exec(
            &format!("CREATE OR REPLACE TABLE {}{} {body}", self.table(name), self.on_cluster()),
            &[("allow_suspicious_low_cardinality_types", "1")],
        )
        .map_err(|e| format!("cannot create exporters table: {e}"))
    }

    fn create_exporters_consumer_view(&self) -> StepResult {
        let mut items = vec![];
        for c in self.schema.columns() {
            if c.key == ColumnKey::TimeReceived || c.name.starts_with("Exporter") {
                items.push(ident(&c.name));
            }
            if c.name.starts_with("InIf") {
                let suffix = &c.name[4..];
                items.push(format!("[InIf{suffix}, OutIf{suffix}][num] AS If{suffix}"));
            }
        }
        let select = format!(
            "SELECT {} FROM {} ARRAY JOIN arrayEnumerate([1, 2]) AS num",
            items.join(", "),
            self.table("flows")
        );
        let name = "exporters_consumer";
        if self.table_already_exists(name, "as_select", &select)? {
            tracing::info!("exporters view already exists, skip migration");
            return skip();
        }
        tracing::info!("create exporters view");
        self.drop_table(name).map_err(|e| format!("cannot drop existing exporters view: {e}"))?;
        self.exec(
            &format!("CREATE MATERIALIZED VIEW {}{} TO exporters AS {select}", ident(name), self.on_cluster()),
            &[],
        )
        .map_err(|e| format!("cannot create exporters view: {e}"))
    }

    fn drop_table(&self, name: &str) -> Result<(), String> {
        self.exec(&format!("DROP TABLE IF EXISTS {}{} SYNC", ident(name), self.on_cluster()), &[])
    }

    fn raw_table_name(&self) -> String {
        format!("flows_{}_raw", self.schema.clickhouse_hash())
    }

    fn create_raw_flows_table(&self) -> StepResult {
        let name = self.raw_table_name();
        let columns =
            self.schema.clickhouse_create_table(&[TableOption::SkipGeneratedColumns, TableOption::SkipAliasedColumns]);
        let body = format!("({columns}) ENGINE = Null");
        let target = format!("CREATE TABLE {} {body}", self.table(&name));
        if self.table_already_exists(&name, "create_table_query", &target)? {
            tracing::info!("raw flows table already exists, skip migration");
            return skip();
        }
        tracing::info!("create raw flows table");
        for t in [format!("{name}_consumer"), name.clone()] {
            self.drop_table(&t).map_err(|e| format!("cannot drop {t}: {e}"))?;
        }
        self.exec(
            &format!("CREATE TABLE {}{} {body}", self.table(&name), self.on_cluster()),
            &[("allow_suspicious_low_cardinality_types", "1")],
        )
        .map_err(|e| format!("cannot create raw flows table: {e}"))
    }

    fn create_raw_flows_consumer_view(&self) -> StepResult {
        let table = self.raw_table_name();
        let view = format!("{table}_consumer");
        let items =
            self.schema.clickhouse_select_columns(&[TableOption::SubstituteGenerates, TableOption::SkipAliasedColumns]);
        let with = match self.schema.lookup_column_by_key(ColumnKey::DstASPath) {
            Some(c) if !c.disabled => "WITH arrayCompact(DstASPath) AS c_DstASPath ",
            _ => "",
        };
        let select = format!("{with}SELECT {} FROM {}", items.join(", "), self.table(&table));
        if self.table_already_exists(&view, "as_select", &select)? {
            tracing::info!("raw flows consumer view already exists, skip migration");
            return skip();
        }
        tracing::info!("create raw flows consumer view");
        self.drop_table(&view).map_err(|e| format!("cannot drop table {view}: {e}"))?;
        self.exec(&format!("CREATE MATERIALIZED VIEW {}{} TO flows AS {select}", ident(&view), self.on_cluster()), &[])
            .map_err(|e| format!("cannot create raw flows consumer view: {e}"))
    }

    fn create_or_update_flows_table(&self, resolution: &ResolutionConfiguration) -> StepResult {
        let lc = [("allow_suspicious_low_cardinality_types", "1")];
        let table = self.local_table(&flows_table(resolution));
        let partition_interval = (resolution.ttl.as_nanos() / self.config.max_partitions as u128) / 1_000_000_000;
        let ttl_expr = format!("TimeReceived + toIntervalSecond({})", resolution.ttl.as_secs());
        let settings = table_settings(&resolution.table_settings);
        let aggregated = !resolution.interval.is_zero();
        let primary_keys = self.schema.clickhouse_primary_keys();
        let sorting_keys = self.schema.clickhouse_sorting_keys();

        if self.table_column(&table, "name")?.is_empty() {
            let options = if aggregated { vec![TableOption::SkipMainOnlyColumns] } else { vec![] };
            let columns = self.schema.clickhouse_create_table(&options);
            let (engine, primary, order) = if !aggregated {
                let five = "toStartOfFiveMinutes(TimeReceived)";
                (
                    self.merge_tree_engine(&table, "", &[]),
                    five.to_string(),
                    format!("({five}, ExporterAddress, InIfName, OutIfName)"),
                )
            } else {
                (
                    self.merge_tree_engine(&table, "Summing", &["(Bytes, Packets)".into()]),
                    format!("({})", primary_keys.iter().map(|k| ident(k)).collect::<Vec<_>>().join(", ")),
                    format!("({})", sorting_keys.iter().map(|k| ident(k)).collect::<Vec<_>>().join(", ")),
                )
            };
            let settings: Vec<String> = settings.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            let sql = format!(
                "CREATE TABLE {}{} ({columns}) ENGINE = {engine} ORDER BY {order} \
                 PARTITION BY toYYYYMMDDhhmmss(toStartOfInterval(TimeReceived, INTERVAL {partition_interval} second)) \
                 PRIMARY KEY {primary} TTL {ttl_expr} SETTINGS {}",
                ident(&table),
                self.on_cluster(),
                settings.join(", ")
            );
            self.exec(&sql, &lc).map_err(|e| format!("cannot create {table}: {e}"))?;
            self.apply_skip_indexes(&table, !aggregated)?;
            return Ok(());
        }

        // Existing columns
        let rows = self.query(&format!(
            "SELECT name, type, compression_codec, is_in_sorting_key, is_in_primary_key, default_kind \
             FROM system.columns WHERE database = {} AND table = {} ORDER BY position ASC",
            quote_string(&self.database),
            quote_string(&table)
        ))?;
        struct Existing {
            name: String,
            r#type: String,
            codec: String,
            sorting: bool,
            primary: bool,
            default_kind: String,
        }
        let existing: Vec<Existing> = rows
            .iter()
            .map(|r| Existing {
                name: string(&r[0]),
                r#type: string(&r[1]),
                codec: string(&r[2]),
                sorting: r[3].as_u64() == Some(1),
                primary: r[4].as_u64() == Some(1),
                default_kind: string(&r[5]),
            })
            .collect();

        let mut alter: Vec<String> = vec![];
        let mut previous = String::new();
        let add_column = |definition: String, previous: &str| {
            if previous.is_empty() {
                format!("ADD COLUMN {definition} FIRST")
            } else {
                format!("ADD COLUMN {definition} AFTER {}", ident(previous))
            }
        };
        for wanted in self.schema.columns() {
            if aggregated && wanted.clickhouse_main_only {
                continue;
            }
            let definition = wanted.clickhouse_definition();
            if let Some(e) = existing.iter().find(|e| e.name == wanted.name) {
                let mut modify = false;
                if wanted.clickhouse_type != e.r#type {
                    modify = true;
                    if primary_keys.contains(&wanted.name) {
                        return Err(format!(
                            "table {table}, primary key column {} has a non-matching type: {} vs {}",
                            wanted.name, e.r#type, wanted.clickhouse_type
                        ));
                    }
                }
                if !wanted.clickhouse_codec.is_empty() && format!("CODEC({})", wanted.clickhouse_codec) != e.codec {
                    modify = true;
                }
                if wanted.clickhouse_alias.is_empty() == (e.default_kind == "ALIAS") {
                    tracing::debug!("column {} alias content has changed, recreating", e.name);
                    self.exec(
                        &format!("ALTER TABLE {}{} DROP COLUMN {}", ident(&table), self.on_cluster(), ident(&e.name)),
                        &lc,
                    )
                    .map_err(|err| format!("cannot drop {} from {table} to cleanup aliasing: {err}", e.name))?;
                    alter.push(add_column(definition.clone(), &previous));
                }
                if aggregated && primary_keys.contains(&wanted.name) && !e.primary {
                    return Err(format!(
                        "table {table}, column {} should be a primary key, cannot change that",
                        wanted.name
                    ));
                }
                if aggregated && !wanted.clickhouse_not_sorting_key && !e.sorting {
                    // Drop it before adding it back, to fix the ordering.
                    self.exec(
                        &format!("ALTER TABLE {}{} DROP COLUMN {}", ident(&table), self.on_cluster(), ident(&e.name)),
                        &lc,
                    )
                    .map_err(|err| format!("cannot drop {} from {table} to fix ordering: {err}", e.name))?;
                    alter.push(add_column(definition.clone(), &previous));
                } else if modify {
                    alter.push(format!("MODIFY COLUMN {definition}"));
                }
                previous = wanted.name.clone();
                continue;
            }
            if aggregated && primary_keys.contains(&wanted.name) {
                return Err(format!("table {table}, column {} is missing but it is a primary key", wanted.name));
            }
            tracing::debug!("add missing column {} to {table}", wanted.name);
            alter.push(add_column(definition, &previous));
            previous = wanted.name.clone();
        }

        let mut modified = false;
        if !alter.is_empty() {
            if aggregated {
                alter.push(format!(
                    "MODIFY ORDER BY ({})",
                    sorting_keys.iter().map(|k| ident(k)).collect::<Vec<_>>().join(", ")
                ));
            }
            tracing::info!("apply {} modifications to {table}", alter.len());
            if aggregated {
                let view = format!("{table}_consumer");
                self.drop_table(&view).map_err(|e| format!("cannot drop {view}: {e}"))?;
            }
            self.exec(&format!("ALTER TABLE {}{} {}", ident(&table), self.on_cluster(), alter.join(", ")), &lc)
                .map_err(|e| format!("cannot update table {table}: {e}"))?;
            modified = true;
        }

        let engine = self.table_engine(&table)?.unwrap_or_default();
        if !engine.settings_match(&settings) {
            tracing::info!("updating settings of {table} to {}", resolution.interval);
            let modify: Vec<String> = settings.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            self.exec(
                &format!("ALTER TABLE {}{} MODIFY SETTING {}", ident(&table), self.on_cluster(), modify.join(", ")),
                &lc,
            )
            .map_err(|e| format!("cannot modify settings for table {table}: {e}"))?;
            modified = true;
        }
        if !engine.ttl_matches(&ttl_expr) {
            tracing::info!("updating TTL of {table} with interval {}", resolution.interval);
            self.exec(
                &format!("ALTER TABLE {}{} MODIFY TTL {ttl_expr}", ident(&table), self.on_cluster()),
                &[("allow_suspicious_low_cardinality_types", "1"), ("materialize_ttl_after_modify", "0")],
            )
            .map_err(|e| format!("cannot modify TTL for table {table}: {e}"))?;
            modified = true;
        }
        if self.apply_skip_indexes(&table, !aggregated)? {
            modified = true;
        }
        if modified { Ok(()) } else { skip() }
    }

    /// Make the skip indexes of a table match the schema. Returns whether
    /// something changed.
    fn apply_skip_indexes(&self, table: &str, main_table: bool) -> Result<bool, String> {
        let rows = self.query(&format!(
            "SELECT name, type_full FROM system.data_skipping_indices WHERE database = {} AND table = {} AND startsWith(name, 'idx_')",
            quote_string(&self.database),
            quote_string(table)
        ))?;
        let existing: BTreeMap<String, String> = rows.iter().map(|r| (string(&r[0]), string(&r[1]))).collect();
        let mut to_drop = vec![];
        let mut to_add = vec![];
        let mut wanted_names = vec![];
        for (key, kind) in self.skip_indexes {
            let Some(col) = self.schema.lookup_column_by_key(*key) else { continue };
            if col.disabled || (col.clickhouse_main_only && !main_table) {
                continue;
            }
            let ch_type = kind.clickhouse_type().map_err(|e| format!("schema index for {}: {e}", col.name))?;
            let name = format!("idx_{}", col.name.to_lowercase());
            wanted_names.push(name.clone());
            let existing_type = existing.get(&name).cloned().unwrap_or_default();
            if !existing_type.is_empty() && existing_type != ch_type {
                to_drop.push(format!("DROP INDEX {}", ident(&name)));
            }
            if existing_type.is_empty() || existing_type != ch_type {
                to_add.push(format!("ADD INDEX {} {} TYPE {ch_type} GRANULARITY 4", ident(&name), ident(&col.name)));
            }
        }
        for name in existing.keys() {
            if !wanted_names.contains(name) {
                to_drop.push(format!("DROP INDEX {}", ident(name)));
            }
        }
        if to_drop.is_empty() && to_add.is_empty() {
            return Ok(false);
        }
        for (what, list) in [("removing", &to_drop), ("adding", &to_add)] {
            if list.is_empty() {
                continue;
            }
            tracing::info!("{what} {} skip index(es) from {table}", list.len());
            self.exec(&format!("ALTER TABLE {}{} {}", ident(table), self.on_cluster(), list.join(", ")), &[])
                .map_err(|e| format!("cannot change skip indexes on {table}: {e}"))?;
        }
        Ok(true)
    }

    fn create_flows_consumer_view(&self, resolution: &ResolutionConfiguration) -> StepResult {
        if resolution.interval.is_zero() {
            // The consumer of the main table is created elsewhere.
            return skip();
        }
        let table = flows_table(resolution);
        let view = format!("{table}_consumer");
        let columns = self.schema.clickhouse_select_columns(&[
            TableOption::SkipTimeReceived,
            TableOption::SkipMainOnlyColumns,
            TableOption::SkipAliasedColumns,
        ]);
        let select = format!(
            "SELECT toStartOfInterval(TimeReceived, toIntervalSecond({})) AS TimeReceived, {} FROM {}",
            resolution.interval.as_secs(),
            columns.join(", "),
            self.table(&self.local_table("flows"))
        );
        if self.table_already_exists(&view, "as_select", &select)? {
            tracing::info!("{view} already exists, skip migration");
            return skip();
        }
        tracing::info!("create {view}");
        self.drop_table(&view).map_err(|e| format!("cannot drop table {view}: {e}"))?;
        self.exec(
            &format!(
                "CREATE MATERIALIZED VIEW {}{} TO {} AS {select}",
                ident(&view),
                self.on_cluster(),
                ident(&self.local_table(&table))
            ),
            &[],
        )
        .map_err(|e| format!("cannot create {view}: {e}"))
    }

    /// The distributed table over the local tables, in a sharded cluster.
    fn create_distributed_table(&self, source: &str) -> StepResult {
        let local = self.local_table(source);
        if local == source {
            return skip();
        }
        let rows = self.query(&format!(
            "SELECT name, type, compression_codec, default_kind, default_expression FROM system.columns \
             WHERE database = {} AND table = {} ORDER BY position ASC",
            quote_string(&self.database),
            quote_string(&local)
        ))?;
        let columns: Vec<String> = rows
            .iter()
            .map(|r| {
                let mut col = format!("{} {}", ident(&string(&r[0])), string(&r[1]));
                let codec = string(&r[2]);
                if !codec.is_empty() {
                    col.push_str(&format!(" {codec}"));
                }
                let kind = string(&r[3]);
                if !kind.is_empty() {
                    col.push_str(&format!(" {kind} {}", string(&r[4])));
                }
                col
            })
            .collect();
        let body = format!(
            "({}) ENGINE = Distributed({}, {}, {}, rand())",
            columns.join(", "),
            quote_string(&self.cluster),
            quote_string(&self.database),
            quote_string(&local)
        );
        let target = format!("CREATE TABLE {} {body}", self.table(source));
        if self.table_already_exists(source, "create_table_query", &target)? {
            tracing::info!("{source} already exists, skip migration");
            return skip();
        }
        tracing::info!("create {source}");
        self.exec(
            &format!("CREATE OR REPLACE TABLE {}{} {body}", self.table(source), self.on_cluster()),
            &[("allow_suspicious_low_cardinality_types", "1")],
        )
        .map_err(|e| format!("cannot create {source}: {e}"))
    }

    /// Reload the dictionaries with a source.
    fn reload_dictionaries(&self) {
        let rows = match self
            .query("SELECT name FROM system.dictionaries WHERE database = currentDatabase() AND source != ''")
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "unable to list dictionaries for reload");
                return;
            }
        };
        for row in rows {
            let name = string(&row[0]);
            if let Err(e) =
                self.exec(&format!("SYSTEM RELOAD DICTIONARY{} {}", self.on_cluster(), self.table(&name)), &[])
            {
                tracing::error!(error = %e, dictionary = name, "unable to reload dictionary");
            }
        }
    }
}

/// An attribute of a dictionary.
struct Attribute {
    name: String,
    r#type: String,
    default: Option<String>,
    injective: bool,
}
